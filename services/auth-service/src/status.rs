use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const FILE_LIMIT: u64 = 1024 * 1024;
const CLI_LIMIT: u64 = 64 * 1024;

#[derive(Clone)]
pub struct Sources {
    pub codex_home: PathBuf,
    pub pi_home: PathBuf,
    pub claude_bin: Option<PathBuf>,
}

#[derive(Serialize, Debug)]
pub struct ProviderStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub state: &'static str,
    pub auth_kind: String,
    pub summary: String,
    pub source: String,
    pub expires_at: Option<u64>,
    pub details: Vec<String>,
}

#[derive(Serialize)]
pub struct Snapshot {
    pub checked_at: u64,
    pub service_version: &'static str,
    pub providers: Vec<ProviderStatus>,
    pub verification: &'static str,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub fn snapshot(sources: &Sources) -> Snapshot {
    let checked_at = now();
    Snapshot {
        checked_at,
        service_version: env!("CARGO_PKG_VERSION"),
        providers: vec![
            codex_status(&sources.codex_home.join("auth.json"), checked_at),
            claude_status(sources.claude_bin.as_deref()),
            pi_status(&sources.pi_home.join("auth.json"), checked_at),
        ],
        verification: "仅检查本地凭据和 Claude CLI 登录状态，未请求模型，也未验证刷新能力。",
    }
}

fn base(id: &'static str, name: &'static str, source: &Path) -> ProviderStatus {
    ProviderStatus {
        id,
        name,
        state: "missing",
        auth_kind: "未配置".into(),
        summary: "未找到本地凭据".into(),
        source: source.display().to_string(),
        expires_at: None,
        details: vec![],
    }
}

fn read_auth(path: &Path) -> Result<Value, &'static str> {
    let file = fs::File::open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "missing",
        _ => "unreadable",
    })?;
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "unreadable")?;
    if bytes.len() as u64 > FILE_LIMIT {
        return Err("invalid");
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid")?;
    if !value.is_object() {
        return Err("invalid");
    }
    Ok(value)
}

fn file_error(mut status: ProviderStatus, error: &str) -> ProviderStatus {
    if error != "missing" {
        status.state = "issue";
        status.summary = match error {
            "unreadable" => "无法读取认证文件，请检查文件权限",
            _ => "认证文件格式无法识别",
        }
        .into();
    }
    status
}

fn present(value: &Value, pointer: &str) -> bool {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}

fn jwt_exp(token: &str) -> Option<u64> {
    let mut parts = token.split('.');
    parts.next()?;
    let payload = parts.next()?;
    parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get("exp")?
        .as_u64()
}

fn token_state(status: &mut ProviderStatus, expires_at: Option<u64>, checked_at: u64) {
    status.expires_at = expires_at;
    let expired = expires_at.is_some_and(|expiry| expiry <= checked_at);
    status.state = if expired { "expired" } else { "configured" };
    status.summary = if expired {
        "本地 access token 已到期；尚未检查是否可续期"
    } else if expires_at.is_some() {
        "本地 access token 尚未到期"
    } else {
        "已检测到凭据；无法从本地确定有效期"
    }
    .into();
}

pub fn codex_status(path: &Path, checked_at: u64) -> ProviderStatus {
    let mut status = base("codex", "Codex", path);
    let auth = match read_auth(path) {
        Ok(auth) => auth,
        Err(error) => return file_error(status, error),
    };
    // Managed ChatGPT auth is selected explicitly before checking API-key fields.
    let mode = auth.get("auth_mode").and_then(Value::as_str);
    if matches!(mode, Some("chatgpt" | "chatgptAuthTokens"))
        || (mode.is_none() && present(&auth, "/tokens/access_token"))
    {
        status.auth_kind = "ChatGPT OAuth".into();
        if !present(&auth, "/tokens/access_token") || !present(&auth, "/tokens/account_id") {
            status.state = "issue";
            status.summary = "ChatGPT 凭据缺少必要字段".into();
            return status;
        }
        let expiry = jwt_exp(auth["tokens"]["access_token"].as_str().unwrap());
        token_state(&mut status, expiry, checked_at);
        status.details.push(
            if present(&auth, "/tokens/refresh_token") {
                "检测到刷新凭据；未尝试刷新"
            } else {
                "未检测到刷新凭据"
            }
            .into(),
        );
        status.details.push("JWT 到期信息未作远端或签名验证".into());
    } else if matches!(mode, None | Some("apikey")) && present(&auth, "/OPENAI_API_KEY") {
        status.auth_kind = "API key".into();
        status.state = "configured";
        status.summary = "已检测到 API key；未验证远端有效性".into();
    } else {
        status.state = "issue";
        status.summary = "认证模式不在本版检查范围内".into();
    }
    status
}

pub fn pi_status(path: &Path, checked_at: u64) -> ProviderStatus {
    let mut status = base("pi", "pi · OpenAI Codex", path);
    if path
        .parent()
        .is_some_and(|root| root.join("extensions/authmanager-codex/index.ts").is_file())
    {
        status.state = "configured";
        status.auth_kind = "AuthBaton 本机服务".into();
        status.summary = "已安装统一账号插件；在 pi 选择 AuthManager Codex provider 使用".into();
        status
            .details
            .push("共享账号管理中的 Codex 账号；默认跟随全局当前，任务进行中固定身份".into());
        status
            .details
            .push("pi 按需取得 access token；认证检查与续期仍由 AuthBaton 协调".into());
        return status;
    }
    let auth = match read_auth(path) {
        Ok(auth) => auth,
        Err(error) => return file_error(status, error),
    };
    let Some(credential) = auth.get("openai-codex") else {
        status.summary = "尚未配置 openai-codex provider".into();
        status
            .details
            .push("其他 provider 的凭据未纳入本页检查".into());
        return status;
    };
    status.auth_kind = "ChatGPT OAuth".into();
    let expiry = credential
        .get("expires")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0 && *v < u64::MAX as f64)
        .map(|milliseconds| (milliseconds / 1000.0).floor() as u64);
    if credential.get("type").and_then(Value::as_str) != Some("oauth")
        || !present(credential, "/access")
        || !present(credential, "/refresh")
        || expiry.is_none()
    {
        status.state = "issue";
        status.summary = "openai-codex 凭据格式不符合 OAuth 结构".into();
        return status;
    }
    token_state(&mut status, expiry, checked_at);
    status
        .details
        .push("当前仍使用 pi 原生凭据；统一取 token 尚未接入".into());
    status
}

fn run_claude_status(binary: &Path) -> Result<Value, &'static str> {
    let mut child = Command::new(binary)
        .args(["auth", "status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "无法启动 Claude CLI")?;
    let stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(CLI_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Claude CLI 状态检查超时或无法完成");
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| "无法读取 Claude CLI 状态")?
        .map_err(|_| "无法读取 Claude CLI 状态")?;
    if bytes.len() as u64 > CLI_LIMIT {
        return Err("Claude CLI 状态输出过大");
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Claude CLI 未返回有效状态")?;
    // A signed-out CLI can exit nonzero; only accept its explicit boolean status.
    if !exit.success() && value.get("loggedIn").and_then(Value::as_bool) != Some(false) {
        return Err("Claude CLI 状态检查失败");
    }
    Ok(value)
}

fn claude_payload(value: &Value, binary: &Path) -> ProviderStatus {
    let mut status = base("claude", "Claude Code", binary);
    match value.get("loggedIn").and_then(Value::as_bool) {
        Some(true) => {
            status.state = "configured";
            status.summary = "Claude CLI 报告已登录".into();
            status.auth_kind = match value.get("authMethod").and_then(Value::as_str) {
                Some("claude.ai") => "Claude 订阅",
                Some("api_key") => "API key",
                Some("oauth_token") => "OAuth token",
                _ => "CLI 管理的认证",
            }
            .into();
            let plan = value.get("subscriptionType").and_then(Value::as_str);
            if let Some(plan @ ("free" | "pro" | "max" | "team" | "enterprise")) = plan {
                status.details.push(format!("订阅类型：{plan}"));
            }
            status
                .details
                .push("原生登录由 Claude Code 管理；未读取 Keychain 内容".into());
        }
        Some(false) => status.summary = "Claude CLI 报告尚未登录".into(),
        None => {
            status.state = "issue";
            status.summary = "Claude CLI 状态缺少登录标识".into();
        }
    }
    status
}

fn claude_status(binary: Option<&Path>) -> ProviderStatus {
    let Some(binary) = binary else {
        let mut status = base("claude", "Claude Code", Path::new("claude"));
        status.state = "unavailable";
        status.summary = "未找到 Claude CLI".into();
        return status;
    };
    match run_claude_status(binary) {
        Ok(value) => claude_payload(&value, binary),
        Err(message) => {
            let mut status = base("claude", "Claude Code", binary);
            status.state = "issue";
            status.summary = message.into();
            status
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn fixture(value: &Value) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "auth-manager-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }

    #[test]
    fn codex_metadata_does_not_expose_secrets() {
        let access = format!(
            "e30.{}.test-signature",
            URL_SAFE_NO_PAD.encode(br#"{"exp":2000}"#)
        );
        let path = fixture(
            &json!({"auth_mode":"chatgpt", "tokens":{"access_token":access,"refresh_token":"synthetic-refresh-secret","account_id":"synthetic-account","id_token":"synthetic-id-secret"}}),
        );
        let status = codex_status(&path, 1000);
        assert_eq!(status.state, "configured");
        assert_eq!(status.expires_at, Some(2000));
        let output = serde_json::to_string(&status).unwrap();
        for secret in [
            &access,
            "synthetic-refresh-secret",
            "synthetic-account",
            "synthetic-id-secret",
        ] {
            assert!(!output.contains(secret));
        }
        assert_eq!(codex_status(&path, 2000).state, "expired");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn pi_accepts_fractional_millisecond_expiry() {
        let path = fixture(
            &json!({"openai-codex":{"type":"oauth","access":"secret-a","refresh":"secret-r","expires":2000000.438}}),
        );
        let status = pi_status(&path, 1000);
        assert_eq!(status.expires_at, Some(2000));
        assert_eq!(status.state, "configured");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_file_does_not_echo_content() {
        let path = fixture(&json!({}));
        fs::write(&path, "not-json synthetic-secret-value").unwrap();
        let status = codex_status(&path, 0);
        assert_eq!(status.state, "issue");
        assert!(
            !serde_json::to_string(&status)
                .unwrap()
                .contains("synthetic-secret-value")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn claude_whitelist_ignores_identity_and_unexpected_fields() {
        let status = claude_payload(
            &json!({"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"team","email":"private@example.test","accessToken":"synthetic-secret"}),
            Path::new("claude"),
        );
        let output = serde_json::to_string(&status).unwrap();
        assert!(!output.contains("private@example.test"));
        assert!(!output.contains("synthetic-secret"));
        assert_eq!(status.auth_kind, "Claude 订阅");
    }
}
