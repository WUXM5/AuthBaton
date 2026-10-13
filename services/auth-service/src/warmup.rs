//! Claude five-hour window warm-up. The window starts at an account's first request (observed
//! once: counted from that time rounded down to 10 minutes, not to the hour), so one tiny
//! official CLI turn a few hours before work makes the window reset early in the working day.
//! Nothing here stores or logs credentials.
use crate::platform::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::JoinHandle;
use std::time::Duration;

/// A slot missed while the Mac slept is still sent this long after its hour; later it no longer
/// moves the reset ahead of a working session.
pub const CATCH_UP: u64 = 5 * 3600;
pub const RETRY: u64 = 300;
pub const ATTEMPTS: u8 = 3;
const CHECK: u64 = 30;
const TIMEOUT: Duration = Duration::from_secs(120);
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

#[derive(Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Local hour, 0–23.
    pub hour: u8,
    /// Start of the last slot that was sent or given up on.
    #[serde(default)]
    pub slot: Option<u64>,
    #[serde(default)]
    pub last: Option<Record>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub at: u64,
    pub ok: bool,
    pub message: String,
}
impl Plan {
    /// The slot to send now, if any.
    pub fn due(&self, time: u64) -> Option<u64> {
        let slot = slot(self.hour, time)?;
        (time < slot + CATCH_UP && self.slot != Some(slot)).then_some(slot)
    }
}

/// Start of the latest local `hour`:00 at or before `time`.
pub fn slot(hour: u8, time: u64) -> Option<u64> {
    let t = libc::time_t::try_from(time).ok()?;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return None;
    }
    if tm.tm_hour < i32::from(hour) {
        tm.tm_mday -= 1;
    }
    (tm.tm_hour, tm.tm_min, tm.tm_sec, tm.tm_isdst) = (hour.into(), 0, 0, -1);
    let start = unsafe { libc::mktime(&mut tm) };
    u64::try_from(start).ok().filter(|s| *s <= time)
}

/// Why an attempt failed. Only one that may have reached Claude uses up one of the attempts; the
/// others are retried every few minutes until the catch-up period ends.
#[derive(Debug, PartialEq)]
pub enum Failure {
    NotSent(&'static str),
    Failed(&'static str),
}
pub type Sent = std::result::Result<(), Failure>;
pub type Launch = fn(&Path, &Path, &str) -> Sent;
#[derive(Default)]
pub struct Runtime {
    job: Option<(String, u64, JoinHandle<Sent>)>,
    /// Failed attempts in an account's current slot and the earliest next try; memory only.
    failures: HashMap<String, (u64, u8, u64)>,
    /// Slots change at most hourly, so plans are looked at every `CHECK` seconds, not every tick.
    next_check: u64,
    #[cfg(test)]
    pub launch: Option<Launch>,
}
impl Runtime {
    /// Whether plans should be looked at now; true at most once every `CHECK` seconds.
    pub fn check_due(&mut self, time: u64) -> bool {
        let due = time >= self.next_check;
        if due {
            self.next_check = time + CHECK;
        }
        due
    }
    /// Look at the plans on the next tick (after a setting change or a finished attempt).
    pub fn recheck(&mut self) {
        self.next_check = 0;
    }
    pub fn forget(&mut self, id: &str) {
        self.failures.remove(id);
    }
    pub fn running(&self, id: &str) -> bool {
        self.job.as_ref().is_some_and(|job| job.0 == id)
    }
    /// Blocks until a running attempt ends, so its result can be recorded before the service
    /// exits instead of the slot being sent again after a restart.
    pub fn wait(&self) {
        while self.job.as_ref().is_some_and(|job| !job.2.is_finished()) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn ready(&self, id: &str, slot: u64, time: u64) -> bool {
        self.job.is_none()
            && self
                .failures
                .get(id)
                .is_none_or(|f| f.0 != slot || time >= f.2)
    }
    pub fn start(
        &mut self,
        id: &str,
        slot: u64,
        prepared: Result<(PathBuf, String)>,
        dir: PathBuf,
    ) {
        #[cfg(test)]
        let launch = self
            .launch
            .unwrap_or(|_, _, _| Err(Failure::NotSent("test launcher not configured")));
        #[cfg(not(test))]
        let launch: Launch = run;
        self.job = Some((
            id.into(),
            slot,
            std::thread::spawn(move || {
                let (binary, token) = prepared.map_err(Failure::NotSent)?;
                launch(&binary, &dir, &token)
            }),
        ));
    }
    /// A finished attempt: the account, the slot it settles (None while retries remain) and the
    /// result to show.
    pub fn poll(&mut self, time: u64) -> Option<(String, Option<u64>, Record)> {
        if !self.job.as_ref().is_some_and(|job| job.2.is_finished()) {
            return None;
        }
        let (id, slot, job) = self.job.take()?;
        let result = job.join().unwrap_or(Err(Failure::Failed("预热进程中断")));
        let used = match self.failures.get(&id) {
            Some(f) if f.0 == slot => f.1,
            _ => 0,
        };
        let (settled, ok, message) = match result {
            Ok(()) => {
                self.failures.remove(&id);
                (Some(slot), true, "已发送，5 小时窗口开始计时".to_string())
            }
            Err(Failure::NotSent(error)) => {
                self.failures.insert(id.clone(), (slot, used, time + RETRY));
                (None, false, format!("{error}；补发时间内每 5 分钟重试"))
            }
            Err(Failure::Failed(error)) => {
                let count = used + 1;
                if count >= ATTEMPTS {
                    self.failures.remove(&id);
                    (
                        Some(slot),
                        false,
                        format!("{error}；已试 {ATTEMPTS} 次，等下次预热"),
                    )
                } else {
                    self.failures
                        .insert(id.clone(), (slot, count, time + RETRY));
                    (None, false, format!("{error}；5 分钟后重试"))
                }
            }
        };
        Some((
            id,
            settled,
            Record {
                at: time,
                ok,
                message,
            },
        ))
    }
}

/// One official CLI turn on the smallest model, isolated from the user's home, settings, plugins
/// and sessions. The access token reaches the CLI only through an inherited pipe (fd 3).
pub fn run(binary: &Path, dir: &Path, token: &str) -> Sent {
    crate::platform::private_dir(dir).map_err(Failure::NotSent)?;
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(Failure::NotSent("无法准备预热请求"));
    }
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in fds {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    // A token is far below the pipe buffer, so this never blocks.
    std::fs::File::from(write)
        .write_all(token.as_bytes())
        .map_err(|_| Failure::NotSent("无法准备预热请求"))?;
    let source = read.as_raw_fd();
    let mut command = Command::new(binary);
    command.env_clear();
    crate::network::configure(&mut command).map_err(Failure::NotSent)?;
    // An npm install of an older CLI is a `#!/usr/bin/env node` script with node beside it.
    let path = match binary.parent().and_then(Path::to_str) {
        Some(bin) if !bin.is_empty() && !bin.contains(':') => format!("{bin}:{SYSTEM_PATH}"),
        _ => SYSTEM_PATH.into(),
    };
    command
        .args([
            "-p",
            "hi",
            "--model",
            "haiku",
            "--tools",
            "",
            "--strict-mcp-config",
            "--setting-sources",
            "",
            "--no-session-persistence",
        ])
        .current_dir(dir)
        .env("HOME", dir)
        .env("PATH", path)
        .env("CLAUDE_CONFIG_DIR", dir)
        .env("CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR", "3")
        .env("CLAUDE_CODE_MAX_RETRIES", "2")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_AUTOUPDATER", "1");
    if let Some(tmp) = std::env::var_os("TMPDIR") {
        command.env("TMPDIR", tmp);
    }
    unsafe {
        command.pre_exec(move || {
            // dup2 onto itself would keep FD_CLOEXEC, so clear the flag instead.
            let ok = if source == 3 {
                libc::fcntl(3, libc::F_SETFD, 0) != -1
            } else {
                libc::dup2(source, 3) == 3
            };
            if ok {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    // Stdin gets no input and closes at once, which `claude -p` reads as an empty addition.
    let result = crate::platform::pipe_status_of(command, &[], TIMEOUT, None);
    drop(read);
    match result.map_err(|_| Failure::Failed("无法启动 Claude CLI 或读取它的结果"))? {
        (Some(0), _) => Ok(()),
        (None, _) => Err(Failure::Failed("Claude CLI 超时未完成")),
        (Some(_), output) => Err(failure(&String::from_utf8_lossy(&output))),
    }
}

/// The CLI prints its API error on stdout; only a fixed summary leaves this function. Its
/// "Failed to authenticate" prefix covers both 401 and 403, and its server-side rate-limit text
/// says "(not your usage limit)", so only specific phrases count.
fn failure(output: &str) -> Failure {
    let has = |phrases: &[&str]| phrases.iter().any(|p| output.contains(p));
    if has(&[
        "ECONNREFUSED",
        "ENOTFOUND",
        "ETIMEDOUT",
        "Unable to connect to API",
        "Connection error",
    ]) {
        Failure::NotSent("网络未连通，请求没有到达 Claude")
    } else if has(&["API Error: 401"]) {
        Failure::Failed("Claude 未接受这个账号的登录（401）")
    } else if has(&["API Error: 403"]) {
        Failure::Failed("Claude 拒绝了请求（403），多见于没有走代理")
    } else if has(&[
        "usage limit reached",
        "usage limit is reached",
        "reached your weekly usage limit",
    ]) {
        Failure::Failed("这个账号的额度已用完")
    } else if has(&[
        "temporarily limiting requests",
        "API Error: 429",
        "API Error: 529",
    ]) {
        Failure::Failed("Claude 服务暂时限流")
    } else {
        Failure::Failed("Claude CLI 未能完成预热请求")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn local(time: u64) -> (i32, i32, i32) {
        let t = time as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&t, &mut tm) };
        (tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
    #[test]
    fn slot_is_the_latest_local_hour_mark_and_due_once_within_catch_up() {
        let time = 1_791_875_000;
        for hour in 0..24 {
            let start = slot(hour, time).unwrap();
            assert!(start <= time && time - start < 25 * 3600);
            assert_eq!(local(start), (i32::from(hour), 0, 0));
        }
        let hour = local(time).0 as u8;
        let start = slot(hour, time).unwrap();
        let mut plan = Plan {
            hour,
            slot: None,
            last: None,
        };
        assert_eq!(plan.due(time), Some(start));
        assert_eq!(plan.due(start + CATCH_UP), None);
        plan.slot = Some(start);
        assert_eq!(plan.due(time), None);
        // Tomorrow's mark is a new slot.
        assert!(plan.due(start + 86400 + 60).is_some_and(|s| s > start));
    }
    #[test]
    fn failures_retry_after_a_pause_and_give_up_on_the_slot() {
        let mut runtime = Runtime {
            launch: Some(|_, _, _| Err(Failure::Failed("synthetic failure"))),
            ..Default::default()
        };
        let wait = |r: &mut Runtime, t| loop {
            if let Some(done) = r.poll(t) {
                break done;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let ok = || Ok((PathBuf::from("claude"), "SYNTHETIC".to_string()));
        for attempt in 1..=ATTEMPTS {
            let time = 1000 + u64::from(attempt) * 2 * RETRY;
            if attempt == 2 {
                // Something that never reached Claude is retried without using an attempt.
                runtime.launch = Some(|_, _, _| Err(Failure::NotSent("synthetic offline")));
                runtime.start("a", 7, ok(), PathBuf::new());
                let (_, settled, record) = wait(&mut runtime, time - RETRY);
                assert_eq!(settled, None);
                assert!(record.message.contains("每 5 分钟重试"));
                runtime.launch = Some(|_, _, _| Err(Failure::Failed("synthetic failure")));
            }
            assert!(runtime.ready("a", 7, time));
            runtime.start("a", 7, ok(), PathBuf::new());
            assert!(runtime.running("a") && !runtime.ready("b", 7, time));
            let (id, settled, record) = wait(&mut runtime, time);
            assert_eq!(id, "a");
            assert!(!record.ok);
            if attempt < ATTEMPTS {
                assert_eq!(settled, None);
                assert!(!runtime.ready("a", 7, time + RETRY - 1));
                assert!(runtime.ready("a", 8, time));
            } else {
                assert_eq!(settled, Some(7));
                assert!(record.message.contains("等下次预热"));
            }
        }
        runtime.launch = Some(|_, _, token| {
            assert_eq!(token, "SYNTHETIC");
            Ok(())
        });
        runtime.start("a", 8, ok(), PathBuf::new());
        let (_, settled, record) = wait(&mut runtime, 5000);
        assert_eq!(settled, Some(8));
        assert!(record.ok);
        // Preparation errors reach the same retry path without launching anything.
        runtime.launch = Some(|_, _, _| panic!("must not launch"));
        runtime.start("a", 9, Err("Access 已到期"), PathBuf::new());
        let (_, settled, record) = wait(&mut runtime, 6000);
        assert_eq!(settled, None);
        assert!(record.message.starts_with("Access 已到期"));
    }
    #[test]
    fn cli_gets_the_token_on_fd_3_with_a_cleared_isolated_environment() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "authmanager-warmup-{}",
            crate::platform::random_id().unwrap()
        ));
        crate::platform::private_dir(&root).unwrap();
        let binary = root.join("claude");
        // An explicit proxy of this process has to survive the cleared environment.
        let proxy = std::env::var("https_proxy").map_or(String::new(), |p| {
            format!("[ \"$https_proxy\" = \"{p}\" ] || exit 7\n")
        });
        let script = r#"#!/bin/sh
token=$(cat <&3)
case "$token" in SYNTHETIC_*) ;; *) echo "bad token"; exit 3;; esac
[ "$HOME" = "$CLAUDE_CONFIG_DIR" ] && [ "$(pwd -P)" = "$(cd "$HOME" && pwd -P)" ] || exit 4
[ -z "$USER" ] && [ -z "$CLAUDE_CODE_OAUTH_TOKEN" ] && [ "$CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR" = 3 ] || exit 5
[ "$*" = "-p hi --model haiku --tools  --strict-mcp-config --setting-sources  --no-session-persistence" ] || exit 6
PROXY_CHECK
case "$token" in *401*) echo "Failed to authenticate. API Error: 401"; exit 1;; esac
echo ok
"#;
        std::fs::write(&binary, script.replace("PROXY_CHECK\n", &proxy)).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let dir = root.join("claude-warmup");
        assert_eq!(run(&binary, &dir, "SYNTHETIC_FD_TOKEN"), Ok(()));
        assert_eq!(
            run(&binary, &dir, "SYNTHETIC_401"),
            Err(Failure::Failed("Claude 未接受这个账号的登录（401）"))
        );
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn an_npm_installed_script_cli_finds_the_node_installed_beside_it() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "authmanager-warmup-npm-{}",
            crate::platform::random_id().unwrap()
        ));
        let bin = root.join("prefix/bin");
        let package = root.join("prefix/lib/node_modules/@anthropic-ai/claude-code");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&package).unwrap();
        // Stands in for node: runs the CLI script with sh.
        let node = bin.join("node");
        std::fs::write(&node, "#!/bin/sh\nexec /bin/sh \"$@\"\n").unwrap();
        let cli = package.join("cli.js");
        std::fs::write(
            &cli,
            "#!/usr/bin/env node\n[ \"$(cat <&3)\" = SYNTHETIC_NPM ] && [ \"$1\" = -p ] || exit 3\n",
        )
        .unwrap();
        for file in [&node, &cli] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::os::unix::fs::symlink(
            "../lib/node_modules/@anthropic-ai/claude-code/cli.js",
            bin.join("claude"),
        )
        .unwrap();
        let result = run(
            &bin.join("claude"),
            &root.join("claude-warmup"),
            "SYNTHETIC_NPM",
        );
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(result, Ok(()));
    }
    #[test]
    #[ignore = "starts the installed Claude CLI and reaches the real API with a synthetic token"]
    fn live_cli_rejects_a_synthetic_token_through_the_isolated_launch() {
        let root = std::env::temp_dir().join(format!(
            "authmanager-warmup-live-{}",
            crate::platform::random_id().unwrap()
        ));
        let binary = std::env::var_os("AUTHMANAGER_LIVE_CLAUDE_BIN")
            .map_or_else(|| PathBuf::from("/opt/homebrew/bin/claude"), PathBuf::from);
        let result = run(&binary, &root, "sk-ant-oat01-SYNTHETIC-INVALID");
        let leaked = std::process::Command::new("/usr/bin/grep")
            .args(["-rl", "SYNTHETIC-INVALID"])
            .arg(&root)
            .output()
            .unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            result,
            Err(Failure::Failed("Claude 未接受这个账号的登录（401）"))
        );
        assert!(leaked.stdout.is_empty());
    }
    #[test]
    fn plans_are_looked_at_every_half_minute_or_right_after_a_change() {
        let mut runtime = Runtime::default();
        assert!(runtime.check_due(100));
        assert!(!runtime.check_due(100 + CHECK - 1));
        assert!(runtime.check_due(100 + CHECK));
        runtime.recheck();
        assert!(runtime.check_due(100 + CHECK + 1));
    }
    #[test]
    fn cli_errors_reduce_to_fixed_summaries() {
        use Failure::*;
        for (output, expected) in [
            (
                "Failed to authenticate. API Error: 401 x",
                Failed("Claude 未接受这个账号的登录（401）"),
            ),
            (
                "Failed to authenticate. API Error: 403 Request not allowed",
                Failed("Claude 拒绝了请求（403），多见于没有走代理"),
            ),
            (
                "API Error: Connection refused — a firewall or proxy may be blocking it (ECONNREFUSED)",
                NotSent("网络未连通，请求没有到达 Claude"),
            ),
            (
                "API Error: Server is temporarily limiting requests (not your usage limit) · x",
                Failed("Claude 服务暂时限流"),
            ),
            (
                "Claude usage limit reached. Resets 3pm",
                Failed("这个账号的额度已用完"),
            ),
            (
                "organization policy limit",
                Failed("Claude CLI 未能完成预热请求"),
            ),
        ] {
            assert_eq!(failure(output), expected, "{output}");
        }
    }
}
