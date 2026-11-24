//! Optional Chrome companion extension sync for Claude Desktop.
//!
//! Architecture & trust boundary:
//! - Bundles an optional Manifest V3 Chrome extension (`integrations/chrome-sync/`) with a fixed
//!   RSA public key (`hkegchjkdgmjdgeioonhbjgdfeaobdfj`) so the extension ID is deterministic on
//!   every Mac without Chrome Web Store hosting.
//! - Installing deploys the unpacked extension under `<root>/chrome-extension/`, writes a private
//!   `0700` native host launcher `<root>/chrome-native-host`, and registers Chrome's Native
//!   Messaging manifest under `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/`
//!   (user-local, no admin prompt required).
//! - The extension holds zero cookies on disk. Chrome's `claude.ai` web session cookies captured by
//!   the extension travel over `<root>/chrome-sync.sock` (`0600`, same-UID verified via
//!   `getpeereid`) and are stored solely in AuthBaton's Keychain Vault under
//!   `chrome-claude-<account_uuid>`. Claude Desktop's own Cookies DB is never decrypted or read.
//! - Metadata stored in `<root>/chrome-sync/meta.json` (`0600`) and snapshots published over UI IPC
//!   never contain cookie values or raw account UUIDs (opaque 12-hex `handle` only).

use crate::claude_desktop::DesktopIdentity;
use crate::platform::{Platform, Result, private_dir};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const EXTENSION_ID: &str = "hkegchjkdgmjdgeioonhbjgdfeaobdfj";
pub const NATIVE_HOST_NAME: &str = "io.github.wuxm5.authbaton.chrome";
const EXTENSION_VERSION: &str = "0.1.0";
const VERSION_FILE: &str = ".authbaton-version";
const VAULT_SERVICE: &str = "io.github.wuxm5.authbaton.vault";
const MAX_NATIVE_MESSAGE_BYTES: usize = 1024 * 1024;
const SWITCH_TIMEOUT_SECS: u64 = 35;

const EMBED_MANIFEST_JSON: &str = include_str!("../../../integrations/chrome-sync/manifest.json");
const EMBED_BACKGROUND_JS: &str = include_str!("../../../integrations/chrome-sync/background.js");
const EMBED_OAUTH_JS: &str =
    include_str!("../../../integrations/chrome-sync/oauth-auto-approve.js");
const EMBED_POPUP_HTML: &str = include_str!("../../../integrations/chrome-sync/popup.html");
const EMBED_POPUP_JS: &str = include_str!("../../../integrations/chrome-sync/popup.js");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstallState {
    #[default]
    NotInstalled,
    Installed,
    NeedsUpdate,
}

impl InstallState {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallState::NotInstalled => "not_installed",
            InstallState::Installed => "installed",
            InstallState::NeedsUpdate => "needs_update",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrgMeta {
    pub uuid: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct SavedAccountMeta {
    account_uuid: String,
    email: String,
    #[serde(default)]
    orgs: Vec<OrgMeta>,
    #[serde(default)]
    cookie_names: Vec<String>,
    saved_at: u64,
    updated_at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct MetaStore {
    version: u32,
    #[serde(default)]
    accounts: BTreeMap<String, SavedAccountMeta>,
}

#[derive(Clone, Debug, Default)]
struct ClaudeExtInfo {
    installed: bool,
    version: String,
    enabled: bool,
    may_disable: bool,
}

#[derive(Default)]
struct SharedState {
    install_state: InstallState,
    connected: bool,
    extension_version: Option<String>,
    chrome_signed_in: bool,
    chrome_account_uuid: Option<String>,
    chrome_email: Option<String>,
    claude_ext: ClaudeExtInfo,
    busy: bool,
    busy_since: Option<u64>,
    pending_request_id: Option<String>,
    pending_target_uuid: Option<String>,
    pending_target_email: Option<String>,
    awaiting_login_uuid: Option<String>,
    awaiting_login_email: Option<String>,
    oauth_armed_until: Option<u64>,
    missing_session_uuid: Option<String>,
    last_sync_at: Option<u64>,
    last_error: Option<String>,
    last_notice: Option<String>,
    meta: MetaStore,
}

struct Context {
    root: PathBuf,
    socket_path: PathBuf,
    extension_dir: PathBuf,
    launcher_path: PathBuf,
    manifest_path: PathBuf,
    meta_path: PathBuf,
    exe_path: PathBuf,
    platform: Platform,
    state: Mutex<SharedState>,
    writer: Mutex<Option<(u64, UnixStream)>>,
    next_conn_id: AtomicU64,
    next_req_id: AtomicU64,
    stop: AtomicBool,
}

pub struct Runtime {
    ctx: Option<Arc<Context>>,
    listener_thread: Option<JoinHandle<()>>,
}

impl Runtime {
    /// Inert runtime for tests and when unavailable: never touches Chrome or sockets.
    pub fn off() -> Self {
        Self {
            ctx: None,
            listener_thread: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.ctx.is_some()
    }

    pub fn start(root: &Path, home: &Path, platform: Platform) -> Self {
        let exe_path = match std::env::current_exe() {
            Ok(p) => p,
            Err(_) => return Self::off(),
        };
        Self::start_with_paths(root, home, exe_path, platform)
    }

    fn start_with_paths(root: &Path, home: &Path, exe_path: PathBuf, platform: Platform) -> Self {
        let socket_path = root.join("chrome-sync.sock");
        let extension_dir = root.join("chrome-extension");
        let launcher_path = root.join("chrome-native-host");
        let manifest_path =
            home.join("Library/Application Support/Google/Chrome/NativeMessagingHosts")
                .join(format!("{NATIVE_HOST_NAME}.json"));
        let meta_path = root.join("chrome-sync/meta.json");

        let meta = load_meta(&meta_path);
        let install_state = detect_install_state(&extension_dir, &launcher_path, &manifest_path);

        let ctx = Arc::new(Context {
            root: root.to_path_buf(),
            socket_path: socket_path.clone(),
            extension_dir,
            launcher_path,
            manifest_path,
            meta_path,
            exe_path,
            platform,
            state: Mutex::new(SharedState {
                install_state,
                meta,
                ..SharedState::default()
            }),
            writer: Mutex::new(None),
            next_conn_id: AtomicU64::new(1),
            next_req_id: AtomicU64::new(1),
            stop: AtomicBool::new(false),
        });

        // If already installed, keep the native launcher path and extension bundle up to date
        // automatically across app upgrades or app relocations.
        if matches!(install_state, InstallState::Installed | InstallState::NeedsUpdate)
            && deploy_files(&ctx).is_ok()
        {
            ctx.state.lock().unwrap().install_state = InstallState::Installed;
        }

        let listener = match bind_private_socket(&socket_path) {
            Ok(l) => l,
            Err(_) => {
                // Even if socket failed to bind, keep install/uninstall inspection inert.
                return Self::off();
            }
        };

        let thread_ctx = ctx.clone();
        let listener_thread = std::thread::spawn(move || {
            accept_loop(listener, thread_ctx);
        });

        Self {
            ctx: Some(ctx),
            listener_thread: Some(listener_thread),
        }
    }

    pub fn stop(&mut self) {
        if let Some(ctx) = &self.ctx {
            ctx.stop.store(true, Ordering::Release);
            if let Ok(mut guard) = ctx.writer.lock() {
                if let Some((_, stream)) = guard.take() {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            }
            let _ = UnixStream::connect(&ctx.socket_path);
        }
        if let Some(handle) = self.listener_thread.take() {
            let _ = handle.join();
        }
        if let Some(ctx) = &self.ctx {
            let _ = fs::remove_file(&ctx.socket_path);
        }
    }

    pub fn install(&self) -> Result<()> {
        let ctx = self.ctx.as_ref().ok_or("Chrome 同步服务未启动")?;
        deploy_files(ctx)?;
        let mut st = ctx.state.lock().unwrap();
        st.install_state = InstallState::Installed;
        st.last_error = None;
        Ok(())
    }

    pub fn uninstall(&self) -> Result<()> {
        let ctx = self.ctx.as_ref().ok_or("Chrome 同步服务未启动")?;
        let uuids: Vec<String> = {
            let st = ctx.state.lock().unwrap();
            st.meta.accounts.keys().cloned().collect()
        };
        for uuid in &uuids {
            let _ = ctx.platform.delete_key(VAULT_SERVICE, &vault_id(uuid));
        }
        let _ = fs::remove_file(&ctx.manifest_path);
        let _ = fs::remove_file(&ctx.launcher_path);
        let _ = fs::remove_dir_all(&ctx.extension_dir);
        let _ = fs::remove_file(&ctx.meta_path);

        if let Ok(mut guard) = ctx.writer.lock() {
            if let Some((_, stream)) = guard.take() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
        let mut st = ctx.state.lock().unwrap();
        *st = SharedState {
            install_state: InstallState::NotInstalled,
            ..SharedState::default()
        };
        Ok(())
    }

    pub fn capture_current(&self) -> Result<()> {
        let ctx = self.ctx.as_ref().ok_or("Chrome 同步服务未启动")?;
        let req_id = next_id(ctx);
        send_to_extension(ctx, &json!({ "type": "capture_current", "request_id": req_id }))
    }

    pub fn clear_for_login(&self) -> Result<()> {
        let ctx = self.ctx.as_ref().ok_or("Chrome 同步服务未启动")?;
        let req_id = next_id(ctx);
        {
            let mut st = ctx.state.lock().unwrap();
            st.busy = true;
            st.busy_since = Some(now_secs());
            st.pending_request_id = Some(req_id.clone());
            st.last_error = None;
            st.last_notice = Some("正在清空 Chrome 本地 Cookie 以登录新账号…".into());
        }
        if let Err(err) =
            send_to_extension(ctx, &json!({ "type": "clear_for_login", "request_id": req_id }))
        {
            let mut st = ctx.state.lock().unwrap();
            st.busy = false;
            st.busy_since = None;
            st.pending_request_id = None;
            st.last_error = Some(err.into());
            return Err(err);
        }
        Ok(())
    }

    pub fn forget_account_by_handle(&self, handle: &str) -> Result<()> {
        let ctx = self.ctx.as_ref().ok_or("Chrome 同步服务未启动")?;
        let uuid = {
            let st = ctx.state.lock().unwrap();
            st.meta
                .accounts
                .keys()
                .find(|u| handle_of(u) == handle)
                .cloned()
        }
        .ok_or("找不到这个已保存的 Chrome 账号会话")?;
        ctx.platform.delete_key(VAULT_SERVICE, &vault_id(&uuid))?;
        let mut st = ctx.state.lock().unwrap();
        st.meta.accounts.remove(&uuid);
        let _ = save_meta(&ctx.meta_path, &st.meta);
        if st.awaiting_login_uuid.as_deref() == Some(uuid.as_str()) {
            st.awaiting_login_uuid = None;
            st.awaiting_login_email = None;
            st.oauth_armed_until = None;
        }
        Ok(())
    }

    pub fn resolve_saved_handle(&self, handle: &str) -> Option<String> {
        let ctx = self.ctx.as_ref()?;
        let st = ctx.state.lock().unwrap();
        st.meta
            .accounts
            .keys()
            .find(|u| handle_of(u) == handle)
            .cloned()
    }

    pub fn sync_to_account(
        &self,
        account_uuid: &str,
        preferred_org_uuid: Option<&str>,
    ) -> Result<()> {
        let ctx = self.ctx.as_ref().ok_or("Chrome 同步服务未启动")?;
        if !valid_uuid_like(account_uuid) {
            return Err("目标账号标识无效");
        }
        let Some(vault_val) = ctx.platform.vault_read_optional(&vault_id(account_uuid))? else {
            let mut st = ctx.state.lock().unwrap();
            st.missing_session_uuid = Some(account_uuid.to_string());
            st.last_error = None;
            return Err("Chrome 尚未保存这个账号的网页会话，请先点「本地切空登新号」在 Chrome 登录一次");
        };
        let cookies = vault_val
            .get("cookies")
            .and_then(Value::as_array)
            .filter(|arr| !arr.is_empty())
            .cloned()
            .ok_or("已保存的 Chrome 网页会话不完整，请在 Chrome 重新登录一次")?;

        let (email, org_uuid, org_name) = {
            let st = ctx.state.lock().unwrap();
            let meta = st.meta.accounts.get(account_uuid);
            let email = meta.map(|m| m.email.clone()).unwrap_or_default();
            let chosen_org = preferred_org_uuid
                .map(str::to_string)
                .or_else(|| meta.and_then(|m| m.orgs.first().map(|o| o.uuid.clone())));
            let chosen_name = chosen_org.as_deref().and_then(|ou| {
                meta.and_then(|m| {
                    m.orgs
                        .iter()
                        .find(|o| o.uuid == ou)
                        .map(|o| o.name.clone())
                })
            });
            (email, chosen_org, chosen_name)
        };

        let req_id = next_id(ctx);
        {
            let mut st = ctx.state.lock().unwrap();
            if !st.connected {
                return Err("还没连上 Chrome 扩展；请先在 Chrome 加载并启用 AuthBaton Chrome Sync");
            }
            st.busy = true;
            st.busy_since = Some(now_secs());
            st.pending_request_id = Some(req_id.clone());
            st.pending_target_uuid = Some(account_uuid.to_string());
            st.pending_target_email = (!email.is_empty()).then_some(email);
            st.missing_session_uuid = None;
            st.last_error = None;
            st.last_notice = Some("正在切换 Chrome 的 claude.ai 登录并重置插件…".into());
        }

        let payload = json!({
            "type": "switch_account",
            "request_id": req_id,
            "target_account_uuid": account_uuid,
            "target_org_uuid": org_uuid.unwrap_or_default(),
            "target_org_name": org_name.unwrap_or_default(),
            "cookies": cookies,
            "reset_claude_ext": true,
            "arm_oauth": true,
            "auto_close_installed_tab": true,
        });
        if let Err(err) = send_to_extension(ctx, &payload) {
            let mut st = ctx.state.lock().unwrap();
            st.busy = false;
            st.busy_since = None;
            st.pending_request_id = None;
            st.pending_target_uuid = None;
            st.pending_target_email = None;
            st.last_error = Some(err.into());
            return Err(err);
        }
        Ok(())
    }

    /// Triggered automatically when Claude Desktop finishes switching to a different account.
    /// Never fails hard: if Chrome isn't connected or doesn't have the session saved yet, records
    /// a quiet status hint for the UI.
    pub fn auto_sync_on_desktop_switch(
        &self,
        account_uuid: &str,
        preferred_org_uuid: Option<&str>,
    ) {
        let Some(ctx) = &self.ctx else {
            return;
        };
        {
            let st = ctx.state.lock().unwrap();
            if !st.connected || st.busy {
                return;
            }
            if st.chrome_signed_in && st.chrome_account_uuid.as_deref() == Some(account_uuid) {
                return;
            }
            if !st.meta.accounts.contains_key(account_uuid) {
                drop(st);
                let mut st = ctx.state.lock().unwrap();
                st.missing_session_uuid = Some(account_uuid.to_string());
                st.awaiting_login_uuid = None;
                st.awaiting_login_email = None;
                st.oauth_armed_until = None;
                st.last_notice = Some(
                    "Chrome 还没保存当前桌面账号的网页会话，点「本地切空登新号」登录一次即可自动同步"
                        .into(),
                );
                return;
            }
        }
        let _ = self.sync_to_account(account_uuid, preferred_org_uuid);
    }

    pub fn tick(&self) {
        let Some(ctx) = &self.ctx else {
            return;
        };
        let now = now_secs();
        let mut st = ctx.state.lock().unwrap();
        if st.busy
            && st
                .busy_since
                .is_some_and(|since| now.saturating_sub(since) >= SWITCH_TIMEOUT_SECS)
        {
            st.busy = false;
            st.busy_since = None;
            st.pending_request_id = None;
            st.pending_target_uuid = None;
            st.pending_target_email = None;
            st.last_error = Some("等待 Chrome 扩展响应超时，请确认 Chrome 已打开后重试".into());
        }
        if st.oauth_armed_until.is_some_and(|until| now > until) {
            st.oauth_armed_until = None;
        }
    }

    pub fn snapshot(&self, enabled: bool, desktop: Option<&DesktopIdentity>) -> Value {
        let Some(ctx) = &self.ctx else {
            return json!({
                "enabled": false,
                "install_state": "not_installed",
                "extension_id": EXTENSION_ID,
                "extension_dir": null,
                "connected": false,
                "claude_ext_installed": false,
                "claude_ext_enabled": false,
                "claude_ext_version": null,
                "claude_ext_may_disable": false,
                "state": "off",
                "message": "可选扩展：换桌面号时同步 Chrome 的 claude.ai 登录并重置插件",
                "busy": false,
                "chrome_email": null,
                "chrome_handle": null,
                "desktop_matches": false,
                "desktop_saved": false,
                "oauth_armed_until": null,
                "saved_accounts": [],
                "last_sync_at": null,
                "last_error": null,
            });
        };

        let st = ctx.state.lock().unwrap();
        let now = now_secs();
        let desktop_uuid = desktop.map(|d| d.account_uuid.as_str());
        let desktop_matches = desktop_uuid.is_some()
            && st.chrome_signed_in
            && st.chrome_account_uuid.as_deref() == desktop_uuid;
        let desktop_saved = desktop_uuid.is_some_and(|u| st.meta.accounts.contains_key(u));
        let armed_active = st.oauth_armed_until.filter(|until| *until > now);

        let (state_code, message): (&str, String) = if !enabled {
            if st.install_state == InstallState::NotInstalled {
                (
                    "off",
                    "可选扩展：换桌面号时同步 Chrome 的 claude.ai 登录并重置插件".into(),
                )
            } else {
                ("off", "已安装扩展，开关当前关闭".into())
            }
        } else if st.install_state == InstallState::NotInstalled {
            ("not_installed", "还没释放扩展目录，请重新点安装".into())
        } else if !st.connected {
            (
                "waiting_extension",
                "扩展已就绪；请打开 chrome://extensions 拖入扩展目录一次".into(),
            )
        } else if st.busy {
            (
                "syncing",
                st.last_notice
                    .clone()
                    .unwrap_or_else(|| "正在同步 Chrome 登录…".into()),
            )
        } else if let Some(err) = &st.last_error {
            ("error", err.clone())
        } else if let Some(awaiting_uuid) = st.awaiting_login_uuid.as_deref()
            && desktop_uuid.is_none_or(|d| d == awaiting_uuid)
        {
            let who = st
                .awaiting_login_email
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("目标账号");
            if armed_active.is_some() {
                (
                    "awaiting_login",
                    format!("已切到 {who}，请在 Chrome 的 Claude 插件点一下 Log in（已就绪自动授权）"),
                )
            } else {
                (
                    "awaiting_login",
                    format!("已切到 {who}，请在 Chrome 的 Claude 插件点一下 Log in 完成授权"),
                )
            }
        } else if desktop_matches {
            let who = st
                .chrome_email
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("当前账号");
            ("synced", format!("已与桌面同号 · {who}"))
        } else if desktop_uuid.is_some() && !desktop_saved {
            (
                "missing_session",
                "Chrome 尚未保存当前桌面账号的网页会话，点「登新号」登录一次即可".into(),
            )
        } else if desktop_uuid.is_some() {
            let who = st
                .chrome_email
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("未登录");
            ("mismatch", format!("Chrome 当前为 {who}，可点立即同步"))
        } else if let Some(who) = st.chrome_email.as_deref().filter(|s| !s.is_empty()) {
            ("synced", format!("已连接 Chrome · {who}"))
        } else {
            ("synced", "已连接 Chrome，等待 claude.ai 登录".into())
        };

        let saved_accounts: Vec<Value> = st
            .meta
            .accounts
            .values()
            .map(|a| {
                json!({
                    "handle": handle_of(&a.account_uuid),
                    "email": a.email,
                    "orgs": a.orgs.iter().map(|o| json!({"name": o.name})).collect::<Vec<_>>(),
                    "cookie_count": a.cookie_names.len(),
                    "saved_at": a.saved_at,
                    "updated_at": a.updated_at,
                    "in_chrome": st.chrome_signed_in && st.chrome_account_uuid.as_deref() == Some(a.account_uuid.as_str()),
                    "in_desktop": desktop_uuid == Some(a.account_uuid.as_str()),
                })
            })
            .collect();

        json!({
            "enabled": enabled,
            "install_state": st.install_state.as_str(),
            "extension_id": EXTENSION_ID,
            "extension_dir": ctx.extension_dir.to_string_lossy(),
            "connected": st.connected,
            "claude_ext_installed": st.claude_ext.installed,
            "claude_ext_enabled": st.claude_ext.enabled,
            "claude_ext_version": (!st.claude_ext.version.is_empty()).then_some(&st.claude_ext.version),
            "claude_ext_may_disable": st.claude_ext.may_disable,
            "state": state_code,
            "message": message,
            "busy": st.busy,
            "chrome_email": st.chrome_email,
            "chrome_handle": st.chrome_account_uuid.as_deref().map(handle_of),
            "desktop_matches": desktop_matches,
            "desktop_saved": desktop_saved,
            "oauth_armed_until": armed_active,
            "saved_accounts": saved_accounts,
            "last_sync_at": st.last_sync_at,
            "last_error": st.last_error,
        })
    }
}

/// Runs as Chrome's Native Messaging Host process (`auth-service chrome-native-host --socket <path>`).
/// Translates Chrome's 4-byte little-endian length-prefixed JSON on stdio <-> newline-delimited
/// JSON on AuthBaton's private `0600` Unix socket.
pub fn run_native_host(socket_path: &Path) -> Result<()> {
    let stream = UnixStream::connect(socket_path).map_err(|_| "无法连接 AuthBaton Chrome 同步服务")?;
    let reader_stream = stream.try_clone().map_err(|_| "无法复制本地连接")?;
    let mut writer_stream = stream;

    // Socket -> Chrome stdout
    let sock_to_stdout = std::thread::spawn(move || {
        let mut reader = BufReader::new(reader_stream);
        let mut stdout = io::stdout().lock();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if write_native_frame(&mut stdout, trimmed.as_bytes()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Chrome stdin -> Socket
    let mut stdin = io::stdin().lock();
    loop {
        let payload = match read_native_frame(&mut stdin) {
            Ok(Some(bytes)) => bytes,
            Ok(None) | Err(_) => break,
        };
        // Validate JSON and strip any embedded newlines so socket framing stays 1 line = 1 message.
        let Ok(value) = serde_json::from_slice::<Value>(&payload) else {
            continue;
        };
        let Ok(compact) = serde_json::to_vec(&value) else {
            continue;
        };
        if writer_stream.write_all(&compact).is_err()
            || writer_stream.write_all(b"\n").is_err()
            || writer_stream.flush().is_err()
        {
            break;
        }
    }

    let _ = writer_stream.shutdown(std::net::Shutdown::Both);
    let _ = sock_to_stdout.join();
    Ok(())
}

fn read_native_frame<R: Read>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_NATIVE_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native frame length out of bounds",
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok(Some(payload))
}

fn write_native_frame<W: Write>(writer: &mut W, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_NATIVE_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "native frame exceeds limit",
        ));
    }
    let len = (payload.len() as u32).to_le_bytes();
    writer.write_all(&len)?;
    writer.write_all(payload)?;
    writer.flush()
}

fn accept_loop(listener: UnixListener, ctx: Arc<Context>) {
    while !ctx.stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                if ctx.stop.load(Ordering::Acquire) {
                    break;
                }
                if !same_uid(&stream) {
                    continue;
                }
                let _ = stream.set_nonblocking(false);
                let Ok(write_clone) = stream.try_clone() else {
                    continue;
                };
                let conn_id = ctx.next_conn_id.fetch_add(1, Ordering::Relaxed);
                {
                    let mut guard = ctx.writer.lock().unwrap();
                    if let Some((_, old)) = guard.replace((conn_id, write_clone)) {
                        let _ = old.shutdown(std::net::Shutdown::Both);
                    }
                }
                {
                    let mut st = ctx.state.lock().unwrap();
                    st.connected = true;
                }
                let reader_ctx = ctx.clone();
                std::thread::spawn(move || handle_connection(conn_id, stream, reader_ctx));
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                if ctx.stop.load(Ordering::Acquire) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn handle_connection(conn_id: u64, stream: UnixStream, ctx: Arc<Context>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if line.len() > MAX_NATIVE_MESSAGE_BYTES {
                    continue;
                }
                if let Ok(msg) = serde_json::from_str::<Value>(line.trim()) {
                    process_inbound(&ctx, &msg);
                }
            }
        }
    }
    // Mark disconnected only if no newer connection has already replaced this writer.
    let mut writer_guard = ctx.writer.lock().unwrap();
    if writer_guard.as_ref().is_some_and(|(id, _)| *id == conn_id) {
        *writer_guard = None;
        let mut st = ctx.state.lock().unwrap();
        st.connected = false;
        st.busy = false;
        st.busy_since = None;
    }
}

fn process_inbound(ctx: &Arc<Context>, msg: &Value) {
    let Some(kind) = msg.get("type").and_then(Value::as_str) else {
        return;
    };
    match kind {
        "hello" | "pong" => {
            let mut st = ctx.state.lock().unwrap();
            st.connected = true;
            if let Some(ver) = msg.get("extension_version").and_then(Value::as_str) {
                st.extension_version = Some(ver.to_string());
            }
            if let Some(ext) = parse_claude_ext(msg.get("claude_ext")) {
                st.claude_ext = ext;
            }
        }
        "state" | "capture_result" => {
            if let Some(ext) = parse_claude_ext(msg.get("claude_ext")) {
                ctx.state.lock().unwrap().claude_ext = ext;
            }
            let signed_in = msg.get("signed_in").and_then(Value::as_bool).unwrap_or(false);
            let account_uuid = msg
                .get("account_uuid")
                .and_then(Value::as_str)
                .filter(|s| valid_uuid_like(s))
                .map(str::to_string);
            let email = msg
                .get("email")
                .and_then(Value::as_str)
                .map(str::to_string);
            let orgs = parse_orgs(msg.get("orgs"));

            if signed_in
                && let Some(uuid) = account_uuid.as_deref()
                && let Some(cookies) = msg.get("cookies").and_then(Value::as_array)
                && !cookies.is_empty()
            {
                let _ = store_account_cookies(
                    ctx,
                    uuid,
                    email.as_deref().unwrap_or(""),
                    &orgs,
                    cookies,
                );
            }

            let mut st = ctx.state.lock().unwrap();
            st.connected = true;
            st.chrome_signed_in = signed_in;
            st.chrome_account_uuid = if signed_in { account_uuid.clone() } else { None };
            st.chrome_email = if signed_in { email } else { None };
            if signed_in
                && let Some(uuid) = account_uuid.as_deref()
                && st.missing_session_uuid.as_deref() == Some(uuid)
                && st.meta.accounts.contains_key(uuid)
            {
                st.missing_session_uuid = None;
                st.last_error = None;
            }
            if kind == "capture_result" {
                st.last_error = if signed_in {
                    None
                } else {
                    Some("当前 Chrome 未登录 claude.ai，未能保存网页会话".into())
                };
            }
        }
        "save_snapshot" => {
            let Some(uuid) = msg
                .get("account_uuid")
                .and_then(Value::as_str)
                .filter(|s| valid_uuid_like(s))
            else {
                return;
            };
            let email = msg.get("email").and_then(Value::as_str).unwrap_or("");
            let orgs = parse_orgs(msg.get("orgs"));
            if let Some(cookies) = msg.get("cookies").and_then(Value::as_array)
                && !cookies.is_empty()
            {
                let _ = store_account_cookies(ctx, uuid, email, &orgs, cookies);
            }
        }
        "switch_result" => {
            let ok = msg.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let account_uuid = msg
                .get("account_uuid")
                .and_then(Value::as_str)
                .filter(|s| valid_uuid_like(s))
                .map(str::to_string);
            let email = msg.get("email").and_then(Value::as_str).map(str::to_string);
            let claude_ext = parse_claude_ext(msg.get("claude_ext"));
            let armed_until = msg.get("armed_until").and_then(Value::as_u64);
            let ext_reset_ok = msg
                .get("ext_reset_ok")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let error = msg
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string);

            let mut st = ctx.state.lock().unwrap();
            st.busy = false;
            st.busy_since = None;
            st.pending_request_id = None;
            let target_uuid = st.pending_target_uuid.take();
            let target_email = st.pending_target_email.take();
            if let Some(ext) = claude_ext {
                st.claude_ext = ext;
            }
            if ok {
                st.chrome_signed_in = true;
                st.chrome_account_uuid = account_uuid.clone().or(target_uuid.clone());
                st.chrome_email = email.clone().or(target_email.clone());
                st.last_sync_at = Some(now_secs());
                st.last_error = None;
                st.missing_session_uuid = None;
                if st.claude_ext.installed && (ext_reset_ok || armed_until.is_some()) {
                    st.awaiting_login_uuid = st.chrome_account_uuid.clone();
                    st.awaiting_login_email = st.chrome_email.clone();
                    st.oauth_armed_until = armed_until;
                    st.last_notice = Some("已切换网页登录并重置插件，点 Claude 插件的 Log in 即可自动授权".into());
                } else {
                    st.awaiting_login_uuid = None;
                    st.awaiting_login_email = None;
                    st.oauth_armed_until = None;
                    st.last_notice = None;
                }
            } else {
                st.awaiting_login_uuid = None;
                st.awaiting_login_email = None;
                st.oauth_armed_until = None;
                st.last_error = error.or_else(|| Some("切换 Chrome 登录失败".into()));
            }
        }
        "clear_result" => {
            let ok = msg.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let claude_ext = parse_claude_ext(msg.get("claude_ext"));
            let error = msg
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string);
            let mut st = ctx.state.lock().unwrap();
            st.busy = false;
            st.busy_since = None;
            st.pending_request_id = None;
            if let Some(ext) = claude_ext {
                st.claude_ext = ext;
            }
            if ok {
                st.chrome_signed_in = false;
                st.chrome_account_uuid = None;
                st.chrome_email = None;
                st.awaiting_login_uuid = None;
                st.awaiting_login_email = None;
                st.oauth_armed_until = None;
                st.last_error = None;
                st.last_notice = Some("已清空本地 Cookie，请在 Chrome 登录新账号".into());
            } else {
                st.last_error = error.or_else(|| Some("清空 Chrome 本地登录失败".into()));
            }
        }
        "oauth_event" => {
            let event = msg.get("event").and_then(Value::as_str).unwrap_or("");
            let detail = msg
                .get("detail")
                .and_then(Value::as_str)
                .map(str::to_string);
            let mut st = ctx.state.lock().unwrap();
            match event {
                "clicked_authorize" => {
                    st.last_notice = Some("已自动点击 Authorize，等待授权完成…".into());
                }
                "completed" => {
                    st.awaiting_login_uuid = None;
                    st.awaiting_login_email = None;
                    st.oauth_armed_until = None;
                    st.last_sync_at = Some(now_secs());
                    st.last_error = None;
                    st.last_notice = Some("Chrome 的 Claude 插件已完成自动授权".into());
                }
                "turnstile" | "click_ignored" | "timeout" => {
                    st.oauth_armed_until = None;
                    st.last_error = detail.or_else(|| Some("自动授权未完成，请在 Chrome 授权页手动点一次 Authorize".into()));
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn store_account_cookies(
    ctx: &Arc<Context>,
    account_uuid: &str,
    email: &str,
    orgs: &[OrgMeta],
    cookies: &[Value],
) -> Result<()> {
    let cookie_names: Vec<String> = cookies
        .iter()
        .filter_map(|c| c.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    if !cookie_names
        .iter()
        .any(|n| n == "sessionKey" || n == "sessionKeyV3")
    {
        return Err("缺少 sessionKey Cookie");
    }
    ctx.platform.vault_write(
        &vault_id(account_uuid),
        &json!({
            "kind": "chrome_claude",
            "cookies": cookies,
        }),
    )?;
    let now = now_secs();
    let mut st = ctx.state.lock().unwrap();
    let saved_at = st
        .meta
        .accounts
        .get(account_uuid)
        .map(|m| m.saved_at)
        .unwrap_or(now);
    let existing_email = st
        .meta
        .accounts
        .get(account_uuid)
        .map(|m| m.email.clone())
        .unwrap_or_default();
    let final_email = if !email.is_empty() {
        email.to_string()
    } else {
        existing_email
    };
    let final_orgs = if !orgs.is_empty() {
        orgs.to_vec()
    } else {
        st.meta
            .accounts
            .get(account_uuid)
            .map(|m| m.orgs.clone())
            .unwrap_or_default()
    };
    st.meta.version = 1;
    st.meta.accounts.insert(
        account_uuid.to_string(),
        SavedAccountMeta {
            account_uuid: account_uuid.to_string(),
            email: final_email,
            orgs: final_orgs,
            cookie_names,
            saved_at,
            updated_at: now,
        },
    );
    let _ = save_meta(&ctx.meta_path, &st.meta);
    Ok(())
}

fn send_to_extension(ctx: &Arc<Context>, payload: &Value) -> Result<()> {
    let mut guard = ctx.writer.lock().unwrap();
    let Some((_, stream)) = guard.as_mut() else {
        return Err("还没连上 Chrome 扩展；请确认已在 Chrome 启用 AuthBaton Chrome Sync");
    };
    let bytes = serde_json::to_vec(payload).map_err(|_| "无法编码扩展消息")?;
    if stream.write_all(&bytes).is_err()
        || stream.write_all(b"\n").is_err()
        || stream.flush().is_err()
    {
        *guard = None;
        ctx.state.lock().unwrap().connected = false;
        return Err("与 Chrome 扩展的连接已断开，请稍后重试");
    }
    Ok(())
}

fn deploy_files(ctx: &Context) -> Result<()> {
    private_dir(&ctx.root)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&ctx.extension_dir)
        .map_err(|_| "无法创建 Chrome 扩展目录")?;
    for (name, content) in [
        ("manifest.json", EMBED_MANIFEST_JSON),
        ("background.js", EMBED_BACKGROUND_JS),
        ("oauth-auto-approve.js", EMBED_OAUTH_JS),
        ("popup.html", EMBED_POPUP_HTML),
        ("popup.js", EMBED_POPUP_JS),
        (VERSION_FILE, EXTENSION_VERSION),
    ] {
        write_atomic(&ctx.extension_dir.join(name), content.as_bytes(), 0o644)
            .map_err(|_| "无法写入 Chrome 扩展文件")?;
    }

    let script = format!(
        "#!/bin/sh\nexec {} chrome-native-host --socket {} \"$@\"\n",
        shell_quote(&ctx.exe_path.to_string_lossy()),
        shell_quote(&ctx.socket_path.to_string_lossy()),
    );
    write_atomic(&ctx.launcher_path, script.as_bytes(), 0o700)
        .map_err(|_| "无法写入 Chrome 本地桥接脚本")?;

    if let Some(parent) = ctx.manifest_path.parent() {
        fs::create_dir_all(parent).map_err(|_| "无法创建 Chrome NativeMessagingHosts 目录")?;
    }
    let manifest = json!({
        "name": NATIVE_HOST_NAME,
        "description": "AuthBaton Chrome Sync Native Messaging Host",
        "path": ctx.launcher_path.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": [
            format!("chrome-extension://{EXTENSION_ID}/")
        ]
    });
    let mut manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|_| "无法生成 Native Messaging 配置")?;
    manifest_bytes.push(b'\n');
    write_atomic(&ctx.manifest_path, &manifest_bytes, 0o644)
        .map_err(|_| "无法写入 Chrome Native Messaging 配置")?;
    Ok(())
}

fn detect_install_state(
    extension_dir: &Path,
    launcher_path: &Path,
    manifest_path: &Path,
) -> InstallState {
    if !extension_dir.join("manifest.json").is_file()
        || !launcher_path.is_file()
        || !manifest_path.is_file()
    {
        return InstallState::NotInstalled;
    }
    let installed_version = fs::read_to_string(extension_dir.join(VERSION_FILE))
        .ok()
        .map(|s| s.trim().to_string());
    if installed_version.as_deref() == Some(EXTENSION_VERSION) {
        InstallState::Installed
    } else {
        InstallState::NeedsUpdate
    }
}

fn bind_private_socket(path: &Path) -> io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {
            fs::remove_file(path)?;
        }
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "chrome sync socket path occupied by non-socket",
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn same_uid(stream: &UnixStream) -> bool {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: valid open unix domain socket fd and valid out pointers.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    rc == 0 && uid == unsafe { libc::geteuid() }
}

fn write_atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent"))?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("chrome-sync"),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    fs::rename(&tmp, path)
}

fn load_meta(path: &Path) -> MetaStore {
    let Ok(bytes) = fs::read(path) else {
        return MetaStore {
            version: 1,
            accounts: BTreeMap::new(),
        };
    };
    serde_json::from_slice(&bytes).unwrap_or(MetaStore {
        version: 1,
        accounts: BTreeMap::new(),
    })
}

fn save_meta(path: &Path, meta: &MetaStore) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let mut data = serde_json::to_vec_pretty(meta)?;
    data.push(b'\n');
    write_atomic(path, &data, 0o600)
}

fn parse_claude_ext(v: Option<&Value>) -> Option<ClaudeExtInfo> {
    let obj = v?.as_object()?;
    Some(ClaudeExtInfo {
        installed: obj
            .get("installed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        version: obj
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        enabled: obj.get("enabled").and_then(Value::as_bool).unwrap_or(false),
        may_disable: obj
            .get("may_disable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn parse_orgs(v: Option<&Value>) -> Vec<OrgMeta> {
    v.and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|o| {
                    let uuid = o.get("uuid")?.as_str()?.to_string();
                    let name = o
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&uuid)
                        .to_string();
                    Some(OrgMeta { uuid, name })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn next_id(ctx: &Context) -> String {
    let n = ctx.next_req_id.fetch_add(1, Ordering::Relaxed);
    format!("cs-{}-{n}", now_secs())
}

fn vault_id(account_uuid: &str) -> String {
    format!("chrome-claude-{account_uuid}")
}

fn valid_uuid_like(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn handle_of(account_uuid: &str) -> String {
    let digest = Sha256::digest(account_uuid.as_bytes());
    let mut out = String::with_capacity(12);
    for byte in &digest[..6] {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::io::Cursor;

    fn make_fixture_platform(dir: &Path) -> Platform {
        let store_path = dir.join("fake-vault.json");
        let helper = dir.join("fake-helper.py");
        let script = format!(
            r#"#!/usr/bin/env python3
import json, os, sys
p = {store_path:?}
req = json.loads(sys.stdin.read())
db = json.load(open(p)) if os.path.exists(p) else {{}}
op = req["operation"]
key = f"{{req.get('service','')}}:{{req.get('account','')}}"
if op == "write":
    db[key] = req["value"]
    open(p, "w").write(json.dumps(db))
    print(json.dumps({{"status": "ok"}}))
elif op == "read":
    if key in db:
        print(json.dumps({{"status": "ok", "value": db[key]}}))
    else:
        print(json.dumps({{"status": "missing"}}))
elif op == "delete":
    db.pop(key, None)
    open(p, "w").write(json.dumps(db))
    print(json.dumps({{"status": "ok"}}))
else:
    print(json.dumps({{"status": "ok"}}))
"#
        );
        fs::write(&helper, script).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        Platform {
            helper,
            recovery_wait: None,
            fixture_keychain: true,
        }
    }

    #[test]
    fn embedded_manifest_rsa_key_produces_exact_extension_id() {
        let manifest: Value = serde_json::from_str(EMBED_MANIFEST_JSON).unwrap();
        assert_eq!(manifest["manifest_version"], 3);
        let der = STANDARD.decode(manifest["key"].as_str().unwrap()).unwrap();
        let digest = Sha256::digest(&der);
        let mut id = String::with_capacity(32);
        for b in &digest[..16] {
            id.push((b'a' + (b >> 4)) as char);
            id.push((b'a' + (b & 0x0f)) as char);
        }
        assert_eq!(id, EXTENSION_ID);
    }

    #[test]
    fn native_messaging_frame_round_trip_and_bounds() {
        let mut buf = Vec::new();
        write_native_frame(&mut buf, br#"{"type":"ping"}"#).unwrap();
        let mut cursor = Cursor::new(buf);
        let decoded = read_native_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(decoded, br#"{"type":"ping"}"#);
        assert!(read_native_frame(&mut cursor).unwrap().is_none());

        // Zero-length frame is rejected.
        let mut bad = Cursor::new(vec![0u8, 0, 0, 0]);
        assert!(read_native_frame(&mut bad).is_err());
    }

    #[test]
    fn install_capture_switch_oauth_and_uninstall_closed_loop_with_zero_secret_leaks() {
        let tmp = PathBuf::from(format!("/tmp/am-cs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let root = tmp.join("root");
        let home = tmp.join("home");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .unwrap();
        fs::create_dir_all(&home).unwrap();
        let platform = make_fixture_platform(&tmp);

        let mut rt = Runtime::start_with_paths(
            &root,
            &home,
            PathBuf::from("/Applications/AuthManager.app/Contents/MacOS/auth-service"),
            platform.clone(),
        );
        assert!(rt.is_active());

        // 1. Install deploys extension dir, launcher, and NativeMessagingHosts manifest.
        rt.install().unwrap();
        assert!(root.join("chrome-extension/manifest.json").is_file());
        assert!(root.join("chrome-native-host").is_file());
        let nm_manifest_path = home
            .join("Library/Application Support/Google/Chrome/NativeMessagingHosts")
            .join(format!("{NATIVE_HOST_NAME}.json"));
        let nm_json: Value =
            serde_json::from_slice(&fs::read(&nm_manifest_path).unwrap()).unwrap();
        assert_eq!(
            nm_json["allowed_origins"][0],
            format!("chrome-extension://{EXTENSION_ID}/")
        );

        // 2. Simulate Chrome native host connecting over the private socket.
        let mut client = UnixStream::connect(root.join("chrome-sync.sock")).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut reader = BufReader::new(client.try_clone().unwrap());

        let acct_a = "11111111-1111-4111-8111-111111111111";
        let acct_b = "22222222-2222-4222-8222-222222222222";
        let secret_a = "sk-ant-sid02-SYNTHETIC-SECRET-A-NEVER-LEAK";
        let secret_b = "sk-ant-sid02-SYNTHETIC-SECRET-B-NEVER-LEAK";

        let state_msg = json!({
            "type": "state",
            "extension_version": EXTENSION_VERSION,
            "signed_in": true,
            "account_uuid": acct_a,
            "email": "nova@example.test",
            "orgs": [{"uuid": "33333333-3333-4333-8333-333333333333", "name": "Nova Team"}],
            "claude_ext": {"installed": true, "version": "1.0.109", "enabled": true, "may_disable": true},
            "cookies": [
                {"name": "sessionKey", "value": secret_a, "domain": ".claude.ai", "secure": true, "httpOnly": true},
                {"name": "lastActiveOrg", "value": "33333333-3333-4333-8333-333333333333", "domain": ".claude.ai", "secure": true, "httpOnly": false}
            ]
        });
        writeln!(client, "{state_msg}").unwrap();
        let save_b = json!({
            "type": "save_snapshot",
            "account_uuid": acct_b,
            "email": "orbit@example.test",
            "orgs": [{"uuid": "33333333-3333-4333-8333-333333333333", "name": "Nova Team"}],
            "cookies": [
                {"name": "sessionKey", "value": secret_b, "domain": ".claude.ai", "secure": true, "httpOnly": true}
            ]
        });
        writeln!(client, "{save_b}").unwrap();
        client.flush().unwrap();

        // Wait for reader thread to persist both snapshots.
        for _ in 0..40 {
            let snap = rt.snapshot(true, None);
            if snap["saved_accounts"].as_array().is_some_and(|a| a.len() == 2) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        // Verify meta.json & UI snapshot contain ZERO cookie values and ZERO raw account UUIDs.
        let meta_raw = fs::read_to_string(root.join("chrome-sync/meta.json")).unwrap();
        assert!(!meta_raw.contains(secret_a));
        assert!(!meta_raw.contains(secret_b));
        let desk_a = DesktopIdentity {
            account_uuid: acct_a.into(),
            org_uuid: Some("33333333-3333-4333-8333-333333333333".into()),
            display: "nova@example.test".into(),
        };
        let snap = rt.snapshot(true, Some(&desk_a));
        let snap_text = snap.to_string();
        assert_eq!(snap["state"], "synced");
        assert_eq!(snap["connected"], true);
        assert!(!snap_text.contains(secret_a));
        assert!(!snap_text.contains(secret_b));
        assert!(!snap_text.contains(acct_a));
        assert!(!snap_text.contains(acct_b));

        // 3. Auto-sync when Desktop switches to acct_b sends switch_account with secret_b from Vault.
        rt.auto_sync_on_desktop_switch(acct_b, Some("33333333-3333-4333-8333-333333333333"));
        let mut cmd_line = String::new();
        reader.read_line(&mut cmd_line).unwrap();
        let cmd: Value = serde_json::from_str(cmd_line.trim()).unwrap();
        assert_eq!(cmd["type"], "switch_account");
        assert_eq!(cmd["target_account_uuid"], acct_b);
        assert_eq!(cmd["cookies"][0]["value"], secret_b);
        assert_eq!(cmd["reset_claude_ext"], true);
        assert_eq!(cmd["arm_oauth"], true);

        // 4. Extension reports switch_result (armed) -> state becomes awaiting_login.
        let armed_until = now_secs() + 90;
        let switch_res = json!({
            "type": "switch_result",
            "request_id": cmd["request_id"],
            "ok": true,
            "matched": true,
            "account_uuid": acct_b,
            "email": "orbit@example.test",
            "claude_ext": {"installed": true, "version": "1.0.109", "enabled": true, "may_disable": true},
            "ext_reset_ok": true,
            "armed_until": armed_until,
        });
        writeln!(client, "{switch_res}").unwrap();
        client.flush().unwrap();

        let desk_b = DesktopIdentity {
            account_uuid: acct_b.into(),
            org_uuid: Some("33333333-3333-4333-8333-333333333333".into()),
            display: "orbit@example.test".into(),
        };
        for _ in 0..40 {
            let s = rt.snapshot(true, Some(&desk_b));
            if s["state"] == "awaiting_login" {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(rt.snapshot(true, Some(&desk_b))["state"], "awaiting_login");

        // 5. Extension reports oauth_event completed -> state returns to synced.
        writeln!(
            client,
            "{}",
            json!({"type": "oauth_event", "event": "completed", "account_uuid": acct_b})
        )
        .unwrap();
        client.flush().unwrap();
        for _ in 0..40 {
            let s = rt.snapshot(true, Some(&desk_b));
            if s["state"] == "synced" {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(rt.snapshot(true, Some(&desk_b))["state"], "synced");

        // 6. Uninstall removes manifest, extension dir, launcher, meta, and Vault entries.
        rt.uninstall().unwrap();
        assert!(!nm_manifest_path.exists());
        assert!(!root.join("chrome-extension").exists());
        assert!(!root.join("chrome-native-host").exists());
        assert!(
            platform
                .vault_read_optional(&vault_id(acct_a))
                .unwrap()
                .is_none()
        );
        assert!(
            platform
                .vault_read_optional(&vault_id(acct_b))
                .unwrap()
                .is_none()
        );

        rt.stop();
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn missing_session_clear_login_forget_and_oauth_failure_states() {
        let tmp = PathBuf::from(format!("/tmp/am-cs-b-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let root = tmp.join("root");
        let home = tmp.join("home");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .unwrap();
        fs::create_dir_all(&home).unwrap();
        let platform = make_fixture_platform(&tmp);

        let mut rt = Runtime::start_with_paths(
            &root,
            &home,
            PathBuf::from("/Applications/AuthManager.app/Contents/MacOS/auth-service"),
            platform.clone(),
        );
        rt.install().unwrap();

        let mut client = UnixStream::connect(root.join("chrome-sync.sock")).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut reader = BufReader::new(client.try_clone().unwrap());

        let acct_a = "aaaaaaaa-1111-4111-8111-111111111111";
        let acct_missing = "bbbbbbbb-2222-4222-8222-222222222222";
        writeln!(
            client,
            "{}",
            json!({
                "type": "state",
                "extension_version": EXTENSION_VERSION,
                "signed_in": true,
                "account_uuid": acct_a,
                "email": "alpha@example.test",
                "orgs": [],
                "claude_ext": {"installed": true, "version": "1.0.109", "enabled": true, "may_disable": true},
                "cookies": [{"name": "sessionKey", "value": "sk-ant-sid02-SYNTH-ALPHA", "domain": ".claude.ai", "secure": true, "httpOnly": true}]
            })
        )
        .unwrap();
        client.flush().unwrap();
        for _ in 0..40 {
            if rt.snapshot(true, None)["saved_accounts"]
                .as_array()
                .is_some_and(|a| a.len() == 1)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        // Auto-sync to an account with no saved Chrome cookies transitions cleanly to missing_session.
        rt.auto_sync_on_desktop_switch(acct_missing, None);
        let desk_missing = DesktopIdentity {
            account_uuid: acct_missing.into(),
            org_uuid: None,
            display: "missing@example.test".into(),
        };
        let snap = rt.snapshot(true, Some(&desk_missing));
        assert_eq!(snap["state"], "missing_session");
        assert_eq!(snap["desktop_saved"], false);

        // clear_for_login sends command and clears signed-in state upon clear_result.
        rt.clear_for_login().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let cmd: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(cmd["type"], "clear_for_login");
        writeln!(
            client,
            "{}",
            json!({"type": "clear_result", "request_id": cmd["request_id"], "ok": true})
        )
        .unwrap();
        client.flush().unwrap();
        for _ in 0..40 {
            if rt.snapshot(true, None)["busy"] == false {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(rt.snapshot(true, None)["chrome_email"].is_null());

        // Turnstile oauth_event surfaces clear error message and clears armed flag.
        writeln!(
            client,
            "{}",
            json!({"type": "oauth_event", "event": "turnstile", "detail": "检测到 Cloudflare 验证"})
        )
        .unwrap();
        client.flush().unwrap();
        for _ in 0..40 {
            if rt.snapshot(true, None)["state"] == "error" {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(rt.snapshot(true, None)["state"], "error");

        // Forget by opaque handle removes both Vault entry and meta entry.
        let handle_a = handle_of(acct_a);
        rt.forget_account_by_handle(&handle_a).unwrap();
        assert!(
            platform
                .vault_read_optional(&vault_id(acct_a))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            rt.snapshot(true, None)["saved_accounts"]
                .as_array()
                .unwrap()
                .len(),
            0
        );

        rt.stop();
        let _ = fs::remove_dir_all(&tmp);
    }
}
