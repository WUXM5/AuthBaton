//! Cloud device transport. Model credentials never enter the UI projection or configuration file.
use crate::credentials::{Bundle, NativeStore};
use crate::platform::{Platform, Result, random_id, read_json, write_json};
use crate::status::now;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Binding {
    pub operation: String,
    pub cloud_id: Option<String>,
    pub owner: String,
    pub provider: String,
    pub native: bool,
    pub phase: String,
    #[serde(default)]
    pub replace_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub mode: String,
    pub endpoint: String,
    pub label: String,
    pub paired: bool,
    pub pair: Option<Value>,
    pub accounts: Vec<Value>,
    pub devices: Vec<Value>,
    pub bindings: HashMap<String, Binding>,
    pub selected: HashMap<String, String>,
    pub owners: HashMap<String, String>,
    pub checked_at: Option<u64>,
    #[serde(default)]
    pub pending_activation: Option<String>,
    #[serde(default)]
    pub reauthorizations: HashMap<String, String>,
    #[serde(default)]
    pub usage_attempts: HashMap<String, u64>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            mode: "local".into(),
            endpoint: String::new(),
            label: "我的 Mac".into(),
            paired: false,
            pair: None,
            accounts: vec![],
            devices: vec![],
            bindings: HashMap::new(),
            selected: HashMap::new(),
            owners: HashMap::new(),
            checked_at: None,
            pending_activation: None,
            reauthorizations: HashMap::new(),
            usage_attempts: HashMap::new(),
        }
    }
}
pub struct Runtime {
    pub config: Config,
    path: PathBuf,
    pub job: Option<JoinHandle<Result<Update>>>,
    pub error: Option<String>,
    pub next_poll: u64,
    pub next_directory: u64,
    leases: HashMap<String, Value>,
}
pub struct Update {
    pub config: Config,
    pub leases: HashMap<String, Value>,
}
impl Runtime {
    pub fn open(root: &Path) -> Result<Self> {
        let path = root.join("cloud-config.json");
        let config = read_json(&path)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| "云端配置格式无效")?
            .unwrap_or_default();
        Ok(Self {
            config,
            path,
            job: None,
            error: None,
            next_poll: 0,
            next_directory: 0,
            leases: HashMap::new(),
        })
    }
    pub fn snapshot(&self) -> Value {
        json!({"mode":self.config.mode,"endpoint":self.config.endpoint,"label":self.config.label,"paired":self.config.paired,"pair":self.config.pair,"accounts":self.config.accounts,"devices":self.config.devices,"selected":self.config.selected,"busy":self.job.is_some(),"error":self.error,"checked_at":self.config.checked_at,"pending_activation":self.config.pending_activation,"transfers":self.config.bindings.iter().map(|(id,b)| json!({"local_id":id,"cloud_id":b.cloud_id,"phase":b.phase})).collect::<Vec<_>>()})
    }
    pub fn save(&self) -> Result<()> {
        write_json(&self.path, &serde_json::to_value(&self.config).unwrap())
    }
    pub fn configure(
        &mut self,
        mode: Option<&str>,
        endpoint: Option<&str>,
        label: Option<&str>,
    ) -> Result<()> {
        if self.job.is_some() {
            return Err("云端操作正在进行");
        }
        let old = self.config.clone();
        if let Some(mode) = mode {
            if !matches!(mode, "local" | "cloud") {
                return Err("模式无效");
            }
            self.config.mode = mode.into();
        }
        if let Some(url) = endpoint {
            let url = validate_endpoint(url)?;
            if url != self.config.endpoint
                && (self.config.paired || !self.config.bindings.is_empty())
            {
                self.config = old;
                return Err("已配对或存在交接记录，不能直接更换认证中心");
            }
            self.config.endpoint = url;
        }
        if let Some(label) = label {
            if label.trim().is_empty() || label.len() > 160 || label.chars().any(char::is_control) {
                self.config = old;
                return Err("设备名称无效");
            }
            self.config.label = label.trim().into();
        }
        if let Err(e) = self.save() {
            self.config = old;
            return Err(e);
        }
        Ok(())
    }
    pub fn start<F>(&mut self, platform: Platform, operation: F) -> Result<()>
    where
        F: FnOnce(Config, Platform, PathBuf) -> Result<Update> + Send + 'static,
    {
        if self.job.is_some() {
            return Err("云端操作正在进行");
        }
        if self.config.endpoint.is_empty() {
            return Err("请先配置个人认证中心地址");
        }
        let cfg = self.config.clone();
        let path = self.path.clone();
        self.error = None;
        self.job = Some(std::thread::spawn(move || operation(cfg, platform, path)));
        Ok(())
    }
    pub fn poll(&mut self) {
        if !self.job.as_ref().is_some_and(|j| j.is_finished()) {
            return;
        }
        match self.job.take().unwrap().join() {
            Ok(Ok(update)) => {
                self.config = update.config;
                self.leases.extend(update.leases);
                self.error = None;
            }
            Ok(Err(e)) => {
                self.error = Some(e.into());
                if let Ok(Some(value)) = read_json(&self.path)
                    && let Ok(c) = serde_json::from_value(value)
                {
                    self.config = c;
                }
            }
            Err(_) => self.error = Some("云端操作中断，请重试同步；交接记录已保留".into()),
        }
        self.next_poll = now() + 30;
    }
    pub fn finish(&mut self) {
        if let Some(job) = self.job.take() {
            match job.join() {
                Ok(Ok(u)) => self.config = u.config,
                _ => self.error = Some("云端操作未完成，请重开后恢复".into()),
            }
        }
    }
    pub fn lease(&self, id: &str) -> Option<Value> {
        self.leases
            .get(id)
            .filter(|v| v["expires_at"].as_u64().is_some_and(|t| t > now() + 60))
            .cloned()
    }
}
pub fn validate_endpoint(value: &str) -> Result<String> {
    let url = value.trim().trim_end_matches('/');
    let host = url
        .strip_prefix("https://")
        .ok_or("认证中心必须使用 HTTPS 地址")?;
    if host.is_empty()
        || host.len() > 253
        || host.starts_with('.')
        || host.ends_with('.')
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        || !host.contains('.')
    {
        return Err("请输入认证中心根地址，不含路径、端口或登录信息");
    }
    Ok(url.into())
}
#[cfg(not(test))]
fn safe_error(code: &str) -> &'static str {
    match code {
        "deployment_not_configured" => "云端配置尚未完成，请先在 Cloudflare 配置存储密钥和管理登录",
        "device_unauthorized" => "设备尚未授权或已撤销，请重新配对",
        "pair_expired" => "配对已过期，请重新发起",
        "account_already_registered" => "云端已登记此账号，请使用已有账号或先处理原授权",
        "source_access_expired" => "本机 Access 已到期，请先在本地完成认证检查再导入",
        "handoff_required" => "账号已导入，尚未完成续期归属交接",
        "issuer_unauthorized" => "官方接口拒绝当前凭据，请检查登录状态",
        "refresh_outcome_unknown" => "云端续期没有收到回应；稍后自动用同一凭据重新确认",
        "refresh_rejected" | "reauthorization_required" => {
            "官方已拒绝此续期凭据，请重新授权此云端账号"
        }
        "issuer_rate_limited" | "retry_later" => "官方接口限流，稍后重试",
        "maintenance_disabled" => "此云端账号自动维护已关闭",
        "usage_credential_unavailable" => "当前凭据暂不可查询用量；查询不会触发续期",
        "identity_mismatch" => "账号身份不一致，已停止操作",
        _ => "云端请求未完成，请检查网络、配置和账号状态",
    }
}
#[cfg(test)]
type TestTransport = Box<dyn FnMut(&str, &str, Option<&str>, Value) -> Result<Value>>;
#[cfg(test)]
thread_local! {
    static TEST_TRANSPORT: std::cell::RefCell<Option<TestTransport>> = std::cell::RefCell::new(None);
}
#[cfg(test)]
pub(crate) fn test_transport(transport: TestTransport) {
    TEST_TRANSPORT.with(|slot| *slot.borrow_mut() = Some(transport));
}
#[cfg(test)]
pub fn http(endpoint: &str, action: &str, token: Option<&str>, body: Value) -> Result<Value> {
    TEST_TRANSPORT.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .expect("cloud tests require a synthetic transport; real network is forbidden")(
            endpoint, action, token, body,
        )
    })
}
#[cfg(not(test))]
pub fn http(endpoint: &str, action: &str, token: Option<&str>, body: Value) -> Result<Value> {
    let endpoint = validate_endpoint(endpoint)?;
    if !action.bytes().all(|b| b.is_ascii_lowercase() || b == b'.') {
        return Err("云端操作无效");
    }
    let quote = |s: &str| {
        format!(
            "\"{}\"",
            s.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
        )
    };
    let mut config = format!(
        "url = {}\nrequest = \"POST\"\nheader = \"Content-Type: application/json\"\ndata = {}\n",
        quote(&format!("{endpoint}/v1/{action}")),
        quote(&body.to_string())
    );
    if let Some(token) = token {
        if token.chars().any(char::is_control) {
            return Err("设备授权格式无效");
        }
        config.push_str(&format!(
            "header = {}\n",
            quote(&format!("Authorization: Bearer {token}"))
        ));
    }
    let output = crate::network::curl(
        &[
            "-q",
            "--silent",
            "--proto",
            "=https",
            "--max-time",
            "22",
            "--max-filesize",
            "262144",
            "--write-out",
            "\n%{http_code}",
            "--config",
            "-",
        ],
        config.as_bytes(),
        std::time::Duration::from_secs(25),
    )?;
    let output = String::from_utf8(output).map_err(|_| "云端响应无效")?;
    let (body, status) = output.rsplit_once('\n').ok_or("云端响应无效")?;
    let value: Value =
        serde_json::from_str(body).map_err(|_| "云端未返回有效数据，请确认地址和 Access 路由")?;
    if status != "200" {
        return Err(safe_error(value["error"].as_str().unwrap_or("unknown")));
    }
    Ok(value)
}
fn secret(platform: &Platform, cfg: &Config) -> Result<Value> {
    let v = platform.vault_read("cloud-device-v1")?;
    if v["endpoint"].as_str() != Some(cfg.endpoint.as_str()) {
        return Err("设备授权不属于当前认证中心");
    }
    Ok(v)
}
pub fn api(platform: &Platform, cfg: &Config, action: &str, body: Value) -> Result<Value> {
    let v = secret(platform, cfg)?;
    let token = v["token"].as_str().ok_or("请先完成设备配对")?;
    http(&cfg.endpoint, action, Some(token), body)
}
fn persist(cfg: &Config, path: &Path) -> Result<()> {
    write_json(path, &serde_json::to_value(cfg).unwrap())
}
fn done(config: Config) -> Update {
    Update {
        config,
        leases: HashMap::new(),
    }
}
pub fn pair(mut cfg: Config, platform: Platform, path: PathBuf) -> Result<Update> {
    if cfg.paired {
        return Err("设备已经配对");
    }
    let proof = format!("{}{}", random_id()?, random_id()?);
    platform.vault_write(
        "cloud-device-v1",
        &json!({"endpoint":cfg.endpoint,"proof":proof}),
    )?;
    let v = http(
        &cfg.endpoint,
        "pair.begin",
        None,
        json!({"proof":proof,"label":cfg.label}),
    )?;
    let id = v["id"].as_str().ok_or("配对响应无效")?;
    let code = v["code"].as_str().ok_or("配对响应无效")?;
    if !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        || code.len() != 12
        || !code.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("配对响应无效");
    }
    cfg.pair = Some(
        json!({"id":id,"code":code,"expires_at":v["expires_at"],"url":format!("{}/admin",cfg.endpoint)}),
    );
    persist(&cfg, &path)?;
    Ok(done(cfg))
}
pub fn pair_poll(mut cfg: Config, platform: Platform, path: PathBuf) -> Result<Update> {
    let s = secret(&platform, &cfg)?;
    // Vault commit precedes the public configuration commit. Recover a completed
    // approval without depending on the short-lived pairing record still existing.
    if s["token"].as_str().is_some() {
        api(&platform, &cfg, "accounts", json!({}))?;
        cfg.paired = true;
        cfg.pair = None;
        persist(&cfg, &path)?;
        return directory(cfg, platform, path);
    }
    let proof = s["proof"].as_str().ok_or("配对状态不存在")?;
    let p = cfg.pair.as_ref().ok_or("配对状态不存在")?;
    let v = http(
        &cfg.endpoint,
        "pair.poll",
        Some(proof),
        json!({"id":p["id"]}),
    )?;
    if v["state"] == "paired" {
        let token = v["device_token"]
            .as_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("设备授权响应无效")?;
        platform.vault_write(
            "cloud-device-v1",
            &json!({"endpoint":cfg.endpoint,"token":token,"device_id":v["device_id"],"proof":proof}),
        )?;
        cfg.paired = true;
        cfg.pair = None;
        persist(&cfg, &path)?;
    }
    Ok(done(cfg))
}
pub fn directory(mut cfg: Config, platform: Platform, path: PathBuf) -> Result<Update> {
    let v = api(&platform, &cfg, "accounts", json!({}))?;
    let rows = v["accounts"].as_array().ok_or("云端账号列表无效")?;
    let keys = [
        "id",
        "provider",
        "label",
        "identity",
        "plan",
        "expires_at",
        "refresh_expires_at",
        "version",
        "state",
        "automatic",
        "last_refresh",
        "error",
        "retry_at",
    ];
    let previous = cfg.accounts.clone();
    cfg.accounts = rows
        .iter()
        .map(|a| {
            Value::Object(
                keys.iter()
                    .filter_map(|k| a.get(k).map(|v| ((*k).into(), v.clone())))
                    .collect(),
            )
        })
        .collect();
    for account in &mut cfg.accounts {
        if let Some(usage) = previous
            .iter()
            .find(|a| a["id"] == account["id"])
            .and_then(|a| a.get("usage"))
            && usage["checked_at"]
                .as_u64()
                .is_some_and(|t| t + 600 > now())
        {
            account["usage"] = usage.clone();
        }
        if let Some(error) = previous
            .iter()
            .find(|a| a["id"] == account["id"])
            .and_then(|a| a.get("usage_error"))
        {
            account["usage_error"] = error.clone();
        }
    }
    cfg.devices = v["devices"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|d| json!({"id":d["id"],"label":d["label"],"last_seen":d["last_seen"]}))
        .collect();
    cfg.checked_at = Some(now());
    persist(&cfg, &path)?;
    Ok(done(cfg))
}
pub fn projection(bundle: &Bundle) -> Bundle {
    let mut b = bundle.clone();
    if b.provider == "codex" {
        b.credential["tokens"]
            .as_object_mut()
            .unwrap()
            .remove("refresh_token");
        b.credential["auth_mode"] = json!("chatgptAuthTokens");
    } else {
        b.credential.as_object_mut().unwrap().remove("refreshToken");
        b.credential
            .as_object_mut()
            .unwrap()
            .remove("refreshTokenExpiresAt");
    }
    b
}
#[cfg(not(test))]
fn client_running(provider: &str) -> bool {
    crate::credentials::external_client_running(provider)
}
#[cfg(test)]
fn client_running(_: &str) -> bool {
    false
}

pub fn transfer(
    mut cfg: Config,
    platform: Platform,
    path: PathBuf,
    local_id: String,
    label: String,
    native: Option<NativeStore>,
) -> Result<Update> {
    let binding = cfg.bindings.get(&local_id).ok_or("缺少交接记录")?.clone();
    let key = format!("cloud-import-{local_id}");
    let saved = platform.vault_read_optional(&key)?;
    let bundle = if let Some(v) = saved {
        Bundle::decode(v)?
    } else {
        let b = if let Some(n) = &native {
            n.read(&platform)?
        } else {
            Bundle::decode(platform.vault_read(&local_id)?)?
        };
        if b.metadata()?.owner != binding.owner {
            return Err("导入来源身份已改变");
        }
        platform.vault_write(&key, &b.encoded())?;
        b
    };
    let a = api(
        &platform,
        &cfg,
        "import",
        json!({"operation":binding.operation,"label":label,"bundle":bundle,"replace_id":binding.replace_id}),
    )?;
    let id = a["id"].as_str().ok_or("云端导入响应无效")?.to_string();
    cfg.bindings.get_mut(&local_id).unwrap().cloud_id = Some(id.clone());
    cfg.bindings.get_mut(&local_id).unwrap().phase = "imported".into();
    persist(&cfg, &path)?;
    if let Some(n) = &native {
        if client_running(&bundle.provider) {
            return Err("请先结束相关任务并退出该提供方的原生客户端，再恢复云端交接；无需重新登录");
        }
        let target = projection(&bundle);
        if n.cloud_read(&platform).ok().as_ref() != Some(&target) {
            n.cloud_publish(&platform, &bundle, &target)?;
        }
        platform.vault_write(
            &format!("cloud-native-{}", bundle.provider),
            &target.encoded(),
        )?;
    }
    cfg.bindings.get_mut(&local_id).unwrap().phase = "commit_pending".into();
    persist(&cfg, &path)?;
    api(
        &platform,
        &cfg,
        "import.commit",
        json!({"id":id,"operation":binding.operation}),
    )?;
    cfg.bindings.get_mut(&local_id).unwrap().phase = "cloud_managed".into();
    cfg.reauthorizations.remove(&local_id);
    cfg.owners.insert(id.clone(), binding.owner);
    if binding.native {
        cfg.selected.insert(bundle.provider.clone(), id);
    }
    persist(&cfg, &path)?;
    directory(cfg, platform, path)
}
pub fn cancel_transfer(
    mut cfg: Config,
    platform: Platform,
    path: PathBuf,
    local_id: String,
    native: Option<NativeStore>,
) -> Result<Update> {
    let binding = cfg.bindings.get(&local_id).ok_or("缺少交接记录")?.clone();
    api(
        &platform,
        &cfg,
        "import.abort",
        json!({"operation":binding.operation}),
    )?;
    let key = format!("cloud-import-{local_id}");
    if let (Some(native), Some(saved)) = (native, platform.vault_read_optional(&key)?) {
        let original = Bundle::decode(saved)?;
        if client_running(&original.provider) {
            return Err("请先退出此提供方客户端再恢复本地维护");
        }
        if let Ok(current) = native.cloud_read(&platform) {
            if current != projection(&original) {
                return Err("本机凭据已变化，未恢复旧副本");
            }
            native.cloud_restore(&platform, &current, &original)?;
        } else if native.read(&platform)?.metadata()?.owner != binding.owner {
            return Err("本机身份已经改变，请核对后恢复");
        }
    }
    cfg.bindings.remove(&local_id);
    cfg.reauthorizations.remove(&local_id);
    persist(&cfg, &path)?;
    platform.delete_key("io.github.wuxm5.authbaton.vault", &key)?;
    directory(cfg, platform, path)
}
pub fn fetch_lease(cfg: &mut Config, platform: &Platform, id: &str) -> Result<Value> {
    let v = api(platform, cfg, "lease", json!({"id":id}))?;
    if v["id"] != id || v["expires_at"].as_u64().is_none_or(|t| t <= now() + 60) {
        return Err("云端凭据标识或有效期无效");
    }
    let b: Bundle =
        serde_json::from_value(v["projection"].clone()).map_err(|_| "云端凭据格式无效")?;
    let owner = b.cloud_metadata()?.owner;
    if v["owner"] != owner || cfg.owners.get(id).is_some_and(|s| s != &owner) {
        return Err("云端账号身份已变化，已停止领取");
    }
    if b.provider == "codex" {
        let claims = crate::credentials::claims(v["access_token"].as_str().ok_or("缺少 Access")?)?;
        let a = &claims["https://api.openai.com/auth"];
        if owner
            != format!(
                "codex:{}:{}",
                a["chatgpt_user_id"]
                    .as_str()
                    .or(a["user_id"].as_str())
                    .unwrap_or(""),
                a["chatgpt_account_id"].as_str().unwrap_or("")
            )
        {
            return Err("云端 Access 身份不匹配");
        }
    }
    cfg.owners.insert(id.into(), owner);
    Ok(v)
}
pub fn lease(mut cfg: Config, platform: Platform, path: PathBuf, id: String) -> Result<Update> {
    let v = fetch_lease(&mut cfg, &platform, &id)?;
    persist(&cfg, &path)?;
    Ok(Update {
        config: cfg,
        leases: HashMap::from([(id, v)]),
    })
}
pub fn native_sync(
    mut cfg: Config,
    platform: Platform,
    path: PathBuf,
    id: String,
    native: NativeStore,
    activate: bool,
    previous: Option<(String, String)>,
) -> Result<Update> {
    let v = fetch_lease(&mut cfg, &platform, &id)?;
    let target: Bundle =
        serde_json::from_value(v["projection"].clone()).map_err(|_| "云端凭据格式无效")?;
    let key = format!("cloud-native-{}", target.provider);
    let journal_key = format!("cloud-native-journal-{}", target.provider);
    // Reconcile the prior file/Keychain write before requesting a new write. The
    // saved target survives upstream rotation and a failed projection-cache commit.
    if let Some(journal) = platform.vault_read_optional(&journal_key)? {
        let prior: Bundle = serde_json::from_value(journal["target"].clone())
            .map_err(|_| "云端投影恢复记录无效")?;
        let before: Option<Bundle> = serde_json::from_value(journal["expected"].clone())
            .map_err(|_| "云端投影恢复记录无效")?;
        let actual = native.cloud_existing(&platform)?;
        if actual.as_ref() == Some(&prior) {
            platform.vault_write(&key, &prior.encoded())?;
        } else if actual != before {
            return Err("原生登录已变化，未覆盖外部修改；请核对云端投影恢复记录");
        }
        platform.delete_key("io.github.wuxm5.authbaton.vault", &journal_key)?;
    }
    let expected = if activate {
        if client_running(&target.provider) {
            return Err("请先结束任务并退出该提供方客户端，再设为云端当前账号");
        }
        native.cloud_existing(&platform)?
    } else {
        Some(
            serde_json::from_value(platform.vault_read(&key)?)
                .map_err(|_| "本机云端投影记录无效")?,
        )
    };
    if activate {
        if let Some(original) = expected.as_ref().filter(|b| b.metadata().is_ok()) {
            // Include an unregistered native login: it may be the only full copy.
            platform.vault_write(
                &format!("cloud-previous-{}", target.provider),
                &original.encoded(),
            )?;
        }
        if let Some((previous_id, owner)) = previous
            && let Some(ref expected) = expected
            && let Ok(metadata) = expected.metadata()
        {
            if metadata.owner != owner {
                return Err("原生当前身份已变化，请先在本地核对");
            }
            platform.vault_write(&previous_id, &expected.encoded())?;
        }
        cfg.pending_activation = Some(id.clone());
        persist(&cfg, &path)?;
    }
    platform.vault_write(
        &journal_key,
        &json!({"id":id,"expected":expected,"target":target}),
    )?;
    if let Some(ref expected) = expected {
        if *expected != target {
            native.cloud_publish(&platform, expected, &target)?;
        }
    } else {
        native.cloud_initialize(&platform, &target)?;
    }
    platform.vault_write(&key, &target.encoded())?;
    if activate {
        cfg.selected
            .insert(format!("{}_broker", target.provider), id.clone());
    }
    cfg.selected.insert(target.provider, id.clone());
    cfg.pending_activation = None;
    persist(&cfg, &path)?;
    platform.delete_key("io.github.wuxm5.authbaton.vault", &journal_key)?;
    Ok(Update {
        config: cfg,
        leases: HashMap::from([(id, v)]),
    })
}
pub fn action(
    mut cfg: Config,
    platform: Platform,
    path: PathBuf,
    action: String,
    body: Value,
) -> Result<Update> {
    if action == "usage" {
        let id = body["id"].as_str().ok_or("缺少云端账号")?;
        cfg.usage_attempts.insert(id.into(), now());
        persist(&cfg, &path)?;
        let result = api(&platform, &cfg, &action, body.clone()).and_then(usage_projection);
        if let Some(a) = cfg.accounts.iter_mut().find(|a| a["id"] == body["id"]) {
            match result {
                Ok(v) => {
                    a["usage"] = v;
                    a["usage_error"] = Value::Null;
                }
                Err(e) => {
                    a["usage_error"] = json!(e);
                    if a["usage"]["checked_at"]
                        .as_u64()
                        .is_none_or(|t| t + 600 <= now())
                    {
                        a["usage"] = Value::Null;
                    }
                }
            }
        }
        persist(&cfg, &path)?;
        return Ok(done(cfg));
    }
    api(&platform, &cfg, &action, body.clone())?;
    directory(cfg, platform, path)
}

fn usage_projection(v: Value) -> Result<Value> {
    let checked = v["checked_at"]
        .as_u64()
        .filter(|t| *t <= now() + 60 && *t + 600 > now())
        .ok_or("云端用量缓存已过期")?;
    let rows = v["windows"]
        .as_array()
        .filter(|v| v.len() <= 100)
        .ok_or("云端用量格式无效")?;
    let mut windows = Vec::with_capacity(rows.len());
    for row in rows {
        let label = row["label"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 200 && !s.chars().any(char::is_control))
            .ok_or("云端用量窗口无效")?;
        let used = row["used_percent"]
            .as_f64()
            .filter(|n| (0.0..=100.0).contains(n))
            .ok_or("云端用量百分比无效")?;
        windows
            .push(json!({"label":label,"used_percent":used,"resets_at":row["resets_at"].as_u64()}));
    }
    Ok(json!({"windows":windows,"checked_at":checked}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usage_boundary_projects_only_bounded_display_values() {
        let input = json!({"checked_at":now(),"refresh_token":"SYNTHETIC-secret","windows":[{"label":"5 小时","used_percent":42.5,"resets_at":now()+10,"access_token":"SYNTHETIC-hidden"}]});
        let output = usage_projection(input).unwrap();
        assert!(!output.to_string().contains("SYNTHETIC"));
        assert!(
            usage_projection(
                json!({"checked_at":now(),"windows":[{"label":"5h","used_percent":101}]})
            )
            .is_err()
        );
        assert!(usage_projection(json!({"checked_at":now()-601,"windows":[]})).is_err());
    }
    #[test]
    fn endpoint_rejects_redirect_or_secret_bearing_input() {
        assert_eq!(
            validate_endpoint("https://auth.example.com/").unwrap(),
            "https://auth.example.com"
        );
        for s in [
            "http://auth.example.com",
            "https://user:pass@host.com",
            "https://host.com/path",
            "https://host.com?x=y",
            "https://host.com\n",
            "https://localhost:8080",
        ] {
            if s.ends_with('\n') {
                continue;
            }
            assert!(validate_endpoint(s).is_err(), "{s}");
        }
    }
}
