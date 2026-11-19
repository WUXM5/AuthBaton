//! Seamless same-Team account switching for Claude Desktop's Code tab (native Bun direct-swap).
//!
//! Architecture:
//! - A tiny vouched managed Mod (`integrations/claude-desktop-relay/`) is installed into
//!   `/Library/Application Support/ClaudeCode/authbaton-desktop-relay/` and registered in
//!   `/Library/Application Support/ClaudeCode/managed-settings.json` with
//!   `"parentSettingsBehavior": "merge"` via an explicit user-confirmed macOS admin prompt.
//! - Requests stay on Claude Desktop's own built-in Bun engine (100% native BoringSSL TLS, HTTP,
//!   proxy, and connection-pool fingerprint; 0 MB bundle bloat; tracks every Desktop upgrade).
//! - Only when `CLAUDE_CODE_ENTRYPOINT === 'claude-desktop'`, the Mod intercepts engine
//!   `POST /v1/messages` (skipping `/count_tokens`), leases a short-lived same-Team OAuth
//!   `access_token` (never `refresh_token`) + `account_uuid` over `<root>/claude-desktop-relay.sock`
//!   (0600, same-UID, identical security boundary to `pi.sock`), swaps `Authorization` and
//!   top-level `metadata.user_id.account_uuid` in place, and hands the request back to Claude's
//!   built-in `next()` fetch.
//! - On 429, the Mod reports the outcome and requests the next same-Team fallback via `/v1/lease`,
//!   calling `next()` a second time inside the same `http.fetch` hook for zero-error failover.
//! - On 401, Claude Code core automatically marks modified-credential responses `x-should-retry: false`
//!   (0 `oauth_token_refresh` sent to Desktop), while `/v1/outcome` asks AuthBaton to check/refresh
//!   the target account in the background.
//! - Fail-Open: whenever AuthBaton is stopped, disabled, or refuses cross-Team traffic, `/v1/lease`
//!   returns `active: false` (or is unreachable) and the Mod immediately returns `next(e)`.

use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const EMBED_MARKETPLACE_JSON: &str =
    include_str!("../../../integrations/claude-desktop-relay/.claude-plugin/marketplace.json");
const EMBED_PLUGIN_JSON: &str =
    include_str!("../../../integrations/claude-desktop-relay/authbaton/.claude-plugin/plugin.json");
const EMBED_HOOKS_JSON: &str =
    include_str!("../../../integrations/claude-desktop-relay/authbaton/hooks/hooks.json");
const EMBED_REGISTER_TS: &str =
    include_str!("../../../integrations/claude-desktop-relay/authbaton/hooks/register.ts");

const MANAGED_DIR: &str = "/Library/Application Support/ClaudeCode";
const MARKETPLACE_NAME: &str = "authbaton-desktop-relay";
const PLUGIN_ID: &str = "authbaton@authbaton-desktop-relay";
const VERSION_FILE: &str = ".authbaton-version";

const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 8 * 1024;
const MAX_CONCURRENT_CONNS: usize = 32;
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);
const OVERRIDE_ACTIVE_TTL: Duration = Duration::from_secs(60);
const LOG_ROTATE_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstallState {
    #[default]
    NotInstalled,
    Installed,
    NeedsUpdate,
    BlockedRemote,
}

impl InstallState {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallState::NotInstalled => "not_installed",
            InstallState::Installed => "installed",
            InstallState::NeedsUpdate => "needs_update",
            InstallState::BlockedRemote => "blocked_remote",
        }
    }
}

#[derive(Clone)]
pub struct RelayAccount {
    pub id: String,
    pub label: String,
    pub account_uuid: String,
    pub org_uuid: String,
    pub access_token: String,
}

impl Drop for RelayAccount {
    fn drop(&mut self) {
        zero_string(&mut self.access_token);
    }
}

fn zero_string(s: &mut String) {
    // SAFETY: writing zeroes into the string's UTF-8 buffer produces valid UTF-8 NUL bytes.
    unsafe {
        for byte in s.as_bytes_mut() {
            std::ptr::write_volatile(byte, 0);
        }
    }
    s.clear();
}

#[derive(Clone, Default)]
pub struct RelaySyncInput {
    pub enabled: bool,
    pub desktop_account_uuid: Option<String>,
    pub desktop_org_uuid: Option<String>,
    pub desktop_label: Option<String>,
    pub active: Option<RelayAccount>,
    pub fallbacks: Vec<RelayAccount>,
}

#[derive(Debug, Clone)]
pub enum RelayEvent {
    Switched {
        to_id: String,
    },
    Observed {
        account_id: String,
        model: Option<String>,
        primary_used: Option<f64>,
        limited: bool,
    },
    Unauthorized {
        account_id: String,
    },
}

#[derive(Default)]
struct SharedState {
    enabled: bool,
    install_state: InstallState,
    desktop_account_uuid: Option<String>,
    desktop_org_uuid: Option<String>,
    desktop_label: Option<String>,
    active: Option<RelayAccount>,
    fallbacks: Vec<RelayAccount>,
    /// Set briefly after a 429 failover before `Accounts::tick()` activates the new native account.
    override_active: Option<(String, Instant)>,
    cooldowns: HashMap<String, Instant>,
    last_switch_at: Option<u64>,
    last_error: Option<String>,
}

impl SharedState {
    fn same_team(&self) -> Option<bool> {
        let active = self.effective_active()?;
        if let Some(desk_org) = self.desktop_org_uuid.as_deref() {
            return Some(active.org_uuid == desk_org);
        }
        if let Some(desk_acct) = self.desktop_account_uuid.as_deref()
            && desk_acct == active.account_uuid
        {
            return Some(true);
        }
        None
    }

    fn effective_active(&self) -> Option<&RelayAccount> {
        if let Some((override_id, until)) = &self.override_active
            && Instant::now() < *until
        {
            if let Some(a) = self.active.as_ref().filter(|a| a.id == *override_id) {
                return Some(a);
            }
            if let Some(f) = self.fallbacks.iter().find(|a| a.id == *override_id) {
                return Some(f);
            }
        }
        self.active.as_ref()
    }

    fn is_relay_active(&self) -> bool {
        self.enabled
            && matches!(
                self.install_state,
                InstallState::Installed | InstallState::NeedsUpdate
            )
            && self.same_team() == Some(true)
            && self.effective_active().is_some()
    }

    /// Returns ordered same-Team candidates excluding `exclude` (non-cooldown first).
    fn candidates(&self, exclude: &[String]) -> Vec<RelayAccount> {
        if !self.is_relay_active() {
            return Vec::new();
        }
        let now = Instant::now();
        let Some(primary) = self.effective_active() else {
            return Vec::new();
        };
        let target_org = primary.org_uuid.as_str();
        let mut all: Vec<&RelayAccount> = Vec::new();
        all.push(primary);
        if let Some(active) = self.active.as_ref()
            && active.id != primary.id
            && active.org_uuid == target_org
        {
            all.push(active);
        }
        for f in &self.fallbacks {
            if f.id != primary.id
                && f.org_uuid == target_org
                && !all.iter().any(|existing| existing.id == f.id)
            {
                all.push(f);
            }
        }
        let mut ready: Vec<RelayAccount> = Vec::new();
        let mut cooled: Vec<RelayAccount> = Vec::new();
        for acct in all {
            if exclude.iter().any(|id| id == &acct.id) {
                continue;
            }
            if self.cooldowns.get(&acct.id).is_some_and(|until| now < *until) {
                cooled.push(acct.clone());
            } else {
                ready.push(acct.clone());
            }
        }
        ready.extend(cooled);
        ready
    }
}

struct BrokerContext {
    state: RwLock<SharedState>,
    events: Mutex<VecDeque<RelayEvent>>,
    relayed_count: AtomicU64,
    active_conns: AtomicUsize,
    log_path: PathBuf,
    log_lock: Mutex<()>,
}

impl BrokerContext {
    fn push_event(&self, event: RelayEvent) {
        let mut queue = self.events.lock().unwrap();
        if queue.len() >= 128 {
            queue.pop_front();
        }
        queue.push_back(event);
    }

    fn write_diag_log(&self, entry: &Value) {
        let _guard = self.log_lock.lock().unwrap();
        if let Some(parent) = self.log_path.parent() {
            let _ = fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent);
        }
        if let Ok(meta) = fs::metadata(&self.log_path)
            && meta.len() >= LOG_ROTATE_BYTES
        {
            let rotated = self.log_path.with_extension("log.1");
            let _ = fs::rename(&self.log_path, rotated);
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.log_path)
        {
            let _ = writeln!(file, "{entry}");
        }
    }
}

enum AdminOp {
    Install { auto_enable: bool },
    Uninstall,
}

struct AdminJob {
    op: AdminOp,
    handle: JoinHandle<Result<(), String>>,
}

pub struct Runtime {
    enabled: bool,
    root: Option<PathBuf>,
    home: Option<PathBuf>,
    sock_path: Option<PathBuf>,
    ctx: Option<Arc<BrokerContext>>,
    stop: Arc<AtomicBool>,
    listener_thread: Option<JoinHandle<()>>,
    admin_job: Option<AdminJob>,
    last_install_check: Option<Instant>,
    auto_enable_ready: bool,
}

impl Runtime {
    /// Inert runtime for tests: never binds a socket or touches `/Library/Application Support/ClaudeCode`.
    pub fn off() -> Self {
        Self {
            enabled: false,
            root: None,
            home: None,
            sock_path: None,
            ctx: None,
            stop: Arc::new(AtomicBool::new(false)),
            listener_thread: None,
            admin_job: None,
            last_install_check: None,
            auto_enable_ready: false,
        }
    }

    /// Starts the dedicated Unix socket accept thread under `<root>/claude-desktop-relay.sock`.
    pub fn start(root: &Path, home: &Path) -> Self {
        let desktop_dir = root.join("claude-desktop");
        if fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&desktop_dir)
            .is_err()
        {
            return Self::off();
        }
        let sock_path = root.join("claude-desktop-relay.sock");
        let Ok(listener) = bind_socket(&sock_path) else {
            return Self::off();
        };
        let remote_settings = home.join(".claude/remote-settings.json");
        let install_state = inspect_install_state_at(Path::new(MANAGED_DIR), &remote_settings);
        let ctx = Arc::new(BrokerContext {
            state: RwLock::new(SharedState {
                install_state,
                ..SharedState::default()
            }),
            events: Mutex::new(VecDeque::new()),
            relayed_count: AtomicU64::new(0),
            active_conns: AtomicUsize::new(0),
            log_path: desktop_dir.join("relay.log"),
            log_lock: Mutex::new(()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread_ctx = ctx.clone();
        let handle = std::thread::spawn(move || accept_loop(listener, thread_ctx, thread_stop));
        Self {
            enabled: true,
            root: Some(root.to_path_buf()),
            home: Some(home.to_path_buf()),
            sock_path: Some(sock_path),
            ctx: Some(ctx),
            stop,
            listener_thread: Some(handle),
            admin_job: None,
            last_install_check: Some(Instant::now()),
            auto_enable_ready: false,
        }
    }

    pub fn is_active_runtime(&self) -> bool {
        self.enabled && self.ctx.is_some()
    }

    pub fn refresh_install_state(&mut self) -> InstallState {
        let Some(ctx) = &self.ctx else {
            return InstallState::NotInstalled;
        };
        let Some(home) = &self.home else {
            return InstallState::NotInstalled;
        };
        let remote_settings = home.join(".claude/remote-settings.json");
        let state = inspect_install_state_at(Path::new(MANAGED_DIR), &remote_settings);
        ctx.state.write().unwrap().install_state = state;
        self.last_install_check = Some(Instant::now());
        state
    }

    pub fn start_install(&mut self, auto_enable: bool) -> Result<(), &'static str> {
        let Some(root) = self.root.clone() else {
            return Err("Claude 桌面无感换号服务未启动");
        };
        if self.ctx.is_none() {
            return Err("Claude 桌面无感换号服务未启动");
        }
        if self.admin_job.is_some() {
            return Err("正在安装或卸载插件，请稍候");
        }
        let state = self.refresh_install_state();
        if state == InstallState::BlockedRemote {
            return Err("当前组织启用了云端托管配置，无法使用本机插件注册");
        }
        if let Some(ctx) = &self.ctx {
            ctx.state.write().unwrap().last_error = None;
        }
        let stage_parent = root.join("claude-desktop");
        let handle = std::thread::spawn(move || run_admin_install(&stage_parent));
        self.admin_job = Some(AdminJob {
            op: AdminOp::Install { auto_enable },
            handle,
        });
        Ok(())
    }

    pub fn start_uninstall(&mut self) -> Result<(), &'static str> {
        let Some(root) = self.root.clone() else {
            return Err("Claude 桌面无感换号服务未启动");
        };
        let Some(ctx) = &self.ctx else {
            return Err("Claude 桌面无感换号服务未启动");
        };
        if self.admin_job.is_some() {
            return Err("正在安装或卸载插件，请稍候");
        }
        {
            let mut state = ctx.state.write().unwrap();
            state.enabled = false;
            state.active = None;
            state.fallbacks.clear();
            state.override_active = None;
            state.last_error = None;
        }
        let stage_parent = root.join("claude-desktop");
        let handle = std::thread::spawn(move || run_admin_uninstall(&stage_parent));
        self.admin_job = Some(AdminJob {
            op: AdminOp::Uninstall,
            handle,
        });
        Ok(())
    }

    /// Updates runtime state from `Accounts`. When disabled, drops all in-memory tokens immediately.
    pub fn sync(&mut self, input: RelaySyncInput) {
        let Some(ctx) = &self.ctx else {
            return;
        };
        let mut state = ctx.state.write().unwrap();
        state.enabled = input.enabled;
        state.desktop_account_uuid = input.desktop_account_uuid;
        state.desktop_org_uuid = input.desktop_org_uuid;
        state.desktop_label = input.desktop_label;
        if input.enabled {
            if let Some((override_id, _)) = &state.override_active
                && input.active.as_ref().is_some_and(|a| a.id == *override_id)
            {
                state.override_active = None;
            }
            state.active = input.active;
            state.fallbacks = input.fallbacks;
        } else {
            state.active = None;
            state.fallbacks.clear();
            state.override_active = None;
            state.cooldowns.clear();
        }
    }

    /// Polls background admin jobs and periodic install-state inspection.
    /// Returns `true` once if an install job with `auto_enable` just succeeded.
    pub fn poll(&mut self) -> bool {
        if self
            .admin_job
            .as_ref()
            .is_some_and(|j| j.handle.is_finished())
        {
            let job = self.admin_job.take().unwrap();
            let result = job
                .handle
                .join()
                .unwrap_or_else(|_| Err("插件配置过程异常中断".to_string()));
            let install_state = self.refresh_install_state();
            if let Some(ctx) = &self.ctx {
                let mut state = ctx.state.write().unwrap();
                match result {
                    Ok(()) => {
                        state.last_error = None;
                        if matches!(job.op, AdminOp::Install { auto_enable: true })
                            && install_state == InstallState::Installed
                        {
                            self.auto_enable_ready = true;
                        }
                    }
                    Err(msg) => {
                        state.last_error = Some(msg);
                    }
                }
            }
        }
        if self.ctx.is_some()
            && self
                .last_install_check
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(15))
        {
            self.refresh_install_state();
        }
        std::mem::take(&mut self.auto_enable_ready)
    }

    pub fn drain_events(&mut self) -> Vec<RelayEvent> {
        let Some(ctx) = &self.ctx else {
            return Vec::new();
        };
        let mut queue = ctx.events.lock().unwrap();
        queue.drain(..).collect()
    }

    pub fn snapshot(&self, config_enabled: bool) -> Value {
        let Some(ctx) = &self.ctx else {
            return json!({
                "enabled": false,
                "install_state": "not_installed",
                "state": "off",
                "message": "Claude 桌面 Code 无感换号未启用",
                "active_label": Value::Null,
                "same_team": Value::Null,
                "busy": false,
                "last_error": Value::Null,
                "relayed_count": 0,
                "last_switch_at": Value::Null,
            });
        };
        let state = ctx.state.read().unwrap();
        let busy_kind = self.admin_job.as_ref().map(|j| match j.op {
            AdminOp::Install { .. } => "installing",
            AdminOp::Uninstall => "uninstalling",
        });
        let same_team = state.same_team();
        let active_label = state.effective_active().map(|a| a.label.clone());
        let (phase, message) = if let Some(kind) = busy_kind {
            if kind == "installing" {
                (
                    "installing",
                    "正在安装系统插件，请在系统弹窗输入管理员密码…".to_string(),
                )
            } else {
                (
                    "uninstalling",
                    "正在卸载系统插件，请在系统弹窗输入管理员密码…".to_string(),
                )
            }
        } else if state.install_state == InstallState::BlockedRemote {
            (
                "blocked_remote",
                "当前组织启用了云端托管策略，无法使用本机插件无感换号".to_string(),
            )
        } else if let Some(err) = state.last_error.as_ref() {
            ("error", err.clone())
        } else if !config_enabled {
            (
                "off",
                if state.install_state == InstallState::NeedsUpdate {
                    "系统插件有新版本，开启时会自动更新".to_string()
                } else {
                    "同 Team 账号不重启桌面、对话不中断".to_string()
                },
            )
        } else if state.install_state == InstallState::NotInstalled {
            ("error", "系统插件未安装，请重新开启完成安装".to_string())
        } else if same_team == Some(false) {
            (
                "team_mismatch",
                "当前选中的 Claude 账号与桌面不在同一 Team，已自动回退官方登录".to_string(),
            )
        } else if state.is_relay_active() {
            let label = active_label.as_deref().unwrap_or("当前账号");
            ("active", format!("已开启 · 当前账号 {label}"))
        } else {
            (
                "waiting_account",
                "等待可用的同 Team Claude 账号，暂时回退官方登录".to_string(),
            )
        };
        json!({
            "enabled": config_enabled,
            "install_state": state.install_state.as_str(),
            "state": phase,
            "message": message,
            "active_label": active_label,
            "same_team": same_team,
            "busy": busy_kind.is_some(),
            "last_error": state.last_error,
            "relayed_count": ctx.relayed_count.load(Ordering::Relaxed),
            "last_switch_at": state.last_switch_at,
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(ctx) = &self.ctx {
            let mut state = ctx.state.write().unwrap();
            state.enabled = false;
            state.active = None;
            state.fallbacks.clear();
            state.override_active = None;
            state.cooldowns.clear();
        }
        if let Some(handle) = self.listener_thread.take() {
            let _ = handle.join();
        }
        if let Some(job) = self.admin_job.take() {
            let _ = job.handle.join();
        }
        if let Some(path) = self.sock_path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
    }
}

// ------------------------------------------------------------------------ Mod version & installation

pub fn mod_version() -> String {
    let mut hasher = Sha256::new();
    for piece in [
        EMBED_MARKETPLACE_JSON,
        EMBED_PLUGIN_JSON,
        EMBED_HOOKS_JSON,
        EMBED_REGISTER_TS,
    ] {
        hasher.update(piece.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())[..12].to_string()
}

pub fn inspect_install_state_at(managed_dir: &Path, remote_settings: &Path) -> InstallState {
    if let Ok(raw) = fs::read_to_string(remote_settings)
        && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&raw)
        && !map.is_empty()
    {
        return InstallState::BlockedRemote;
    }
    let settings_path = managed_dir.join("managed-settings.json");
    let relay_dir = managed_dir.join(MARKETPLACE_NAME);
    let register_ts = relay_dir.join("authbaton/hooks/register.ts");
    let Ok(settings_raw) = fs::read_to_string(&settings_path) else {
        return InstallState::NotInstalled;
    };
    let Ok(Value::Object(settings)) = serde_json::from_str::<Value>(&settings_raw) else {
        return InstallState::NotInstalled;
    };
    let has_mkt = settings
        .get("extraKnownMarketplaces")
        .and_then(Value::as_object)
        .is_some_and(|m| m.contains_key(MARKETPLACE_NAME));
    let has_plugin = settings
        .get("enabledPlugins")
        .and_then(Value::as_object)
        .and_then(|m| m.get(PLUGIN_ID))
        .and_then(Value::as_bool)
        == Some(true);
    if !has_mkt || !has_plugin || !register_ts.is_file() {
        return InstallState::NotInstalled;
    }
    let merge_ok = settings
        .get("parentSettingsBehavior")
        .and_then(Value::as_str)
        == Some("merge");
    let installed_version = fs::read_to_string(relay_dir.join(VERSION_FILE))
        .ok()
        .map(|s| s.trim().to_string());
    if merge_ok && installed_version.as_deref() == Some(mod_version().as_str()) {
        InstallState::Installed
    } else {
        InstallState::NeedsUpdate
    }
}

fn merge_managed_settings(existing: Option<&str>, marketplace_path: &str) -> Result<String, String> {
    let mut root: Map<String, Value> = match existing.map(str::trim).filter(|s| !s.is_empty()) {
        Some(text) => serde_json::from_str::<Value>(text)
            .map_err(|_| "现有的 managed-settings.json 格式无效，未覆盖".to_string())?
            .as_object()
            .cloned()
            .ok_or_else(|| "现有的 managed-settings.json 不是 JSON 对象，未覆盖".to_string())?,
        None => Map::new(),
    };
    root.entry("parentSettingsBehavior".to_string())
        .or_insert_with(|| Value::String("merge".to_string()));
    let marketplaces = root
        .entry("extraKnownMarketplaces".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(mkt_map) = marketplaces.as_object_mut() else {
        return Err("managed-settings.json 中的 extraKnownMarketplaces 格式无效".to_string());
    };
    mkt_map.insert(
        MARKETPLACE_NAME.to_string(),
        json!({
            "source": {
                "source": "directory",
                "path": marketplace_path,
            }
        }),
    );
    let enabled = root
        .entry("enabledPlugins".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(enabled_map) = enabled.as_object_mut() else {
        return Err("managed-settings.json 中的 enabledPlugins 格式无效".to_string());
    };
    enabled_map.insert(PLUGIN_ID.to_string(), Value::Bool(true));
    let mut out = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|_| "无法序列化 managed-settings.json".to_string())?;
    out.push('\n');
    Ok(out)
}

fn unmerge_managed_settings(existing: &str) -> Result<Option<String>, String> {
    let trimmed = existing.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let mut root = serde_json::from_str::<Value>(trimmed)
        .map_err(|_| "现有的 managed-settings.json 格式无效，未修改".to_string())?
        .as_object()
        .cloned()
        .ok_or_else(|| "现有的 managed-settings.json 不是 JSON 对象，未修改".to_string())?;
    if let Some(mkt_map) = root
        .get_mut("extraKnownMarketplaces")
        .and_then(Value::as_object_mut)
    {
        mkt_map.remove(MARKETPLACE_NAME);
        if mkt_map.is_empty() {
            root.remove("extraKnownMarketplaces");
        }
    }
    if let Some(enabled_map) = root
        .get_mut("enabledPlugins")
        .and_then(Value::as_object_mut)
    {
        enabled_map.remove(PLUGIN_ID);
        if enabled_map.is_empty() {
            root.remove("enabledPlugins");
        }
    }
    let only_merge = root.len() == 1
        && root
            .get("parentSettingsBehavior")
            .and_then(Value::as_str)
            == Some("merge");
    if root.is_empty() || only_merge {
        return Ok(None);
    }
    let mut out = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|_| "无法序列化 managed-settings.json".to_string())?;
    out.push('\n');
    Ok(Some(out))
}

struct StageCleanup(PathBuf);
impl Drop for StageCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_staged_mod(stage_dir: &Path) -> Result<(), String> {
    let mkt = stage_dir.join(MARKETPLACE_NAME);
    let mkt_meta = mkt.join(".claude-plugin");
    let plugin_meta = mkt.join("authbaton/.claude-plugin");
    let hooks_dir = mkt.join("authbaton/hooks");
    for dir in [&mkt_meta, &plugin_meta, &hooks_dir] {
        fs::create_dir_all(dir).map_err(|_| "无法创建插件暂存目录".to_string())?;
    }
    fs::write(mkt_meta.join("marketplace.json"), EMBED_MARKETPLACE_JSON)
        .map_err(|_| "无法写入 marketplace.json".to_string())?;
    fs::write(plugin_meta.join("plugin.json"), EMBED_PLUGIN_JSON)
        .map_err(|_| "无法写入 plugin.json".to_string())?;
    fs::write(hooks_dir.join("hooks.json"), EMBED_HOOKS_JSON)
        .map_err(|_| "无法写入 hooks.json".to_string())?;
    fs::write(hooks_dir.join("register.ts"), EMBED_REGISTER_TS)
        .map_err(|_| "无法写入 register.ts".to_string())?;
    fs::write(mkt.join(VERSION_FILE), format!("{}\n", mod_version()))
        .map_err(|_| "无法写入插件版本标记".to_string())?;
    Ok(())
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn run_osascript_admin(shell_script: &str) -> Result<(), String> {
    let output = Command::new("/usr/bin/osascript")
        .env_clear()
        .args([
            "-e",
            "on run argv",
            "-e",
            "do shell script (item 1 of argv) with administrator privileges",
            "-e",
            "end run",
            "--",
            shell_script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|_| "无法唤起 macOS 管理员授权窗口".to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("User canceled") || stderr.contains("(-128)") {
        Err("已取消管理员授权，未修改系统文件".to_string())
    } else {
        Err("系统目录写入未完成，请重试管理员授权".to_string())
    }
}

fn build_install_script(
    managed_dir: &str,
    staged_mkt: &Path,
    staged_settings: &Path,
    with_chown: bool,
) -> String {
    let target_mkt = format!("{managed_dir}/{MARKETPLACE_NAME}");
    let target_mkt_tmp = format!("{target_mkt}.tmp");
    let target_settings = format!("{managed_dir}/managed-settings.json");
    let target_settings_tmp = format!("{target_settings}.tmp");
    let chown_mkt = if with_chown {
        format!("/usr/sbin/chown -R root:wheel {}\n", sh_quote(&target_mkt_tmp))
    } else {
        String::new()
    };
    let chown_settings = if with_chown {
        format!(
            "/usr/sbin/chown root:wheel {}\n",
            sh_quote(&target_settings_tmp)
        )
    } else {
        String::new()
    };
    format!(
        "set -e\n\
         /bin/mkdir -p {managed}\n\
         /bin/rm -rf {mkt_tmp}\n\
         /bin/cp -R {src_mkt} {mkt_tmp}\n\
         {chown_mkt}\
         /bin/chmod -R a+rX,go-w {mkt_tmp}\n\
         /bin/rm -rf {mkt}\n\
         /bin/mv {mkt_tmp} {mkt}\n\
         /bin/cp {src_settings} {settings_tmp}\n\
         {chown_settings}\
         /bin/chmod 0644 {settings_tmp}\n\
         /bin/mv {settings_tmp} {settings}",
        managed = sh_quote(managed_dir),
        mkt_tmp = sh_quote(&target_mkt_tmp),
        src_mkt = sh_quote(&staged_mkt.to_string_lossy()),
        mkt = sh_quote(&target_mkt),
        src_settings = sh_quote(&staged_settings.to_string_lossy()),
        settings_tmp = sh_quote(&target_settings_tmp),
        settings = sh_quote(&target_settings),
    )
}

fn build_uninstall_script(
    managed_dir: &str,
    staged_settings: Option<&Path>,
    with_chown: bool,
) -> String {
    let target_mkt = format!("{managed_dir}/{MARKETPLACE_NAME}");
    let target_settings = format!("{managed_dir}/managed-settings.json");
    let settings_step = if let Some(staged) = staged_settings {
        let target_settings_tmp = format!("{target_settings}.tmp");
        let chown_settings = if with_chown {
            format!(
                "/usr/sbin/chown root:wheel {}\n",
                sh_quote(&target_settings_tmp)
            )
        } else {
            String::new()
        };
        format!(
            "/bin/cp {src} {tmp}\n\
             {chown_settings}\
             /bin/chmod 0644 {tmp}\n\
             /bin/mv {tmp} {dst}",
            src = sh_quote(&staged.to_string_lossy()),
            tmp = sh_quote(&target_settings_tmp),
            dst = sh_quote(&target_settings),
        )
    } else {
        format!(
            "/bin/rm -f {dst}\n\
             /bin/rmdir {managed} 2>/dev/null || true",
            dst = sh_quote(&target_settings),
            managed = sh_quote(managed_dir),
        )
    };
    format!(
        "set -e\n\
         /bin/rm -rf {mkt}\n\
         {settings_step}",
        mkt = sh_quote(&target_mkt),
    )
}

fn run_admin_install(stage_parent: &Path) -> Result<(), String> {
    let stage_dir = stage_parent.join(format!("relay-stage-install-{}", std::process::id()));
    let _ = fs::remove_dir_all(&stage_dir);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&stage_dir)
        .map_err(|_| "无法创建插件安装暂存目录".to_string())?;
    let _cleanup = StageCleanup(stage_dir.clone());
    write_staged_mod(&stage_dir)?;

    let managed_settings_path = Path::new(MANAGED_DIR).join("managed-settings.json");
    let existing = match fs::read_to_string(&managed_settings_path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return Err("无法读取现有 managed-settings.json".to_string()),
    };
    let target_mkt = format!("{MANAGED_DIR}/{MARKETPLACE_NAME}");
    let merged = merge_managed_settings(existing.as_deref(), &target_mkt)?;
    let staged_settings = stage_dir.join("managed-settings.json");
    fs::write(&staged_settings, merged)
        .map_err(|_| "无法写入暂存 managed-settings.json".to_string())?;

    let staged_mkt = stage_dir.join(MARKETPLACE_NAME);
    let script = build_install_script(MANAGED_DIR, &staged_mkt, &staged_settings, true);
    run_osascript_admin(&script)
}

fn run_admin_uninstall(stage_parent: &Path) -> Result<(), String> {
    let stage_dir = stage_parent.join(format!("relay-stage-uninstall-{}", std::process::id()));
    let _ = fs::remove_dir_all(&stage_dir);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&stage_dir)
        .map_err(|_| "无法创建插件卸载暂存目录".to_string())?;
    let _cleanup = StageCleanup(stage_dir.clone());

    let managed_settings_path = Path::new(MANAGED_DIR).join("managed-settings.json");
    let remaining_settings = match fs::read_to_string(&managed_settings_path) {
        Ok(text) => unmerge_managed_settings(&text)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return Err("无法读取现有 managed-settings.json".to_string()),
    };

    let staged_settings = if let Some(json_text) = remaining_settings {
        let path = stage_dir.join("managed-settings.json");
        fs::write(&path, json_text)
            .map_err(|_| "无法写入卸载后配置暂存文件".to_string())?;
        Some(path)
    } else {
        None
    };

    let script = build_uninstall_script(MANAGED_DIR, staged_settings.as_deref(), true);
    run_osascript_admin(&script)
}

// ------------------------------------------------------------------------ Local Unix socket broker

fn bind_socket(path: &Path) -> Result<UnixListener, &'static str> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {
            fs::remove_file(path).map_err(|_| "无法清理旧桌面无感换号 socket")?;
        }
        Ok(_) => return Err("桌面无感换号 socket 路径被占用"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法检查桌面无感换号 socket"),
    }
    let listener = UnixListener::bind(path).map_err(|_| "无法启动桌面无感换号接口")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| "无法保护桌面无感换号 socket")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "无法设置桌面无感换号 socket")?;
    Ok(listener)
}

struct ConnGuard(Arc<BrokerContext>);
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.active_conns.fetch_sub(1, Ordering::Relaxed);
    }
}

fn accept_loop(listener: UnixListener, ctx: Arc<BrokerContext>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                if ctx.active_conns.fetch_add(1, Ordering::Relaxed) >= MAX_CONCURRENT_CONNS {
                    ctx.active_conns.fetch_sub(1, Ordering::Relaxed);
                    continue;
                }
                let worker_ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _guard = ConnGuard(worker_ctx.clone());
                    let _ = handle_connection(stream, &worker_ctx);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn same_user(stream: &UnixStream) -> io::Result<()> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: getpeereid only writes the two out-parameters for this connected socket.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0
        || uid != unsafe { libc::geteuid() }
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

#[derive(Deserialize, Default)]
struct LeaseRequest {
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct OutcomeRequest {
    id: String,
    status: u16,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    ms: Option<u64>,
    #[serde(default)]
    uid_rewritten: bool,
    #[serde(default)]
    switched: bool,
    #[serde(default)]
    switched_from: Option<String>,
    #[serde(default)]
    rl_status: Option<String>,
    #[serde(default)]
    rl_5h: Option<f64>,
    #[serde(default)]
    rl_7d: Option<f64>,
}

enum Route {
    Lease,
    Outcome,
}

fn handle_connection(mut stream: UnixStream, ctx: &BrokerContext) -> io::Result<()> {
    // An accepted socket inherits the listener's non-blocking flag on macOS.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    same_user(&stream)?;

    let (status, payload) = match read_request(&mut stream) {
        Ok((Route::Lease, body)) => {
            let req = if body.is_empty() {
                LeaseRequest::default()
            } else {
                match serde_json::from_slice::<LeaseRequest>(&body) {
                    Ok(r) => r,
                    Err(_) => {
                        return write_json_reply(
                            &mut stream,
                            "400 Bad Request",
                            &json!({"error": "invalid lease request"}),
                        );
                    }
                }
            };
            let _ = &req.model;
            let state = ctx.state.read().unwrap();
            let candidates = state.candidates(&req.exclude);
            if let Some(first) = candidates.first() {
                (
                    "200 OK",
                    json!({
                        "active": true,
                        "id": first.id,
                        "label": first.label,
                        "account_uuid": first.account_uuid,
                        "access_token": first.access_token,
                        "has_fallback": candidates.len() > 1,
                    }),
                )
            } else {
                ("200 OK", json!({"active": false}))
            }
        }
        Ok((Route::Outcome, body)) => match serde_json::from_slice::<OutcomeRequest>(&body) {
            Ok(outcome) if !outcome.id.is_empty() && outcome.id.len() <= 128 => {
                apply_outcome(ctx, outcome);
                ("200 OK", json!({"ok": true}))
            }
            _ => ("400 Bad Request", json!({"error": "invalid outcome"})),
        },
        Err(code) => (code, json!({"error": code})),
    };
    write_json_reply(&mut stream, status, &payload)
}

fn apply_outcome(ctx: &BrokerContext, outcome: OutcomeRequest) {
    let model = outcome.model.filter(|m| m.len() <= 128);
    let primary_used = match (outcome.rl_5h, outcome.rl_7d) {
        (Some(a), Some(b)) => Some((a.max(b) * 100.0).clamp(0.0, 100.0)),
        (Some(a), None) | (None, Some(a)) => Some((a * 100.0).clamp(0.0, 100.0)),
        (None, None) => None,
    };

    if outcome.status == 429 {
        let mut state = ctx.state.write().unwrap();
        state
            .cooldowns
            .insert(outcome.id.clone(), Instant::now() + RATE_LIMIT_COOLDOWN);
    } else if outcome.status > 0 {
        ctx.relayed_count.fetch_add(1, Ordering::Relaxed);
    }

    if outcome.switched && outcome.status != 429 && outcome.status != 401 {
        let now_secs = unix_secs();
        {
            let mut state = ctx.state.write().unwrap();
            state.override_active =
                Some((outcome.id.clone(), Instant::now() + OVERRIDE_ACTIVE_TTL));
            state.last_switch_at = Some(now_secs);
        }
        ctx.push_event(RelayEvent::Switched {
            to_id: outcome.id.clone(),
        });
    }

    if outcome.status == 401 {
        ctx.push_event(RelayEvent::Unauthorized {
            account_id: outcome.id.clone(),
        });
    } else if primary_used.is_some() || outcome.status == 429 || model.is_some() {
        ctx.push_event(RelayEvent::Observed {
            account_id: outcome.id.clone(),
            model: model.clone(),
            primary_used: primary_used.or_else(|| (outcome.status == 429).then_some(100.0)),
            limited: outcome.status == 429,
        });
    }

    let _ = &outcome.switched_from;
    ctx.write_diag_log(&json!({
        "at": unix_secs(),
        "path": "/v1/messages",
        "model": model,
        "status": outcome.status,
        "ms": outcome.ms.unwrap_or(0),
        "switched": outcome.switched,
        "uid_rewritten": outcome.uid_rewritten,
        "rl_status": outcome.rl_status,
        "rl_5h": outcome.rl_5h,
        "rl_7d": outcome.rl_7d,
    }));
}

fn read_request(stream: &mut UnixStream) -> Result<(Route, Vec<u8>), &'static str> {
    const BAD: &str = "400 Bad Request";
    let mut reader = BufReader::new(stream.take(MAX_REQUEST_BYTES as u64));
    let mut line = String::new();
    if reader.read_line(&mut line).map_err(|_| BAD)? == 0 {
        return Err(BAD);
    }
    let mut parts = line.split_whitespace();
    if parts.next() != Some("POST") {
        return Err("405 Method Not Allowed");
    }
    let route = match parts.next() {
        Some("/v1/lease") => Route::Lease,
        Some("/v1/outcome") => Route::Outcome,
        _ => return Err("404 Not Found"),
    };
    let mut length: Option<usize> = None;
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|_| BAD)? == 0 {
            return Err(BAD);
        }
        let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>().map_err(|_| BAD)?);
        }
    }
    let len = length.unwrap_or(0);
    if len > MAX_BODY_BYTES {
        return Err("413 Content Too Large");
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut body).map_err(|_| BAD)?;
    }
    Ok((route, body))
}

fn write_json_reply(stream: &mut UnixStream, status: &str, body: &Value) -> io::Result<()> {
    let payload = body.to_string();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    )?;
    stream.flush()
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ------------------------------------------------------------------------ Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "authbaton-direct-swap-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn exchange(ctx: &BrokerContext, path: &str, body: &Value) -> Value {
        let payload = body.to_string();
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: authbaton\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
            payload.len()
        );
        let (mut client, server) = UnixStream::pair().unwrap();
        client.write_all(req.as_bytes()).unwrap();
        handle_connection(server, ctx).unwrap();
        let mut raw = String::new();
        client.read_to_string(&mut raw).unwrap();
        let (_, json_part) = raw.split_once("\r\n\r\n").unwrap();
        serde_json::from_str(json_part).unwrap()
    }

    #[test]
    fn managed_settings_merge_and_unmerge_preserve_user_keys() {
        let existing = r#"{"customPolicy": true, "enabledPlugins": {"other@mkt": true}}"#;
        let merged = merge_managed_settings(
            Some(existing),
            "/Library/Application Support/ClaudeCode/authbaton-desktop-relay",
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed["customPolicy"], true);
        assert_eq!(parsed["parentSettingsBehavior"], "merge");
        assert_eq!(parsed["enabledPlugins"]["other@mkt"], true);
        assert_eq!(parsed["enabledPlugins"][PLUGIN_ID], true);

        let after = unmerge_managed_settings(&merged).unwrap().unwrap();
        let parsed_after: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(parsed_after["customPolicy"], true);
        assert_eq!(parsed_after["enabledPlugins"]["other@mkt"], true);
        assert!(parsed_after["enabledPlugins"].get(PLUGIN_ID).is_none());
        assert!(parsed_after.get("extraKnownMarketplaces").is_none());

        let fresh = merge_managed_settings(
            None,
            "/Library/Application Support/ClaudeCode/authbaton-desktop-relay",
        )
        .unwrap();
        assert!(unmerge_managed_settings(&fresh).unwrap().is_none());
    }

    #[test]
    fn install_state_detects_remote_block_missing_and_version_drift() {
        let dir = temp_dir("install-state");
        let managed = dir.join("ClaudeCode");
        let remote = dir.join("remote-settings.json");

        fs::write(&remote, "{}").unwrap();
        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::NotInstalled
        );

        write_staged_mod(&managed).unwrap();
        let settings = merge_managed_settings(
            None,
            &managed.join(MARKETPLACE_NAME).to_string_lossy(),
        )
        .unwrap();
        fs::write(managed.join("managed-settings.json"), &settings).unwrap();
        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::Installed
        );

        fs::write(
            managed.join(MARKETPLACE_NAME).join(VERSION_FILE),
            "oldhash\n",
        )
        .unwrap();
        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::NeedsUpdate
        );

        fs::write(&remote, r#"{"managedPolicy": true}"#).unwrap();
        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::BlockedRemote
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_and_uninstall_scripts_complete_full_closed_loop() {
        let dir = temp_dir("install-uninstall-loop");
        let managed = dir.join("ClaudeCode");
        let managed_str = managed.to_string_lossy().to_string();
        let remote = dir.join("remote-settings.json");
        fs::write(&remote, "{}").unwrap();
        let stage = dir.join("stage");
        fs::create_dir_all(&stage).unwrap();

        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::NotInstalled
        );
        write_staged_mod(&stage).unwrap();
        let target_mkt = format!("{managed_str}/{MARKETPLACE_NAME}");
        let merged = merge_managed_settings(None, &target_mkt).unwrap();
        let staged_settings = stage.join("managed-settings.json");
        fs::write(&staged_settings, &merged).unwrap();
        let install_sh = build_install_script(
            &managed_str,
            &stage.join(MARKETPLACE_NAME),
            &staged_settings,
            false,
        );
        assert!(Command::new("/bin/sh").arg("-c").arg(&install_sh).status().unwrap().success());
        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::Installed
        );
        assert_eq!(
            fs::metadata(managed.join("managed-settings.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );

        let existing = fs::read_to_string(managed.join("managed-settings.json")).unwrap();
        assert!(unmerge_managed_settings(&existing).unwrap().is_none());
        let uninstall_sh = build_uninstall_script(&managed_str, None, false);
        assert!(Command::new("/bin/sh").arg("-c").arg(&uninstall_sh).status().unwrap().success());
        assert!(!managed.exists());
        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::NotInstalled
        );

        // Preserve user's other keys on uninstall.
        fs::create_dir_all(&managed).unwrap();
        fs::write(
            managed.join("managed-settings.json"),
            r#"{"cleanupPeriodDays": 365, "enabledPlugins": {"other@corp": true}}"#,
        )
        .unwrap();
        let existing2 = fs::read_to_string(managed.join("managed-settings.json")).unwrap();
        let merged2 = merge_managed_settings(Some(&existing2), &target_mkt).unwrap();
        fs::write(&staged_settings, &merged2).unwrap();
        assert!(Command::new("/bin/sh").arg("-c").arg(&install_sh).status().unwrap().success());
        let after_install = fs::read_to_string(managed.join("managed-settings.json")).unwrap();
        let remaining = unmerge_managed_settings(&after_install).unwrap().unwrap();
        let staged_rem = stage.join("managed-settings-rem.json");
        fs::write(&staged_rem, &remaining).unwrap();
        let uninstall_preserve = build_uninstall_script(&managed_str, Some(&staged_rem), false);
        assert!(Command::new("/bin/sh").arg("-c").arg(&uninstall_preserve).status().unwrap().success());
        let final_settings: Value = serde_json::from_str(
            &fs::read_to_string(managed.join("managed-settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(final_settings["cleanupPeriodDays"], 365);
        assert_eq!(final_settings["enabledPlugins"]["other@corp"], true);
        assert!(final_settings["enabledPlugins"].get(PLUGIN_ID).is_none());
        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::NotInstalled
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_team_enforcement_blocks_cross_team_and_allows_same_team() {
        let mut st = SharedState {
            enabled: true,
            install_state: InstallState::Installed,
            desktop_org_uuid: Some("org-team-1".into()),
            active: Some(RelayAccount {
                id: "a1".into(),
                label: "账号A".into(),
                account_uuid: "acct-1".into(),
                org_uuid: "org-team-1".into(),
                access_token: "synthetic-tok-a".into(),
            }),
            ..SharedState::default()
        };
        assert_eq!(st.same_team(), Some(true));
        assert!(st.is_relay_active());

        st.active.as_mut().unwrap().org_uuid = "org-other-2".into();
        assert_eq!(st.same_team(), Some(false));
        assert!(!st.is_relay_active());
        assert!(st.candidates(&[]).is_empty());
    }

    #[test]
    fn lease_and_outcome_handle_failover_cooldown_401_and_fail_open_without_leaking_secrets() {
        let dir = temp_dir("lease-outcome");
        let log_path = dir.join("relay.log");
        let ctx = BrokerContext {
            state: RwLock::new(SharedState {
                enabled: true,
                install_state: InstallState::Installed,
                desktop_org_uuid: Some("org-same".into()),
                active: Some(RelayAccount {
                    id: "acct-a".into(),
                    label: "主号 A".into(),
                    account_uuid: "uuid-a".into(),
                    org_uuid: "org-same".into(),
                    access_token: "synthetic-secret-a".into(),
                }),
                fallbacks: vec![RelayAccount {
                    id: "acct-b".into(),
                    label: "备用 B".into(),
                    account_uuid: "uuid-b".into(),
                    org_uuid: "org-same".into(),
                    access_token: "synthetic-secret-b".into(),
                }],
                ..SharedState::default()
            }),
            events: Mutex::new(VecDeque::new()),
            relayed_count: AtomicU64::new(0),
            active_conns: AtomicUsize::new(0),
            log_path: log_path.clone(),
            log_lock: Mutex::new(()),
        };

        // 1. Initial lease returns Account A with has_fallback: true
        let lease1 = exchange(&ctx, "/v1/lease", &json!({"exclude": []}));
        assert_eq!(lease1["active"], true);
        assert_eq!(lease1["id"], "acct-a");
        assert_eq!(lease1["account_uuid"], "uuid-a");
        assert_eq!(lease1["access_token"], "synthetic-secret-a");
        assert_eq!(lease1["has_fallback"], true);

        // 2. Report 429 for Account A -> puts A in cooldown and leases Account B next
        let out429 = exchange(
            &ctx,
            "/v1/outcome",
            &json!({"id": "acct-a", "status": 429, "model": "claude-fable-5-2", "rl_5h": 1.0}),
        );
        assert_eq!(out429["ok"], true);

        let lease2 = exchange(&ctx, "/v1/lease", &json!({"exclude": ["acct-a"]}));
        assert_eq!(lease2["active"], true);
        assert_eq!(lease2["id"], "acct-b");
        assert_eq!(lease2["access_token"], "synthetic-secret-b");
        assert_eq!(lease2["has_fallback"], false);

        // 3. Report 200 switched outcome for B -> updates override_active + emits Switched event
        let out200 = exchange(
            &ctx,
            "/v1/outcome",
            &json!({
                "id": "acct-b", "status": 200, "model": "claude-fable-5-2",
                "ms": 420, "uid_rewritten": true, "switched": true,
                "switched_from": "主号 A", "rl_status": "allowed", "rl_5h": 0.12
            }),
        );
        assert_eq!(out200["ok"], true);
        assert_eq!(ctx.relayed_count.load(Ordering::Relaxed), 1);

        // Subsequent lease (empty exclude) immediately returns B (override_active)
        let lease3 = exchange(&ctx, "/v1/lease", &json!({"exclude": []}));
        assert_eq!(lease3["id"], "acct-b");

        // 4. Report 401 -> emits Unauthorized event
        let _ = exchange(&ctx, "/v1/outcome", &json!({"id": "acct-b", "status": 401}));
        let events: Vec<RelayEvent> = ctx.events.lock().unwrap().drain(..).collect();
        assert!(events.iter().any(|e| matches!(e, RelayEvent::Switched { to_id } if to_id == "acct-b")));
        assert!(events.iter().any(|e| matches!(e, RelayEvent::Unauthorized { account_id } if account_id == "acct-b")));

        // 5. Disabled or cross-Team -> Fail-Open active: false
        ctx.state.write().unwrap().enabled = false;
        let lease_off = exchange(&ctx, "/v1/lease", &json!({"exclude": []}));
        assert_eq!(lease_off["active"], false);
        assert!(lease_off.get("access_token").is_none());

        // 6. Diagnostics log never contains credentials or UUIDs
        let log_text = fs::read_to_string(&log_path).unwrap();
        assert!(log_text.contains("\"switched\":true"));
        assert!(!log_text.contains("synthetic-secret"));
        assert!(!log_text.contains("uuid-a"));
        assert!(!log_text.contains("uuid-b"));
        assert!(!log_text.contains("org-same"));

        let _ = fs::remove_dir_all(&dir);
    }
}
