//! Official tools own credential writes. This module owns only a login attempt.
use serde::Serialize;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

#[derive(Clone, Serialize)]
pub struct LoginState {
    pub attempt_id: String,
    pub provider: String,
    pub phase: String,
    pub message: String,
    pub verification_url: Option<String>,
    pub user_code: Option<String>,
}

impl LoginState {
    pub fn active(&self) -> bool {
        matches!(self.phase.as_str(), "starting" | "waiting" | "cancelling")
    }
}

pub struct LoginManager {
    pub state: Option<LoginState>,
    running: Option<Attempt>,
    serial: u64,
}

struct Attempt {
    child: Child,
    input: ChildStdin,
    output: Receiver<String>,
    login_id: Option<String>,
    deadline: Instant,
    cancelling: bool,
}

impl Drop for Attempt {
    fn drop(&mut self) {
        // Each official tool gets its own process group, including browser helpers.
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        unsafe {
            kill(-(self.child.id() as i32), 9);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Attempt {
    fn send(&mut self, value: Value) -> Result<(), &'static str> {
        serde_json::to_writer(&mut self.input, &value).map_err(|_| "官方登录连接中断")?;
        self.input
            .write_all(b"\n")
            .map_err(|_| "官方登录连接中断")?;
        self.input.flush().map_err(|_| "官方登录连接中断")
    }
}

impl LoginManager {
    pub fn new() -> Self {
        Self {
            state: None,
            running: None,
            serial: 0,
        }
    }

    pub fn start(
        &mut self,
        provider: &str,
        binary: &Path,
        codex_home: &Path,
    ) -> Result<(), &'static str> {
        if self.state.as_ref().is_some_and(LoginState::active) {
            return Err("已有登录正在进行，请先完成或取消");
        }
        let mut command = Command::new(binary);
        crate::network::configure(&mut command)?;
        match provider {
            "codex" => {
                command
                    .args(["-c", "cli_auth_credentials_store=\"file\"", "app-server"])
                    .env("CODEX_HOME", codex_home);
            }
            "claude" => {
                command
                    .args(["auth", "login", "--claudeai"])
                    .env("CLAUDE_CONFIG_DIR", codex_home)
                    .env("ANTHROPIC_CONFIG_DIR", codex_home.join("anthropic"));
                for key in [
                    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
                    "ANTHROPIC_PROFILE",
                    "ANTHROPIC_API_KEY",
                    "ANTHROPIC_AUTH_TOKEN",
                    "CLAUDE_CODE_OAUTH_TOKEN",
                    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
                    "CLAUDE_CODE_OAUTH_SCOPES",
                    "CLAUDE_CODE_OAUTH_CLIENT_ID",
                    "CLAUDE_CODE_USE_BEDROCK",
                    "CLAUDE_CODE_USE_VERTEX",
                    "CLAUDE_CODE_USE_FOUNDRY",
                    "ANTHROPIC_BASE_URL",
                ] {
                    command.env_remove(key);
                }
            }
            _ => return Err("此入口不支持登录"),
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|_| "无法启动官方登录工具")?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(16);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = String::new();
                // Do not retain unbounded output or echo raw CLI errors.
                match reader.by_ref().take(65537).read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.len() > 65536 => break,
                    Ok(_) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        self.serial += 1;
        self.state = Some(LoginState {
            attempt_id: format!("{}-{}", std::process::id(), self.serial),
            provider: provider.into(),
            phase: "starting".into(),
            message: "正在启动官方登录…".into(),
            verification_url: None,
            user_code: None,
        });
        let mut attempt = Attempt {
            child,
            input,
            output: receiver,
            login_id: None,
            deadline: Instant::now() + Duration::from_secs(60),
            cancelling: false,
        };
        if provider == "codex" {
            if attempt.send(json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"authmanager","version":env!("CARGO_PKG_VERSION")}}})).is_err() {
                self.finish("failed", "无法建立官方登录连接");
                return Err("无法建立官方登录连接");
            }
        } else {
            attempt.deadline = Instant::now() + Duration::from_secs(900);
            let state = self.state.as_mut().unwrap();
            state.phase = "waiting".into();
            state.message = "官方 Claude CLI 正在打开浏览器；请完成订阅账号授权。".into();
        }
        self.running = Some(attempt);
        Ok(())
    }

    fn finish(&mut self, phase: &str, message: &str) {
        self.running = None;
        if let Some(state) = &mut self.state {
            state.phase = phase.into();
            state.message = message.into();
            state.verification_url = None;
            state.user_code = None;
        }
    }

    fn check_attempt(&self, id: &str) -> Result<(), &'static str> {
        if self
            .state
            .as_ref()
            .is_some_and(|state| state.attempt_id == id && state.active())
        {
            Ok(())
        } else {
            Err("登录已经结束或已被新的尝试替代，请刷新状态")
        }
    }

    pub fn cancel(&mut self, id: &str) -> Result<(), &'static str> {
        self.check_attempt(id)?;
        let attempt = self.running.as_mut().ok_or("没有运行中的登录")?;
        if attempt.cancelling {
            return Ok(());
        }
        if let Some(login_id) = &attempt.login_id {
            let sent = attempt.send(
                json!({"id":3,"method":"account/login/cancel","params":{"loginId":login_id}}),
            );
            if sent.is_ok() {
                attempt.cancelling = true;
                attempt.deadline = Instant::now() + Duration::from_secs(2);
                let state = self.state.as_mut().unwrap();
                state.phase = "cancelling".into();
                state.message = "正在取消官方登录…".into();
                return Ok(());
            }
        }
        self.finish(
            "stopped",
            "本次登录进程已停止；若授权刚刚完成，请刷新本地状态确认。停止不会撤销已保存的登录。",
        );
        Ok(())
    }

    pub fn submit_code(&mut self, id: &str, code: &str) -> Result<(), &'static str> {
        self.check_attempt(id)?;
        if self.state.as_ref().unwrap().provider != "claude"
            || code.is_empty()
            || code.len() > 2048
            || !code.is_ascii()
            || code.chars().any(char::is_whitespace)
        {
            return Err("请输入官方页面提供的完整授权代码");
        }
        let attempt = self.running.as_mut().ok_or("没有运行中的登录")?;
        attempt
            .input
            .write_all(code.as_bytes())
            .and_then(|_| attempt.input.write_all(b"\n"))
            .and_then(|_| attempt.input.flush())
            .map_err(|_| "无法提交授权代码")?;
        self.state.as_mut().unwrap().message = "已交给官方 CLI，正在等待登录结果…".into();
        Ok(())
    }

    pub fn poll(&mut self) {
        let Some(attempt) = &self.running else {
            return;
        };
        let mut lines = Vec::new();
        let mut output_closed = false;
        for _ in 0..64 {
            match attempt.output.try_recv() {
                Ok(line) => lines.push(line),
                Err(TryRecvError::Disconnected) => {
                    output_closed = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        for line in lines {
            if self.running.is_none() {
                return;
            }
            if self.state.as_ref().unwrap().provider == "codex" {
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    self.codex_event(value);
                }
            } else if let Some(url) = claude_url(&line) {
                self.state.as_mut().unwrap().verification_url = Some(url);
            }
        }
        let Some(attempt) = &mut self.running else {
            return;
        };
        if Instant::now() >= attempt.deadline {
            self.finish(
                "stopped",
                "登录等待已结束；请刷新本地状态后重试。停止不会撤销已保存的登录。",
            );
        } else if output_closed && let Ok(Some(status)) = attempt.child.try_wait() {
            // Completion notifications are authoritative; drain stdout through EOF first.
            if self.state.as_ref().unwrap().provider == "claude" && status.success() {
                self.finish("succeeded", "官方 Claude CLI 已完成登录并保存凭据。");
            } else if attempt.cancelling {
                self.finish("stopped", "登录进程已停止；请刷新本地状态确认最终结果。");
            } else {
                self.finish(
                    "failed",
                    "官方登录进程已结束，未确认授权成功。请检查网络和官方账号设置后重试。",
                );
            }
        }
    }

    fn codex_event(&mut self, value: Value) {
        let Some(attempt) = &mut self.running else {
            return;
        };
        if value.get("error").is_some() && matches!(value["id"].as_u64(), Some(1..=3)) {
            self.finish(
                "failed",
                "官方登录接口返回错误；请检查 CLI 版本、网络和账号的设备登录设置。",
            );
            return;
        }
        match value["id"].as_u64() {
            Some(1) if value.get("result").is_some() => {
                let sent = attempt.send(json!({"method":"initialized"})).and_then(|_| attempt.send(json!({"id":2,"method":"account/login/start","params":{"type":"chatgptDeviceCode"}})));
                if sent.is_err() {
                    self.finish("failed", "官方登录连接中断");
                }
            }
            Some(2) => {
                let result = &value["result"];
                let fields = (
                    result["loginId"].as_str(),
                    result["verificationUrl"].as_str(),
                    result["userCode"].as_str(),
                );
                if let (Some(id), Some("https://auth.openai.com/codex/device"), Some(code)) = fields
                    && result["type"] == "chatgptDeviceCode"
                    && !id.is_empty()
                    && id.len() <= 256
                    && !code.is_empty()
                    && code.len() <= 32
                    && code.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                {
                    attempt.login_id = Some(id.into());
                    attempt.deadline = Instant::now() + Duration::from_secs(900);
                    let state = self.state.as_mut().unwrap();
                    state.phase = "waiting".into();
                    state.message = "在官方页面输入授权码，确认你希望在本机使用的账号。".into();
                    state.verification_url = Some("https://auth.openai.com/codex/device".into());
                    state.user_code = Some(code.into());
                } else {
                    self.finish(
                        "failed",
                        "官方工具返回了无法识别的设备登录信息，请更新 CLI 后重试。",
                    );
                }
            }
            Some(3) if value["result"]["status"] == "canceled" => self.finish(
                "cancelled",
                "官方工具已取消本次授权等待；已保存的登录不会被撤销。",
            ),
            Some(3) => self.finish("stopped", "授权等待已结束；请刷新本地状态确认最终结果。"),
            _ if value["method"] == "account/login/completed" => {
                let params = &value["params"];
                if params["loginId"].as_str() == attempt.login_id.as_deref()
                    && attempt.login_id.is_some()
                {
                    if params["success"] == true {
                        self.finish("succeeded", "官方 Codex 已完成登录并保存凭据。请在桌面 App 中确认账号；必要时重新启动桌面 App。");
                    } else if attempt.cancelling {
                        self.finish(
                            "cancelled",
                            "官方工具已取消本次授权等待；已保存的登录不会被撤销。",
                        );
                    } else {
                        self.finish("failed", "本次授权未完成。请检查官方页面并重试。");
                    }
                }
            }
            _ => {}
        }
    }
}

fn claude_url(line: &str) -> Option<String> {
    // CLI output is private to this adapter. Only the manual authorization URL crosses IPC.
    line.split_whitespace()
        .find(|part| {
            part.starts_with("https://claude.ai/oauth/authorize?")
                && part.len() <= 8192
                && part.bytes().all(|c| c.is_ascii_graphic() && c != b'\x1b')
        })
        .map(str::to_owned)
}

pub fn find_codex(home: &Path) -> Option<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
        home.join(".local/bin/codex"),
    ];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path).map(|p| p.join("codex")));
    }
    paths.into_iter().find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new(body: &str) -> Self {
            static SERIAL: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "auth-login-test-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            let path = dir.join("official-tool");
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(dir)
        }
        fn binary(&self) -> PathBuf {
            self.0.join("official-tool")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    const HANDSHAKE: &str = r#"
IFS= read -r request
printf '%s\n' '{"id":1,"result":{}}'
IFS= read -r initialized
IFS= read -r start
printf '%s\n' '{"id":2,"result":{"type":"chatgptDeviceCode","loginId":"test-login","verificationUrl":"https://auth.openai.com/codex/device","userCode":"FAKE-1234"}}'
"#;

    fn wait_until(manager: &mut LoginManager, predicate: impl Fn(&LoginState) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            manager.poll();
            if manager.state.as_ref().is_some_and(&predicate) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "synthetic login did not reach expected phase"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn codex_success_drains_burst_before_process_exit() {
        let fixture = Fixture::new(&format!(
            r#"{HANDSHAKE}
i=0
while [ "$i" -lt 100 ]; do printf '%s\n' '{{"method":"irrelevant"}}'; i=$((i+1)); done
printf '%s\n' '{{"method":"account/login/completed","params":{{"loginId":"test-login","success":true}}}}'
"#
        ));
        let mut manager = LoginManager::new();
        manager
            .start("codex", &fixture.binary(), &fixture.0)
            .unwrap();
        wait_until(&mut manager, |s| !s.active());
        assert_eq!(manager.state.unwrap().phase, "succeeded");
    }

    #[test]
    fn cancel_notification_before_response_is_not_failure() {
        let fixture = Fixture::new(&format!(
            r#"{HANDSHAKE}
IFS= read -r cancel
printf '%s\n' '{{"method":"account/login/completed","params":{{"loginId":"test-login","success":false,"error":"synthetic-secret"}}}}'
printf '%s\n' '{{"id":3,"result":{{"status":"canceled"}}}}'
sleep 10
"#
        ));
        let mut manager = LoginManager::new();
        manager
            .start("codex", &fixture.binary(), &fixture.0)
            .unwrap();
        wait_until(&mut manager, |s| s.phase == "waiting");
        let id = manager.state.as_ref().unwrap().attempt_id.clone();
        manager.cancel(&id).unwrap();
        wait_until(&mut manager, |s| !s.active());
        assert_eq!(manager.state.as_ref().unwrap().phase, "cancelled");
        let output = serde_json::to_string(&manager.state).unwrap();
        assert!(!output.contains("synthetic-secret"));
        assert!(!output.contains("FAKE-1234"));
    }

    #[test]
    fn rejects_stale_cancel_and_overlapping_login() {
        let fixture = Fixture::new(&format!("{HANDSHAKE}\nsleep 10"));
        let mut manager = LoginManager::new();
        manager
            .start("codex", &fixture.binary(), &fixture.0)
            .unwrap();
        wait_until(&mut manager, |s| s.phase == "waiting");
        assert!(
            manager
                .start("claude", &fixture.binary(), &fixture.0)
                .is_err()
        );
        assert!(manager.cancel("old-attempt").is_err());
        assert_eq!(manager.state.as_ref().unwrap().phase, "waiting");
        manager.running.as_mut().unwrap().deadline = Instant::now();
        manager.poll();
        assert_eq!(manager.state.as_ref().unwrap().phase, "stopped");
        assert!(manager.running.is_none());
    }

    #[test]
    fn claude_manual_code_and_exit_status_are_used_without_raw_output() {
        let fixture = Fixture::new(
            r#"
printf '%s\n' 'https://claude.ai/oauth/authorize?state=synthetic' 'synthetic-secret'
IFS= read -r code
[ "$code" = 'test#state' ] || exit 1
exit 0
"#,
        );
        let mut manager = LoginManager::new();
        manager
            .start("claude", &fixture.binary(), &fixture.0)
            .unwrap();
        wait_until(&mut manager, |s| s.verification_url.is_some());
        let id = manager.state.as_ref().unwrap().attempt_id.clone();
        assert!(manager.submit_code(&id, "code\nextra").is_err());
        manager.submit_code(&id, "test#state").unwrap();
        wait_until(&mut manager, |s| !s.active());
        assert_eq!(manager.state.as_ref().unwrap().phase, "succeeded");
        assert!(
            !serde_json::to_string(&manager.state)
                .unwrap()
                .contains("synthetic")
        );
    }

    #[test]
    fn cli_success_text_does_not_override_nonzero_exit() {
        let fixture = Fixture::new("printf '%s\\n' 'Login successful. synthetic-secret'\nexit 1");
        let mut manager = LoginManager::new();
        manager
            .start("claude", &fixture.binary(), &fixture.0)
            .unwrap();
        wait_until(&mut manager, |s| !s.active());
        assert_eq!(manager.state.as_ref().unwrap().phase, "failed");
        assert!(
            !serde_json::to_string(&manager.state)
                .unwrap()
                .contains("synthetic-secret")
        );
    }

    #[test]
    fn untrusted_urls_never_cross_ipc() {
        assert!(claude_url("https://claude.ai.evil/oauth/authorize?x=1").is_none());
        assert!(claude_url("https://claude.ai@evil/oauth/authorize?x=1").is_none());
        let bad = HANDSHAKE.replace(
            "https://auth.openai.com/codex/device",
            "https://evil.example/login",
        );
        let fixture = Fixture::new(&format!("{bad}\nsleep 10"));
        let mut manager = LoginManager::new();
        manager
            .start("codex", &fixture.binary(), &fixture.0)
            .unwrap();
        wait_until(&mut manager, |s| !s.active());
        assert_eq!(manager.state.as_ref().unwrap().phase, "failed");
        assert!(manager.state.as_ref().unwrap().verification_url.is_none());
    }
}
