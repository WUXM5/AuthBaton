use crate::credentials::{Bundle, claims};
use crate::platform::{Result, pipe_command};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub port: u16,
    pub selected: Option<String>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 9522,
            selected: None,
        }
    }
}
#[derive(Clone)]
pub struct Client {
    pub helper: PathBuf,
    pub port: u16,
    pub target: Option<String>,
}
impl Client {
    pub fn call(&self, operation: &str, params: Value) -> Result<Value> {
        let value = self.raw(operation, params)?;
        if value["ok"] == true {
            return Ok(value);
        }
        Err(error_message(value["code"].as_str()))
    }
    // The bridge reply as is, so a refusal can carry details such as which threads are busy.
    pub fn raw(&self, operation: &str, params: Value) -> Result<Value> {
        let input =
            json!({"operation":operation,"params":params,"port":self.port,"target":self.target});
        let bytes = pipe_command(
            &self.helper,
            &[],
            &serde_json::to_vec(&input).unwrap(),
            Duration::from_secs(
                if matches!(operation, "apply" | "auto_prepare" | "auto_verify") {
                    45
                } else {
                    25
                },
            ),
        )?;
        serde_json::from_slice(&bytes).map_err(|_| "桌面桥返回格式无效")
    }
}
pub fn error_message(code: Option<&str>) -> &'static str {
    match code {
        Some("debug_port_unavailable") => "调试端口未监听；请使用调试模式启动 Codex 桌面",
        Some("port_owner_mismatch") => "调试端口归属无法确认；请检查端口是否由官方 Codex 桌面独占",
        Some("target_unavailable") => "请打开并聚焦 Codex 主窗口，再重新连接",
        Some("bridge_install_failed") => {
            "已找到 Codex 主窗口，但桌面桥初始化失败；请更新 AuthManager 适配"
        }
        Some("rpc_timeout") => "调试端口已连接，但 Codex 账号服务响应超时；请稍后重试",
        Some("rpc_failed") => "调试端口已连接，但 Codex 账号服务拒绝了请求",
        Some("unsupported_auth") => {
            "桌面尚未提供可用的 ChatGPT 订阅登录；请在 Codex 中确认登录状态"
        }
        Some("task_probe_failed") => "无法确认桌面任务状态；切换尚未开始，请稍后重试",
        Some("ui_adapter_unavailable") => "桌面页面保留接口结构不兼容，需要更新热切换适配",
        Some("ui_adapter_busy") => "页面仍在切换或恢复，请稍后重试",
        Some("ui_scope_unsupported") => "当前云端或远程页面尚不支持保留上下文；请先打开本地任务",
        Some("ui_services_unavailable") => "账号服务尚未重新连接",
        Some("tasks_active") => "Codex 仍有运行或等待中的任务，请结束后切换",
        Some("tasks_waiting") => "有任务正在等待审批或输入；自动切换已延后",
        Some("tasks_changed") => "任务已由其他操作改变；已停止本次自动切换，请核对会话",
        Some("pause_unconfirmed") => "任务暂停尚未确认；没有继续切换，请核对会话",
        Some("resume_unconfirmed") => "任务接续结果尚未确认；不会重复发送继续指令",
        Some("batch_unavailable" | "batch_busy" | "invalid_batch") => {
            "桌面任务批次已失效；请核对原会话后结束本次流程"
        }
        Some("identity_changed" | "identity_mismatch") => "桌面账号已由其他操作改变，请重新核对",
        Some("refresh_pending") => "桌面正在续期，请稍后切换",
        Some("stale_refresh" | "refresh_unconfirmed") => "桌面续期回送尚未确认，请重新核对连接",
        _ => "桌面连接不可用；请确认调试端口、桌面接口与主窗口",
    }
}
// A refused preflight, naming up to three busy threads when the bridge listed them.
pub fn refusal(value: &Value) -> String {
    let base = error_message(value["code"].as_str());
    let tasks: Vec<String> = value["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .take(3)
        .map(|task| {
            let name: String = task["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or("未命名任务")
                .chars()
                .take(24)
                .collect();
            let flags: Vec<&str> = task["flags"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let state = if flags.contains(&"waitingOnApproval") {
                "等待审批"
            } else if flags.contains(&"waitingOnUserInput") {
                "等待输入"
            } else {
                "运行中"
            };
            format!("「{name}」{state}")
        })
        .collect();
    if tasks.is_empty() {
        base.into()
    } else {
        format!("{base}：{}", tasks.join("、"))
    }
}
// The bridge answered and declined because of task state; the connection itself is fine.
pub fn declined_for_tasks(value: &Value) -> bool {
    matches!(
        value["code"].as_str(),
        Some("tasks_active" | "tasks_waiting")
    )
}
pub enum Completion {
    Observe(Result<Value>),
    Preflight {
        id: String,
        recover: bool,
        result: Result<Value>,
    },
    Reconcile(Result<Value>),
    Activate {
        id: String,
        result: Result<Value>,
    },
    Reply(Result<Value>),
    Update(Result<Value>),
    Relaunch {
        id: String,
        result: Result<()>,
    },
    Restart(Result<()>),
}
#[derive(Clone)]
pub struct Refresh {
    pub id: Value,
    pub epoch: Value,
    pub instance: String,
    pub account_id: String,
    pub deadline: u64,
    pub hash: String,
    pub attempted: bool,
}
pub struct Runtime {
    pub action: Option<Value>,
    pub bound: bool,
    pub other_clients: fn() -> bool,
    pub relaunch: fn(u16) -> Result<()>,
    pub turns_running: fn(&Path) -> Result<bool>,
    // Account to switch to once a desktop restarted for that switch connects, with a deadline.
    pub pending_switch: Option<(String, u64)>,
    pub undebugged: fn(u16) -> Result<Undebugged>,
    pub startup_finished: fn(&Path, u32, u64) -> Option<u64>,
    // The last such desktop already handled: restarted, or left running.
    pub settled_pid: Option<u32>,
    pub last_watch: u64,
    pub restart_failed: bool,
    pub skip_quit_confirmation: fn(bool) -> Result<()>,
    // What the login session was last told; set again after every reset of this runtime.
    pub quit_confirmation_skipped: Option<bool>,
    #[cfg(test)]
    pub helper: Option<PathBuf>,
    pub target: Option<String>,
    pub instance: Option<String>,
    pub owner: Option<String>,
    pub mode: Option<String>,
    pub hash: Option<String>,
    pub message: String,
    pub connected: bool,
    pub observed_at: Option<u64>,
    pub last_poll: u64,
    pub job: Option<JoinHandle<Completion>>,
    pub refresh: Option<Refresh>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            action: None,
            bound: false,
            other_clients: other_codex_clients,
            relaunch,
            turns_running,
            pending_switch: None,
            #[cfg(not(test))]
            undebugged: undebugged_desktop,
            #[cfg(test)]
            undebugged: |_| Ok(None),
            #[cfg(not(test))]
            startup_finished,
            // Tests treat every desktop as having finished starting when it launched.
            #[cfg(test)]
            startup_finished: |_, _, age| Some(crate::status::now().saturating_sub(age)),
            settled_pid: None,
            last_watch: 0,
            restart_failed: false,
            #[cfg(not(test))]
            skip_quit_confirmation,
            #[cfg(test)]
            skip_quit_confirmation: |_| Ok(()),
            quit_confirmation_skipped: None,
            #[cfg(test)]
            helper: None,
            target: None,
            instance: None,
            owner: None,
            mode: None,
            hash: None,
            message: "未启用桌面实验连接".into(),
            connected: false,
            observed_at: None,
            last_poll: 0,
            job: None,
            refresh: None,
        }
    }
}
impl Runtime {
    pub fn client(&self, config: &Config) -> Client {
        let helper = std::env::current_exe()
            .unwrap_or_default()
            .with_file_name("desktop-bridge");
        #[cfg(test)]
        let helper = self.helper.clone().unwrap_or(helper);
        Client {
            helper,
            port: config.port,
            target: self.target.clone(),
        }
    }
    pub fn observed(&mut self, value: &Value) -> Result<()> {
        let field = |name| {
            value[name]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from)
                .ok_or("桌面未返回可确认的身份")
        };
        self.target = Some(field("target")?);
        self.instance = Some(field("instance")?);
        self.owner = Some(field("owner")?);
        self.mode = Some(field("mode")?);
        self.hash = Some(field("hash")?);
        self.bound = value["boundOwner"].as_str() == self.owner.as_deref();
        self.connected = true;
        self.observed_at = Some(crate::status::now());
        Ok(())
    }
    pub fn snapshot(&self, config: &Config) -> Value {
        json!({"enabled":config.enabled,"port":config.port,"selected":config.selected,"connected":self.connected,"busy":self.job.is_some(),"message":self.message,"action":self.action})
    }
    pub fn action_feedback(&mut self, id: &str, state: &str) {
        // The time lets the UI present a finished attempt as history rather than current state.
        self.action =
            Some(json!({"id":id,"state":state,"message":self.message,"at":crate::status::now()}));
    }
}
pub fn external_auth(bundle: &Bundle) -> Result<Value> {
    let metadata = bundle.metadata()?;
    if bundle.provider != "codex" {
        return Err("桌面实验连接仅支持 Codex");
    }
    let access = bundle.credential["tokens"]["access_token"]
        .as_str()
        .ok_or("缺少 Access token")?;
    let data = claims(access)?;
    let auth = &data["https://api.openai.com/auth"];
    let user = auth["chatgpt_user_id"]
        .as_str()
        .or(auth["user_id"].as_str())
        .ok_or("Access token 缺少用户身份")?;
    let workspace = bundle.credential["tokens"]["account_id"]
        .as_str()
        .ok_or("缺少工作区身份")?;
    if auth["chatgpt_account_id"] != workspace
        || metadata.owner != format!("codex:{user}:{workspace}")
    {
        return Err("Access token 与保存账号不一致");
    }
    Ok(json!({"accessToken":access,"chatgptAccountId":workspace,"chatgptPlanType":metadata.plan}))
}

pub const DESKTOP_APPS: [&str; 2] = ["/Applications/ChatGPT.app", "/Applications/Codex.app"];
// The switch and its page reconnect poll from inside the renderer, which Chromium throttles to
// about one timer a minute once the window is hidden; these switches keep it running.
pub fn launch_args(port: u16) -> Vec<String> {
    vec![
        "--remote-debugging-address=127.0.0.1".into(),
        format!("--remote-debugging-port={port}"),
        "--disable-background-timer-throttling".into(),
        "--disable-backgrounding-occluded-windows".into(),
        "--disable-renderer-backgrounding".into(),
    ]
}
// Codex desktop reads this once at launch; "1" quits without its confirmation, which otherwise
// waits for a click whenever scheduled tasks are on.
const QUIT_CONFIRMATION_SWITCH: &str = "CODEX_ELECTRON_DISABLE_QUIT_CONFIRMATION";
// While hot switch is on, a desktop opened from the Dock or restarted by its updater gets the
// switch from the login session, so its restart in debug mode never waits for that click.
#[cfg_attr(test, allow(dead_code))]
pub fn skip_quit_confirmation(skip: bool) -> Result<()> {
    let mut launchctl = std::process::Command::new("/bin/launchctl");
    if skip {
        launchctl.args(["setenv", QUIT_CONFIRMATION_SWITCH, "1"]);
    } else {
        launchctl.args(["unsetenv", QUIT_CONFIRMATION_SWITCH]);
    }
    match launchctl.status() {
        Ok(status) if status.success() => Ok(()),
        _ => Err("无法设置 Codex 桌面的退出确认"),
    }
}
// Starts the desktop with the debug port. The desktop AuthManager starts keeps its quit
// confirmation: the session switch above is only for desktops about to be restarted.
pub fn open_desktop(app: &str, port: u16) -> std::process::Command {
    let mut open = std::process::Command::new("/usr/bin/open");
    open.args(["-a", app, "--env"])
        .arg(format!("{QUIT_CONFIRMATION_SWITCH}=0"))
        .arg("--args")
        .args(launch_args(port));
    open
}
pub fn desktop_running() -> Result<bool> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "comm="])
        .output()
        .map_err(|_| "无法检查桌面进程")?;
    Ok(String::from_utf8_lossy(&output.stdout).lines().any(|s| {
        s.trim() == "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"
            || s.trim() == "/Applications/Codex.app/Contents/MacOS/Codex"
    }))
}
// A desktop started without the debug port: its pid and seconds since it started.
pub type Undebugged = Option<(u32, u64)>;
const DESKTOP_EXECUTABLES: [&str; 2] = [
    "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT",
    "/Applications/Codex.app/Contents/MacOS/Codex",
];
// Seconds after its startup finished that a desktop counts as just started; one running longer
// may be in use.
pub const FRESH_DESKTOP_SECS: u64 = 60;
// How long to wait for a desktop to finish starting, such as after an update that rebuilds its
// history; one still starting after this is left alone.
pub const STARTUP_WAIT_SECS: u64 = 600;
// Codex desktop logs this once its startup has finished. Quitting it earlier makes it show
// "failed to start" and stay open, so a restart waits for the line.
const STARTUP_FINISHED: &str = "Host startup critical-path phases completed";
// When the desktop with this pid finished starting, from its own main log; None while it is
// still starting. `age` is how long the process has run.
pub fn startup_finished(home: &Path, pid: u32, age: u64) -> Option<u64> {
    let now = crate::status::now();
    let launched = now.saturating_sub(age + 1);
    let logs = home.join("Library/Logs/com.openai.codex");
    let name = format!("-{pid}-t0-");
    // Each process's main log is under the UTC date it started on.
    let mut days = vec![utc_day(launched)?, utc_day(now)?];
    days.dedup();
    for day in days {
        let Ok(entries) = std::fs::read_dir(logs.join(day)) else {
            continue;
        };
        for entry in entries.flatten() {
            let file = entry.file_name();
            let file = file.to_string_lossy();
            // A log last written before this process started belongs to an earlier one.
            let current = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .is_some_and(|t| t.as_secs() >= launched);
            if !file.starts_with("codex-desktop-") || !file.contains(&name) || !current {
                continue;
            }
            let text = std::fs::read(entry.path()).ok()?;
            return String::from_utf8_lossy(&text)
                .lines()
                .find(|line| line.contains(STARTUP_FINISHED))
                .and_then(|line| crate::quota::iso_time(line.split(' ').next()?));
        }
    }
    None
}
fn utc_day(secs: u64) -> Option<String> {
    let time = libc::time_t::try_from(secs).ok()?;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::gmtime_r(&time, &mut tm) }.is_null() {
        return None;
    }
    Some(format!(
        "{}/{:02}/{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday
    ))
}
// Tests use a synthetic process list instead, so they never restart the person's desktop.
#[cfg_attr(test, allow(dead_code))]
pub fn undebugged_desktop(port: u16) -> Result<Undebugged> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,etime=,args="])
        .output()
        .map_err(|_| "无法检查桌面进程")?;
    Ok(undebugged_in(
        &String::from_utf8_lossy(&output.stdout),
        port,
    ))
}
fn undebugged_in(listing: &str, port: u16) -> Undebugged {
    let flag = format!("--remote-debugging-port={port}");
    listing.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let pid = fields.next()?.parse().ok()?;
        let age = elapsed(fields.next()?)?;
        let executable = fields.next()?;
        (DESKTOP_EXECUTABLES.contains(&executable) && !fields.any(|arg| arg == flag))
            .then_some((pid, age))
    })
}
// `ps` elapsed time: [[dd-]hh:]mm:ss.
fn elapsed(text: &str) -> Option<u64> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, text),
    };
    let parts: Vec<u64> = clock
        .split(':')
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    let (hours, minutes, seconds) = match parts[..] {
        [minutes, seconds] => (0, minutes, seconds),
        [hours, minutes, seconds] => (hours, minutes, seconds),
        _ => return None,
    };
    Some(((days * 24 + hours) * 60 + minutes) * 60 + seconds)
}
// Quits a running desktop the normal way (never forcibly), then starts it with the debug port.
pub fn relaunch(port: u16) -> Result<()> {
    let app = DESKTOP_APPS
        .into_iter()
        .find(|p| Path::new(p).exists())
        .ok_or("未找到官方桌面应用")?;
    if desktop_running()? {
        let mut quit = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", &format!("tell application \"{app}\" to quit")])
            .spawn()
            .map_err(|_| "无法请求 Codex 桌面退出")?;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while desktop_running()? {
            if std::time::Instant::now() > deadline {
                let _ = quit.kill();
                return Err("Codex 桌面没有退出，可能在等待确认；请手动退出后再切换");
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let _ = quit.kill();
        let _ = quit.wait();
    }
    let status = open_desktop(app, port)
        .status()
        .map_err(|_| "无法启动官方桌面")?;
    if !status.success() {
        return Err("无法启动官方桌面");
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        if std::time::Instant::now() > deadline {
            return Err("桌面已重新启动，但调试端口没有监听");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}
// A Codex turn is running when the latest one in a recently written rollout started and has
// not finished. Codex writes each event as it happens and creates a thread's file with its
// first message, so a running turn is already on disk.
pub fn turns_running(codex_home: &Path) -> Result<bool> {
    let cutoff = std::time::SystemTime::now() - Duration::from_secs(86400);
    Ok(!open_turns(codex_home, cutoff)?.is_empty())
}
// A running turn that started at or after `since`; one without a readable start time counts.
pub fn turns_started_since(codex_home: &Path, since: u64) -> Result<bool> {
    let cutoff = std::time::UNIX_EPOCH + Duration::from_secs(since);
    Ok(open_turns(codex_home, cutoff)?
        .into_iter()
        .any(|started| started.is_none_or(|t| t >= since)))
}
// Start times of the open turns in rollouts written since `cutoff`.
fn open_turns(codex_home: &Path, cutoff: std::time::SystemTime) -> Result<Vec<Option<u64>>> {
    let mut starts = Vec::new();
    let mut dirs = vec![codex_home.join("sessions")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let metadata = entry.metadata().map_err(|_| "无法读取 Codex 会话记录")?;
            if metadata.is_dir() {
                dirs.push(path);
                continue;
            }
            let rollout = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"));
            if !rollout || metadata.modified().map_or(true, |t| t < cutoff) {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|_| "无法读取 Codex 会话记录")?;
            let mut open = None;
            for line in text.lines().filter(|l| l.contains("\"event_msg\"")) {
                let Ok(value) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                match value["payload"]["type"].as_str() {
                    Some("task_started") => {
                        open = Some(value["timestamp"].as_str().and_then(crate::quota::iso_time))
                    }
                    Some("task_complete" | "turn_aborted") => open = None,
                    _ => {}
                }
            }
            starts.extend(open);
        }
    }
    Ok(starts)
}

// Only the inspected desktop's descendants may share the managed native credential file.
// This process snapshot is deliberately not described as an OS-wide refresh lock.
pub fn other_codex_clients() -> bool {
    let Ok(output) = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,comm="])
        .output()
    else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let rows: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim().splitn(2, char::is_whitespace);
            let pid = parts.next()?.parse::<u32>().ok()?;
            let mut rest = parts.next()?.trim().splitn(2, char::is_whitespace);
            Some((pid, rest.next()?.parse::<u32>().ok()?, rest.next()?.trim()))
        })
        .collect();
    let desktops: Vec<_> = rows
        .iter()
        .filter(|(_, _, name)| {
            matches!(
                *name,
                "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"
                    | "/Applications/Codex.app/Contents/MacOS/Codex"
            )
        })
        .map(|r| r.0)
        .collect();
    rows.iter().any(|(pid, _, name)| {
        let base = std::path::Path::new(name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if !matches!(base, "codex" | "codex-bin" | "codex.exe") {
            return false;
        }
        let mut p = *pid;
        for _ in 0..64 {
            if desktops.contains(&p) {
                return false;
            }
            let Some(row) = rows.iter().find(|r| r.0 == p) else {
                break;
            };
            if row.1 == p || row.1 <= 1 {
                break;
            }
            p = row.1;
        }
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_a_main_desktop_process_without_this_debug_port_is_reported() {
        let listing = "  101 02-01:16:00 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT
  102 00:07 /Applications/ChatGPT.app/Contents/Frameworks/ChatGPT Helper.app/Contents/MacOS/ChatGPT Helper --type=renderer
";
        assert_eq!(
            undebugged_in(listing, 9522),
            Some((101, 2 * 86400 + 3600 + 960))
        );
        let debug = "  103 00:07 /Applications/Codex.app/Contents/MacOS/Codex --remote-debugging-address=127.0.0.1 --remote-debugging-port=9522\n";
        assert_eq!(undebugged_in(debug, 9522), None);
        assert_eq!(undebugged_in(debug, 9600), Some((103, 7)));
        assert_eq!(undebugged_in("  104 00:07 /tmp/ChatGPT\n", 9522), None);
        assert_eq!(elapsed("1:02:03"), Some(3723));
        for bad in ["", "7", "a:01", "1-2", "1:2:3:4"] {
            assert_eq!(elapsed(bad), None);
        }
    }
    #[test]
    fn startup_finished_reads_the_line_from_this_processs_own_log() {
        let home = std::env::temp_dir().join(format!("authmanager-startup-{}", std::process::id()));
        let day = home
            .join("Library/Logs/com.openai.codex")
            .join(utc_day(crate::status::now()).unwrap());
        std::fs::create_dir_all(&day).unwrap();
        let log =
            day.join("codex-desktop-00000000-0000-0000-0000-000000000000-4242-t0-i1-043653-0.log");
        let starting = "2026-10-14T04:36:53.942Z info Launching app buildFlavor=prod\n";
        std::fs::write(&log, starting).unwrap();
        assert_eq!(startup_finished(&home, 4242, 3), None);
        std::fs::write(&log, format!("{starting}2026-10-14T04:36:55.000Z info [startup] [1.040] Host startup critical-path phases completed startupElapsedMs=1175\n")).unwrap();
        assert_eq!(
            startup_finished(&home, 4242, 3),
            crate::quota::iso_time("2026-10-14T04:36:55.000Z")
        );
        // Another process's log, or one last written before this process started, does not count.
        assert_eq!(startup_finished(&home, 424, 3), None);
        let earlier = std::time::SystemTime::now() - Duration::from_secs(120);
        std::fs::File::options()
            .write(true)
            .open(&log)
            .unwrap()
            .set_modified(earlier)
            .unwrap();
        assert_eq!(startup_finished(&home, 4242, 3), None);
        std::fs::remove_dir_all(&home).unwrap();
    }
    #[test]
    fn a_desktop_started_here_keeps_its_quit_confirmation() {
        let open = open_desktop("/Applications/ChatGPT.app", 9522);
        let args: Vec<_> = open.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(open.get_program(), "/usr/bin/open");
        assert_eq!(
            args[..5],
            [
                "-a",
                "/Applications/ChatGPT.app",
                "--env",
                "CODEX_ELECTRON_DISABLE_QUIT_CONFIRMATION=0",
                "--args"
            ]
        );
        assert_eq!(args[5..], launch_args(9522));
    }
}
