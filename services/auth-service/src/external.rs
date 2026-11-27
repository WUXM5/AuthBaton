//! External account sources: a credential center that lists its accounts and lends this Mac their
//! access tokens. Refresh tokens stay at the center. The device key and the center's entry token
//! live in the Vault only, never in the configuration file, the database, the views or logs.
use crate::credentials::{Bundle, NativeStore};
use crate::platform::{Platform, Result, read_json, write_json};
use crate::status::now;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

const VAULT: &str = "io.github.wuxm5.authbaton.vault";
pub const MODES: [&str; 3] = ["view", "reset", "switch"];
/// A source's name until the center gives its own.
pub const PLACEHOLDER: &str = "外部来源";
/// What an external row's card says about it.
pub const ROW_MESSAGE: &str = "由外部来源续期；这台 Mac 只用领来的 Access";
/// A lease is dropped this long before its access expires.
const MARGIN: u64 = 600;
/// A lease asked for with purpose `use` this recently was fetched for the action at hand.
const USE_FRESH: u64 = 120;
/// The access in a native store is renewed once this little of it is left.
pub const NATIVE_LEAD: u64 = 1800;
/// What a renewal of the access in a native store asks the center to leave on it: the center renews
/// first when less is left, so the renewal never gets back the access it is replacing.
const NATIVE_MIN_VALIDITY: u64 = NATIVE_LEAD + MARGIN;
pub const DIRECTORY_EVERY: u64 = 60;
/// How long an account's mode outlives the account leaving the directory or the source.
pub const PARKED_FOR: u64 = 30 * 86400;
/// A lease, renewal or read that failed waits this long before it is asked for again.
pub const RETRY: u64 = 60;
const BAD_ENROLLMENT: &str = "配对串无效：请在来源网页点「连接 AuthBaton」重新生成，并完整粘贴";
const NOT_FOR_US: &str = "这个配对串不是给 AuthBaton 的：请在来源网页点「连接 AuthBaton」重新生成";
const KEYCHAIN: &str = "配对信息存不进钥匙串，请解锁钥匙串后重试";
const NETWORK: &str = "网络不通或超时";
const INVALID: &str = "回复无效";
pub const CARRIES_REFRESH: &str = "外部来源发来了刷新令牌，已拒绝";
pub const IDENTITY_CHANGED: &str = "外部账号身份与来源目录不一致，已停止领取";

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Source {
    pub id: String,
    pub label: String,
    pub site_url: String,
    pub manage_url: String,
    pub host_id: String,
    pub device_label: String,
    pub connected_at: u64,
    pub default_mode: String,
    /// The accounts as the center last listed them, unknown fields left out.
    pub directory: Vec<Value>,
    pub read_at: Option<u64>,
    /// `connected` or `error`.
    pub state: String,
    pub error: Option<String>,
}
impl Source {
    pub fn entry(&self, account_ref: &str) -> Option<&Value> {
        self.directory
            .iter()
            .find(|e| e["account_ref"] == account_ref)
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Row {
    pub source: String,
    pub account_ref: String,
    pub mode: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Parked {
    pub mode: String,
    pub until: u64,
}
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub sources: Vec<Source>,
    /// Row id → the account of a source it shows.
    pub rows: HashMap<String, Row>,
    /// `<source id>:<account_ref>` → the mode of an account that left; restored when it is back.
    pub parked: HashMap<String, Parked>,
    /// Source id → accounts shown only through this Mac's own row of the same person.
    #[serde(default)]
    pub merged: HashMap<String, Vec<Value>>,
    /// Provider → the external row whose access the native store holds.
    #[serde(default)]
    pub selected: HashMap<String, String>,
    /// Provider → the row of this Mac in use before an external row; 断开 can switch back to it.
    #[serde(default)]
    pub previous: HashMap<String, String>,
    /// An external row being put in use; a restart settles it from the native store.
    #[serde(default)]
    pub pending_activation: Option<String>,
}
pub fn key(source: &str, account_ref: &str) -> String {
    format!("{source}:{account_ref}")
}
/// A source's id, the same each time this Mac pairs with the same center, so modes parked when it
/// was disconnected come back when it is connected again.
pub fn source_id(site_url: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(format!("external-source:{site_url}"))
    )[..16]
        .to_string()
}
/// AuthBaton's name for the center's provider.
pub fn provider(entry: &Value) -> &'static str {
    if entry["provider"] == "openai" {
        "codex"
    } else {
        "claude"
    }
}
/// Whose account a directory entry is, in the form `Metadata.owner` takes; none without ids.
pub fn owner(entry: &Value) -> Option<String> {
    let identity = &entry["identity"];
    let id = |k: &str| identity[k].as_str().filter(|s| !s.is_empty());
    match entry["provider"].as_str()? {
        "openai" => Some(format!("codex:{}:{}", id("user_id")?, id("workspace_id")?)),
        "claude" => Some(format!(
            "claude:{}:{}",
            id("account_uuid")?,
            id("organization_uuid")?
        )),
        _ => None,
    }
}

/// What a pairing string carries. The entry token stays in memory until the Vault holds it.
pub struct Enrollment {
    pub site_url: String,
    code: String,
    client_id: String,
    client_secret: String,
}
impl Enrollment {
    /// base64url of `{"site_url","code","access_client_id","access_client_secret"}`, without `=`.
    pub fn decode(text: &str) -> Result<Self> {
        let text = text.trim().trim_end_matches('=');
        if text.is_empty()
            || text.len() > 2048
            || !text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(BAD_ENROLLMENT);
        }
        let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|_| BAD_ENROLLMENT)?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| BAD_ENROLLMENT)?;
        let site_url =
            crate::cloud::validate_endpoint(value["site_url"].as_str().ok_or(BAD_ENROLLMENT)?)
                .map_err(|_| "配对串里的站点地址无效：必须是 HTTPS 根地址")?;
        let field = |k: &str| secret_text(&value[k], 512).ok_or(BAD_ENROLLMENT);
        Ok(Self {
            site_url,
            code: field("code")?,
            client_id: field("access_client_id")?,
            client_secret: field("access_client_secret")?,
        })
    }
}
/// A token that goes into a request header: visible ASCII only.
fn secret_text(value: &Value, max: usize) -> Option<String> {
    value
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= max
                && s.bytes()
                    .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
        })
        .map(str::to_owned)
}
/// An identifier the center gives: an account ref, a user, workspace or organization id.
fn ident(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
        .map(str::to_owned)
}
fn text(value: &Value, max: usize) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control))
        .map(str::to_owned)
}
/// The center's account page: an HTTPS address on the center's own site.
fn manage_url(value: &Value, site_url: &str) -> Option<String> {
    text(value, 512).filter(|u| {
        (u == site_url || u.starts_with(&format!("{site_url}/")))
            && !u
                .chars()
                .any(|c| c.is_whitespace() || c == '"' || c == '\\')
    })
}

#[derive(Serialize, Deserialize)]
struct Secret {
    site_url: String,
    client_id: String,
    client_secret: String,
    device_key: String,
}
pub fn vault_key(source: &str) -> String {
    format!("external-{source}")
}

/// Why a request to the center gave nothing usable.
#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    /// The pairing no longer works: the device key is missing, invalid or revoked.
    Unpaired,
    /// The center's fixed code, about the account or about itself.
    Code(String),
    /// No answer from the center: the network, a timeout or a reply that is not the center's.
    Unreachable(&'static str),
    /// An answer this Mac does not accept.
    Refused(&'static str),
}
impl Fail {
    /// What the source card says; `label` is the source's name.
    pub fn message(&self, label: &str) -> String {
        match self {
            Fail::Unpaired => {
                format!("{label} 的配对已失效，暂时拉不到最新额度，也不能给外部账号用重置卡。")
            }
            Fail::Unreachable(reason) => format!("连不上 {label}：{reason}"),
            Fail::Code(code) if code == "credential_service_unavailable" => {
                format!("连不上 {label}：来源暂时不可用")
            }
            Fail::Code(code) => format!("来源出错（{code}）"),
            Fail::Refused(message) => (*message).into(),
        }
    }
}
type Reply = std::result::Result<Value, Fail>;
/// A refusal from the center, by its fixed code; never the upstream text.
fn classify(body: &Value) -> Fail {
    let code = body["code"].as_str().filter(|c| {
        !c.is_empty()
            && c.len() <= 64
            && c.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    });
    match code {
        Some("device_key_required" | "device_key_invalid" | "device_revoked")
        | Some("device_not_account_viewer") => Fail::Unpaired,
        Some(code) => Fail::Code(code.into()),
        None => Fail::Unreachable(INVALID),
    }
}
#[cfg(test)]
pub type TestTransport = Box<dyn FnMut(&str, &Value, Option<&str>) -> Reply>;
#[cfg(test)]
thread_local! {
    static TEST_TRANSPORT: std::cell::RefCell<Option<TestTransport>> = std::cell::RefCell::new(None);
}
/// Answers every request this thread sends to a center: `(operation, body, device key)`.
#[cfg(test)]
pub(crate) fn test_transport(transport: TestTransport) {
    TEST_TRANSPORT.with(|slot| *slot.borrow_mut() = Some(transport));
}
#[cfg(test)]
fn post(
    _site_url: &str,
    path: &str,
    _access: (&str, &str),
    device_key: Option<&str>,
    body: &Value,
) -> Reply {
    let operation = path.rsplit('/').next().unwrap_or(path);
    TEST_TRANSPORT.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .expect("external tests require a synthetic transport; real network is forbidden")(
            operation, body, device_key,
        )
    })
}
#[cfg(not(test))]
fn post(
    site_url: &str,
    path: &str,
    access: (&str, &str),
    device_key: Option<&str>,
    body: &Value,
) -> Reply {
    let url = format!("{site_url}{path}");
    let quote = |s: &str| {
        format!(
            "\"{}\"",
            s.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
        )
    };
    let mut headers = vec![
        format!("CF-Access-Client-Id: {}", access.0),
        format!("CF-Access-Client-Secret: {}", access.1),
        "Content-Type: application/json".to_string(),
        format!("User-Agent: AuthBaton/{}", env!("CARGO_PKG_VERSION")),
    ];
    if let Some(key) = device_key {
        headers.push(format!("X-Device-Key: {key}"));
    }
    // The tokens go to curl on stdin, never in its arguments.
    let mut config = format!("url = {}\nrequest = \"POST\"\n", quote(&url));
    for header in headers {
        config.push_str(&format!("header = {}\n", quote(&header)));
    }
    config.push_str(&format!("data = {}\n", quote(&body.to_string())));
    let output = crate::network::curl(
        &url,
        &[
            "-q",
            "--silent",
            "--proto",
            "=https",
            "--max-time",
            "22",
            "--max-filesize",
            "1048576",
            "--write-out",
            "\n%{http_code}",
            "--config",
            "-",
        ],
        config.as_bytes(),
        std::time::Duration::from_secs(25),
    )
    .map_err(|_| Fail::Unreachable(NETWORK))?;
    let output = String::from_utf8(output).map_err(|_| Fail::Unreachable(INVALID))?;
    let (body, status) = output.rsplit_once('\n').ok_or(Fail::Unreachable(INVALID))?;
    let value: Option<Value> = serde_json::from_str(body).ok();
    match (status.trim(), value) {
        ("200", Some(value)) => Ok(value),
        (_, Some(value)) => Err(classify(&value)),
        ("401" | "403", None) => Err(Fail::Unreachable("入口令牌被拒绝")),
        _ => Err(Fail::Unreachable(INVALID)),
    }
}
/// A request of a paired source, with the device key and entry token from the Vault.
fn call(platform: &Platform, source: &Source, operation: &str, body: Value) -> Reply {
    let secret: Secret = match platform.vault_read_optional(&vault_key(&source.id)) {
        Ok(Some(value)) => serde_json::from_value(value).map_err(|_| Fail::Unpaired)?,
        Ok(None) => return Err(Fail::Unpaired),
        Err(_) => return Err(Fail::Unreachable("读不到钥匙串里的配对信息")),
    };
    if secret.site_url != source.site_url {
        return Err(Fail::Unpaired);
    }
    post(
        &secret.site_url,
        &format!("/api/device/cloud-auth/external/{operation}"),
        (&secret.client_id, &secret.client_secret),
        Some(&secret.device_key),
        &body,
    )
}

/// What one read of the directory found.
pub struct Directory {
    label: String,
    manage_url: String,
    accounts: Vec<Value>,
    read_at: u64,
}
/// A directory entry with only the fields this Mac uses; none for an account it does not show.
fn directory_entry(a: &Value) -> Option<Value> {
    let account_ref = ident(&a["account_ref"])?;
    let provider = a["provider"]
        .as_str()
        .filter(|p| matches!(*p, "openai" | "claude"))?;
    let identity = &a["identity"];
    // A state this build does not know is not one it can use.
    let status = a["status"]
        .as_str()
        .filter(|s| matches!(*s, "ready" | "needs_reauthorization" | "disabled"))
        .unwrap_or("needs_reauthorization");
    Some(json!({
        "account_ref": account_ref,
        "provider": provider,
        "label": text(&a["label"], 160),
        "identity": {
            "email": text(&identity["email"], 160),
            "user_id": ident(&identity["user_id"]),
            "workspace_id": ident(&identity["workspace_id"]),
            "account_uuid": ident(&identity["account_uuid"]),
            "organization_uuid": ident(&identity["organization_uuid"]),
        },
        "plan": text(&a["plan"], 40),
        "status": status,
        "login_expires_at": a["login_expires_at"].as_u64(),
        "quota_readable": a["quota_readable"] == true,
        "credential_version": a["credential_version"].as_u64(),
    }))
}
fn read_directory(platform: &Platform, source: &Source) -> std::result::Result<Directory, Fail> {
    let read_at = now();
    let v = call(platform, source, "accounts", json!({}))?;
    let accounts = v["accounts"]
        .as_array()
        .ok_or(Fail::Unreachable(INVALID))?
        .iter()
        .take(256)
        .filter_map(directory_entry)
        .collect();
    let label = text(&v["source"]["label"], 60).unwrap_or_else(|| {
        if source.label.is_empty() {
            PLACEHOLDER.into()
        } else {
            source.label.clone()
        }
    });
    Ok(Directory {
        label,
        manage_url: manage_url(&v["source"]["manage_url"], &source.site_url)
            .unwrap_or_else(|| source.site_url.clone()),
        accounts,
        read_at,
    })
}

/// Access leased for an account, kept in memory only.
#[derive(Clone)]
pub struct Lease {
    pub bundle: Bundle,
    /// Whose access it is, worked out once when it came in.
    pub metadata: crate::credentials::Metadata,
    pub version: u64,
    pub expires_at: u64,
    /// When access was last asked for with purpose `use`.
    pub used_at: Option<u64>,
}
/// The access-only login in a lease, checked against the directory entry it was asked for. One
/// that carries a refresh token, or names another account, is refused.
pub fn lease_of(v: &Value, entry: &Value) -> Result<Lease> {
    if v["account_ref"] != entry["account_ref"] || v["provider"] != entry["provider"] {
        return Err(IDENTITY_CHANGED);
    }
    let c = &v["credential"];
    if c.get("refresh_token").is_some()
        || c.get("refreshToken").is_some()
        || c.pointer("/tokens/refresh_token").is_some()
    {
        return Err(CARRIES_REFRESH);
    }
    let identity = &v["identity"];
    let bundle = if entry["provider"] == "openai" {
        Bundle {
            provider: "codex".into(),
            credential: json!({"auth_mode":"chatgptAuthTokens","tokens":{
                "id_token": c["id_token"], "access_token": c["access_token"], "account_id": c["account_id"]}}),
            // As Codex's own store reads back: the identity is in the tokens.
            identity: Value::Null,
        }
    } else {
        let mut credential = serde_json::Map::new();
        credential.insert("accessToken".into(), c["access_token"].clone());
        credential.insert(
            "expiresAt".into(),
            c["expires_at_ms"]
                .as_u64()
                .or_else(|| v["expires_at"].as_u64().map(|t| t * 1000))
                .into(),
        );
        credential.insert("scopes".into(), c["scopes"].clone());
        for (from, to) in [
            ("subscription_type", "subscriptionType"),
            ("rate_limit_tier", "rateLimitTier"),
        ] {
            if let Some(value) = c[from].as_str() {
                credential.insert(to.into(), value.into());
            }
        }
        Bundle {
            provider: "claude".into(),
            credential: Value::Object(credential),
            identity: json!({"accountUuid": identity["account_uuid"],
                "organizationUuid": identity["organization_uuid"], "emailAddress": identity["email"]}),
        }
    };
    let metadata = bundle.cloud_metadata()?;
    if bundle.provider == "codex" {
        let token = bundle.credential["tokens"]["access_token"]
            .as_str()
            .ok_or("缺少 Access")?;
        let claims = crate::credentials::claims(token)?;
        let auth = &claims["https://api.openai.com/auth"];
        if metadata.owner
            != format!(
                "codex:{}:{}",
                auth["chatgpt_user_id"]
                    .as_str()
                    .or(auth["user_id"].as_str())
                    .unwrap_or(""),
                auth["chatgpt_account_id"].as_str().unwrap_or("")
            )
        {
            return Err(IDENTITY_CHANGED);
        }
    }
    if owner(entry).is_some_and(|o| o != metadata.owner) {
        return Err(IDENTITY_CHANGED);
    }
    let expires_at = v["expires_at"]
        .as_u64()
        .into_iter()
        .chain(metadata.expires_at)
        .min()
        .filter(|t| *t > now() + 60)
        .ok_or("外部账号的 Access 有效期无效")?;
    Ok(Lease {
        bundle,
        metadata,
        version: v["credential_version"].as_u64().unwrap_or(0),
        expires_at,
        used_at: None,
    })
}
/// `min_validity`: seconds the access must have left, or the center renews it first; without, the
/// center's own 600.
fn fetch(
    platform: &Platform,
    source: &Source,
    entry: &Value,
    purpose: &str,
    rejected: Option<u64>,
    min_validity: Option<u64>,
) -> std::result::Result<Lease, Fail> {
    let mut body = json!({"account_ref": entry["account_ref"], "purpose": purpose, "rejected_version": rejected});
    if let Some(seconds) = min_validity {
        body["min_validity_s"] = json!(seconds);
    }
    let v = call(platform, source, "lease", body)?;
    let mut lease = lease_of(&v, entry).map_err(Fail::Refused)?;
    if purpose == "use" {
        lease.used_at = Some(now());
    }
    Ok(lease)
}

/// What a background request brought back.
pub enum Done {
    Connected {
        host_id: String,
        device_label: String,
        directory: std::result::Result<Directory, Fail>,
    },
    Directory(Directory),
    Lease {
        key: String,
        lease: Lease,
    },
    /// The access of the row in use was renewed in the native store.
    Native {
        key: String,
        provider: String,
        lease: Lease,
    },
    Reported,
    /// `key`: the account a lease or renewal was for; none for a read of the whole source.
    Failed {
        key: Option<String>,
        fail: Fail,
    },
}
/// Pairs this Mac with the center the string names, keeps the pairing in the Vault and reads
/// the directory once.
pub fn connect(platform: &Platform, source: &Source, enrollment: Enrollment) -> Done {
    let failed = |fail| Done::Failed { key: None, fail };
    let v = match post(
        &enrollment.site_url,
        "/api/device/enroll",
        (&enrollment.client_id, &enrollment.client_secret),
        None,
        &json!({"code": enrollment.code}),
    ) {
        Ok(v) => v,
        Err(fail) => return failed(fail),
    };
    if v["role"] != "account_viewer" {
        return failed(Fail::Refused(NOT_FOR_US));
    }
    let Some(device_key) = secret_text(&v["device_key"], 512) else {
        return failed(Fail::Unreachable(INVALID));
    };
    let secret = Secret {
        site_url: enrollment.site_url,
        client_id: enrollment.client_id,
        client_secret: enrollment.client_secret,
        device_key,
    };
    if platform
        .vault_write(
            &vault_key(&source.id),
            &serde_json::to_value(&secret).unwrap(),
        )
        .is_err()
    {
        return failed(Fail::Refused(KEYCHAIN));
    }
    Done::Connected {
        host_id: ident(&v["host_id"]).unwrap_or_default(),
        device_label: text(&v["label"], 80).unwrap_or_default(),
        directory: read_directory(platform, source),
    }
}
pub fn sync(platform: &Platform, source: &Source) -> Done {
    match read_directory(platform, source) {
        Ok(directory) => Done::Directory(directory),
        Err(fail) => Done::Failed { key: None, fail },
    }
}
pub fn lease(
    platform: &Platform,
    source: &Source,
    entry: &Value,
    purpose: &str,
    rejected: Option<u64>,
) -> Done {
    let key = key(&source.id, entry["account_ref"].as_str().unwrap_or(""));
    match fetch(platform, source, entry, purpose, rejected, None) {
        Ok(lease) => Done::Lease { key, lease },
        Err(fail) => Done::Failed {
            key: Some(key),
            fail,
        },
    }
}
/// The access-only login of the account in use, leased again for its use and written over the
/// projection written last. Every native writer takes the same in-process lock.
pub fn native_sync(
    platform: &Platform,
    source: &Source,
    entry: &Value,
    native: &NativeStore,
    row: &str,
) -> Done {
    let key = key(&source.id, entry["account_ref"].as_str().unwrap_or(""));
    let failed = |fail| Done::Failed {
        key: Some(key.clone()),
        fail,
    };
    let lease = match fetch(
        platform,
        source,
        entry,
        "use",
        None,
        Some(NATIVE_MIN_VALIDITY),
    ) {
        Ok(lease) => lease,
        Err(fail) => return failed(fail),
    };
    match crate::cloud::write_projection(platform, native, row, &lease.bundle, None) {
        Ok(()) => Done::Native {
            provider: lease.bundle.provider.clone(),
            key,
            lease,
        },
        Err(message) => failed(Fail::Refused(message)),
    }
}
/// Sends a reading to the center. It is sent once: a failure waits for the next reading.
pub fn report(platform: &Platform, source: &Source, body: Value) -> Done {
    let _ = call(platform, source, "observations", body);
    Done::Reported
}

pub struct Light {
    pub tag: String,
    source: String,
    generation: u64,
    work: Work,
}
enum Work {
    #[cfg_attr(test, allow(dead_code))]
    Running(JoinHandle<Done>),
    /// Tests do the work on the calling thread, where their synthetic transport lives.
    #[cfg(test)]
    Done(Box<Done>),
}
impl Light {
    fn finished(&self) -> bool {
        match &self.work {
            Work::Running(handle) => handle.is_finished(),
            #[cfg(test)]
            Work::Done(_) => true,
        }
    }
    fn join(self) -> Done {
        let interrupted = || Done::Failed {
            key: None,
            fail: Fail::Unreachable("请求中断"),
        };
        match self.work {
            Work::Running(handle) => handle.join().unwrap_or_else(|_| interrupted()),
            #[cfg(test)]
            Work::Done(done) => *done,
        }
    }
}
/// A switch asked for by a person that waits for access leased for it.
pub struct Follow {
    pub row: String,
    pub key: String,
    pub started: u64,
    pub until: u64,
}
pub struct Runtime {
    pub config: Config,
    path: PathBuf,
    pub light: Vec<Light>,
    /// Counts pairings and disconnections; a request from before one describes what no longer holds.
    generation: u64,
    leases: HashMap<String, Lease>,
    /// Account key (or `native:<provider>`) → when it was last asked for.
    pub tried: HashMap<String, u64>,
    /// Account key → when its last lease failed and why.
    pub failed: HashMap<String, (u64, String)>,
    pub next_directory: u64,
    pub next_settle: u64,
    /// Provider → expiry and version of the access last written into its native store.
    pub written: HashMap<String, (u64, u64)>,
    /// Row id → the reading last reported to the center.
    pub reported: HashMap<String, u64>,
    pub follow: Option<Follow>,
}
impl Runtime {
    pub fn open(root: &Path) -> Result<Self> {
        let path = root.join("external-config.json");
        let config = read_json(&path)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| "外部来源配置格式无效")?
            .unwrap_or_default();
        Ok(Self {
            config,
            path,
            light: vec![],
            generation: 0,
            leases: HashMap::new(),
            tried: HashMap::new(),
            failed: HashMap::new(),
            next_directory: 0,
            next_settle: 0,
            written: HashMap::new(),
            reported: HashMap::new(),
            follow: None,
        })
    }
    pub fn save(&self) -> Result<()> {
        write_json(&self.path, &serde_json::to_value(&self.config).unwrap())
    }
    pub fn source(&self, id: &str) -> Option<&Source> {
        self.config.sources.iter().find(|s| s.id == id)
    }
    /// A lease good for more than ten more minutes.
    pub fn lease(&self, key: &str) -> Option<&Lease> {
        self.leases
            .get(key)
            .filter(|l| l.expires_at > now() + MARGIN)
    }
    /// The latest lease, whatever is left of it: what the card shows about the account.
    pub fn last_lease(&self, key: &str) -> Option<&Lease> {
        self.leases.get(key)
    }
    /// Access asked for with purpose `use` a moment ago, good for a while yet.
    pub fn use_ready(&self, key: &str) -> bool {
        self.lease(key)
            .and_then(|l| l.used_at)
            .is_some_and(|at| now().saturating_sub(at) <= USE_FRESH)
    }
    pub fn running(&self, tag: &str) -> bool {
        self.light.iter().any(|l| l.tag == tag)
    }
    /// Still writing: a job whose result has not come back yet.
    pub fn busy(&self, tag: &str) -> bool {
        self.light.iter().any(|l| l.tag == tag && !l.finished())
    }
    pub fn job(&self, source: &str) -> Option<&'static str> {
        if self.running(&format!("connect:{source}")) {
            Some("connecting")
        } else if self.running(&format!("directory:{source}")) {
            Some("syncing")
        } else {
            None
        }
    }
    /// Starts a background request unless one with the same tag is running. It never holds other
    /// work back: its result is applied when it ends.
    pub fn start<F>(&mut self, tag: &str, source: &str, work: F)
    where
        F: FnOnce() -> Done + Send + 'static,
    {
        if self.running(tag) {
            return;
        }
        #[cfg(test)]
        let work = Work::Done(Box::new(work()));
        #[cfg(not(test))]
        let work = Work::Running(std::thread::spawn(work));
        self.light.push(Light {
            tag: tag.into(),
            source: source.into(),
            generation: self.generation,
            work,
        });
    }
    /// Requests from before a pairing or a disconnection are dropped when they end.
    pub fn renew_generation(&mut self) {
        self.generation += 1;
    }
    pub fn forget(&mut self, key: &str) {
        self.leases.remove(key);
        self.tried.remove(key);
        self.failed.remove(key);
    }
    pub fn clear(&mut self) {
        self.leases.clear();
        self.tried.clear();
        self.failed.clear();
        self.written.clear();
        self.reported.clear();
        self.follow = None;
    }
    pub fn finish(&mut self) {
        // Background requests are dropped: each is asked for again after a restart.
        self.light.clear();
    }
    /// Applies what finished requests brought back. True when the configuration changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let mut index = 0;
        while index < self.light.len() {
            if !self.light[index].finished() {
                index += 1;
                continue;
            }
            let job = self.light.remove(index);
            let (source, generation) = (job.source.clone(), job.generation);
            let done = job.join();
            if generation == self.generation {
                changed |= self.apply(&source, done);
            }
        }
        changed
    }
    fn apply(&mut self, source: &str, done: Done) -> bool {
        let Some(index) = self.config.sources.iter().position(|s| s.id == source) else {
            return false;
        };
        match done {
            Done::Connected {
                host_id,
                device_label,
                directory,
            } => {
                let s = &mut self.config.sources[index];
                s.host_id = host_id;
                s.device_label = device_label;
                s.connected_at = now();
                s.state = "connected".into();
                s.error = None;
                self.next_directory = now() + DIRECTORY_EVERY;
                match directory {
                    Ok(directory) => self.take(index, directory),
                    Err(fail) => self.broke(index, &fail),
                }
                true
            }
            Done::Directory(directory) => {
                self.take(index, directory);
                true
            }
            Done::Lease { key, lease } => {
                self.keep(key, lease);
                false
            }
            Done::Native {
                key,
                provider,
                lease,
            } => {
                self.written
                    .insert(provider, (lease.expires_at, lease.version));
                self.keep(key, lease);
                false
            }
            Done::Reported => false,
            Done::Failed { key: None, fail } => {
                self.broke(index, &fail);
                true
            }
            Done::Failed {
                key: Some(key),
                fail,
            } => {
                let label = self.config.sources[index].label.clone();
                self.failed
                    .insert(key.clone(), (now(), fail.message(&label)));
                match fail {
                    Fail::Unpaired => {
                        self.broke(index, &fail);
                        true
                    }
                    // Until the next directory read says the same.
                    Fail::Code(code) => {
                        let account_ref = key.split_once(':').map_or("", |(_, r)| r);
                        let Some(entry) = self.config.sources[index]
                            .directory
                            .iter_mut()
                            .find(|e| e["account_ref"] == account_ref)
                        else {
                            return false;
                        };
                        match code.as_str() {
                            "account_not_quota_readable" => entry["quota_readable"] = json!(false),
                            "account_disabled" => entry["status"] = json!("disabled"),
                            "needs_reauthorization"
                            | "reauthorization_required"
                            | "refresh_rejected" => {
                                entry["status"] = json!("needs_reauthorization")
                            }
                            _ => return false,
                        }
                        true
                    }
                    _ => false,
                }
            }
        }
    }
    fn take(&mut self, index: usize, directory: Directory) {
        let s = &mut self.config.sources[index];
        s.label = directory.label;
        s.manage_url = directory.manage_url;
        s.directory = directory.accounts;
        s.read_at = Some(directory.read_at);
        s.state = "connected".into();
        s.error = None;
    }
    fn broke(&mut self, index: usize, fail: &Fail) {
        let s = &mut self.config.sources[index];
        s.state = "error".into();
        s.error = Some(fail.message(&s.label));
    }
    /// A newer lease replaces the one held; it keeps when access was last asked for to be used.
    fn keep(&mut self, key: String, mut lease: Lease) {
        if let Some(old) = self.leases.get(&key) {
            if old.metadata.owner != lease.metadata.owner {
                self.failed
                    .insert(key, (now(), IDENTITY_CHANGED.to_string()));
                return;
            }
            lease.used_at = lease.used_at.max(old.used_at);
        }
        self.failed.remove(&key);
        self.leases.insert(key, lease);
    }
    /// Deletes a source's pairing from the Vault; the center revokes the device on its own page.
    pub fn forget_pairing(&self, platform: &Platform, source: &str) -> Result<()> {
        platform.delete_key(VAULT, &vault_key(source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn encode(value: Value) -> String {
        URL_SAFE_NO_PAD.encode(value.to_string())
    }
    #[test]
    fn pairing_strings_decode_and_bad_ones_say_how_to_get_a_new_one() {
        let good = json!({"site_url":"https://lab.example/","code":"pair-123","access_client_id":"synthetic.access","access_client_secret":"SYNTHETIC-secret"});
        let e = Enrollment::decode(&format!("  {}==\n", encode(good.clone()))).unwrap();
        assert_eq!(e.site_url, "https://lab.example");
        assert_eq!(e.code, "pair-123");
        for bad in [
            String::new(),
            "not base64!".into(),
            URL_SAFE_NO_PAD.encode("not json"),
            encode(json!({"site_url":"https://lab.example","code":"x"})),
            encode(
                json!({"site_url":"http://lab.example","code":"x","access_client_id":"a","access_client_secret":"b"}),
            ),
            encode(
                json!({"site_url":"https://lab.example/path","code":"x","access_client_id":"a","access_client_secret":"b"}),
            ),
            encode(
                json!({"site_url":"https://lab.example","code":"x\ny","access_client_id":"a","access_client_secret":"b"}),
            ),
            encode(
                json!({"site_url":"https://lab.example","code":"x","access_client_id":"a b","access_client_secret":"b"}),
            ),
        ] {
            assert!(Enrollment::decode(&bad).is_err(), "{bad}");
        }
        assert_eq!(Enrollment::decode("@@").err(), Some(BAD_ENROLLMENT));
    }
    #[test]
    fn center_refusals_map_to_fixed_codes_and_never_carry_upstream_text() {
        assert_eq!(classify(&json!({"code":"device_revoked"})), Fail::Unpaired);
        assert_eq!(
            classify(&json!({"code":"device_key_invalid"})),
            Fail::Unpaired
        );
        assert_eq!(
            classify(&json!({"code":"account_not_found","detail":"SYNTHETIC upstream"})),
            Fail::Code("account_not_found".into())
        );
        assert_eq!(
            classify(&json!({"code":"Upstream said <html>"})),
            Fail::Unreachable(INVALID)
        );
        assert_eq!(
            Fail::Unpaired.message("lab-center"),
            "lab-center 的配对已失效，暂时拉不到最新额度，也不能给外部账号用重置卡。"
        );
        assert_eq!(
            Fail::Unreachable(NETWORK).message("lab-center"),
            "连不上 lab-center：网络不通或超时"
        );
        assert_eq!(
            Fail::Code("weird_state".into()).message("lab-center"),
            "来源出错（weird_state）"
        );
    }
}
