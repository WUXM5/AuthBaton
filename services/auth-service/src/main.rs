mod accounts;
mod automation;
mod claude;
mod claude_locks;
mod claude_mod;
mod claude_resets;
mod clients;
mod cloud;
mod codex_api;
mod credentials;
mod desktop;
mod login;
mod maintenance;
mod network;
mod pi_broker;
mod platform;
mod quota;
mod resets;
mod status;
mod warmup;

use serde::Deserialize;
use serde_json::{Value, json};
use status::Sources;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
    id: String,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AccountParams {
    provider: Option<String>,
    id: Option<String>,
    label: Option<String>,
    online: Option<bool>,
    confirm: Option<bool>,
    automatic: Option<bool>,
    path: Option<PathBuf>,
    port: Option<u16>,
    enabled: Option<bool>,
    credit_id: Option<String>,
    next_grant: Option<bool>,
    reset_id: Option<String>,
    cloud_action: Option<String>,
    mode: Option<String>,
    endpoint: Option<String>,
    cloud_account_id: Option<String>,
    strategy: Option<automation::Config>,
    before: Option<String>,
    limit: Option<usize>,
    hour: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptParams {
    attempt_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeParams {
    attempt_id: String,
    code: String,
}

fn sources(home: &Path) -> Sources {
    let find_binary = || {
        let mut candidates = vec![
            home.join(".local/bin/claude"),
            PathBuf::from("/opt/homebrew/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
        ];
        if let Some(paths) = env::var_os("PATH") {
            candidates.extend(env::split_paths(&paths).map(|path| path.join("claude")));
        }
        candidates.extend(npm_claude(home));
        candidates.into_iter().find(|path| path.is_file())
    };
    Sources {
        codex_home: env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex")),
        pi_home: env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".pi/agent")),
        claude_bin: find_binary(),
    }
}

/// Where `npm install -g` puts `claude` outside the paths above, for a service started from Finder
/// without the user's shell PATH: the prefix set in ~/.npmrc, ~/.npm-global, then nvm's Node
/// versions from newest to oldest.
fn npm_claude(home: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_to_string(home.join(".npmrc"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "prefix").then_some(value.trim())
        })
        .map(|prefix| {
            ["~/", "${HOME}/", "$HOME/"]
                .iter()
                .find_map(|home_prefix| prefix.strip_prefix(home_prefix))
                .map_or_else(|| PathBuf::from(prefix), |rest| home.join(rest))
        })
        .filter(|prefix| prefix.is_absolute())
        .map(|prefix| prefix.join("bin/claude"))
        .collect();
    found.push(home.join(".npm-global/bin/claude"));
    let mut versions: Vec<((u32, u32, u32), PathBuf)> =
        std::fs::read_dir(home.join(".nvm/versions/node"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let mut parts = name.strip_prefix('v')?.split('.').map(|p| p.parse().ok());
                let version = (parts.next()??, parts.next()??, parts.next()??);
                Some((version, entry.path().join("bin/claude")))
            })
            .collect();
    versions.sort_by_key(|(version, _)| std::cmp::Reverse(*version));
    found.extend(versions.into_iter().map(|(_, path)| path));
    found
}

fn response(
    input: &[u8],
    sources: &Sources,
    _codex: Option<&Path>,
    login: &mut login::LoginManager,
    shutdown: &mut bool,
    accounts: &mut Option<accounts::Accounts>,
) -> Value {
    let request = match serde_json::from_slice::<Request>(input) {
        Ok(request) if request.version == 1 && !request.id.is_empty() && request.id.len() <= 80 => {
            request
        }
        _ => {
            return json!({"version":1,"id":null,"error":{"code":"invalid_request","message":"请求格式无效"}});
        }
    };
    login.poll();
    if let Some(accounts) = accounts.as_mut() {
        accounts.finish_login(login);
    }
    let operation = || -> Result<Value, &'static str> {
        if !request.method.starts_with("accounts.")
            && !matches!(
                request.method.as_str(),
                "login.start" | "login.cancel" | "login.code"
            )
            && !request.params.is_null()
        {
            return Err("此操作不接受参数");
        }
        Ok(match request.method.as_str() {
            "ping" => json!({"service_version":env!("CARGO_PKG_VERSION")}),
            "snapshot" => serde_json::to_value(status::snapshot(sources)).unwrap(),
            "login.status" => json!({"attempt":login.state}),
            "login.start" => return Err("请使用账号列表的添加或重新登录入口"),
            method if method.starts_with("accounts.") => {
                let manager = accounts.as_mut().ok_or("账号存储服务不可用")?;
                let params: AccountParams = if request.params.is_null() {
                    AccountParams::default()
                } else {
                    serde_json::from_value(request.params).map_err(|_| "账号操作参数无效")?
                };
                match method {
                    "accounts.cloud" => manager.cloud_command(
                        params.cloud_action.as_deref().unwrap_or("refresh"),
                        params.mode.as_deref(),
                        params.endpoint.as_deref(),
                        params.label.as_deref(),
                        params.id.as_deref(),
                        params.confirm == Some(true),
                        params.automatic,
                    )?,
                    "accounts.list" => {}
                    "accounts.reset_history" => {
                        return manager.reset_history(
                            params.id.as_deref(),
                            params.before.as_deref(),
                            params.limit,
                        );
                    }
                    "accounts.strategy" => manager.configure_strategy(
                        params.strategy.ok_or("缺少额度策略")?,
                        params.confirm == Some(true),
                    )?,
                    "accounts.strategy_stop" => {
                        manager.stop_strategy(params.confirm == Some(true))?
                    }
                    "accounts.strategy_forget" => {
                        manager.forget_strategy(params.confirm == Some(true))?
                    }
                    "accounts.clients" => manager.refresh_clients()?,
                    "accounts.quota" => manager.request_quota(params.id.as_deref())?,
                    "accounts.warmup" => manager.configure_warmup(
                        params.id.as_deref().ok_or("缺少账号标识")?,
                        params.enabled.ok_or("缺少预热开关")?,
                        params.hour,
                    )?,
                    "accounts.warmup_now" => {
                        manager.warmup_now(params.id.as_deref().ok_or("缺少账号标识")?)?
                    }
                    "accounts.reset_quota" => {
                        let id = params.id.as_deref().ok_or("缺少账号标识")?;
                        let reset_id = params.reset_id.as_deref().ok_or("缺少重置操作标识")?;
                        let credit_id = match params.credit_id {
                            Some(credit_id) => credit_id,
                            // Claude grant IDs never reach the views: a new Claude reset asks for the next grant.
                            None if params.next_grant == Some(true) => manager.next_grant(id)?,
                            None => manager.retry_credit(reset_id)?,
                        };
                        manager.reset_quota(
                            id,
                            &credit_id,
                            reset_id,
                            params.confirm == Some(true),
                        )?
                    }
                    "accounts.remove" => manager.remove(
                        params.id.as_deref().ok_or("缺少账号标识")?,
                        params.confirm == Some(true),
                    )?,
                    "accounts.desktop" => manager.configure_desktop(params.enabled, params.port)?,
                    "accounts.desktop_launch" => manager.launch_desktop()?,
                    "accounts.desktop_relaunch_switch" => manager.relaunch_desktop_and_activate(
                        params.id.as_deref().ok_or("缺少账号标识")?,
                    )?,
                    "accounts.import" => manager.import(
                        params.provider.as_deref().ok_or("缺少账号类型")?,
                        params.label.as_deref().unwrap_or(""),
                    )?,
                    "accounts.import_file" => manager.import_codex_file(
                        params.path.as_deref().ok_or("缺少认证文件路径")?,
                        params.label.as_deref().ok_or("缺少账号名称")?,
                    )?,
                    "accounts.login" if params.cloud_account_id.is_some() => manager
                        .begin_cloud_login(params.cloud_account_id.as_deref().unwrap(), login)?,
                    "accounts.login" => manager.begin_login(
                        params.provider.as_deref().ok_or("缺少账号类型")?,
                        params.label.as_deref().ok_or("缺少账号名称")?,
                        params.id.as_deref(),
                        login,
                    )?,
                    "accounts.check" => {
                        let id = params.id.as_deref().ok_or("缺少账号标识")?;
                        if params.online == Some(true) {
                            manager.request_check(id, true)?;
                        } else {
                            manager.local_check(id)?;
                        }
                    }
                    "accounts.activate" => manager.activate(
                        params.id.as_deref().ok_or("缺少账号标识")?,
                        params.confirm == Some(true),
                    )?,
                    "accounts.update" => manager.configure(
                        params.id.as_deref().ok_or("缺少账号标识")?,
                        params.label.as_deref(),
                        params.automatic,
                    )?,
                    "accounts.recover" => manager.recover()?,
                    "accounts.retry_save" => {
                        manager.retry_save(params.id.as_deref().ok_or("缺少账号标识")?)?
                    }
                    _ => return Err("不支持的账号操作"),
                }
                let mut directory = manager.snapshot();
                let cloud = manager.cloud_snapshot();
                directory["busy"] = json!(directory["busy"] == true || cloud["busy"] == true);
                directory["cloud"] = cloud;
                json!({"directory":directory,"attempt":login.state})
            }
            "login.cancel" => {
                let params: AttemptParams =
                    serde_json::from_value(request.params).map_err(|_| "取消请求无效")?;
                login.cancel(&params.attempt_id)?;
                json!({"attempt":login.state})
            }
            "login.code" => {
                let params: CodeParams =
                    serde_json::from_value(request.params).map_err(|_| "授权请求无效")?;
                login.submit_code(&params.attempt_id, &params.code)?;
                json!({"attempt":login.state})
            }
            "shutdown" => {
                *shutdown = true;
                json!({"service_version":env!("CARGO_PKG_VERSION")})
            }
            _ => return Err("不支持的操作"),
        })
    };
    match operation() {
        Ok(result) => json!({"version":1,"id":request.id,"result":result}),
        Err(message) => {
            json!({"version":1,"id":request.id,"error":{"code":"operation_failed","message":message}})
        }
    }
}

struct SocketOwner {
    path: PathBuf,
    _lock: File,
}

impl Drop for SocketOwner {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn bind_socket(path: &Path) -> Result<(UnixListener, SocketOwner), &'static str> {
    let parent = path.parent().ok_or("socket 路径无效")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(|_| "无法创建运行目录")?;
    let metadata = fs::symlink_metadata(parent).map_err(|_| "无法检查运行目录")?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err("运行目录必须是私有目录（0700）；服务不会修改已有目录权限");
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(parent.join("service.lockfile"))
        .map_err(|_| "无法打开服务锁")?;
    lock.try_lock()
        .map_err(|_| "认证服务已经运行，或服务锁不可用")?;
    // Only the lock owner may retire a socket left by an earlier process.
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {
            fs::remove_file(path).map_err(|_| "无法移除旧 socket")?
        }
        Ok(_) => return Err("socket 路径已被其他文件占用"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法检查 socket 路径"),
    }
    let listener = UnixListener::bind(path).map_err(|_| "无法监听本地 socket")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| "无法设置 socket 权限")?;
    Ok((
        listener,
        SocketOwner {
            path: path.to_path_buf(),
            _lock: lock,
        },
    ))
}

fn handle(
    stream: &mut UnixStream,
    sources: &Sources,
    codex: Option<&Path>,
    login: &mut login::LoginManager,
    shutdown: &mut bool,
    accounts: &mut Option<accounts::Accounts>,
) -> std::io::Result<()> {
    // An accepted socket inherits the listener's non-blocking flag on macOS.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut input = Vec::new();
    let mut reader = BufReader::new(&mut *stream);
    // A small, single-line protocol; oversized requests never reach credential reads.
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(());
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|i| i + 1)
            .unwrap_or(available.len());
        if input.len() + count > 4096 {
            return Ok(());
        }
        input.extend_from_slice(&available[..count]);
        reader.consume(count);
        if input.last() == Some(&b'\n') {
            break;
        }
    }
    let output = response(&input, sources, codex, login, shutdown, accounts);
    serde_json::to_writer(&mut *stream, &output)?;
    stream.write_all(b"\n")
}

fn run() -> Result<(), &'static str> {
    let home = env::var_os("HOME").map(PathBuf::from).ok_or("缺少 HOME")?;
    let args: Vec<String> = env::args().skip(1).collect();
    let sources = sources(&home);
    match args.as_slice() {
        [command] if command == "snapshot" => {
            serde_json::to_writer_pretty(std::io::stdout(), &status::snapshot(&sources))
                .map_err(|_| "无法输出状态")?;
            println!();
            Ok(())
        }
        [command, flag, socket, rest @ ..]
            if command == "serve"
                && flag == "--socket"
                && (rest.is_empty() || rest == ["--managed"]) =>
        {
            let (listener, _owner) = bind_socket(Path::new(socket))?;
            listener
                .set_nonblocking(true)
                .map_err(|_| "无法设置服务监听")?;
            let parent_closed = Arc::new(AtomicBool::new(false));
            if !rest.is_empty() {
                let closed = parent_closed.clone();
                std::thread::spawn(move || {
                    let mut buffer = [0u8; 1];
                    while matches!(std::io::stdin().read(&mut buffer), Ok(1..)) {}
                    closed.store(true, Ordering::Release);
                });
            }
            let codex = login::find_codex(&home);
            let mut login = login::LoginManager::new();
            let root = Path::new(socket).parent().ok_or("运行目录无效")?;
            let mut accounts = Some(accounts::Accounts::open(
                root,
                &home,
                sources.clone(),
                codex.clone(),
            )?);
            let mut shutdown = false;
            let pi = pi_broker::Listener::bind(root)?;
            let claude_mod = claude_mod::Listener::bind(root)?;
            while !shutdown && !parent_closed.load(Ordering::Acquire) {
                login.poll();
                if let Some(manager) = accounts.as_mut() {
                    manager.finish_login(&mut login);
                    manager.tick();
                    pi.poll(manager);
                    claude_mod.poll(manager);
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = handle(
                            &mut stream,
                            &sources,
                            codex.as_deref(),
                            &mut login,
                            &mut shutdown,
                            &mut accounts,
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(25))
                    }
                    Err(_) => return Err("本地连接中断"),
                }
            }
            if let Some(state) = login.state.clone().filter(login::LoginState::active) {
                let _ = login.cancel(&state.attempt_id);
                while login.state.as_ref().is_some_and(login::LoginState::active) {
                    login.poll();
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
            if let Some(manager) = accounts.as_mut() {
                manager.finish_login(&mut login);
                manager.stop();
            }
            Ok(())
        }
        _ => Err("用法：auth-service snapshot | serve --socket <absolute-path> [--managed]"),
    }
}

fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unused_sources() -> Sources {
        Sources {
            codex_home: PathBuf::from("/unused"),
            pi_home: PathBuf::from("/unused"),
            claude_bin: None,
        }
    }

    #[test]
    fn rejects_unknown_versions_and_extra_fields() {
        for request in [
            br#"{"version":2,"id":"a","method":"ping"}"#.as_slice(),
            br#"{"version":1,"id":"a","method":"ping","token":"synthetic-secret"}"#.as_slice(),
        ] {
            let result = response(
                request,
                &unused_sources(),
                None,
                &mut login::LoginManager::new(),
                &mut false,
                &mut None,
            );
            assert_eq!(result["error"]["code"], "invalid_request");
            assert!(!result.to_string().contains("synthetic-secret"));
        }
    }

    #[test]
    fn npm_installs_outside_the_shell_path_are_found_newest_nvm_first() {
        let home = env::temp_dir().join(format!("auth-manager-npm-{}", std::process::id()));
        for version in ["v20.1.0", "v22.10.0", "v22.9.3", "system"] {
            fs::create_dir_all(home.join(".nvm/versions/node").join(version)).unwrap();
        }
        fs::write(
            home.join(".npmrc"),
            "registry=https://example.invalid/\nprefix = ~/custom\nprefix=relative\n",
        )
        .unwrap();
        let found = npm_claude(&home);
        fs::remove_dir_all(&home).unwrap();
        let node = |v: &str| home.join(".nvm/versions/node").join(v).join("bin/claude");
        assert_eq!(
            found,
            vec![
                home.join("custom/bin/claude"),
                home.join(".npm-global/bin/claude"),
                node("v22.10.0"),
                node("v22.9.3"),
                node("v20.1.0"),
            ]
        );
    }

    #[test]
    fn login_requires_explicit_confirmation_and_strict_parameters() {
        let mut login = login::LoginManager::new();
        for params in [
            json!({"provider":"codex","confirm_replace":false}),
            json!({"provider":"codex","confirm_replace":true,"extra":"synthetic-secret"}),
            json!({"provider":"pi","confirm_replace":true}),
        ] {
            let request = json!({"version":1,"id":"test","method":"login.start","params":params});
            let result = response(
                request.to_string().as_bytes(),
                &unused_sources(),
                Some(Path::new("/unused")),
                &mut login,
                &mut false,
                &mut None,
            );
            assert!(result.get("error").is_some());
            assert!(!result.to_string().contains("synthetic-secret"));
            assert!(login.state.is_none());
        }
    }

    #[test]
    fn owner_lock_prevents_duplicate_service_and_recovers_stale_socket() {
        let directory =
            env::temp_dir().join(format!("auth-manager-socket-test-{}", std::process::id()));
        let path = directory.join("service.sock");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let old = UnixListener::bind(&path).unwrap();
        drop(old);
        let (listener, owner) = bind_socket(&path).unwrap();
        assert!(bind_socket(&path).is_err());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(listener);
        drop(owner);
        assert!(!path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn existing_public_directory_is_rejected_without_chmod() {
        let directory =
            env::temp_dir().join(format!("auth-manager-mode-test-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(bind_socket(&directory.join("service.sock")).is_err());
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn ui_socket_answers_a_request_written_after_accept() {
        let directory = env::temp_dir().join(format!("am-ui-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let listener = UnixListener::bind(directory.join("service.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut client = UnixStream::connect(directory.join("service.sock")).unwrap();
        let late = std::thread::spawn(move || -> std::io::Result<String> {
            std::thread::sleep(Duration::from_millis(100));
            client.write_all(b"{\"version\":1,\"id\":\"late\",\"method\":\"ping\"}\n")?;
            let mut line = String::new();
            BufReader::new(client).read_line(&mut line)?;
            Ok(line)
        });
        let (mut stream, _) = listener.accept().unwrap();
        let _ = handle(
            &mut stream,
            &unused_sources(),
            None,
            &mut login::LoginManager::new(),
            &mut false,
            &mut None,
        );
        drop(stream);
        let line = late.join().unwrap();
        fs::remove_dir_all(directory).unwrap();
        let reply: Value =
            serde_json::from_str(&line.expect("service dropped the connection")).unwrap();
        assert_eq!(reply["id"], "late");
        assert!(reply.get("result").is_some());
    }
}
