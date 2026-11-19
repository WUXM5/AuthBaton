//! Seamless same-Team account switching for Claude Desktop's Code tab.
//!
//! Architecture:
//! - A tiny vouched managed Mod (`integrations/claude-desktop-relay/`) is installed into
//!   `/Library/Application Support/ClaudeCode/authbaton-desktop-relay/` and registered in
//!   `/Library/Application Support/ClaudeCode/managed-settings.json` with
//!   `"parentSettingsBehavior": "merge"` via an explicit user-confirmed macOS admin prompt.
//! - The Mod holds zero credentials. Only when `CLAUDE_CODE_ENTRYPOINT === 'claude-desktop'`,
//!   it checks `GET /v1/health` over `<root>/claude-desktop-relay.sock` (0600, same-UID) and,
//!   when active, strips `Authorization` and forwards only engine `POST /v1/messages` requests
//!   (skipping `/count_tokens`) to this local Unix socket.
//! - This module verifies same-Team (`organizationUuid` matches Desktop's signed-in org), rewrites
//!   `metadata.user_id.account_uuid` to the active AuthBaton Claude account, injects its Bearer
//!   token strictly via `/usr/bin/curl --config -` stdin (never argv, env, or disk), streams SSE
//!   responses chunk-by-chunk, and transparently fails over to the next same-Team candidate on 429.
//! - Fail-Open: if AuthBaton stops, the switch is off, or same-Team fails, `/v1/health` reports
//!   `active: false` (and any racing POST receives `503` with `X-AuthBaton-Pass: 1`), so the Mod
//!   immediately falls back to Claude Desktop's native request path.

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
const UPSTREAM_BASE_DEFAULT: &str = "https://api.anthropic.com";

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
const MAX_GZIP_OUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_CONCURRENT_CONNS: usize = 64;
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);
const OVERRIDE_ACTIVE_TTL: Duration = Duration::from_secs(60);
const LOG_ROTATE_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallState {
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

struct SharedState {
    enabled: bool,
    install_state: InstallState,
    desktop_account_uuid: Option<String>,
    desktop_org_uuid: Option<String>,
    desktop_label: Option<String>,
    active: Option<RelayAccount>,
    fallbacks: Vec<RelayAccount>,
    /// Set briefly when the relay transparently switches on 429 before `Accounts::tick()` activates it.
    override_active: Option<(String, Instant)>,
    cooldowns: HashMap<String, Instant>,
    last_switch_at: Option<u64>,
    last_error: Option<String>,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            enabled: false,
            install_state: InstallState::NotInstalled,
            desktop_account_uuid: None,
            desktop_org_uuid: None,
            desktop_label: None,
            active: None,
            fallbacks: Vec::new(),
            override_active: None,
            cooldowns: HashMap::new(),
            last_switch_at: None,
            last_error: None,
        }
    }
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

    /// Returns ordered same-Team candidates to attempt for a request (primary first, then fallbacks).
    fn candidates(&self) -> Vec<RelayAccount> {
        if !self.is_relay_active() {
            return Vec::new();
        }
        let now = Instant::now();
        let Some(primary) = self.effective_active() else {
            return Vec::new();
        };
        let target_org = primary.org_uuid.clone();
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
        // Keep primary first unless it is currently in 429 cooldown and a non-cooldown fallback exists.
        let mut ready: Vec<RelayAccount> = Vec::new();
        let mut cooled: Vec<RelayAccount> = Vec::new();
        for acct in all {
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

struct RelayContext {
    state: RwLock<SharedState>,
    events: Mutex<VecDeque<RelayEvent>>,
    relayed_count: AtomicU64,
    active_conns: AtomicUsize,
    upstream_base: String,
    #[cfg(test)]
    upstream_unix_socket: Option<PathBuf>,
    tmp_dir: PathBuf,
    log_path: PathBuf,
    log_lock: Mutex<()>,
    use_system_proxy: bool,
}

impl RelayContext {
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
    ctx: Option<Arc<RelayContext>>,
    stop: Arc<AtomicBool>,
    listener_thread: Option<JoinHandle<()>>,
    admin_job: Option<AdminJob>,
    last_install_check: Option<Instant>,
    auto_enable_ready: bool,
}

impl Runtime {
    /// Inert runtime for tests: never binds a socket, never touches `/Library/Application Support/ClaudeCode`.
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
        let tmp_dir = desktop_dir.join("relay-tmp");
        let _ = fs::remove_dir_all(&tmp_dir);
        if fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&tmp_dir)
            .is_err()
        {
            return Self::off();
        }
        let sock_path = root.join("claude-desktop-relay.sock");
        let Ok(listener) = bind_relay_socket(&sock_path) else {
            return Self::off();
        };
        let remote_settings = home.join(".claude/remote-settings.json");
        let install_state = inspect_install_state_at(Path::new(MANAGED_DIR), &remote_settings);
        let ctx = Arc::new(RelayContext {
            state: RwLock::new(SharedState {
                install_state,
                ..SharedState::default()
            }),
            events: Mutex::new(VecDeque::new()),
            relayed_count: AtomicU64::new(0),
            active_conns: AtomicUsize::new(0),
            upstream_base: UPSTREAM_BASE_DEFAULT.to_string(),
            #[cfg(test)]
            upstream_unix_socket: None,
            tmp_dir,
            log_path: desktop_dir.join("relay.log"),
            log_lock: Mutex::new(()),
            use_system_proxy: true,
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
            return Err("Claude 桌面接力服务未启动");
        };
        if self.ctx.is_none() {
            return Err("Claude 桌面接力服务未启动");
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
            return Err("Claude 桌面接力服务未启动");
        };
        let Some(ctx) = &self.ctx else {
            return Err("Claude 桌面接力服务未启动");
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

    /// Updates relay runtime state from `Accounts`. When disabled, drops all in-memory tokens.
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
                ("installing", "正在安装系统插件，请在系统弹窗输入管理员密码…".to_string())
            } else {
                ("uninstalling", "正在卸载系统插件，请在系统弹窗输入管理员密码…".to_string())
            }
        } else if state.install_state == InstallState::BlockedRemote {
            (
                "blocked_remote",
                "当前组织启用了云端托管策略，无法使用本机插件接力".to_string(),
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
                "当前选中的 Claude 账号与桌面不在同一 Team，已自动回退官方直连".to_string(),
            )
        } else if state.is_relay_active() {
            let label = active_label.as_deref().unwrap_or("当前账号");
            ("active", format!("已开启 · 当前账号 {label}"))
        } else {
            (
                "waiting_account",
                "等待可用的同 Team Claude 账号，暂时回退官方直连".to_string(),
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
        if let Some(ctx) = &self.ctx {
            let _ = fs::remove_dir_all(&ctx.tmp_dir);
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
    let target_mkt_tmp = format!("{target_mkt}.tmp");
    let target_settings = format!("{MANAGED_DIR}/managed-settings.json");
    let target_settings_tmp = format!("{target_settings}.tmp");
    let script = format!(
        "set -e\n\
         /bin/mkdir -p {managed}\n\
         /bin/rm -rf {mkt_tmp}\n\
         /bin/cp -R {src_mkt} {mkt_tmp}\n\
         /usr/sbin/chown -R root:wheel {mkt_tmp}\n\
         /bin/chmod -R a+rX,go-w {mkt_tmp}\n\
         /bin/rm -rf {mkt}\n\
         /bin/mv {mkt_tmp} {mkt}\n\
         /bin/cp {src_settings} {settings_tmp}\n\
         /usr/sbin/chown root:wheel {settings_tmp}\n\
         /bin/chmod 0644 {settings_tmp}\n\
         /bin/mv {settings_tmp} {settings}",
        managed = sh_quote(MANAGED_DIR),
        mkt_tmp = sh_quote(&target_mkt_tmp),
        src_mkt = sh_quote(&staged_mkt.to_string_lossy()),
        mkt = sh_quote(&target_mkt),
        src_settings = sh_quote(&staged_settings.to_string_lossy()),
        settings_tmp = sh_quote(&target_settings_tmp),
        settings = sh_quote(&target_settings),
    );
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
    let target_mkt = format!("{MANAGED_DIR}/{MARKETPLACE_NAME}");
    let target_settings = format!("{MANAGED_DIR}/managed-settings.json");
    let remaining_settings = match fs::read_to_string(&managed_settings_path) {
        Ok(text) => unmerge_managed_settings(&text)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return Err("无法读取现有 managed-settings.json".to_string()),
    };

    let settings_step = if let Some(json_text) = remaining_settings {
        let staged_settings = stage_dir.join("managed-settings.json");
        fs::write(&staged_settings, json_text)
            .map_err(|_| "无法写入卸载后配置暂存文件".to_string())?;
        let target_settings_tmp = format!("{target_settings}.tmp");
        format!(
            "/bin/cp {src} {tmp}\n\
             /usr/sbin/chown root:wheel {tmp}\n\
             /bin/chmod 0644 {tmp}\n\
             /bin/mv {tmp} {dst}",
            src = sh_quote(&staged_settings.to_string_lossy()),
            tmp = sh_quote(&target_settings_tmp),
            dst = sh_quote(&target_settings),
        )
    } else {
        format!(
            "/bin/rm -f {dst}\n\
             /bin/rmdir {managed} 2>/dev/null || true",
            dst = sh_quote(&target_settings),
            managed = sh_quote(MANAGED_DIR),
        )
    };

    let script = format!(
        "set -e\n\
         /bin/rm -rf {mkt}\n\
         {settings_step}",
        mkt = sh_quote(&target_mkt),
    );
    run_osascript_admin(&script)
}

// ------------------------------------------------------------------------ Unix socket listener

fn bind_relay_socket(path: &Path) -> Result<UnixListener, &'static str> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {
            fs::remove_file(path).map_err(|_| "无法清理旧桌面接力 socket")?;
        }
        Ok(_) => return Err("桌面接力 socket 路径被占用"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法检查桌面接力 socket"),
    }
    let listener = UnixListener::bind(path).map_err(|_| "无法启动桌面接力 socket")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| "无法保护桌面接力 socket")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "无法设置桌面接力 socket")?;
    Ok(listener)
}

struct ConnGuard(Arc<RelayContext>);
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.active_conns.fetch_sub(1, Ordering::Relaxed);
    }
}

fn accept_loop(listener: UnixListener, ctx: Arc<RelayContext>, stop: Arc<AtomicBool>) {
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

// ------------------------------------------------------------------------ HTTP/1.1 handling

struct HttpRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn handle_connection(mut stream: UnixStream, ctx: &RelayContext) -> io::Result<()> {
    // An accepted socket inherits the listener's non-blocking flag on macOS.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(60)))?;
    same_user(&stream)?;

    let req = match read_http_request(&mut stream) {
        Ok(req) => req,
        Err((status, msg)) => {
            return write_json_reply(&mut stream, status, &[], &json!({"error": msg}));
        }
    };

    if req.method == "GET" && req.target == "/v1/health" {
        let state = ctx.state.read().unwrap();
        let active = state.is_relay_active();
        let label = if active {
            state
                .effective_active()
                .map(|a| a.label.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        return write_json_reply(
            &mut stream,
            "200 OK",
            &[],
            &json!({"active": active, "label": label}),
        );
    }

    if req.method == "POST" && is_messages_target(&req.target) {
        return handle_messages(&mut stream, req, ctx);
    }

    write_json_reply(
        &mut stream,
        "404 Not Found",
        &[],
        &json!({"error": "not found"}),
    )
}

fn is_messages_target(target: &str) -> bool {
    let path = target.split('?').next().unwrap_or(target);
    path == "/v1/messages"
}

fn read_http_request(stream: &mut UnixStream) -> Result<HttpRequest, (&'static str, &'static str)> {
    let mut reader = BufReader::new(stream);
    let mut header_bytes = 0usize;

    let mut request_line = String::new();
    let n = reader
        .read_line(&mut request_line)
        .map_err(|_| ("400 Bad Request", "bad request line"))?;
    if n == 0 || n > 4096 {
        return Err(("400 Bad Request", "empty or oversized request line"));
    }
    header_bytes += n;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or(("400 Bad Request", "missing method"))?
        .to_string();
    let target = parts
        .next()
        .ok_or(("400 Bad Request", "missing target"))?
        .to_string();

    let mut headers = Vec::new();
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|_| ("400 Bad Request", "failed reading headers"))?;
        if read == 0 {
            return Err(("400 Bad Request", "unexpected EOF in headers"));
        }
        header_bytes += read;
        if header_bytes > MAX_HEADER_BYTES {
            return Err(("431 Request Header Fields Too Large", "headers too large"));
        }
        let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            let name = name.trim().to_string();
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| ("400 Bad Request", "invalid content-length"))?,
                );
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
            headers.push((name, value));
        }
    }

    let body = if chunked {
        read_chunked_body(&mut reader, MAX_BODY_BYTES)?
    } else if let Some(len) = content_length {
        if len > MAX_BODY_BYTES {
            return Err(("413 Content Too Large", "body too large"));
        }
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .map_err(|_| ("400 Bad Request", "incomplete body"))?;
        buf
    } else {
        Vec::new()
    };

    Ok(HttpRequest {
        method,
        target,
        headers,
        body,
    })
}

fn read_chunked_body<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> Result<Vec<u8>, (&'static str, &'static str)> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        let n = reader
            .read_line(&mut size_line)
            .map_err(|_| ("400 Bad Request", "bad chunk header"))?;
        if n == 0 {
            return Err(("400 Bad Request", "truncated chunked body"));
        }
        let hex = size_line
            .trim()
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let chunk_len = usize::from_str_radix(hex, 16)
            .map_err(|_| ("400 Bad Request", "invalid chunk size"))?;
        if chunk_len == 0 {
            // Drain trailing headers until empty line.
            loop {
                let mut trailer = String::new();
                let tn = reader
                    .read_line(&mut trailer)
                    .map_err(|_| ("400 Bad Request", "bad chunk trailer"))?;
                if tn == 0 || trailer.trim().is_empty() {
                    break;
                }
            }
            break;
        }
        if out.len().saturating_add(chunk_len) > limit {
            return Err(("413 Content Too Large", "chunked body too large"));
        }
        let start = out.len();
        out.resize(start + chunk_len, 0);
        reader
            .read_exact(&mut out[start..])
            .map_err(|_| ("400 Bad Request", "truncated chunk data"))?;
        let mut crlf = [0u8; 2];
        reader
            .read_exact(&mut crlf)
            .map_err(|_| ("400 Bad Request", "missing chunk CRLF"))?;
    }
    Ok(out)
}

fn write_json_reply(
    stream: &mut UnixStream,
    status: &str,
    extra_headers: &[(&str, &str)],
    body: &Value,
) -> io::Result<()> {
    let payload = body.to_string();
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        payload.len()
    );
    for (k, v) in extra_headers {
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}

// ------------------------------------------------------------------------ Forwarding & streaming

fn handle_messages(
    stream: &mut UnixStream,
    req: HttpRequest,
    ctx: &RelayContext,
) -> io::Result<()> {
    let candidates = ctx.state.read().unwrap().candidates();
    if candidates.is_empty() {
        return write_json_reply(
            stream,
            "503 Service Unavailable",
            &[("X-AuthBaton-Pass", "1")],
            &json!({"error": "relay_inactive"}),
        );
    }

    let is_gzip = req.headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-encoding") && v.to_ascii_lowercase().contains("gzip")
    });
    let plain_body = if is_gzip {
        match gunzip_bytes(&req.body) {
            Ok(decoded) => decoded,
            Err(_) => {
                return write_json_reply(
                    stream,
                    "503 Service Unavailable",
                    &[("X-AuthBaton-Pass", "1")],
                    &json!({"error": "gzip_decode_failed"}),
                );
            }
        }
    } else {
        req.body
    };

    let (model, is_stream) = extract_request_meta(&plain_body);
    let started = Instant::now();
    let first_label = candidates[0].label.clone();

    for (idx, candidate) in candidates.iter().enumerate() {
        let has_more = idx + 1 < candidates.len();
        let (send_body, uid_rewritten) =
            rewrite_metadata_user_id(&plain_body, &candidate.account_uuid);

        let attempt = forward_once(
            ctx,
            &req.target,
            &req.headers,
            &send_body,
            &candidate.access_token,
        );
        let mut upstream = match attempt {
            Ok(up) => up,
            Err(_) => {
                // Transport failure before headers: let Mod Fail-Open cleanly.
                ctx.write_diag_log(&json!({
                    "at": unix_secs(),
                    "path": "/v1/messages",
                    "model": model,
                    "stream": is_stream,
                    "status": 0,
                    "ms": started.elapsed().as_millis() as u64,
                    "switched": idx > 0,
                    "uid_rewritten": uid_rewritten,
                    "error": "upstream_transport",
                }));
                return write_json_reply(
                    stream,
                    "503 Service Unavailable",
                    &[("X-AuthBaton-Pass", "1")],
                    &json!({"error": "upstream_transport"}),
                );
            }
        };

        let rl = parse_rate_limits(&upstream.headers);

        // Transparent 429 switch across same-Team candidates before any headers reach Claude Desktop!
        if upstream.status == 429 && has_more {
            {
                let mut state = ctx.state.write().unwrap();
                state
                    .cooldowns
                    .insert(candidate.id.clone(), Instant::now() + RATE_LIMIT_COOLDOWN);
            }
            ctx.push_event(RelayEvent::Observed {
                account_id: candidate.id.clone(),
                model: model.clone(),
                primary_used: rl.primary_used.or(Some(100.0)),
                limited: true,
            });
            upstream.abort();
            continue;
        }

        // Commit this candidate's response to the client stream.
        ctx.relayed_count.fetch_add(1, Ordering::Relaxed);
        let switched = idx > 0;
        if switched {
            let now_secs = unix_secs();
            {
                let mut state = ctx.state.write().unwrap();
                state.override_active =
                    Some((candidate.id.clone(), Instant::now() + OVERRIDE_ACTIVE_TTL));
                state.last_switch_at = Some(now_secs);
            }
            ctx.push_event(RelayEvent::Switched {
                to_id: candidate.id.clone(),
            });
        }
        if upstream.status == 401 {
            ctx.push_event(RelayEvent::Unauthorized {
                account_id: candidate.id.clone(),
            });
        } else if rl.primary_used.is_some() || upstream.status == 429 || model.is_some() {
            ctx.push_event(RelayEvent::Observed {
                account_id: candidate.id.clone(),
                model: model.clone(),
                primary_used: rl.primary_used,
                limited: upstream.status == 429,
            });
        }

        // Build downstream response headers (chunked streaming, zero hop-by-hop headers).
        let mut head = format!(
            "HTTP/1.1 {} {}\r\n",
            upstream.status, upstream.reason
        );
        for (k, v) in &upstream.headers {
            if is_hop_by_hop_response_header(k) {
                continue;
            }
            head.push_str(k);
            head.push_str(": ");
            head.push_str(v);
            head.push_str("\r\n");
        }
        head.push_str("X-AuthBaton-Account: ");
        head.push_str(&percent_encode(&candidate.label));
        head.push_str("\r\n");
        if switched {
            head.push_str("X-AuthBaton-Switched-From: ");
            head.push_str(&percent_encode(&first_label));
            head.push_str("\r\n");
        }
        head.push_str("Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
        if stream.write_all(head.as_bytes()).is_err() || stream.flush().is_err() {
            upstream.abort();
            return Ok(());
        }

        let upstream_status = upstream.status;
        // Stream response body chunk-by-chunk with immediate flush (critical for SSE!).
        let stream_ok = stream_chunked_body(&mut upstream.reader, stream);
        if stream_ok.is_err() {
            upstream.abort();
        } else {
            upstream.finish();
        }

        ctx.write_diag_log(&json!({
            "at": unix_secs(),
            "path": "/v1/messages",
            "model": model,
            "stream": is_stream,
            "status": upstream_status,
            "ms": started.elapsed().as_millis() as u64,
            "switched": switched,
            "uid_rewritten": uid_rewritten,
            "rl_status": rl.status,
            "rl_5h": rl.five_hour,
            "rl_7d": rl.seven_day,
        }));
        return Ok(());
    }

    Ok(())
}

struct TempBodyFile(PathBuf);
impl Drop for TempBodyFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct UpstreamStream {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    reader: BufReader<std::process::ChildStdout>,
    child: std::process::Child,
    _body_file: TempBodyFile,
}

impl UpstreamStream {
    fn abort(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn finish(mut self) {
        let _ = self.child.wait();
    }
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(1);

fn forward_once(
    ctx: &RelayContext,
    target: &str,
    incoming_headers: &[(String, String)],
    body: &[u8],
    access_token: &str,
) -> Result<UpstreamStream, &'static str> {
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let body_path = ctx
        .tmp_dir
        .join(format!("body-{}-{seq}.bin", std::process::id()));
    {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&body_path)
            .map_err(|_| "temp body open failed")?;
        f.write_all(body).map_err(|_| "temp body write failed")?;
    }
    let body_guard = TempBodyFile(body_path.clone());

    let url = format!("{}{}", ctx.upstream_base.trim_end_matches('/'), target);
    let mut cfg = String::new();
    cfg.push_str(&format!("url = \"{}\"\n", escape_curl_config(&url)));
    cfg.push_str("request = \"POST\"\n");
    cfg.push_str(&format!(
        "data-binary = \"@{}\"\n",
        escape_curl_config(&body_path.to_string_lossy())
    ));
    cfg.push_str(&format!(
        "header = \"Authorization: Bearer {}\"\n",
        escape_curl_config(access_token)
    ));
    let mut has_content_type = false;
    for (k, v) in incoming_headers {
        if is_hop_by_hop_request_header(k) {
            continue;
        }
        if k.eq_ignore_ascii_case("content-type") {
            has_content_type = true;
        }
        let clean_k = strip_crlf(k);
        let clean_v = strip_crlf(v);
        if clean_k.is_empty() {
            continue;
        }
        cfg.push_str(&format!(
            "header = \"{}: {}\"\n",
            escape_curl_config(&clean_k),
            escape_curl_config(&clean_v)
        ));
    }
    if !has_content_type {
        cfg.push_str("header = \"Content-Type: application/json\"\n");
    }
    cfg.push_str("header = \"Accept-Encoding: identity\"\n");
    // Suppress Expect: 100-continue latency on large POST bodies.
    cfg.push_str("header = \"Expect:\"\n");

    let mut cmd = Command::new("/usr/bin/curl");
    cmd.env_clear();
    if ctx.use_system_proxy {
        crate::network::configure(&mut cmd).map_err(|_| "proxy configuration failed")?;
    }
    cmd.args([
        "-q",
        "--silent",
        "--show-error",
        "--no-buffer",
        "--http1.1",
        "--include",
        "--connect-timeout",
        "20",
        "--max-time",
        "1200",
    ]);
    if !ctx.use_system_proxy {
        cmd.args(["--noproxy", "*"]);
    }
    #[cfg(test)]
    if let Some(sock) = &ctx.upstream_unix_socket {
        cmd.arg("--unix-socket").arg(sock);
    }
    cmd.args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().map_err(|_| "curl spawn failed")?;
    if let Some(mut stdin) = child.stdin.take() {
        let write_res = stdin.write_all(cfg.as_bytes());
        zero_string(&mut cfg);
        drop(stdin);
        if write_res.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("curl stdin write failed");
        }
    } else {
        zero_string(&mut cfg);
        let _ = child.kill();
        let _ = child.wait();
        return Err("curl stdin unavailable");
    }

    let stdout = match child.stdout.take() {
        Some(out) => out,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("curl stdout unavailable");
        }
    };
    let mut reader = BufReader::new(stdout);
    let Ok(head) = read_upstream_response_headers(&mut reader) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("upstream header parse failed");
    };

    Ok(UpstreamStream {
        status: head.status,
        reason: head.reason,
        headers: head.headers,
        reader,
        child,
        _body_file: body_guard,
    })
}

struct ResponseHead {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
}

/// Parses one or more HTTP response header blocks from `curl --include`, skipping
/// `1xx` interim responses and HTTP proxy `200 Connection established` tunnel headers.
fn read_upstream_response_headers<R: BufRead>(
    reader: &mut R,
) -> Result<ResponseHead, &'static str> {
    for _ in 0..8 {
        let mut status_line = String::new();
        let n = reader
            .read_line(&mut status_line)
            .map_err(|_| "read status failed")?;
        if n == 0 {
            return Err("empty upstream response");
        }
        let trimmed = status_line.trim_end_matches(&['\r', '\n'][..]);
        let mut parts = trimmed.splitn(3, ' ');
        let version = parts.next().unwrap_or("");
        if !version.starts_with("HTTP/") {
            return Err("invalid HTTP status line");
        }
        let status: u16 = parts
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or("invalid HTTP status code")?;
        let reason = parts.next().unwrap_or("").to_string();

        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            let read = reader
                .read_line(&mut line)
                .map_err(|_| "read header failed")?;
            if read == 0 {
                break;
            }
            let h = line.trim_end_matches(&['\r', '\n'][..]);
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.push((k.trim().to_string(), v.trim().to_string()));
            }
        }

        let is_proxy_connect =
            status == 200 && reason.to_ascii_lowercase().contains("connection established");
        if (100..200).contains(&status) || is_proxy_connect {
            continue;
        }
        return Ok(ResponseHead {
            status,
            reason,
            headers,
        });
    }
    Err("too many interim responses")
}

fn stream_chunked_body<R: BufRead, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<()> {
    // First drain any buffered bytes already read past the header terminator.
    let buffered = reader.fill_buf()?.to_vec();
    if !buffered.is_empty() {
        let len = buffered.len();
        reader.consume(len);
        write_chunk(writer, &buffered)?;
    }
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        write_chunk(writer, &buf[..n])?;
    }
    writer.write_all(b"0\r\n\r\n")?;
    writer.flush()
}

fn write_chunk<W: Write>(writer: &mut W, data: &[u8]) -> io::Result<()> {
    write!(writer, "{:x}\r\n", data.len())?;
    writer.write_all(data)?;
    writer.write_all(b"\r\n")?;
    writer.flush()
}

fn gunzip_bytes(compressed: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut child = Command::new("/usr/bin/gzip")
        .env_clear()
        .arg("-dc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "gzip spawn failed")?;
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("gzip stdin unavailable");
    };
    let input = compressed.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let mut out = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        stdout
            .take(MAX_GZIP_OUT_BYTES as u64 + 1)
            .read_to_end(&mut out)
            .map_err(|_| "gzip read failed")?;
    }
    let _ = writer.join();
    let status = child.wait().map_err(|_| "gzip wait failed")?;
    if !status.success() || out.len() > MAX_GZIP_OUT_BYTES {
        return Err("gzip decode failed");
    }
    Ok(out)
}

// ------------------------------------------------------------------------ Request & response helpers

fn extract_request_meta(body: &[u8]) -> (Option<String>, bool) {
    let Ok(val) = serde_json::from_slice::<Value>(body) else {
        return (None, false);
    };
    let model = val
        .get("model")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 128)
        .map(str::to_string);
    let stream = val.get("stream").and_then(Value::as_bool).unwrap_or(false);
    (model, stream)
}

/// Rewrites `metadata.user_id`'s `account_uuid` to match the target same-Team account while
/// preserving every other byte of the JSON request (key order, `thinking` blocks, cryptographic
/// `signature` strings, `system`, `tools`, and `cache_control` markers stay bit-for-bit identical).
/// Returns `(bytes, rewritten)`. Leaves body untouched if no rewrite is needed.
fn rewrite_metadata_user_id(body: &[u8], target_account_uuid: &str) -> (Vec<u8>, bool) {
    let Ok(root) = serde_json::from_slice::<Value>(body) else {
        return (body.to_vec(), false);
    };
    let Some(user_id) = root
        .get("metadata")
        .and_then(|m| m.get("user_id"))
        .and_then(Value::as_str)
    else {
        return (body.to_vec(), false);
    };

    let updated = if user_id.trim_start().starts_with('{') {
        if let Ok(mut uid_obj) = serde_json::from_str::<Value>(user_id)
            && let Some(map) = uid_obj.as_object_mut()
            && map
                .get("account_uuid")
                .and_then(Value::as_str)
                .is_some_and(|cur| cur != target_account_uuid)
        {
            map.insert(
                "account_uuid".to_string(),
                Value::String(target_account_uuid.to_string()),
            );
            serde_json::to_string(&uid_obj).ok()
        } else {
            None
        }
    } else if let Some(acct_pos) = user_id.find("_account_") {
        let after_acct = acct_pos + "_account_".len();
        if let Some(sess_rel) = user_id[after_acct..].find("_session_") {
            let current = &user_id[after_acct..after_acct + sess_rel];
            if current != target_account_uuid {
                Some(format!(
                    "{}{}{}",
                    &user_id[..after_acct],
                    target_account_uuid,
                    &user_id[after_acct + sess_rel..]
                ))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let Some(new_uid) = updated else {
        return (body.to_vec(), false);
    };
    let Ok(new_literal) = serde_json::to_string(&new_uid) else {
        return (body.to_vec(), false);
    };

    // Prefer exact byte-preserving splice of root["metadata"]["user_id"] so no other part of the
    // request (thinking blocks, signatures, system/tool order, cache_control) is re-serialized.
    if let Some((start, end)) = find_metadata_user_id_span(body) {
        let mut out = Vec::with_capacity(body.len() + new_literal.len());
        out.extend_from_slice(&body[..start]);
        out.extend_from_slice(new_literal.as_bytes());
        out.extend_from_slice(&body[end..]);
        return (out, true);
    }

    // Fallback: structured JSON rewrite if the input has unusual syntax.
    let mut root = root;
    if let Some(meta) = root.get_mut("metadata").and_then(Value::as_object_mut) {
        meta.insert("user_id".to_string(), Value::String(new_uid));
        if let Ok(bytes) = serde_json::to_vec(&root) {
            return (bytes, true);
        }
    }
    (body.to_vec(), false)
}

/// Finds the exact `[start, end)` byte range of the JSON string literal value at
/// `root["metadata"]["user_id"]` (including its surrounding quotes).
fn find_metadata_user_id_span(input: &[u8]) -> Option<(usize, usize)> {
    let mut i = skip_ws(input, 0);
    if input.get(i) != Some(&b'{') {
        return None;
    }
    i = skip_ws(input, i + 1);
    while i < input.len() {
        if input[i] == b'}' {
            return None;
        }
        let (key_start, key_end) = scan_json_string(input, i)?;
        let key: String = serde_json::from_slice(&input[key_start..key_end]).ok()?;
        i = skip_ws(input, key_end);
        if input.get(i) != Some(&b':') {
            return None;
        }
        i = skip_ws(input, i + 1);
        if key == "metadata" {
            return find_string_field_in_object(input, i, "user_id");
        }
        i = skip_json_value(input, i)?;
        i = skip_ws(input, i);
        match input.get(i) {
            Some(&b',') => i = skip_ws(input, i + 1),
            Some(&b'}') => return None,
            _ => return None,
        }
    }
    None
}

fn find_string_field_in_object(input: &[u8], start: usize, target: &str) -> Option<(usize, usize)> {
    let mut i = skip_ws(input, start);
    if input.get(i) != Some(&b'{') {
        return None;
    }
    i = skip_ws(input, i + 1);
    while i < input.len() {
        if input[i] == b'}' {
            return None;
        }
        let (key_start, key_end) = scan_json_string(input, i)?;
        let key: String = serde_json::from_slice(&input[key_start..key_end]).ok()?;
        i = skip_ws(input, key_end);
        if input.get(i) != Some(&b':') {
            return None;
        }
        i = skip_ws(input, i + 1);
        if key == target {
            return scan_json_string(input, i);
        }
        i = skip_json_value(input, i)?;
        i = skip_ws(input, i);
        match input.get(i) {
            Some(&b',') => i = skip_ws(input, i + 1),
            Some(&b'}') => return None,
            _ => return None,
        }
    }
    None
}

fn skip_ws(input: &[u8], mut i: usize) -> usize {
    while i < input.len() && matches!(input[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    i
}

fn scan_json_string(input: &[u8], start: usize) -> Option<(usize, usize)> {
    if input.get(start) != Some(&b'"') {
        return None;
    }
    let mut i = start + 1;
    while i < input.len() {
        match input[i] {
            b'\\' => i = i.checked_add(2)?,
            b'"' => return Some((start, i + 1)),
            _ => i += 1,
        }
    }
    None
}

fn skip_json_value(input: &[u8], start: usize) -> Option<usize> {
    let i = skip_ws(input, start);
    match *input.get(i)? {
        b'"' => Some(scan_json_string(input, i)?.1),
        b'{' | b'[' => {
            let mut pos = i;
            let mut depth = 0usize;
            while pos < input.len() {
                match input[pos] {
                    b'"' => pos = scan_json_string(input, pos)?.1,
                    b'{' | b'[' => {
                        depth += 1;
                        pos += 1;
                    }
                    b'}' | b']' => {
                        depth -= 1;
                        pos += 1;
                        if depth == 0 {
                            return Some(pos);
                        }
                    }
                    _ => pos += 1,
                }
            }
            None
        }
        _ => {
            let mut pos = i;
            while pos < input.len()
                && !matches!(input[pos], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n')
            {
                pos += 1;
            }
            (pos > i).then_some(pos)
        }
    }
}

#[derive(Default)]
struct RateLimits {
    status: Option<String>,
    five_hour: Option<f64>,
    seven_day: Option<f64>,
    primary_used: Option<f64>,
}

fn parse_rate_limits(headers: &[(String, String)]) -> RateLimits {
    let mut rl = RateLimits::default();
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("anthropic-ratelimit-unified-status") {
            rl.status = Some(v.clone());
        } else if k.eq_ignore_ascii_case("anthropic-ratelimit-unified-5h-utilization") {
            rl.five_hour = v.parse::<f64>().ok();
        } else if k.eq_ignore_ascii_case("anthropic-ratelimit-unified-7d-utilization") {
            rl.seven_day = v.parse::<f64>().ok();
        }
    }
    let max_util = match (rl.five_hour, rl.seven_day) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    rl.primary_used = max_util.map(|u| (u * 100.0).clamp(0.0, 100.0));
    rl
}

fn is_hop_by_hop_request_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "authorization"
            | "x-api-key"
            | "content-length"
            | "content-encoding"
            | "transfer-encoding"
            | "connection"
            | "accept-encoding"
            | "keep-alive"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "upgrade"
    )
}

fn is_hop_by_hop_response_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "transfer-encoding"
            | "content-length"
            | "content-encoding"
            | "connection"
            | "keep-alive"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "upgrade"
    )
}

fn strip_crlf(s: &str) -> String {
    s.chars().filter(|&c| c != '\r' && c != '\n').collect()
}

fn escape_curl_config(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' | '\n' => {}
            other => out.push(other),
        }
    }
    out
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
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
            "authbaton-relay-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
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
        assert_eq!(
            parsed["extraKnownMarketplaces"][MARKETPLACE_NAME]["source"]["path"],
            "/Library/Application Support/ClaudeCode/authbaton-desktop-relay"
        );

        let after = unmerge_managed_settings(&merged).unwrap().unwrap();
        let parsed_after: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(parsed_after["customPolicy"], true);
        assert_eq!(parsed_after["enabledPlugins"]["other@mkt"], true);
        assert!(parsed_after["enabledPlugins"].get(PLUGIN_ID).is_none());
        assert!(parsed_after.get("extraKnownMarketplaces").is_none());

        // When AuthBaton was the only entry, unmerge removes the file cleanly.
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

        // Empty remote-settings.json ({}) does not block.
        fs::write(&remote, "{}").unwrap();
        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::NotInstalled
        );

        // Stage full valid install into temp managed dir.
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

        // Stale version triggers NeedsUpdate.
        fs::write(managed.join(MARKETPLACE_NAME).join(VERSION_FILE), "oldhash\n").unwrap();
        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::NeedsUpdate
        );

        // Non-empty remote-settings.json triggers BlockedRemote.
        fs::write(&remote, r#"{"managedPolicy": true}"#).unwrap();
        assert_eq!(
            inspect_install_state_at(&managed, &remote),
            InstallState::BlockedRemote
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewrites_metadata_user_id_for_both_json_and_legacy_formats() {
        let json_body = serde_json::to_vec(&json!({
            "model": "claude-haiku-5-5",
            "metadata": {
                "user_id": r#"{"device_id":"dev1","account_uuid":"old-acct","session_id":"s1"}"#
            }
        }))
        .unwrap();
        let (out, changed) = rewrite_metadata_user_id(&json_body, "new-acct");
        assert!(changed);
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        let uid: Value = serde_json::from_str(parsed["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(uid["account_uuid"], "new-acct");
        assert_eq!(uid["session_id"], "s1");

        // Same account leaves body untouched.
        let (_, changed2) = rewrite_metadata_user_id(&out, "new-acct");
        assert!(!changed2);

        // Legacy _account_<uuid>_session_ format.
        let legacy = serde_json::to_vec(&json!({
            "metadata": {
                "user_id": "user_abc_account_old-acct_session_s1"
            }
        }))
        .unwrap();
        let (out3, changed3) = rewrite_metadata_user_id(&legacy, "new-acct");
        assert!(changed3);
        let parsed3: Value = serde_json::from_slice(&out3).unwrap();
        assert_eq!(
            parsed3["metadata"]["user_id"],
            "user_abc_account_new-acct_session_s1"
        );

        // Bit-for-bit byte preservation: non-alphabetical key order, thinking + signature blocks,
        // cache_control order, and a fake "metadata" inside message content stay untouched!
        let raw_thinking_req = concat!(
            r#"{"model":"claude-fable-5-2","thinking":{"type":"enabled","budget_tokens":4096},"#,
            r#""system":[{"type":"text","text":"sys","cache_control":{"type":"ephemeral"}}],"#,
            r#""messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"step 1 \"quoted\"","signature":"EqoBCgIYAhIMabc+/=="},{"type":"text","text":"{\"metadata\":{\"user_id\":\"do-not-touch\"}}"}]}],"#,
            r#""metadata":{"user_id":"{\"device_id\":\"d1\",\"account_uuid\":\"old-acct\",\"session_id\":\"s1\"}"},"stream":true}"#
        );
        let (spliced, changed4) = rewrite_metadata_user_id(raw_thinking_req.as_bytes(), "new-acct");
        assert!(changed4);
        let spliced_str = String::from_utf8(spliced).unwrap();
        // Prefix before metadata stays 100% byte-identical (non-alphabetical order preserved!).
        let prefix = concat!(
            r#"{"model":"claude-fable-5-2","thinking":{"type":"enabled","budget_tokens":4096},"#,
            r#""system":[{"type":"text","text":"sys","cache_control":{"type":"ephemeral"}}],"#,
            r#""messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"step 1 \"quoted\"","signature":"EqoBCgIYAhIMabc+/=="},{"type":"text","text":"{\"metadata\":{\"user_id\":\"do-not-touch\"}}"}]}],"#,
            r#""metadata":{"user_id":"#
        );
        assert!(spliced_str.starts_with(prefix), "prefix altered: {spliced_str}");
        assert!(spliced_str.ends_with(r#"},"stream":true}"#), "suffix altered: {spliced_str}");
        assert!(spliced_str.contains("do-not-touch"));
        let parsed4: Value = serde_json::from_str(&spliced_str).unwrap();
        let uid4: Value = serde_json::from_str(parsed4["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(uid4["account_uuid"], "new-acct");
    }

    #[test]
    fn skips_proxy_connect_and_100_continue_headers() {
        let wire = concat!(
            "HTTP/1.1 200 Connection established\r\n",
            "Proxy-Agent: test\r\n",
            "\r\n",
            "HTTP/1.1 100 Continue\r\n",
            "\r\n",
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: text/event-stream\r\n",
            "anthropic-ratelimit-unified-5h-utilization: 0.42\r\n",
            "\r\n",
            "data: hello\n\n"
        );
        let mut cursor = BufReader::new(wire.as_bytes());
        let head = read_upstream_response_headers(&mut cursor).unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.reason, "OK");
        let rl = parse_rate_limits(&head.headers);
        assert_eq!(rl.five_hour, Some(0.42));
        assert_eq!(rl.primary_used, Some(42.0));
        let mut rest = String::new();
        cursor.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "data: hello\n\n");
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

        // Cross-team is strictly rejected.
        st.active.as_mut().unwrap().org_uuid = "org-other-2".into();
        assert_eq!(st.same_team(), Some(false));
        assert!(!st.is_relay_active());
        assert!(st.candidates().is_empty());
    }

    #[test]
    fn end_to_end_unix_socket_streams_sse_and_fails_over_on_429_without_leaking_tokens() {
        let dir = temp_dir("e2e-relay");
        let upstream_sock = dir.join("upstream.sock");
        let upstream = UnixListener::bind(&upstream_sock).unwrap();

        // Mock upstream: first request (tok-a) gets 429; second request (tok-b) gets streaming 200 SSE.
        let server = std::thread::spawn(move || {
            let mut seen_auths = Vec::new();
            let mut seen_uids = Vec::new();
            for attempt in 0..2 {
                let (mut conn, _) = upstream.accept().unwrap();
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut req_line = String::new();
                reader.read_line(&mut req_line).unwrap();
                let mut auth = String::new();
                let mut content_len = 0usize;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = line.split_once(':') {
                        if k.eq_ignore_ascii_case("authorization") {
                            auth = v.trim().to_string();
                        } else if k.eq_ignore_ascii_case("content-length") {
                            content_len = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut body = vec![0u8; content_len];
                reader.read_exact(&mut body).unwrap();
                let body_val: Value = serde_json::from_slice(&body).unwrap();
                seen_auths.push(auth);
                seen_uids.push(
                    body_val["metadata"]["user_id"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                );
                if attempt == 0 {
                    let resp = concat!(
                        "HTTP/1.1 429 Too Many Requests\r\n",
                        "Content-Type: application/json\r\n",
                        "anthropic-ratelimit-unified-5h-utilization: 1.0\r\n",
                        "Content-Length: 19\r\n",
                        "Connection: close\r\n\r\n",
                        "{\"error\":\"limited\"}"
                    );
                    conn.write_all(resp.as_bytes()).unwrap();
                } else {
                    conn.write_all(
                        concat!(
                            "HTTP/1.1 200 OK\r\n",
                            "Content-Type: text/event-stream\r\n",
                            "anthropic-ratelimit-unified-5h-utilization: 0.12\r\n",
                            "Connection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                    conn.write_all(b"event: ping\ndata: {\"ok\":1}\n\n").unwrap();
                    conn.flush().unwrap();
                    std::thread::sleep(Duration::from_millis(20));
                    conn.write_all(b"event: message_stop\ndata: {}\n\n").unwrap();
                    conn.flush().unwrap();
                }
            }
            (seen_auths, seen_uids)
        });

        let tmp_dir = dir.join("relay-tmp");
        fs::create_dir_all(&tmp_dir).unwrap();
        let log_path = dir.join("relay.log");
        let ctx = Arc::new(RelayContext {
            state: RwLock::new(SharedState {
                enabled: true,
                install_state: InstallState::Installed,
                desktop_org_uuid: Some("org-same".into()),
                active: Some(RelayAccount {
                    id: "acct-a".into(),
                    label: "主号 A".into(),
                    account_uuid: "uuid-a".into(),
                    org_uuid: "org-same".into(),
                    access_token: "synthetic-secret-token-a".into(),
                }),
                fallbacks: vec![RelayAccount {
                    id: "acct-b".into(),
                    label: "备用 B".into(),
                    account_uuid: "uuid-b".into(),
                    org_uuid: "org-same".into(),
                    access_token: "synthetic-secret-token-b".into(),
                }],
                ..SharedState::default()
            }),
            events: Mutex::new(VecDeque::new()),
            relayed_count: AtomicU64::new(0),
            active_conns: AtomicUsize::new(0),
            upstream_base: "http://mock-anthropic.local".to_string(),
            upstream_unix_socket: Some(upstream_sock),
            tmp_dir,
            log_path: log_path.clone(),
            log_lock: Mutex::new(()),
            use_system_proxy: false,
        });

        let sock_path = dir.join("relay.sock");
        let listener = bind_relay_socket(&sock_path).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let t_ctx = ctx.clone();
        let t_stop = stop.clone();
        let accept = std::thread::spawn(move || accept_loop(listener, t_ctx, t_stop));

        // 1. Check /v1/health
        {
            let mut client = UnixStream::connect(&sock_path).unwrap();
            client
                .write_all(b"GET /v1/health HTTP/1.1\r\nHost: authbaton\r\n\r\n")
                .unwrap();
            let mut reply = String::new();
            client.read_to_string(&mut reply).unwrap();
            assert!(reply.contains("200 OK"), "{reply}");
            assert!(reply.contains(r#""active":true"#), "{reply}");
            assert!(reply.contains("主号 A"), "{reply}");
            assert!(!reply.contains("synthetic-secret"));
        }

        // 2. POST /v1/messages?beta=true → transparent 429 switch A -> B + SSE streamed back
        {
            let body = serde_json::to_string(&json!({
                "model": "claude-haiku-5-5",
                "stream": true,
                "metadata": {
                    "user_id": r#"{"account_uuid":"uuid-a","session_id":"sess-1"}"#
                }
            }))
            .unwrap();
            let req = format!(
                "POST /v1/messages?beta=true HTTP/1.1\r\nHost: authbaton\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let mut client = UnixStream::connect(&sock_path).unwrap();
            client.write_all(req.as_bytes()).unwrap();
            let mut reply = String::new();
            client.read_to_string(&mut reply).unwrap();
            assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
            assert!(
                reply.contains(&format!("X-AuthBaton-Account: {}", percent_encode("备用 B"))),
                "{reply}"
            );
            assert!(
                reply.contains(&format!(
                    "X-AuthBaton-Switched-From: {}",
                    percent_encode("主号 A")
                )),
                "{reply}"
            );
            assert!(reply.contains("event: ping"), "{reply}");
            assert!(reply.contains("event: message_stop"), "{reply}");
            assert!(!reply.contains("synthetic-secret"));
        }

        // 3. When disabled, /v1/health reports active:false and POST returns 503 + X-AuthBaton-Pass: 1.
        {
            ctx.state.write().unwrap().enabled = false;
            let mut client = UnixStream::connect(&sock_path).unwrap();
            client
                .write_all(b"GET /v1/health HTTP/1.1\r\nHost: authbaton\r\n\r\n")
                .unwrap();
            let mut reply = String::new();
            client.read_to_string(&mut reply).unwrap();
            assert!(reply.contains(r#""active":false"#), "{reply}");

            let mut client2 = UnixStream::connect(&sock_path).unwrap();
            client2
                .write_all(b"POST /v1/messages HTTP/1.1\r\nHost: authbaton\r\nContent-Length: 2\r\n\r\n{}")
                .unwrap();
            let mut reply2 = String::new();
            client2.read_to_string(&mut reply2).unwrap();
            assert!(reply2.starts_with("HTTP/1.1 503 Service Unavailable\r\n"), "{reply2}");
            assert!(reply2.contains("X-AuthBaton-Pass: 1"), "{reply2}");
        }

        stop.store(true, Ordering::Release);
        accept.join().unwrap();
        let (seen_auths, seen_uids) = server.join().unwrap();
        assert_eq!(
            seen_auths,
            vec![
                "Bearer synthetic-secret-token-a",
                "Bearer synthetic-secret-token-b"
            ]
        );
        assert!(seen_uids[0].contains("uuid-a"));
        assert!(seen_uids[1].contains("uuid-b"));

        // Verify diagnostics log contains zero credentials or account UUIDs.
        let log_text = fs::read_to_string(&log_path).unwrap();
        assert!(log_text.contains("\"switched\":true"), "{log_text}");
        assert!(!log_text.contains("synthetic-secret"));
        assert!(!log_text.contains("uuid-a"));
        assert!(!log_text.contains("uuid-b"));
        assert!(!log_text.contains("org-same"));

        // Verify Switched event was queued.
        let events: Vec<RelayEvent> = ctx.events.lock().unwrap().drain(..).collect();
        assert!(events.iter().any(|e| matches!(
            e,
            RelayEvent::Switched { to_id } if to_id == "acct-b"
        )));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn gzip_chunked_and_curl_config_escaping_work_safely() {
        // Chunked decoding
        let wire = b"5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\n\r\n";
        let mut reader = BufReader::new(&wire[..]);
        assert_eq!(read_chunked_body(&mut reader, 1024).unwrap(), b"hello world");

        // Gzip roundtrip using /usr/bin/gzip -c
        let mut child = Command::new("/usr/bin/gzip")
            .env_clear()
            .arg("-c")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(br#"{"model":"claude-haiku-5-5"}"#)
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        assert_eq!(
            gunzip_bytes(&out.stdout).unwrap(),
            br#"{"model":"claude-haiku-5-5"}"#
        );

        // Curl config escaping strips CR/LF and escapes quotes & backslashes.
        assert_eq!(
            escape_curl_config("a\"b\\c\r\nd"),
            "a\\\"b\\\\cd"
        );
    }

    #[test]
    fn health_and_fail_open_work_over_unix_stream_pair() {
        let dir = temp_dir("pair-health");
        let ctx = RelayContext {
            state: RwLock::new(SharedState {
                enabled: true,
                install_state: InstallState::Installed,
                desktop_org_uuid: Some("org-1".into()),
                active: Some(RelayAccount {
                    id: "a1".into(),
                    label: "研发主号".into(),
                    account_uuid: "uuid-1".into(),
                    org_uuid: "org-1".into(),
                    access_token: "synthetic-tok".into(),
                }),
                ..SharedState::default()
            }),
            events: Mutex::new(VecDeque::new()),
            relayed_count: AtomicU64::new(0),
            active_conns: AtomicUsize::new(0),
            upstream_base: "http://unused".to_string(),
            upstream_unix_socket: None,
            tmp_dir: dir.join("tmp"),
            log_path: dir.join("relay.log"),
            log_lock: Mutex::new(()),
            use_system_proxy: false,
        };

        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .write_all(b"GET /v1/health HTTP/1.1\r\nHost: authbaton\r\n\r\n")
            .unwrap();
        handle_connection(server, &ctx).unwrap();
        let mut out = String::new();
        client.read_to_string(&mut out).unwrap();
        assert!(out.contains(r#""active":true"#), "{out}");
        assert!(out.contains("研发主号"), "{out}");
        assert!(!out.contains("synthetic-tok"));

        // Cross-team fails open with 503 + X-AuthBaton-Pass: 1
        ctx.state.write().unwrap().desktop_org_uuid = Some("org-different".into());
        let (mut client2, server2) = UnixStream::pair().unwrap();
        client2
            .write_all(b"POST /v1/messages HTTP/1.1\r\nHost: authbaton\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
        handle_connection(server2, &ctx).unwrap();
        let mut out2 = String::new();
        client2.read_to_string(&mut out2).unwrap();
        assert!(out2.starts_with("HTTP/1.1 503 Service Unavailable\r\n"), "{out2}");
        assert!(out2.contains("X-AuthBaton-Pass: 1"), "{out2}");
        let _ = fs::remove_dir_all(&dir);
    }
}
