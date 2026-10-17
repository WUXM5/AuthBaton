//! Claude five-hour window warm-up. The window starts at an account's first request (observed:
//! counted from that time rounded down to 10 minutes, not to the hour), so one tiny official CLI
//! turn starts it early. From the daily start hour, each time a window ends another turn starts
//! the next one, while it can still end before the next day's start: four windows a day.
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

/// Warm-ups are sent within this long after the day's hour. A window counts from its request
/// rounded down to 10 minutes, so the last one starts by 19 hours and has ended by the next
/// day's start.
pub const SPAN: u64 = 19 * 3600 + 600;
/// A window this plan started runs at least this long after its attempt ended: the window counts
/// from the request rounded down to 10 minutes, and the attempt ends at most `TIMEOUT` after it.
const SHORTEST: u64 = 5 * 3600 - 600 - TIMEOUT.as_secs();
/// Only a reading this recent decides.
const FRESH: u64 = 300;
/// A window's end is confirmed by a reading taken this long after its reset time.
const SETTLE: u64 = 60;
pub const RETRY: u64 = 300;
pub const ATTEMPTS: u8 = 3;
/// After `ATTEMPTS` failures the empty window is left alone for one window's length.
const SKIP: u64 = 5 * 3600;
const CHECK: u64 = 30;
const TIMEOUT: Duration = Duration::from_secs(120);
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

#[derive(Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Local hour, 0–23, at which each day's warm-ups start.
    pub hour: u8,
    /// Start of the last day an automatic warm-up was sent in. Builds before 0.20 send once a
    /// day and skip a day recorded here.
    #[serde(default)]
    pub slot: Option<u64>,
    /// When the last automatic warm-up ended; it started a window.
    #[serde(default)]
    pub sent: Option<u64>,
    #[serde(default)]
    pub last: Option<Record>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub at: u64,
    pub ok: bool,
    pub message: String,
}
/// What the last successful usage reading says.
pub struct Reading {
    pub checked_at: u64,
    /// The five-hour window's reset time; Claude gives none while the window is idle.
    pub resets_at: Option<u64>,
    /// Remaining percent of the seven-day window.
    pub weekly: Option<f64>,
}
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Outside the day's span, or a window is running; the next empty window starts afresh.
    Rest,
    /// No reading recent enough, or none taken since the last attempt.
    Wait,
    /// The running window has reset; a reading taken after this time decides.
    Query(u64),
    /// No window is running, but the seven-day quota is at or below the account's line.
    Hold,
    /// No window is running: send, for the day that starts at this time.
    Send(u64),
}
impl Plan {
    /// Start of the day whose span `time` is in, unless a window this plan started is running.
    pub fn day(&self, time: u64) -> Option<u64> {
        let day = slot(self.hour, time).filter(|d| time < d + SPAN)?;
        self.sent
            .is_none_or(|sent| time >= sent + SHORTEST)
            .then_some(day)
    }
    /// `tried`: when the account's last attempt (automatic or by hand) ended; only a reading
    /// taken after it shows the window that attempt may have started.
    pub fn step(
        &self,
        time: u64,
        tried: Option<u64>,
        reading: Option<&Reading>,
        line: f64,
    ) -> Step {
        let Some(day) = self.day(time) else {
            return Step::Rest;
        };
        let Some(reading) = reading.filter(|r| {
            r.checked_at <= time
                && time - r.checked_at <= FRESH
                && tried.is_none_or(|t| r.checked_at >= t)
        }) else {
            return Step::Wait;
        };
        match reading.resets_at {
            // A reset time already past when read means the window had ended.
            Some(end) if end > reading.checked_at => {
                if time >= end + SETTLE {
                    Step::Query(end + SETTLE)
                } else {
                    Step::Rest
                }
            }
            _ if reading.weekly.is_some_and(|w| w <= line) => Step::Hold,
            _ => Step::Send(day),
        }
    }
    /// When the next warm-up is expected: as the running window resets if a new one may start
    /// then, otherwise at the next day's start. None while one is due now.
    pub fn next(&self, time: u64, resets_at: Option<u64>) -> Option<u64> {
        let after = resets_at.filter(|end| *end > time).unwrap_or(time);
        let day = slot(self.hour, after)?;
        if after < day + SPAN {
            (after > time).then_some(after)
        } else {
            slot(self.hour, after + 86400)
        }
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
/// others are retried every few minutes while the window stays empty.
#[derive(Debug, PartialEq)]
pub enum Failure {
    NotSent(&'static str),
    Failed(&'static str),
}
pub type Sent = std::result::Result<(), Failure>;
pub type Launch = fn(&Path, &Path, &str) -> Sent;
#[derive(Default)]
pub struct Runtime {
    /// The attempt running: its account, the day an automatic one belongs to (None by hand).
    job: Option<(String, Option<u64>, JoinHandle<Sent>)>,
    /// Accounts asked to warm up by hand, in order; memory only.
    manual: Vec<String>,
    /// An account's failed automatic attempts on the current empty window and the earliest
    /// next try; memory only.
    failures: HashMap<String, (u8, u64)>,
    /// When each account's last attempt ended; memory only, like the readings it is compared to.
    tried: HashMap<String, u64>,
    /// Plans are looked at every `CHECK` seconds, not every tick.
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
        self.tried.remove(id);
        self.manual.retain(|m| m != id);
    }
    /// The window failures counted against has ended or started; the next empty one starts afresh.
    pub fn clear_failures(&mut self, id: &str) {
        self.failures.remove(id);
    }
    pub fn running(&self, id: &str) -> bool {
        self.job.as_ref().is_some_and(|job| job.0 == id)
    }
    pub fn queued(&self, id: &str) -> bool {
        self.manual.iter().any(|m| m == id)
    }
    pub fn tried(&self, id: &str) -> Option<u64> {
        self.tried.get(id).copied()
    }
    /// One warm-up by hand, sent on the next tick the service admits it.
    pub fn request(&mut self, id: &str) -> Result<()> {
        if self.running(id) {
            return Err("这个账号正在预热");
        }
        if !self.queued(id) {
            self.manual.push(id.into());
        }
        self.recheck();
        Ok(())
    }
    /// The account whose warm-up by hand goes next, if nothing is running.
    pub fn next_manual(&self) -> Option<&str> {
        self.manual
            .first()
            .filter(|_| self.job.is_none())
            .map(String::as_str)
    }
    /// Blocks until a running attempt ends, so its result can be recorded before the service
    /// exits instead of the window being warmed again after a restart.
    pub fn wait(&self) {
        while self.job.as_ref().is_some_and(|job| !job.2.is_finished()) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn ready(&self, id: &str, time: u64) -> bool {
        self.job.is_none() && self.failures.get(id).is_none_or(|f| time >= f.1)
    }
    /// `day`: the day an automatic attempt belongs to; None for one by hand.
    pub fn start(
        &mut self,
        id: &str,
        day: Option<u64>,
        prepared: Result<(PathBuf, String)>,
        dir: PathBuf,
    ) {
        if day.is_none() {
            self.manual.retain(|m| m != id);
        }
        #[cfg(test)]
        let launch = self
            .launch
            .unwrap_or(|_, _, _| Err(Failure::NotSent("test launcher not configured")));
        #[cfg(not(test))]
        let launch: Launch = run;
        self.job = Some((
            id.into(),
            day,
            std::thread::spawn(move || {
                let (binary, token) = prepared.map_err(Failure::NotSent)?;
                launch(&binary, &dir, &token)
            }),
        ));
    }
    /// A finished attempt: the account, the day an automatic one belongs to and the result to
    /// show.
    pub fn poll(&mut self, time: u64) -> Option<(String, Option<u64>, Record)> {
        if !self.job.as_ref().is_some_and(|job| job.2.is_finished()) {
            return None;
        }
        let (id, day, job) = self.job.take()?;
        let result = job.join().unwrap_or(Err(Failure::Failed("预热进程中断")));
        self.tried.insert(id.clone(), time);
        let used = self.failures.get(&id).map_or(0, |f| f.0);
        let (ok, message) = match (day, result) {
            (None, Ok(())) => (true, "手动预热已发送".to_string()),
            (None, Err(Failure::NotSent(error) | Failure::Failed(error))) => {
                (false, format!("手动预热未完成：{error}"))
            }
            (Some(_), Ok(())) => {
                self.failures.remove(&id);
                (true, "已发送，5 小时窗口开始计时".to_string())
            }
            (Some(_), Err(Failure::NotSent(error))) => {
                self.failures.insert(id.clone(), (used, time + RETRY));
                (false, format!("{error}；每 5 分钟重试"))
            }
            (Some(_), Err(Failure::Failed(error))) => {
                let count = used + 1;
                if count >= ATTEMPTS {
                    self.failures.insert(id.clone(), (0, time + SKIP));
                    (
                        false,
                        format!("{error}；已试 {ATTEMPTS} 次，跳过这个 5 小时"),
                    )
                } else {
                    self.failures.insert(id.clone(), (count, time + RETRY));
                    (false, format!("{error}；5 分钟后重试"))
                }
            }
        };
        Some((
            id,
            day,
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
    fn plan(hour: u8) -> Plan {
        Plan {
            hour,
            slot: None,
            sent: None,
            last: None,
        }
    }
    fn reading(checked_at: u64, resets_at: Option<u64>, weekly: f64) -> Reading {
        Reading {
            checked_at,
            resets_at,
            weekly: Some(weekly),
        }
    }
    #[test]
    fn slot_is_the_latest_local_hour_mark() {
        let time = 1_791_875_000;
        for hour in 0..24 {
            let start = slot(hour, time).unwrap();
            assert!(start <= time && time - start < 25 * 3600);
            assert_eq!(local(start), (i32::from(hour), 0, 0));
        }
    }
    #[test]
    fn a_day_chains_four_windows_and_the_next_day_starts_on_time() {
        let day = slot(7, 1_791_875_000).unwrap();
        let mut plan = plan(7);
        // A window starts at its first request rounded down to 10 minutes and lasts 5 hours; the
        // usage reading taken every half minute shows its reset time while it runs.
        let mut end: Option<u64> = None;
        let mut sends = vec![];
        let mut time = day + 15;
        while time < day + 2 * 86400 {
            let resets_at = end.filter(|e| *e > time);
            if let Step::Send(of) =
                plan.step(time, plan.sent, Some(&reading(time, resets_at, 50.0)), 2.0)
            {
                assert_eq!(of, slot(7, time).unwrap());
                sends.push(time);
                plan.sent = Some(time);
                end = Some(time / 600 * 600 + 5 * 3600);
            }
            time += 30;
        }
        let next = slot(7, day + 86400 + 7200).unwrap();
        assert_eq!(sends.len(), 8);
        assert_eq!(sends[4], next + 15);
        assert!(sends[3] / 600 * 600 + 5 * 3600 <= next);
        assert!(sends.windows(2).all(|w| w[1] - w[0] >= 5 * 3600));
        // Within a day each window starts as the one before it ends.
        for i in [1, 2, 3, 5, 6, 7] {
            assert_eq!(sends[i] / 600 * 600, sends[i - 1] / 600 * 600 + 5 * 3600);
        }
    }
    #[test]
    fn only_a_recent_reading_of_an_idle_window_above_the_line_sends() {
        let day = slot(7, 1_791_875_000).unwrap();
        let mut plan = plan(7);
        let time = day + 3600;
        let idle = reading(time - 10, None, 50.0);
        assert_eq!(plan.step(time, None, Some(&idle), 2.0), Step::Send(day));
        // Before the day's start, and after its span, nothing is sent.
        assert_eq!(
            plan.step(day - 60, None, Some(&reading(day - 70, None, 50.0)), 2.0),
            Step::Rest
        );
        assert_eq!(
            plan.step(
                day + SPAN - 1,
                None,
                Some(&reading(day + SPAN - 5, None, 50.0)),
                2.0
            ),
            Step::Send(day)
        );
        assert_eq!(
            plan.step(
                day + SPAN,
                None,
                Some(&reading(day + SPAN - 5, None, 50.0)),
                2.0
            ),
            Step::Rest
        );
        // No reading, an old one, or one from before the last attempt decides nothing.
        assert_eq!(plan.step(time, None, None, 2.0), Step::Wait);
        assert_eq!(
            plan.step(
                time,
                None,
                Some(&reading(time - FRESH - 1, None, 50.0)),
                2.0
            ),
            Step::Wait
        );
        assert_eq!(
            plan.step(time, Some(time - 5), Some(&idle), 2.0),
            Step::Wait
        );
        // A running window, started by the user or anyone, is left alone until a reading taken
        // after its reset time.
        let running = reading(time - 10, Some(time + 1800), 50.0);
        assert_eq!(plan.step(time, None, Some(&running), 2.0), Step::Rest);
        let late = time + 1800 + SETTLE;
        let before = reading(late - 100, Some(time + 1800), 50.0);
        assert_eq!(plan.step(late - 1, None, Some(&before), 2.0), Step::Rest);
        assert_eq!(plan.step(late, None, Some(&before), 2.0), Step::Query(late));
        assert_eq!(
            plan.step(late + 5, None, Some(&reading(late + 2, None, 50.0)), 2.0),
            Step::Send(day)
        );
        // A reset time already past when read is an ended window.
        assert_eq!(
            plan.step(
                late + 5,
                None,
                Some(&reading(late + 2, Some(late), 50.0)),
                2.0
            ),
            Step::Send(day)
        );
        // The seven-day quota at or below the line holds it back; no line still holds at 0.
        assert_eq!(
            plan.step(time, None, Some(&reading(time - 10, None, 2.0)), 2.0),
            Step::Hold
        );
        assert_eq!(
            plan.step(time, None, Some(&reading(time - 10, None, 2.5)), 2.0),
            Step::Send(day)
        );
        assert_eq!(
            plan.step(time, None, Some(&reading(time - 10, None, 0.0)), 0.0),
            Step::Hold
        );
        // A window this plan started is not warmed again, even by a reading that misses it.
        plan.sent = Some(time);
        assert_eq!(
            plan.step(
                time + 600,
                None,
                Some(&reading(time + 590, None, 50.0)),
                2.0
            ),
            Step::Rest
        );
        assert_eq!(
            plan.step(
                time + SHORTEST,
                None,
                Some(&reading(time + SHORTEST - 1, None, 50.0)),
                2.0
            ),
            Step::Send(day)
        );
    }
    #[test]
    fn the_next_warmup_is_the_running_window_end_or_the_next_day_start() {
        let day = slot(7, 1_791_875_000).unwrap();
        let plan = plan(7);
        let tomorrow = slot(7, day + 86400 + 7200).unwrap();
        assert_eq!(
            plan.next(day + 3600, Some(day + 5 * 3600)),
            Some(day + 5 * 3600)
        );
        // Due now, or blocked: nothing to promise.
        assert_eq!(plan.next(day + 3600, None), None);
        assert_eq!(plan.next(day + 3600, Some(day + 1800)), None);
        // A window ending after the span, and a time after it, wait for tomorrow.
        assert_eq!(
            plan.next(day + 16 * 3600, Some(day + 21 * 3600)),
            Some(tomorrow)
        );
        assert_eq!(plan.next(day + 20 * 3600, None), Some(tomorrow));
        assert_eq!(plan.next(day - 3600, None), Some(day));
    }
    #[test]
    fn failures_retry_after_a_pause_and_skip_the_window() {
        let mut runtime = Runtime {
            launch: Some(|_, _, _| Err(Failure::Failed("synthetic failure"))),
            ..Default::default()
        };
        let ok = || Ok((PathBuf::from("claude"), "SYNTHETIC".to_string()));
        let mut time = 1000;
        for attempt in 1..=ATTEMPTS {
            if attempt == 2 {
                // Something that never reached Claude is retried without using an attempt.
                runtime.launch = Some(|_, _, _| Err(Failure::NotSent("synthetic offline")));
                runtime.start("a", Some(7), ok(), PathBuf::new());
                let (_, day, record) = finish(&mut runtime, time);
                assert_eq!(day, Some(7));
                assert!(record.message.contains("每 5 分钟重试"));
                time += RETRY;
                runtime.launch = Some(|_, _, _| Err(Failure::Failed("synthetic failure")));
            }
            assert!(runtime.ready("a", time));
            runtime.start("a", Some(7), ok(), PathBuf::new());
            assert!(runtime.running("a") && !runtime.ready("b", time));
            let (id, _, record) = finish(&mut runtime, time);
            assert_eq!(id, "a");
            assert!(!record.ok);
            assert_eq!(runtime.tried("a"), Some(time));
            if attempt < ATTEMPTS {
                assert!(record.message.ends_with("5 分钟后重试"));
                assert!(!runtime.ready("a", time + RETRY - 1));
                assert!(runtime.ready("b", time));
            } else {
                assert!(record.message.contains("跳过这个 5 小时"));
                assert!(!runtime.ready("a", time + SKIP - 1));
                assert!(runtime.ready("a", time + SKIP));
            }
            time += RETRY;
        }
        // A window that starts or ends in between starts the count afresh.
        runtime.clear_failures("a");
        assert!(runtime.ready("a", time));
        runtime.launch = Some(|_, _, token| {
            assert_eq!(token, "SYNTHETIC");
            Ok(())
        });
        runtime.start("a", Some(8), ok(), PathBuf::new());
        let (_, day, record) = finish(&mut runtime, time);
        assert_eq!(day, Some(8));
        assert!(record.ok);
        // Preparation errors reach the same retry path without launching anything.
        runtime.launch = Some(|_, _, _| panic!("must not launch"));
        runtime.start("a", Some(9), Err("Access 已到期"), PathBuf::new());
        let (_, _, record) = finish(&mut runtime, time);
        assert!(record.message.starts_with("Access 已到期"));
    }
    #[test]
    fn a_warmup_by_hand_is_sent_once_in_turn_and_never_retried() {
        let mut runtime = Runtime {
            launch: Some(|_, _, _| Err(Failure::Failed("synthetic failure"))),
            ..Default::default()
        };
        let ok = || Ok((PathBuf::from("claude"), "SYNTHETIC".to_string()));
        runtime.start("b", Some(7), ok(), PathBuf::new());
        // Asked for while another account sends: waits its turn, once however often asked.
        runtime.request("a").unwrap();
        runtime.request("a").unwrap();
        assert!(runtime.queued("a") && runtime.next_manual().is_none());
        finish(&mut runtime, 1000);
        assert_eq!(runtime.next_manual(), Some("a"));
        runtime.start("a", None, ok(), PathBuf::new());
        assert!(runtime.running("a") && !runtime.queued("a"));
        assert_eq!(runtime.request("a"), Err("这个账号正在预热"));
        let (id, day, record) = finish(&mut runtime, 1100);
        assert_eq!((id.as_str(), day), ("a", None));
        assert_eq!(record.message, "手动预热未完成：synthetic failure");
        assert_eq!(runtime.tried("a"), Some(1100));
        // A failure by hand neither counts against automatic attempts nor schedules a retry.
        assert!(runtime.ready("a", 1100) && runtime.next_manual().is_none());
        runtime.launch = Some(|_, _, _| Ok(()));
        runtime.request("a").unwrap();
        runtime.start("a", None, ok(), PathBuf::new());
        let (_, _, record) = finish(&mut runtime, 1200);
        assert!(record.ok && record.message == "手动预热已发送");
        runtime.request("a").unwrap();
        runtime.forget("a");
        assert!(!runtime.queued("a") && runtime.tried("a").is_none());
    }
    fn finish(runtime: &mut Runtime, time: u64) -> (String, Option<u64>, Record) {
        loop {
            if let Some(done) = runtime.poll(time) {
                break done;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
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
