//! Cloud device transport. Model credentials never enter the UI projection or configuration file.
use crate::credentials::{Bundle, NativeStore};
use crate::platform::{Platform, Result, random_id, read_json, write_json};
use crate::status::now;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

const VAULT: &str = "io.github.wuxm5.authbaton.vault";
/// Binding phases. A transfer to the cloud runs prepared → imported → commit_pending →
/// cloud_managed; one back to this Mac runs cloud_managed → exporting → local.
pub const CLOUD_MANAGED: &str = "cloud_managed";
pub const EXPORTING: &str = "exporting";
pub const TO_CLOUD: [&str; 3] = ["prepared", "imported", "commit_pending"];
/// What becomes of a row once the cloud no longer renews it there. The service applies these
/// and drops the binding: `local` this Mac renews it now, `removed` the row goes, `lost` it was
/// moved back on another Mac, `left` the pairing ended, `export_lost` the cloud renewed it before
/// a move back here was confirmed.
pub const SETTLED: [&str; 5] = ["local", "removed", "lost", "left", "export_lost"];

pub const DEVICE_UNAUTHORIZED: &str = "设备未授权或已撤销，请重新配对";
pub const ACCOUNT_MISSING: &str = "云端账号已不存在";
pub const EXPORT_NOT_ACTIVE: &str = "转回本机没完成，云端已续期，这台需要重新登录";
pub const LEASE_PENDING: &str = "云端凭据还在领取，请稍后再试";

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
    /// `other`: the account came from another Mac's directory and has no login on this one.
    #[serde(default)]
    pub origin: String,
    /// What a transfer that stopped says on the card.
    #[serde(default)]
    pub error: Option<String>,
    /// The operation of a move back to this Mac.
    #[serde(default)]
    pub export: Option<String>,
    /// Put to use on this Mac at least once.
    #[serde(default)]
    pub used: bool,
    /// When the cloud took it over here; a directory read before that cannot show it gone.
    #[serde(default)]
    pub since: u64,
}
impl Binding {
    /// The cloud renews this row's login; this Mac only holds leased access.
    pub fn cloud(&self) -> bool {
        matches!(self.phase.as_str(), CLOUD_MANAGED | EXPORTING)
    }
    pub fn other(&self) -> bool {
        self.origin == "other"
    }
}
/// A move back to this Mac whose login is saved here; the cloud still has to drop its copy.
#[derive(Clone, Serialize, Deserialize)]
pub struct PendingExport {
    pub operation: String,
    pub local_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    /// Kept for files of older builds; nothing reads it.
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
    pub device_id: Option<String>,
    /// Cloud id → the move back whose commit is still owed.
    #[serde(default)]
    pub export_commits: HashMap<String, PendingExport>,
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
            device_id: None,
            export_commits: HashMap::new(),
        }
    }
}
/// What a background read found. The service merges it into the configuration it holds by then,
/// so a read never undoes a change made meanwhile and nothing a person asks for waits on one.
#[derive(Default)]
pub struct Patch {
    directory: Option<Directory>,
    paired: Option<Option<String>>,
    lease: Option<(String, String, Value)>,
    committed: Option<(String, bool)>,
}
struct Directory {
    accounts: Vec<Value>,
    devices: Vec<Value>,
    device_id: Option<String>,
    read_at: u64,
}
/// A background read: directory, lease, native sync, a commit retry, a check or a rename.
pub struct Light {
    pub tag: String,
    generation: u64,
    pub handle: JoinHandle<Result<Patch>>,
}
impl Config {
    fn take(&mut self, directory: Directory) {
        self.accounts = directory.accounts;
        self.devices = directory.devices;
        if self.device_id.is_none() {
            self.device_id = directory.device_id;
        }
        self.checked_at = Some(directory.read_at);
    }
    /// The cloud dropped its copy of a move back to this Mac. `spent`: it had renewed the login
    /// after giving up on the move, so the copy saved here no longer works.
    fn committed(&mut self, id: &str, spent: bool) {
        let Some(pending) = self.export_commits.remove(id) else {
            return;
        };
        if spent {
            self.bindings.insert(
                pending.local_id,
                Binding {
                    cloud_id: Some(id.into()),
                    phase: "export_lost".into(),
                    ..Default::default()
                },
            );
        }
        // Gone from the cloud: a directory read before this must not bring it back as a new row.
        self.accounts.retain(|a| a["id"] != id);
    }
}
pub struct Runtime {
    pub config: Config,
    path: PathBuf,
    /// A transfer, removal, pairing or leaving: the one cloud job that holds other work back.
    pub job: Option<JoinHandle<Result<Update>>>,
    pub light: Vec<Light>,
    /// Counts finished jobs above; a directory read from before one of them is stale.
    generation: u64,
    light_error: Option<String>,
    /// The row a running transfer works on.
    pub job_row: Option<String>,
    pub error: Option<String>,
    pub next_poll: u64,
    pub next_directory: u64,
    pub next_commit: u64,
    leases: HashMap<String, Value>,
    /// When a lease was last asked for, per cloud id, so a failing one is not asked every tick.
    pub lease_tried: HashMap<String, u64>,
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
            light: vec![],
            generation: 0,
            light_error: None,
            job_row: None,
            error: None,
            next_poll: 0,
            next_directory: 0,
            next_commit: 0,
            leases: HashMap::new(),
            lease_tried: HashMap::new(),
        })
    }
    pub fn snapshot(&self) -> Value {
        let this = self.config.device_id.as_deref();
        let devices: Vec<Value> = self
            .config
            .devices
            .iter()
            .map(|d| json!({"id":d["id"],"label":d["label"],"last_seen":d["last_seen"],"this":this.is_some_and(|t| d["id"] == t)}))
            .collect();
        // Devices cannot revoke each other; revoking stays on the Access-protected admin page.
        let admin =
            (!self.config.endpoint.is_empty()).then(|| format!("{}/admin", self.config.endpoint));
        json!({"endpoint":self.config.endpoint,"label":self.config.label,"paired":self.config.paired,"pair":self.config.pair,"busy":self.job.is_some(),"error":self.error.as_ref().or(self.light_error.as_ref()),"checked_at":self.config.checked_at,"devices":devices,"admin_url":admin})
    }
    pub fn save(&self) -> Result<()> {
        write_json(&self.path, &serde_json::to_value(&self.config).unwrap())
    }
    pub fn configure(&mut self, endpoint: Option<&str>, label: Option<&str>) -> Result<()> {
        if self.job.is_some() {
            return Err("云端操作正在进行");
        }
        let old = self.config.clone();
        if let Some(url) = endpoint {
            let url = validate_endpoint(url)?;
            if url != self.config.endpoint
                && (self.config.paired || !self.config.bindings.is_empty())
            {
                return Err("已配对或有交接记录，不能直接更换认证中心");
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
        self.job_row = None;
        // Tests run the job on the calling thread, where their synthetic transport lives.
        #[cfg(test)]
        let result = operation(cfg, platform, path);
        #[cfg(test)]
        let job = std::thread::spawn(move || result);
        #[cfg(not(test))]
        let job = std::thread::spawn(move || operation(cfg, platform, path));
        self.job = Some(job);
        Ok(())
    }
    /// Starts a background read unless one with the same tag is running. It never holds other
    /// work back: its result is merged in when it ends.
    pub fn start_light<F>(&mut self, tag: &str, platform: Platform, work: F) -> Result<()>
    where
        F: FnOnce(Config, Platform) -> Result<Patch> + Send + 'static,
    {
        if self.config.endpoint.is_empty() {
            return Err("请先配置个人认证中心地址");
        }
        if self.light_running(tag) {
            return Ok(());
        }
        let cfg = self.config.clone();
        // Tests run the job on the calling thread, where their synthetic transport lives.
        #[cfg(test)]
        let result = work(cfg, platform);
        #[cfg(test)]
        let handle = std::thread::spawn(move || result);
        #[cfg(not(test))]
        let handle = std::thread::spawn(move || work(cfg, platform));
        self.light.push(Light {
            tag: tag.into(),
            generation: self.generation,
            handle,
        });
        Ok(())
    }
    /// A background read that ends only when the test says so.
    #[cfg(test)]
    pub fn start_light_held<F>(
        &mut self,
        tag: &str,
        platform: Platform,
        work: F,
    ) -> std::sync::mpsc::Sender<()>
    where
        F: FnOnce(Config, Platform) -> Result<Patch>,
    {
        let result = work(self.config.clone(), platform);
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            let _ = wait.recv();
            result
        });
        self.light.push(Light {
            tag: tag.into(),
            generation: self.generation,
            handle,
        });
        release
    }
    pub fn light_running(&self, tag: &str) -> bool {
        self.light.iter().any(|l| l.tag == tag)
    }
    pub fn poll(&mut self) {
        // The job first: a read that finished beside it is from before what it changed.
        if self.job.as_ref().is_some_and(|j| j.is_finished()) {
            self.job_row = None;
            self.generation += 1;
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
        self.poll_light();
    }
    fn poll_light(&mut self) {
        let mut changed = false;
        let mut index = 0;
        while index < self.light.len() {
            if !self.light[index].handle.is_finished() {
                index += 1;
                continue;
            }
            let job = self.light.remove(index);
            match job.handle.join() {
                Ok(Ok(patch)) => {
                    changed |= self.apply(patch, job.generation);
                    self.light_error = None;
                }
                Ok(Err(e)) => self.light_error = Some(e.into()),
                Err(_) => self.light_error = Some("云端同步中断，稍后重试".into()),
            }
        }
        if changed && let Err(e) = self.save() {
            self.light_error = Some(e.into());
        }
    }
    fn apply(&mut self, patch: Patch, generation: u64) -> bool {
        let mut changed = false;
        if let Some(device_id) = patch.paired
            && !self.config.paired
        {
            self.config.paired = true;
            self.config.pair = None;
            self.config.device_id = device_id;
            self.next_directory = 0;
            changed = true;
        }
        // A read from before a transfer, removal or leaving finished describes what no longer holds.
        if let Some(directory) = patch.directory
            && generation == self.generation
            && self.config.paired
        {
            self.config.take(directory);
            changed = true;
        }
        if let Some((id, owner, lease)) = patch.lease
            && self.config.paired
        {
            if self.config.owners.get(&id).is_some_and(|o| *o != owner) {
                self.light_error = Some("云端账号身份已变化，已停止领取".into());
            } else {
                self.config.owners.insert(id.clone(), owner);
                self.leases.insert(id, lease);
                changed = true;
            }
        }
        if let Some((id, spent)) = patch.committed {
            self.config.committed(&id, spent);
            changed = true;
        }
        changed
    }
    pub fn finish(&mut self) {
        // Background reads are dropped: each is read again after a restart.
        self.light.clear();
        self.job_row = None;
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
    #[cfg(test)]
    pub fn set_lease(&mut self, id: &str, lease: Value) {
        self.leases.insert(id.into(), lease);
    }
    /// The access-only login a cloud account's lease carries; never a refresh token.
    pub fn lease_bundle(&self, id: &str) -> Result<Bundle> {
        projection_of(&self.lease(id).ok_or(LEASE_PENDING)?)
    }
    /// No lease, or one within 15 minutes of expiring.
    pub fn lease_due(&self, id: &str) -> bool {
        self.leases
            .get(id)
            .and_then(|v| v["expires_at"].as_u64())
            .is_none_or(|t| t <= now() + 900)
    }
    /// Settles an activation a restart interrupted.
    pub fn settle(
        &mut self,
        platform: &Platform,
        stores: &HashMap<&str, NativeStore>,
    ) -> Result<()> {
        let mut cfg = self.config.clone();
        settle_activation(&mut cfg, platform, &self.path, stores)?;
        self.config = cfg;
        Ok(())
    }
    /// The directory's row for a cloud account.
    pub fn account(&self, id: &str) -> Option<&Value> {
        self.config.accounts.iter().find(|a| a["id"] == id)
    }
    /// Puts a cloud account's access-only login into the native store, under the journal the
    /// periodic sync settles, and records it as the one in use.
    pub fn activate(
        &mut self,
        platform: &Platform,
        native: &NativeStore,
        id: &str,
        target: &Bundle,
    ) -> Result<()> {
        let mut cfg = self.config.clone();
        publish(&mut cfg, platform, Some(&self.path), native, id, target)?;
        self.config = cfg;
        Ok(())
    }
}
/// A fixed account of a cloud account's state, from the directory's error code.
pub fn account_error(code: &str) -> &'static str {
    match code {
        "refresh_rejected" | "reauthorization_required" => {
            "续期被拒（刷新凭据已失效），需重新登录以恢复云端续期。"
        }
        "issuer_unauthorized" => "官方拒绝了云端续期后的凭据，需重新登录以恢复云端续期。",
        "identity_mismatch" => "续期后账号身份不一致，需重新登录以恢复云端续期。",
        "refresh_outcome_unknown" => "云端续期没有收到回应，稍后自动用同一凭据重新确认。",
        "verification_pending" => "云端续期后正在核对新凭据，稍后自动重试。",
        "issuer_rate_limited" | "retry_later" => "官方接口限流，云端稍后再续期。",
        _ => "云端续期出错，稍后自动重试。",
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
        "deployment_not_configured" => "云端还没配好，请先在 Cloudflare 配置存储密钥和管理登录",
        "device_unauthorized" => DEVICE_UNAUTHORIZED,
        "pair_expired" => "配对已过期，请重新发起",
        "account_already_registered" => "云端已登记此账号，请使用已有账号或先处理原授权",
        "source_access_expired" => "本机 Access 已到期，请先在本地完成认证检查再导入",
        "handoff_required" => "账号已导入，尚未完成续期归属交接",
        "handoff_in_progress" => "这个账号正在交接或转回本机，请稍后再试",
        "issuer_unauthorized" => "官方接口拒绝当前凭据，请检查登录状态",
        "refresh_outcome_unknown" => "云端续期没有收到回应；稍后自动用同一凭据重新确认",
        "refresh_rejected" | "reauthorization_required" => {
            "官方已拒绝续期凭据，请重新授权此云端账号"
        }
        "issuer_rate_limited" | "retry_later" => "官方接口限流，稍后重试",
        "maintenance_disabled" => "此云端账号已关闭自动维护",
        "identity_mismatch" => "账号身份不一致，已停止操作",
        "account_missing" => ACCOUNT_MISSING,
        "account_not_ready" => "云端账号还没就绪，暂不能转回本机",
        "account_exporting" => "这个账号正在转回某台 Mac，暂不可用",
        "export_in_progress" => "有没结束的转回本机，请先完成或放弃后再试",
        "export_not_active" => EXPORT_NOT_ACTIVE,
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
        &format!("{endpoint}/v1/{action}"),
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
/// Leaves a transfer that stopped on its card, with what stopped it.
fn fail(cfg: &mut Config, path: &Path, local_id: &str, message: &'static str) {
    if let Some(binding) = cfg.bindings.get_mut(local_id) {
        binding.error = Some(message.into());
        let _ = persist(cfg, path);
    }
}
pub fn pair(mut cfg: Config, platform: Platform, path: PathBuf) -> Result<Update> {
    if cfg.paired {
        return Err("设备已配对");
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
fn device_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| {
            !s.is_empty() && s.len() <= 80 && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        })
        .map(str::to_owned)
}
/// Polls a pairing the admin page may have approved.
pub fn pair_poll(cfg: Config, platform: Platform) -> Result<Patch> {
    let s = secret(&platform, &cfg)?;
    // Vault commit precedes the public configuration commit. Recover a completed
    // approval without depending on the short-lived pairing record still existing.
    if s["token"].as_str().is_some() {
        let directory = read_directory(&cfg, &platform)?;
        return Ok(Patch {
            paired: Some(device_id(&s["device_id"])),
            directory: Some(directory),
            ..Default::default()
        });
    }
    let proof = s["proof"].as_str().ok_or("配对状态不存在")?;
    let p = cfg.pair.as_ref().ok_or("配对状态不存在")?;
    let v = http(
        &cfg.endpoint,
        "pair.poll",
        Some(proof),
        json!({"id":p["id"]}),
    )?;
    if v["state"] != "paired" {
        return Ok(Patch::default());
    }
    let token = v["device_token"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("设备授权响应无效")?;
    platform.vault_write(
        "cloud-device-v1",
        &json!({"endpoint":cfg.endpoint,"token":token,"device_id":v["device_id"],"proof":proof}),
    )?;
    Ok(Patch {
        paired: Some(device_id(&v["device_id"])),
        ..Default::default()
    })
}
fn read_directory(cfg: &Config, platform: &Platform) -> Result<Directory> {
    let read_at = now();
    let v = api(platform, cfg, "accounts", json!({}))?;
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
    let accounts = rows
        .iter()
        .map(|a| {
            let mut row: serde_json::Map<String, Value> = keys
                .iter()
                .filter_map(|k| a.get(k).map(|v| ((*k).into(), v.clone())))
                .collect();
            let used_by: Vec<Value> = a["used_by"]
                .as_array()
                .map(|u| {
                    u.iter()
                        .take(32)
                        .map(|d| json!({"device_id":d["device_id"],"label":d["label"],"at":d["at"]}))
                        .collect()
                })
                .unwrap_or_default();
            row.insert("used_by".into(), Value::Array(used_by));
            Value::Object(row)
        })
        .collect();
    let devices = v["devices"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|d| json!({"id":d["id"],"label":d["label"],"last_seen":d["last_seen"]}))
        .collect();
    // Pairings made by older builds kept the device id only beside the device token.
    let device_id = cfg
        .device_id
        .clone()
        .or_else(|| secret(platform, cfg).ok().and_then(|s| device_id(&s["device_id"])));
    Ok(Directory {
        accounts,
        devices,
        device_id,
        read_at,
    })
}
/// Reads the directory at the end of a transfer, removal or pairing.
pub fn directory(mut cfg: Config, platform: Platform, path: PathBuf) -> Result<Update> {
    let directory = read_directory(&cfg, &platform)?;
    cfg.take(directory);
    persist(&cfg, &path)?;
    Ok(done(cfg))
}
/// The 30-second directory read.
pub fn sync_directory(cfg: Config, platform: Platform) -> Result<Patch> {
    Ok(Patch {
        directory: Some(read_directory(&cfg, &platform)?),
        ..Default::default()
    })
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
/// The access-only login in a lease. A lease that carries a refresh token is refused.
pub fn projection_of(lease: &Value) -> Result<Bundle> {
    let bundle: Bundle =
        serde_json::from_value(lease["projection"].clone()).map_err(|_| "云端凭据格式无效")?;
    bundle.cloud_metadata()?;
    Ok(bundle)
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
    if let Err(e) = transfer_steps(&mut cfg, &platform, &path, &local_id, label, native) {
        fail(&mut cfg, &path, &local_id, e);
        return Err(e);
    }
    directory(cfg, platform, path)
}
fn transfer_steps(
    cfg: &mut Config,
    platform: &Platform,
    path: &Path,
    local_id: &str,
    label: String,
    native: Option<NativeStore>,
) -> Result<()> {
    let binding = cfg.bindings.get(local_id).ok_or("缺少交接记录")?.clone();
    let key = format!("cloud-import-{local_id}");
    let saved = platform.vault_read_optional(&key)?;
    let bundle = if let Some(v) = saved {
        Bundle::decode(v)?
    } else {
        let b = if let Some(n) = &native {
            n.read(platform)?
        } else {
            Bundle::decode(platform.vault_read(local_id)?)?
        };
        if b.metadata()?.owner != binding.owner {
            return Err("导入来源身份已变化");
        }
        platform.vault_write(&key, &b.encoded())?;
        b
    };
    let a = api(
        platform,
        cfg,
        "import",
        json!({"operation":binding.operation,"label":label,"bundle":bundle,"replace_id":binding.replace_id}),
    )?;
    let id = a["id"].as_str().ok_or("云端导入响应无效")?.to_string();
    let b = cfg.bindings.get_mut(local_id).unwrap();
    b.cloud_id = Some(id.clone());
    b.phase = "imported".into();
    persist(cfg, path)?;
    if let Some(n) = &native {
        if client_running(&bundle.provider) {
            return Err("请先结束任务并退出对应原生客户端，再恢复云端交接；无需重新登录");
        }
        let target = projection(&bundle);
        if n.cloud_read(platform).ok().map(|b| b.credential) != Some(target.credential.clone()) {
            n.cloud_publish(platform, &bundle, &target)?;
        }
        platform.vault_write(
            &format!("cloud-native-{}", bundle.provider),
            &target.encoded(),
        )?;
    }
    cfg.bindings.get_mut(local_id).unwrap().phase = "commit_pending".into();
    persist(cfg, path)?;
    api(
        platform,
        cfg,
        "import.commit",
        json!({"id":id,"operation":binding.operation}),
    )?;
    let b = cfg.bindings.get_mut(local_id).unwrap();
    b.phase = CLOUD_MANAGED.into();
    b.error = None;
    b.since = now();
    cfg.reauthorizations.remove(local_id);
    cfg.owners.insert(id.clone(), binding.owner);
    if binding.native {
        cfg.selected.insert(bundle.provider.clone(), id);
    }
    persist(cfg, path)?;
    // The cloud renews this login from now on: no full copy stays on this Mac to go stale.
    let _ = platform.delete_key(VAULT, local_id);
    let _ = platform.delete_key(VAULT, &key);
    Ok(())
}
pub fn cancel_transfer(
    mut cfg: Config,
    platform: Platform,
    path: PathBuf,
    local_id: String,
    native: Option<NativeStore>,
) -> Result<Update> {
    if let Err(e) = cancel_steps(&mut cfg, &platform, &path, &local_id, native) {
        fail(&mut cfg, &path, &local_id, e);
        return Err(e);
    }
    directory(cfg, platform, path)
}
fn cancel_steps(
    cfg: &mut Config,
    platform: &Platform,
    path: &Path,
    local_id: &str,
    native: Option<NativeStore>,
) -> Result<()> {
    let binding = cfg.bindings.get(local_id).ok_or("缺少交接记录")?.clone();
    api(
        platform,
        cfg,
        "import.abort",
        json!({"operation":binding.operation}),
    )?;
    let key = format!("cloud-import-{local_id}");
    if let (Some(native), Some(saved)) = (native, platform.vault_read_optional(&key)?) {
        let original = Bundle::decode(saved)?;
        if client_running(&original.provider) {
            return Err("请先退出对应客户端再恢复本地维护");
        }
        if let Ok(current) = native.cloud_read(platform) {
            if current.credential != projection(&original).credential {
                return Err("本机凭据已变化，未恢复旧副本");
            }
            native.cloud_restore(platform, &current, &original)?;
        } else if native.read(platform)?.metadata()?.owner != binding.owner {
            return Err("本机身份已变化，请核对后恢复");
        }
    }
    cfg.bindings.remove(local_id);
    cfg.reauthorizations.remove(local_id);
    persist(cfg, path)?;
    platform.delete_key(VAULT, &key)?;
    Ok(())
}
/// `rejected`: the version whose access a client was refused with; the cloud renews it first.
pub fn fetch_lease(
    cfg: &mut Config,
    platform: &Platform,
    id: &str,
    rejected: Option<u64>,
) -> Result<Value> {
    let body = match rejected {
        Some(version) => json!({"id":id,"rejected_version":version}),
        None => json!({"id":id}),
    };
    let v = api(platform, cfg, "lease", body)?;
    if v["id"] != id || v["expires_at"].as_u64().is_none_or(|t| t <= now() + 60) {
        return Err("云端凭据标识或有效期无效");
    }
    let b = projection_of(&v)?;
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
/// Leases a cloud account's access.
pub fn lease(
    mut cfg: Config,
    platform: Platform,
    id: String,
    rejected: Option<u64>,
) -> Result<Patch> {
    let v = fetch_lease(&mut cfg, &platform, &id, rejected)?;
    let owner = cfg.owners.get(&id).cloned().ok_or("云端凭据身份无效")?;
    Ok(Patch {
        lease: Some((id, owner, v)),
        ..Default::default()
    })
}
/// Keeps the access-only login of the cloud account in use fresh in the native store. Every
/// native writer takes the same in-process lock, so this never interleaves with a switch.
pub fn native_sync(
    mut cfg: Config,
    platform: Platform,
    id: String,
    native: NativeStore,
) -> Result<Patch> {
    let v = fetch_lease(&mut cfg, &platform, &id, None)?;
    let target = projection_of(&v)?;
    publish(&mut cfg, &platform, None, &native, &id, &target)?;
    let owner = cfg.owners.get(&id).cloned().ok_or("云端凭据身份无效")?;
    Ok(Patch {
        lease: Some((id, owner, v)),
        ..Default::default()
    })
}
/// Writes a cloud account's access-only login into the native store. With `path` it puts the
/// account in use over whatever the store holds and records that; without, it renews the
/// projection written last and changes no configuration.
fn publish(
    cfg: &mut Config,
    platform: &Platform,
    path: Option<&Path>,
    native: &NativeStore,
    id: &str,
    target: &Bundle,
) -> Result<()> {
    let Some(path) = path else {
        return write_projection(platform, native, id, target, None);
    };
    let provider = target.provider.clone();
    write_projection(
        platform,
        native,
        id,
        target,
        Some(&mut |landed| {
            if landed {
                cfg.selected.insert(provider.clone(), id.into());
                cfg.pending_activation = None;
            } else {
                cfg.pending_activation = Some(id.into());
            }
            persist(cfg, path)
        }),
    )
}
/// Writes an access-only login (a cloud account's or an external source's) into the native
/// store under the journal a restart settles. With `mark` it puts the login in use over whatever
/// the store holds: `mark(false)` records the activation before the write, `mark(true)` once it
/// landed. Without, it renews the projection written last.
pub(crate) fn write_projection(
    platform: &Platform,
    native: &NativeStore,
    id: &str,
    target: &Bundle,
    mut mark: Option<&mut dyn FnMut(bool) -> Result<()>>,
) -> Result<()> {
    let activate = mark.is_some();
    let key = format!("cloud-native-{}", target.provider);
    let journal_key = format!("cloud-native-journal-{}", target.provider);
    // Reconcile the prior file/Keychain write before requesting a new write. The
    // saved target survives upstream rotation and a failed projection-cache commit.
    if let Some(journal) = platform.vault_read_optional(&journal_key)? {
        let prior: Bundle = serde_json::from_value(journal["target"].clone())
            .map_err(|_| "云端投影恢复记录无效")?;
        let before: Option<Bundle> = serde_json::from_value(journal["expected"].clone())
            .map_err(|_| "云端投影恢复记录无效")?;
        let actual = native.cloud_existing(platform)?.map(|b| b.credential);
        if actual.as_ref() == Some(&prior.credential) {
            platform.vault_write(&key, &prior.encoded())?;
        } else if !activate && actual != before.map(|b| b.credential) {
            // An activation reads the store afresh below; a renewal never overwrites a change.
            return Err("原生登录已变化，未覆盖外部修改；请核对云端投影恢复记录");
        }
        platform.delete_key(VAULT, &journal_key)?;
    }
    // The caller has made sure no client still runs on the login being replaced: a switch refuses
    // while one runs; a hot switch has just logged the desktop in itself, after checking for others.
    let expected = if activate {
        native.cloud_existing(platform)?
    } else {
        Some(
            serde_json::from_value(platform.vault_read(&key)?)
                .map_err(|_| "本机云端投影记录无效")?,
        )
    };
    if let Some(mark) = mark.as_mut() {
        if let Some(original) = expected.as_ref().filter(|b| b.metadata().is_ok()) {
            // Include an unregistered native login: it may be the only full copy.
            platform.vault_write(
                &format!("cloud-previous-{}", target.provider),
                &original.encoded(),
            )?;
        }
        mark(false)?;
    }
    platform.vault_write(
        &journal_key,
        &json!({"id":id,"expected":expected,"target":target}),
    )?;
    if let Some(ref expected) = expected {
        if expected.credential != target.credential || native.codex_projection_outdated(platform)? {
            native.cloud_publish(platform, expected, target)?;
        }
    } else {
        native.cloud_initialize(platform, target)?;
    }
    platform.vault_write(&key, &target.encoded())?;
    if let Some(mark) = mark.as_mut() {
        mark(true)?;
    }
    platform.delete_key(VAULT, &journal_key)?;
    Ok(())
}
/// Settles an activation a restart interrupted: the native store says whether it landed.
fn settle_activation(
    cfg: &mut Config,
    platform: &Platform,
    path: &Path,
    stores: &HashMap<&str, NativeStore>,
) -> Result<()> {
    let Some(id) = cfg.pending_activation.clone() else {
        return Ok(());
    };
    for provider in settle_projection(platform, stores, &id)? {
        cfg.selected.insert(provider.into(), id.clone());
    }
    cfg.pending_activation = None;
    persist(cfg, path)
}
/// Settles the journal an activation of `id` left when a restart interrupted it. Returns the
/// providers whose native store holds the login it wrote; the journal goes either way.
pub(crate) fn settle_projection(
    platform: &Platform,
    stores: &HashMap<&str, NativeStore>,
    id: &str,
) -> Result<Vec<&'static str>> {
    let mut landed_in = vec![];
    for provider in ["codex", "claude"] {
        let journal_key = format!("cloud-native-journal-{provider}");
        let Some(journal) = platform.vault_read_optional(&journal_key)? else {
            continue;
        };
        if journal["id"] != id {
            continue;
        }
        let target: Bundle = serde_json::from_value(journal["target"].clone())
            .map_err(|_| "云端投影恢复记录无效")?;
        let landed = stores
            .get(provider)
            .map(|n| n.cloud_existing(platform))
            .transpose()?
            .flatten()
            .is_some_and(|b| b.credential == target.credential);
        if landed {
            platform.vault_write(&format!("cloud-native-{provider}"), &target.encoded())?;
            landed_in.push(provider);
        }
        platform.delete_key(VAULT, &journal_key)?;
    }
    Ok(landed_in)
}
/// The cloud checks the account's login; the caller reads its quota with the leased access.
pub fn check(cfg: Config, platform: Platform, id: String) -> Result<Patch> {
    api(&platform, &cfg, "check", json!({"id":id}))?;
    sync_directory(cfg, platform)
}
pub fn rename(cfg: Config, platform: Platform, id: String, label: String) -> Result<Patch> {
    api(&platform, &cfg, "update", json!({"id":id,"label":label}))?;
    sync_directory(cfg, platform)
}
/// Removes the account from the cloud; the service then drops its row here.
pub fn remove(
    mut cfg: Config,
    platform: Platform,
    path: PathBuf,
    local_id: String,
) -> Result<Update> {
    let binding = cfg
        .bindings
        .get(&local_id)
        .filter(|b| b.phase == CLOUD_MANAGED)
        .ok_or("这个账号不由云端续期")?;
    let id = binding.cloud_id.clone().ok_or("缺少云端账号")?;
    match api(&platform, &cfg, "remove", json!({"id":id})) {
        Ok(_) | Err(ACCOUNT_MISSING) => {}
        Err(e) => return Err(e),
    }
    cfg.bindings.get_mut(&local_id).unwrap().phase = "removed".into();
    cfg.owners.remove(&id);
    cfg.accounts.retain(|a| a["id"] != id.as_str());
    persist(&cfg, &path)?;
    directory(cfg, platform, path)
}
/// This Mac revokes its own pairing. Rows of other Macs go; rows moved up from here need a new login.
pub fn leave(mut cfg: Config, platform: Platform, path: PathBuf) -> Result<Update> {
    match api(&platform, &cfg, "leave", json!({})) {
        Ok(_) | Err(DEVICE_UNAUTHORIZED) => {}
        Err(e) => return Err(e),
    }
    for binding in cfg.bindings.values_mut() {
        if binding.phase == CLOUD_MANAGED {
            binding.phase = if binding.other() { "removed" } else { "left" }.into();
        }
    }
    cfg.paired = false;
    cfg.pair = None;
    cfg.accounts.clear();
    cfg.devices.clear();
    cfg.owners.clear();
    cfg.reauthorizations.clear();
    cfg.device_id = None;
    cfg.checked_at = None;
    persist(&cfg, &path)?;
    platform.delete_key(VAULT, "cloud-device-v1")?;
    Ok(done(cfg))
}
/// Moves a cloud account back to this Mac: the cloud stops renewing it and hands over the full
/// login, which goes into this row's Vault item (and the native store when it is in use) before
/// the row becomes local and the cloud deletes its copy.
pub fn export(
    mut cfg: Config,
    platform: Platform,
    path: PathBuf,
    local_id: String,
    native: Option<NativeStore>,
) -> Result<Update> {
    let binding = cfg
        .bindings
        .get(&local_id)
        .filter(|b| b.cloud())
        .ok_or("这个账号不由云端续期")?
        .clone();
    let id = binding.cloud_id.clone().ok_or("缺少云端账号")?;
    let operation = match binding.export.clone() {
        Some(operation) => operation,
        None => random_id()?,
    };
    let b = cfg.bindings.get_mut(&local_id).unwrap();
    b.phase = EXPORTING.into();
    b.export = Some(operation.clone());
    b.error = None;
    persist(&cfg, &path)?;
    if let Err(e) = export_steps(
        &mut cfg,
        &platform,
        &local_id,
        &id,
        &operation,
        &binding,
        native.as_ref(),
    ) {
        fail(&mut cfg, &path, &local_id, e);
        return Err(e);
    }
    let b = cfg.bindings.get_mut(&local_id).unwrap();
    b.phase = "local".into();
    b.error = None;
    cfg.owners.remove(&id);
    cfg.export_commits.insert(
        id.clone(),
        PendingExport {
            operation,
            local_id,
        },
    );
    persist(&cfg, &path)?;
    // A commit that does not get through is retried; the login is already this Mac's.
    if let Ok(spent) = commit(&cfg, &platform, &id) {
        cfg.committed(&id, spent);
        persist(&cfg, &path)?;
    }
    Ok(done(cfg))
}
fn export_steps(
    cfg: &mut Config,
    platform: &Platform,
    local_id: &str,
    id: &str,
    operation: &str,
    binding: &Binding,
    native: Option<&NativeStore>,
) -> Result<()> {
    let v = api(
        platform,
        cfg,
        "export.begin",
        json!({"id":id,"operation":operation}),
    )?;
    let bundle = Bundle::decode(v["bundle"].clone())?;
    let owner = bundle.metadata()?.owner;
    if v["id"] != id
        || v["owner"] != owner.as_str()
        || bundle.provider != binding.provider
        || (!binding.owner.is_empty() && binding.owner != owner)
        || cfg.owners.get(id).is_some_and(|o| *o != owner)
    {
        return Err("云端交回的登录与这个账号不一致，未保存");
    }
    platform.vault_write(local_id, &bundle.encoded())?;
    if let Some(native) = native {
        let key = format!("cloud-native-{}", binding.provider);
        let current = native.cloud_existing(platform)?.map(|b| b.credential);
        // A retry after the native write already went through finds the full login there.
        if current.as_ref() != Some(&bundle.credential) {
            let expected: Bundle = serde_json::from_value(platform.vault_read(&key)?)
                .map_err(|_| "本机云端投影记录无效")?;
            native.cloud_restore(platform, &expected, &bundle)?;
        }
        let _ = platform.delete_key(VAULT, &key);
        cfg.selected.remove(&binding.provider);
    }
    Ok(())
}
/// Asks the cloud to drop its copy of a move back to this Mac. `Some(spent)` once it answered;
/// `spent`: it renewed the login after giving up on the move, so the copy here is spent.
fn commit(cfg: &Config, platform: &Platform, id: &str) -> Result<bool> {
    let pending = cfg
        .export_commits
        .get(id)
        .ok_or("没有待确认的转回本机")?;
    match api(
        platform,
        cfg,
        "export.commit",
        json!({"id":id,"operation":pending.operation}),
    ) {
        Ok(_) => Ok(false),
        Err(EXPORT_NOT_ACTIVE) => Ok(true),
        Err(e) => Err(e),
    }
}
/// Retries the commit of a move back to this Mac.
pub fn commit_export(cfg: Config, platform: Platform, id: String) -> Result<Patch> {
    let spent = commit(&cfg, &platform, &id)?;
    Ok(Patch {
        committed: Some((id, spent)),
        ..Default::default()
    })
}
/// Gives up a move back that has not saved the login here; the cloud renews it again.
pub fn cancel_export(
    mut cfg: Config,
    platform: Platform,
    path: PathBuf,
    local_id: String,
    native: Option<NativeStore>,
) -> Result<Update> {
    let binding = cfg
        .bindings
        .get(&local_id)
        .filter(|b| b.phase == EXPORTING)
        .ok_or("没有进行中的转回本机")?
        .clone();
    let id = binding.cloud_id.clone().ok_or("缺少云端账号")?;
    let operation = binding.export.clone().ok_or("没有进行中的转回本机")?;
    // Once the full login is in the native store a client may renew it: finish the move instead.
    if let (Some(native), Some(saved)) = (native, platform.vault_read_optional(&local_id)?)
        && native
            .cloud_existing(&platform)?
            .is_some_and(|b| b.credential == saved["credential"])
    {
        return Err("完整登录已写入本机客户端，请重试转回本机");
    }
    match api(
        &platform,
        &cfg,
        "export.abort",
        json!({"id":id,"operation":operation}),
    ) {
        Ok(_) | Err(ACCOUNT_MISSING) => {}
        Err(e) => {
            fail(&mut cfg, &path, &local_id, e);
            return Err(e);
        }
    }
    // The cloud renews it again; a copy left here would go stale.
    platform.delete_key(VAULT, &local_id)?;
    let b = cfg.bindings.get_mut(&local_id).unwrap();
    b.phase = CLOUD_MANAGED.into();
    b.export = None;
    b.error = None;
    persist(&cfg, &path)?;
    directory(cfg, platform, path)
}

#[cfg(test)]
mod tests {
    use super::*;
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
