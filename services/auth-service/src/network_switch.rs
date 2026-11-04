//! Safe network switching (v0.22). A switch the person asks for waits until no model reply is
//! in flight, holds new Claude Code requests through the companion mod meanwhile, joins the
//! chosen Wi-Fi or the person's iPhone hotspot with `networksetup`, and checks the AI services
//! answer again. Nothing here switches on its own.
//!
//! What "in flight" means comes from two places: the mod reports each model request of every
//! Claude Code session it runs in (and holds the next one while a switch is pending), and the
//! traffic of the other clients is read from mihomo's controller, or from `nettop` per process
//! when there is none. Those clients cannot be held, only waited for.
//!
//! Wi-Fi passwords are typed in the switch sheet, kept in memory until the join worked, then
//! stored in the AuthBaton Vault; a refused join forgets them. An iPhone hotspot needs none: the
//! hotspot helper wakes it over Continuity and gets its current name and password. Neither ever
//! goes into a log, the database, a reply or the snapshot.
use crate::platform::{Platform, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a switch waits for a quiet moment before it gives up and leaves the network alone.
pub const WAIT_LIMIT: Duration = Duration::from_secs(300);
/// Traffic this recent still counts as a reply in flight.
const QUIET: Duration = Duration::from_secs(10);
/// A request the mod reported without a heartbeat for this long was cut off with its session.
const HEARTBEAT_STALE: Duration = Duration::from_secs(45);
/// A held mod request asks again at least this often, so a held session shows as held.
const HELD_FRESH: Duration = Duration::from_secs(25);
const SESSION_FORGET: Duration = Duration::from_secs(600);
const GROUP_FORGET: Duration = Duration::from_secs(600);
/// The page or menu bar asked to watch within this long.
const INTEREST: Duration = Duration::from_secs(15);
/// Bytes per sample that make a connection count as busy: a streamed reply always passes the
/// first, a request being sent the second; telemetry and keep-alives stay under both.
const DOWN_ACTIVE: u64 = 512;
const UP_ACTIVE: u64 = 4096;
const HOTSPOT_PREFIX: &str = "172.20.10.";
const NETWORKSETUP: &str = "/usr/sbin/networksetup";
const HISTORY: usize = 20;
const LOG_LIMIT: u64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Phone(String),
    Wifi(String),
}
impl Target {
    fn name(&self) -> &str {
        match self {
            Target::Phone(name) | Target::Wifi(name) => name,
        }
    }
}

/// `accounts.network`: watch, switch, cancel, forget.
pub struct Command<'a> {
    pub action: &'a str,
    pub phone: Option<&'a str>,
    pub ssid: Option<&'a str>,
    pub password: Option<&'a str>,
}

/// One message from the Claude Code mod about one model request.
#[derive(Deserialize, Clone, Debug)]
pub struct ModEvent {
    pub session: String,
    #[serde(default)]
    pub project: String,
    /// begin, end, alive
    pub event: String,
    #[serde(default)]
    pub request: String,
}

#[derive(Debug, PartialEq)]
pub enum ModReply {
    Go,
    Hold,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Phone {
    pub name: String,
    pub battery: Option<i64>,
    pub network: Option<String>,
    pub signal: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Join {
    Joined,
    NotFound,
    Refused(String),
    Failed(String),
}

/// What the worker needs from the machine; tests give it a scripted one.
pub trait Sys: Send + Sync {
    fn ip(&self, device: &str) -> Option<String>;
    fn join(&self, device: &str, ssid: &str, password: Option<&str>) -> Join;
    fn reachable(&self) -> bool;
    fn wake_phone(&self, name: &str) -> std::result::Result<(String, String), &'static str>;
    fn read_password(&self, ssid: &str) -> Result<Option<String>>;
    fn save_password(&self, ssid: &str, password: &str) -> Result<()>;
    fn forget_password(&self, ssid: &str) -> Result<()>;
    fn sleep(&self, duration: Duration);
}

#[derive(Clone, Debug)]
struct Operation {
    target: Target,
    from: String,
    step: &'static str,
    started_unix: u64,
    started: Instant,
    deadline: Instant,
    running: bool,
    ok: Option<bool>,
    message: Option<String>,
    waited: Option<u64>,
}

#[derive(Clone, Debug)]
struct Record {
    at: u64,
    from: String,
    to: String,
    result: &'static str,
    waited: Option<u64>,
    message: Option<String>,
}
impl Record {
    fn json(&self) -> Value {
        json!({"at":self.at,"from":self.from,"to":self.to,"result":self.result,"waited":self.waited,"message":self.message})
    }
}

#[derive(Default)]
struct Session {
    project: String,
    inflight: HashMap<String, Instant>,
    held_at: Option<Instant>,
    last_seen: Option<Instant>,
}

struct Group {
    last_active: Option<Instant>,
    last_seen: Instant,
}

struct State {
    device: Option<String>,
    interest_until: Option<Instant>,
    ip: Option<String>,
    known: Vec<String>,
    known_at: Option<Instant>,
    phones: Vec<Phone>,
    phones_at: Option<Instant>,
    remembered: BTreeSet<String>,
    monitor: Option<&'static str>,
    sampled_at: Option<Instant>,
    groups: BTreeMap<String, Group>,
    sessions: HashMap<String, Session>,
    operation: Option<Operation>,
    history: VecDeque<Record>,
    /// The network AuthBaton last joined and the address it got there: the only way to name
    /// the current Wi-Fi without Location Services.
    joined: Option<(String, String)>,
}

struct Shared {
    root: Option<PathBuf>,
    state: Mutex<State>,
    cancel: AtomicBool,
    stop: AtomicBool,
    /// A monitor thread reads traffic: a switch then waits for a reading taken after it began,
    /// never for the absence of one.
    monitored: AtomicBool,
}

pub struct Runtime {
    shared: Arc<Shared>,
    sys: Option<Arc<dyn Sys>>,
    helper: Option<PathBuf>,
    monitor: Option<JoinHandle<()>>,
    worker: Option<JoinHandle<()>>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn new_state(device: Option<String>) -> State {
    State {
        device,
        interest_until: None,
        ip: None,
        known: vec![],
        known_at: None,
        phones: vec![],
        phones_at: None,
        remembered: BTreeSet::new(),
        monitor: None,
        sampled_at: None,
        groups: BTreeMap::new(),
        sessions: HashMap::new(),
        operation: None,
        history: VecDeque::new(),
        joined: None,
    }
}

impl Runtime {
    /// No machine behind it: tests of the account service and builds without Wi-Fi.
    pub fn off() -> Runtime {
        Runtime {
            shared: Arc::new(Shared {
                root: None,
                state: Mutex::new(new_state(None)),
                cancel: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                monitored: AtomicBool::new(false),
            }),
            sys: None,
            helper: None,
            monitor: None,
            worker: None,
        }
    }

    pub fn start(root: &Path, platform: Platform, helper: Option<PathBuf>) -> Runtime {
        let device = wifi_device();
        let mut state = new_state(device.clone());
        state.remembered = load_remembered(root);
        state.history = load_history(root);
        let shared = Arc::new(Shared {
            root: Some(root.to_path_buf()),
            state: Mutex::new(state),
            cancel: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            monitored: AtomicBool::new(device.is_some()),
        });
        let sys: Arc<dyn Sys> = Arc::new(RealSys {
            platform,
            helper: helper.clone(),
        });
        let monitor = device.is_some().then(|| {
            let shared = shared.clone();
            let helper = helper.clone();
            std::thread::spawn(move || monitor(&shared, helper.as_deref()))
        });
        Runtime {
            shared,
            sys: Some(sys),
            helper,
            monitor,
            worker: None,
        }
    }

    #[cfg(test)]
    fn with_sys(root: Option<&Path>, sys: Arc<dyn Sys>) -> Runtime {
        let mut state = new_state(Some("en0".into()));
        if let Some(root) = root {
            state.remembered = load_remembered(root);
            state.history = load_history(root);
        }
        Runtime {
            shared: Arc::new(Shared {
                root: root.map(Path::to_path_buf),
                state: Mutex::new(state),
                cancel: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                monitored: AtomicBool::new(false),
            }),
            sys: Some(sys),
            helper: None,
            monitor: None,
            worker: None,
        }
    }

    pub fn tick(&mut self) {
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            let _ = self.worker.take().map(JoinHandle::join);
        }
    }

    pub fn stop(&mut self) {
        self.shared.cancel.store(true, Ordering::Release);
        self.shared.stop.store(true, Ordering::Release);
        // A join already under way finishes on its own; the service does not wait for it.
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
    }

    pub fn command(&mut self, command: Command<'_>) -> Result<()> {
        match command.action {
            "watch" => {
                let mut state = self.shared.state.lock().unwrap();
                state.interest_until = Some(Instant::now() + INTEREST);
                Ok(())
            }
            "cancel" => {
                let state = self.shared.state.lock().unwrap();
                match &state.operation {
                    Some(op) if op.running && matches!(op.step, "waiting" | "waking") => {
                        self.shared.cancel.store(true, Ordering::Release);
                        Ok(())
                    }
                    Some(op) if op.running => Err("正在加入新网络，这一步不能停"),
                    _ => Ok(()),
                }
            }
            "forget" => {
                let ssid = command.ssid.ok_or("缺少 Wi-Fi 名称")?;
                let sys = self.sys.as_ref().ok_or("这台 Mac 没有 Wi-Fi")?;
                sys.forget_password(ssid)?;
                let mut state = self.shared.state.lock().unwrap();
                state.remembered.remove(ssid);
                save_remembered(self.shared.root.as_deref(), &state.remembered);
                Ok(())
            }
            "switch" => self.switch(command),
            _ => Err("不支持的网络操作"),
        }
    }

    fn switch(&mut self, command: Command<'_>) -> Result<()> {
        let sys = self.sys.clone().ok_or("这台 Mac 没有 Wi-Fi")?;
        let target = match (command.phone, command.ssid) {
            (Some(name), None) if !name.trim().is_empty() => Target::Phone(name.into()),
            (None, Some(ssid)) if valid_ssid(ssid) => Target::Wifi(ssid.into()),
            _ => return Err("要切到的网络无效"),
        };
        if matches!(target, Target::Phone(_)) && self.helper.is_none() && cfg!(not(test)) {
            return Err("这个版本缺少热点组件，切不了 iPhone 热点");
        }
        let password = command
            .password
            .filter(|p| !p.is_empty())
            .map(str::to_owned);
        if let Some(password) = &password
            && (password.len() < 8 || password.len() > 63)
        {
            return Err("Wi-Fi 密码应为 8 到 63 个字符");
        }
        let mut state = self.shared.state.lock().unwrap();
        let device = state.device.clone().ok_or("这台 Mac 没有 Wi-Fi")?;
        if state.operation.as_ref().is_some_and(|op| op.running) {
            return Err("上一次切换还没结束");
        }
        if let Target::Wifi(ssid) = &target
            && password.is_none()
            && !state.remembered.contains(ssid)
        {
            return Err("要先输入这个 Wi-Fi 的密码");
        }
        let now = Instant::now();
        state.operation = Some(Operation {
            from: current_name(&state),
            target: target.clone(),
            step: "waiting",
            started_unix: now_unix(),
            started: now,
            deadline: now + WAIT_LIMIT,
            running: true,
            ok: None,
            message: None,
            waited: None,
        });
        drop(state);
        self.shared.cancel.store(false, Ordering::Release);
        let shared = self.shared.clone();
        self.worker = Some(std::thread::spawn(move || {
            run_switch(&shared, sys.as_ref(), &device, target, password)
        }));
        Ok(())
    }

    /// A model request of a Claude Code session, as the mod reports it.
    pub fn mod_event(&self, event: &ModEvent) -> ModReply {
        let mut state = self.shared.state.lock().unwrap();
        let now = Instant::now();
        let held = state.operation.as_ref().is_some_and(|op| op.running);
        let session = state.sessions.entry(event.session.clone()).or_default();
        if !event.project.is_empty() {
            session.project = event.project.clone();
        }
        session.last_seen = Some(now);
        match event.event.as_str() {
            "begin" if held => {
                session.held_at = Some(now);
                ModReply::Hold
            }
            "begin" => {
                session.held_at = None;
                session.inflight.insert(event.request.clone(), now);
                ModReply::Go
            }
            "alive" => {
                if let Some(seen) = session.inflight.get_mut(&event.request) {
                    *seen = now;
                }
                if held { ModReply::Hold } else { ModReply::Go }
            }
            _ => {
                session.inflight.remove(&event.request);
                ModReply::Go
            }
        }
    }

    pub fn snapshot(&self) -> Value {
        let state = self.shared.state.lock().unwrap();
        snapshot(&state, Instant::now())
    }
}

fn valid_ssid(ssid: &str) -> bool {
    !ssid.is_empty() && ssid.len() <= 32 && !ssid.contains('\0')
}

fn current_name(state: &State) -> String {
    match (&state.ip, &state.joined) {
        (Some(ip), Some((name, joined_ip))) if ip == joined_ip => name.clone(),
        (Some(ip), _) if ip.starts_with(HOTSPOT_PREFIX) => "iPhone 热点".into(),
        (Some(_), _) => "Wi-Fi".into(),
        (None, _) => "没有网络".into(),
    }
}

/// Every source the page lists: the mod's sessions (which can be held) and the traffic of the
/// other clients (which can only be waited for).
fn sources(state: &State, now: Instant) -> Vec<(String, &'static str, bool)> {
    let mut list = vec![];
    let mut sessions: Vec<&Session> = state
        .sessions
        .values()
        .filter(|s| {
            s.last_seen
                .is_some_and(|t| now.duration_since(t) < SESSION_FORGET)
        })
        .collect();
    sessions.sort_by(|a, b| a.project.cmp(&b.project));
    for session in &sessions {
        let live = session
            .inflight
            .values()
            .any(|t| now.duration_since(*t) < HEARTBEAT_STALE);
        let held = session
            .held_at
            .is_some_and(|t| now.duration_since(t) < HELD_FRESH);
        let state_name = if live {
            "live"
        } else if held {
            "held"
        } else {
            "idle"
        };
        let name = if session.project.is_empty() {
            "Claude Code".to_string()
        } else {
            format!("Claude Code · {}", session.project)
        };
        list.push((name, state_name, true));
    }
    for (name, group) in &state.groups {
        if now.duration_since(group.last_seen) >= GROUP_FORGET {
            continue;
        }
        // The mod already lists Claude Code's sessions one by one.
        if name == "Claude Code" && !sessions.is_empty() {
            continue;
        }
        let live = group
            .last_active
            .is_some_and(|t| now.duration_since(t) < QUIET);
        list.push((name.clone(), if live { "live" } else { "idle" }, false));
    }
    list
}

/// Nothing in flight anywhere: no mod request without a fresh heartbeat's end, no traffic
/// within `QUIET` (which also covers Claude Code sessions the mod does not run in).
fn quiet(state: &State, now: Instant) -> bool {
    let requests = state.sessions.values().any(|s| {
        s.inflight
            .values()
            .any(|t| now.duration_since(*t) < HEARTBEAT_STALE)
    });
    let traffic = state
        .groups
        .values()
        .any(|g| g.last_active.is_some_and(|t| now.duration_since(t) < QUIET));
    !requests && !traffic
}

fn snapshot(state: &State, now: Instant) -> Value {
    let kind = match &state.ip {
        None => "none",
        Some(ip) if ip.starts_with(HOTSPOT_PREFIX) => "hotspot",
        Some(_) => "wifi",
    };
    let name = match (&state.ip, &state.joined) {
        (Some(ip), Some((name, joined_ip))) if ip == joined_ip => Some(name.clone()),
        _ => None,
    };
    let sources: Vec<Value> = sources(state, now)
        .into_iter()
        .map(|(name, s, gated)| json!({"name":name,"state":s,"gated":gated}))
        .collect();
    let phone_names: BTreeSet<&str> = state.phones.iter().map(|p| p.name.as_str()).collect();
    let networks: Vec<Value> = state
        .known
        .iter()
        .filter(|ssid| !phone_names.contains(ssid.as_str()))
        .map(|ssid| {
            json!({"ssid":ssid,"remembered":state.remembered.contains(ssid),"current":name.as_deref()==Some(ssid.as_str())})
        })
        .collect();
    let phones: Vec<Value> = state
        .phones
        .iter()
        .map(|p| json!({"name":p.name,"battery":p.battery,"network":p.network,"signal":p.signal}))
        .collect();
    let operation = state.operation.as_ref().map(|op| {
        let (kind, target) = match &op.target {
            Target::Phone(n) => ("phone", n),
            Target::Wifi(n) => ("wifi", n),
        };
        json!({
            "kind":kind,"target":target,"from":op.from,"step":op.step,"running":op.running,"ok":op.ok,
            "message":op.message,"started_at":op.started_unix,"waited":op.waited,
            "remaining":op.running.then(|| op.deadline.saturating_duration_since(now).as_secs()),
        })
    });
    json!({
        "available": state.device.is_some(),
        "current": {"kind":kind,"name":name},
        "safe": quiet(state, now),
        "watching": state.interest_until.is_some_and(|t| t > now) || state.operation.as_ref().is_some_and(|o| o.running),
        "monitor": state.monitor,
        "sources": sources,
        "phones": phones,
        "networks": networks,
        "operation": operation,
        "history": state.history.iter().rev().map(Record::json).collect::<Vec<_>>(),
    })
}

fn set_step(shared: &Shared, step: &'static str) {
    if let Some(op) = shared.state.lock().unwrap().operation.as_mut() {
        op.step = step;
    }
}

/// Ends the operation: the gate opens because it is no longer running.
fn finish(
    shared: &Shared,
    step: &'static str,
    ok: bool,
    message: Option<&str>,
    joined: Option<(String, String)>,
) {
    let mut state = shared.state.lock().unwrap();
    if let Some((name, ip)) = joined {
        state.ip = Some(ip.clone());
        state.joined = Some((name, ip));
    }
    let Some(op) = state.operation.as_mut() else {
        return;
    };
    op.step = step;
    op.running = false;
    op.ok = Some(ok);
    op.message = message.map(str::to_owned);
    let record = Record {
        at: now_unix(),
        from: op.from.clone(),
        to: op.target.name().to_owned(),
        result: step,
        waited: op.waited,
        message: op.message.clone(),
    };
    state.history.push_back(record.clone());
    while state.history.len() > HISTORY {
        state.history.pop_front();
    }
    drop(state);
    append_history(shared.root.as_deref(), &record);
}

fn run_switch(
    shared: &Shared,
    sys: &dyn Sys,
    device: &str,
    target: Target,
    password: Option<String>,
) {
    let started = {
        let state = shared.state.lock().unwrap();
        state.operation.as_ref().unwrap().started
    };
    // 1. Wait for quiet. The gate is closed from the start, so Claude Code finishes the reply it
    // is sending and holds the next one; two quiet readings in a row after a fresh traffic
    // sample count as a quiet moment.
    let mut quiet_in_row = 0;
    loop {
        if shared.cancel.load(Ordering::Acquire) {
            return finish(shared, "cancelled", false, Some("已取消，网络没动"), None);
        }
        let now = Instant::now();
        let deadline = shared
            .state
            .lock()
            .unwrap()
            .operation
            .as_ref()
            .map(|op| op.deadline);
        if deadline.is_none_or(|deadline| now >= deadline) {
            return finish(
                shared,
                "timeout",
                false,
                Some("5 分钟没等到空闲，网络没动"),
                None,
            );
        }
        {
            let state = shared.state.lock().unwrap();
            // No reading yet is not quiet: without one, a switch waits out its deadline.
            let fresh = !shared.monitored.load(Ordering::Acquire)
                || state
                    .sampled_at
                    .is_some_and(|t| t > started + Duration::from_millis(900));
            quiet_in_row = if fresh && quiet(&state, now) {
                quiet_in_row + 1
            } else {
                0
            };
        }
        if quiet_in_row >= 2 {
            break;
        }
        sys.sleep(Duration::from_millis(500));
    }
    let waited = started.elapsed().as_secs();
    if let Some(op) = shared.state.lock().unwrap().operation.as_mut() {
        op.waited = Some(waited);
    }
    // 2. What to join with.
    let (ssid, password, remember) = match &target {
        Target::Phone(name) => {
            set_step(shared, "waking");
            match sys.wake_phone(name) {
                Ok((ssid, password)) => (ssid, Some(password), false),
                Err(message) => return finish(shared, "failed", false, Some(message), None),
            }
        }
        Target::Wifi(ssid) => match password {
            Some(password) => (ssid.clone(), Some(password), true),
            None => match sys.read_password(ssid) {
                Ok(Some(password)) => (ssid.clone(), Some(password), false),
                _ => {
                    return finish(
                        shared,
                        "wrong_password",
                        false,
                        Some("记住的密码读不出来了，请重新输入"),
                        None,
                    );
                }
            },
        },
    };
    if shared.cancel.load(Ordering::Acquire) {
        return finish(shared, "cancelled", false, Some("已取消，网络没动"), None);
    }
    // 3. Join. From here the switch no longer stops: a refused join has already let go of the
    // old network.
    set_step(shared, "joining");
    let attempts = if matches!(target, Target::Phone(_)) {
        4
    } else {
        1
    };
    let mut result = Join::NotFound;
    for attempt in 0..attempts {
        if attempt > 0 {
            sys.sleep(Duration::from_secs(3));
        }
        result = sys.join(device, &ssid, password.as_deref());
        if result != Join::NotFound {
            break;
        }
    }
    match result {
        Join::Joined => {}
        Join::NotFound => {
            let message = match &target {
                Target::Phone(_) => "iPhone 的热点没出现，网络没动",
                Target::Wifi(_) => "附近找不到这个 Wi-Fi，网络没动",
            };
            return finish(shared, "not_found", false, Some(message), None);
        }
        Join::Refused(_) | Join::Failed(_) if matches!(target, Target::Wifi(_)) => {
            // A refused join is how a wrong or changed password shows; never keep it.
            if !remember {
                let _ = sys.forget_password(&ssid);
                let mut state = shared.state.lock().unwrap();
                state.remembered.remove(&ssid);
                save_remembered(shared.root.as_deref(), &state.remembered);
            }
            return finish(
                shared,
                "wrong_password",
                false,
                Some("密码不对，已连回原来的网络"),
                None,
            );
        }
        Join::Refused(_) | Join::Failed(_) => {
            return finish(
                shared,
                "failed",
                false,
                Some("iPhone 热点拒绝加入，已连回原来的网络"),
                None,
            );
        }
    }
    // 4. Check: an address on the new network, then the AI services answer through the proxy.
    set_step(shared, "checking");
    let mut ip = None;
    for _ in 0..40 {
        ip = sys
            .ip(device)
            .filter(|ip| !matches!(target, Target::Phone(_)) || ip.starts_with(HOTSPOT_PREFIX));
        if ip.is_some() {
            break;
        }
        sys.sleep(Duration::from_millis(500));
    }
    let Some(ip) = ip else {
        return finish(
            shared,
            "failed",
            false,
            Some("已加入，但没拿到网络地址"),
            None,
        );
    };
    let mut reached = false;
    for attempt in 0..6 {
        if attempt > 0 {
            sys.sleep(Duration::from_secs(3));
        }
        if sys.reachable() {
            reached = true;
            break;
        }
    }
    let joined = Some((target.name().to_owned(), ip));
    // The join took the password, so it is right: keep it now, even if the check below fails.
    if remember
        && let Target::Wifi(ssid) = &target
        && let Some(password) = &password
        && sys.save_password(ssid, password).is_ok()
    {
        let mut state = shared.state.lock().unwrap();
        state.remembered.insert(ssid.clone());
        save_remembered(shared.root.as_deref(), &state.remembered);
    }
    if reached {
        finish(shared, "done", true, None, joined);
    } else {
        finish(
            shared,
            "unreachable",
            false,
            Some("已切过去，但连不上 AI 服务"),
            joined,
        );
    }
}

// ---- What the monitor reads ----

fn monitor(shared: &Shared, helper: Option<&Path>) {
    let device = shared.state.lock().unwrap().device.clone().unwrap();
    let mut traffic = Traffic::default();
    let mut last_ip: Option<Instant> = None;
    while !shared.stop.load(Ordering::Acquire) {
        let now = Instant::now();
        let (active, phones_due, known_due) = {
            let state = shared.state.lock().unwrap();
            let running = state.operation.as_ref().is_some_and(|op| op.running);
            let interest = state.interest_until.is_some_and(|t| t > now);
            let due = |at: Option<Instant>, every: u64| {
                at.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(every))
            };
            (
                running || interest,
                interest && !running && due(state.phones_at, 30),
                interest && due(state.known_at, 60),
            )
        };
        if !active {
            traffic.reset();
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        let sample = traffic.sample();
        if last_ip.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(2)) {
            last_ip = Some(now);
            let ip = ipconfig(&device);
            shared.state.lock().unwrap().ip = ip;
        }
        {
            let mut state = shared.state.lock().unwrap();
            state.monitor = traffic.source;
            if let Some(sample) = sample {
                apply_sample(&mut state, &sample, now);
                state.sampled_at = Some(now);
            }
        }
        if known_due {
            let known = preferred_networks(&device);
            let mut state = shared.state.lock().unwrap();
            state.known = known;
            state.known_at = Some(now);
        }
        if phones_due && let Some(helper) = helper {
            let phones = browse_phones(helper);
            let mut state = shared.state.lock().unwrap();
            state.phones = phones;
            state.phones_at = Some(Instant::now());
        }
        let spent = now.elapsed();
        if spent < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_secs(1) - spent);
        }
    }
}

/// Bytes moved per client since the previous sample, by display group.
type Sample = BTreeMap<String, (u64, u64)>;

fn apply_sample(state: &mut State, sample: &Sample, now: Instant) {
    for (group, (up, down)) in sample {
        let entry = state.groups.entry(group.clone()).or_insert(Group {
            last_active: None,
            last_seen: now,
        });
        entry.last_seen = now;
        if *down >= DOWN_ACTIVE || *up >= UP_ACTIVE {
            entry.last_active = Some(now);
        }
    }
}

#[derive(Default)]
struct Traffic {
    source: Option<&'static str>,
    mihomo: Option<PathBuf>,
    looked_up: Option<Instant>,
    previous: HashMap<String, (u64, u64)>,
    primed: bool,
}
impl Traffic {
    fn reset(&mut self) {
        self.previous.clear();
        self.primed = false;
    }
    fn sample(&mut self) -> Option<Sample> {
        if self.mihomo.is_none()
            && self
                .looked_up
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(60))
        {
            self.looked_up = Some(Instant::now());
            self.mihomo = mihomo_socket();
        }
        let readings = match &self.mihomo {
            Some(socket) => match mihomo_connections(socket) {
                Some(readings) => {
                    if self.source != Some("mihomo") {
                        self.reset();
                    }
                    self.source = Some("mihomo");
                    readings
                }
                None => {
                    self.mihomo = None;
                    return None;
                }
            },
            None => {
                if self.source != Some("nettop") {
                    self.reset();
                }
                self.source = Some("nettop");
                nettop_readings()?
            }
        };
        let sample = deltas(&mut self.previous, readings);
        // The first reading only sets the baseline.
        let primed = self.primed;
        self.primed = true;
        primed.then_some(sample)
    }
}

/// One reading per connection (mihomo) or per process (nettop): key, display group, totals.
type Reading = (String, String, u64, u64);

fn deltas(previous: &mut HashMap<String, (u64, u64)>, readings: Vec<Reading>) -> Sample {
    let mut sample = Sample::new();
    let mut next = HashMap::new();
    for (key, group, up, down) in readings {
        let (old_up, old_down) = previous.get(&key).copied().unwrap_or((0, 0));
        let entry = sample.entry(group).or_insert((0, 0));
        entry.0 += up.saturating_sub(old_up);
        entry.1 += down.saturating_sub(old_down);
        next.insert(key, (up, down));
    }
    *previous = next;
    sample
}

/// The display group of a client, by its process name.
fn group(process: &str) -> Option<String> {
    let browser = [
        "Google Chrome",
        "Safari",
        "com.apple.WebKit",
        "Microsoft Edge",
        "Arc",
        "Firefox",
        "Brave",
        "ego",
    ];
    Some(match process {
        "claude" => "Claude Code".into(),
        "codex" => "Codex CLI".into(),
        p if p.starts_with("Claude") => "Claude 桌面".into(),
        p if p.starts_with("Codex") || p.starts_with("ChatGPT") => "Codex 桌面".into(),
        p if browser.iter().any(|b| p.starts_with(b)) => "浏览器".into(),
        "" => return None,
        p => p.into(),
    })
}

fn ai_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    [
        "anthropic.com",
        "claude.ai",
        "claude.com",
        "openai.com",
        "chatgpt.com",
    ]
    .iter()
    .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

fn parse_mihomo(body: &[u8]) -> Option<Vec<Reading>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let connections = value.get("connections")?;
    let mut readings = vec![];
    for c in connections.as_array().into_iter().flatten() {
        let meta = &c["metadata"];
        let host = meta["host"]
            .as_str()
            .filter(|h| !h.is_empty())
            .or_else(|| meta["sniffHost"].as_str())
            .unwrap_or("");
        if !ai_host(host) {
            continue;
        }
        let Some(group) = group(meta["process"].as_str().unwrap_or("")) else {
            continue;
        };
        let Some(id) = c["id"].as_str() else { continue };
        readings.push((
            id.to_owned(),
            group,
            c["upload"].as_u64().unwrap_or(0),
            c["download"].as_u64().unwrap_or(0),
        ));
    }
    Some(readings)
}

/// `nettop -P -L 1 -x -J bytes_in,bytes_out`: `name.pid,in,out,` per process. Without host
/// names only the AI clients themselves count, and browsers are left out.
fn parse_nettop(text: &str) -> Vec<Reading> {
    let mut readings = vec![];
    for line in text.lines().skip(1) {
        let mut fields = line.split(',');
        let (Some(name), Some(down), Some(up)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let process = name.rsplit_once('.').map_or(name, |(p, _)| p);
        let Some(group) = group(process) else {
            continue;
        };
        if !matches!(
            group.as_str(),
            "Claude Code" | "Codex CLI" | "Claude 桌面" | "Codex 桌面"
        ) {
            continue;
        }
        let (Ok(down), Ok(up)) = (down.trim().parse(), up.trim().parse()) else {
            continue;
        };
        readings.push((name.to_owned(), group, up, down));
    }
    readings
}

fn run(binary: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let (status, bytes) =
        crate::platform::pipe_status_with_cancel(Path::new(binary), args, b"", timeout, None, &[])
            .ok()?;
    (status == Some(0)).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

fn mihomo_socket() -> Option<PathBuf> {
    let text = run("/bin/ps", &["-axo", "args="], Duration::from_secs(3))?;
    text.lines().find_map(|line| {
        let rest = &line[line.find("-ext-ctl-unix ")? + "-ext-ctl-unix ".len()..];
        let path = rest.split_whitespace().next()?;
        path.starts_with('/').then(|| PathBuf::from(path))
    })
}

fn mihomo_connections(socket: &Path) -> Option<Vec<Reading>> {
    let socket = socket.to_str()?;
    let body = run(
        "/usr/bin/curl",
        &[
            "-s",
            "--noproxy",
            "*",
            "--max-time",
            "2",
            "--unix-socket",
            socket,
            "http://mihomo/connections",
        ],
        Duration::from_secs(3),
    )?;
    parse_mihomo(body.as_bytes())
}

fn nettop_readings() -> Option<Vec<Reading>> {
    let text = run(
        "/usr/bin/nettop",
        &["-P", "-L", "1", "-x", "-J", "bytes_in,bytes_out"],
        Duration::from_secs(5),
    )?;
    Some(parse_nettop(&text))
}

fn ipconfig(device: &str) -> Option<String> {
    run(
        "/usr/sbin/ipconfig",
        &["getifaddr", device],
        Duration::from_secs(2),
    )
    .map(|s| s.trim().to_owned())
    .filter(|s| !s.is_empty())
}

fn wifi_device() -> Option<String> {
    let text = run(
        NETWORKSETUP,
        &["-listallhardwareports"],
        Duration::from_secs(5),
    )?;
    parse_hardware_ports(&text)
}

fn parse_hardware_ports(text: &str) -> Option<String> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "Hardware Port: Wi-Fi" {
            return lines
                .next()?
                .trim()
                .strip_prefix("Device: ")
                .map(str::to_owned);
        }
    }
    None
}

fn preferred_networks(device: &str) -> Vec<String> {
    run(
        NETWORKSETUP,
        &["-listpreferredwirelessnetworks", device],
        Duration::from_secs(5),
    )
    .map(|text| parse_preferred(&text))
    .unwrap_or_default()
}

fn parse_preferred(text: &str) -> Vec<String> {
    text.lines()
        .skip_while(|line| !line.starts_with("Preferred networks on"))
        .skip(1)
        .map(|line| line.trim_start_matches('\t').trim_end().to_owned())
        .filter(|ssid| valid_ssid(ssid))
        .collect()
}

/// What `networksetup -setairportnetwork` printed. It exits 0 either way.
pub fn parse_join(output: &str) -> Join {
    let text = output.trim();
    if text.is_empty() {
        Join::Joined
    } else if text.contains("Could not find network") {
        Join::NotFound
    } else if text.contains("Failed to join network") {
        let code = text
            .split("Error: ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("")
            .to_owned();
        Join::Refused(code)
    } else {
        Join::Failed(text.lines().next().unwrap_or("").chars().take(80).collect())
    }
}

fn browse_phones(helper: &Path) -> Vec<Phone> {
    let Some(text) = run(
        helper.to_str().unwrap_or(""),
        &["browse"],
        Duration::from_secs(10),
    ) else {
        return vec![];
    };
    parse_phones(&text)
}

fn parse_phones(text: &str) -> Vec<Phone> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return vec![];
    };
    value["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(Phone {
                name: d["name"].as_str().filter(|n| !n.is_empty())?.to_owned(),
                battery: d["battery"].as_i64(),
                network: d["network"].as_str().map(str::to_owned),
                signal: d["signal"].as_i64(),
            })
        })
        .collect()
}

// ---- Files: which networks have a stored password, and the switch log ----

fn load_remembered(root: &Path) -> BTreeSet<String> {
    crate::platform::read_json(&root.join("network.json"))
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value::<Vec<String>>(v["remembered"].clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn save_remembered(root: Option<&Path>, remembered: &BTreeSet<String>) {
    if let Some(root) = root {
        let _ = crate::platform::write_json(
            &root.join("network.json"),
            &json!({"remembered": remembered}),
        );
    }
}

fn load_history(root: &Path) -> VecDeque<Record> {
    let text = std::fs::read_to_string(root.join("network-log.jsonl")).unwrap_or_default();
    let mut history: VecDeque<Record> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|v| {
            let result = match v["result"].as_str()? {
                "done" => "done",
                "timeout" => "timeout",
                "cancelled" => "cancelled",
                "wrong_password" => "wrong_password",
                "not_found" => "not_found",
                "unreachable" => "unreachable",
                _ => "failed",
            };
            Some(Record {
                at: v["at"].as_u64()?,
                from: v["from"].as_str()?.to_owned(),
                to: v["to"].as_str()?.to_owned(),
                result,
                waited: v["waited"].as_u64(),
                message: v["message"].as_str().map(str::to_owned),
            })
        })
        .collect();
    while history.len() > HISTORY {
        history.pop_front();
    }
    history
}

fn append_history(root: Option<&Path>, record: &Record) {
    let Some(root) = root else { return };
    let path = root.join("network-log.jsonl");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::rename(&path, root.join("network-log.1.jsonl"));
    }
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)
    {
        let _ = writeln!(file, "{}", record.json());
    }
}

fn vault_id(ssid: &str) -> String {
    let hex: String = ssid.bytes().map(|b| format!("{b:02x}")).collect();
    format!("wifi-{hex}")
}

struct RealSys {
    platform: Platform,
    helper: Option<PathBuf>,
}
impl Sys for RealSys {
    fn ip(&self, device: &str) -> Option<String> {
        ipconfig(device)
    }
    fn join(&self, device: &str, ssid: &str, password: Option<&str>) -> Join {
        let mut args = vec!["-setairportnetwork", device, ssid];
        if let Some(password) = password {
            // The password is in this argument vector for the few seconds networksetup runs
            // (the person accepted that); it is never logged or put through a shell.
            args.push(password);
        }
        match crate::platform::pipe_status_with_cancel(
            Path::new(NETWORKSETUP),
            &args,
            b"",
            Duration::from_secs(90),
            None,
            &[],
        ) {
            Ok((Some(0), bytes)) => parse_join(&String::from_utf8_lossy(&bytes)),
            Ok(_) => Join::Failed("networksetup 超时或失败".into()),
            Err(message) => Join::Failed(message.into()),
        }
    }
    fn reachable(&self) -> bool {
        matches!(
            crate::network::curl_status(
                &[
                    "-s",
                    "-o",
                    "/dev/null",
                    "-w",
                    "%{http_code}",
                    "--max-time",
                    "8",
                    "https://api.anthropic.com/v1/messages",
                ],
                b"",
                Duration::from_secs(10),
            ),
            Ok((Some(0), body)) if String::from_utf8_lossy(&body).trim().parse::<u16>().is_ok_and(|c| (100..600).contains(&c))
        )
    }
    fn wake_phone(&self, name: &str) -> std::result::Result<(String, String), &'static str> {
        let helper = self.helper.as_ref().ok_or("这个版本缺少热点组件")?;
        let (status, bytes) = crate::platform::pipe_status_with_cancel(
            helper,
            &["enable", name],
            b"",
            Duration::from_secs(40),
            None,
            &[],
        )
        .map_err(|_| "打不开 iPhone 的热点")?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "打不开 iPhone 的热点")?;
        match (status, value["ssid"].as_str(), value["password"].as_str()) {
            (Some(0), Some(ssid), Some(password)) if !ssid.is_empty() && !password.is_empty() => {
                Ok((ssid.to_owned(), password.to_owned()))
            }
            _ => Err(match value["error"].as_str() {
                Some("not_found") => "附近没找到这台 iPhone（蓝牙关着或离得太远）",
                _ => "iPhone 没有打开热点",
            }),
        }
    }
    fn read_password(&self, ssid: &str) -> Result<Option<String>> {
        Ok(self
            .platform
            .vault_read_optional(&vault_id(ssid))?
            .and_then(|v| v["password"].as_str().map(str::to_owned)))
    }
    fn save_password(&self, ssid: &str, password: &str) -> Result<()> {
        self.platform
            .vault_write(&vault_id(ssid), &json!({"kind":"wifi","password":password}))
    }
    fn forget_password(&self, ssid: &str) -> Result<()> {
        self.platform
            .delete_key("io.github.wuxm5.authbaton.vault", &vault_id(ssid))
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct Fake {
        joins: Mutex<Vec<(String, Option<String>)>>,
        results: Mutex<VecDeque<Join>>,
        ip: Mutex<Option<String>>,
        reachable: AtomicBool,
        stored: Mutex<HashMap<String, String>>,
        forgotten: Mutex<Vec<String>>,
        woken: AtomicUsize,
        sleeps: AtomicUsize,
    }
    impl Sys for Fake {
        fn ip(&self, _: &str) -> Option<String> {
            self.ip.lock().unwrap().clone()
        }
        fn join(&self, _: &str, ssid: &str, password: Option<&str>) -> Join {
            self.joins
                .lock()
                .unwrap()
                .push((ssid.into(), password.map(str::to_owned)));
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Join::Joined)
        }
        fn reachable(&self) -> bool {
            self.reachable.load(Ordering::Acquire)
        }
        fn wake_phone(&self, name: &str) -> std::result::Result<(String, String), &'static str> {
            self.woken.fetch_add(1, Ordering::AcqRel);
            Ok((name.into(), "phone-secret".into()))
        }
        fn read_password(&self, ssid: &str) -> Result<Option<String>> {
            Ok(self.stored.lock().unwrap().get(ssid).cloned())
        }
        fn save_password(&self, ssid: &str, password: &str) -> Result<()> {
            self.stored
                .lock()
                .unwrap()
                .insert(ssid.into(), password.into());
            Ok(())
        }
        fn forget_password(&self, ssid: &str) -> Result<()> {
            self.stored.lock().unwrap().remove(ssid);
            self.forgotten.lock().unwrap().push(ssid.into());
            Ok(())
        }
        fn sleep(&self, _: Duration) {
            self.sleeps.fetch_add(1, Ordering::AcqRel);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn runtime(fake: Arc<Fake>) -> Runtime {
        Runtime::with_sys(None, fake)
    }
    fn switch(
        rt: &mut Runtime,
        phone: Option<&str>,
        ssid: Option<&str>,
        password: Option<&str>,
    ) -> Result<()> {
        rt.command(Command {
            action: "switch",
            phone,
            ssid,
            password,
        })
    }
    fn wait(rt: &mut Runtime) -> Value {
        if let Some(worker) = rt.worker.take() {
            worker.join().unwrap();
        }
        rt.snapshot()["operation"].clone()
    }
    fn event(session: &str, event: &str, request: &str) -> ModEvent {
        ModEvent {
            session: session.into(),
            project: "notes".into(),
            event: event.into(),
            request: request.into(),
        }
    }

    #[test]
    fn a_wifi_switch_saves_its_password_only_after_the_join_worked() {
        let fake = Arc::new(Fake::default());
        *fake.ip.lock().unwrap() = Some("192.168.1.9".into());
        fake.reachable.store(true, Ordering::Release);
        let mut rt = runtime(fake.clone());
        assert_eq!(
            switch(&mut rt, None, Some("Cafe Guest"), None),
            Err("要先输入这个 Wi-Fi 的密码")
        );
        switch(&mut rt, None, Some("Cafe Guest"), Some("guest-pass")).unwrap();
        let op = wait(&mut rt);
        assert_eq!(op["step"], "done", "{op}");
        assert_eq!(op["ok"], true);
        assert_eq!(
            fake.joins.lock().unwrap()[0],
            ("Cafe Guest".into(), Some("guest-pass".into()))
        );
        assert_eq!(
            fake.stored
                .lock()
                .unwrap()
                .get("Cafe Guest")
                .map(String::as_str),
            Some("guest-pass")
        );
        let snap = rt.snapshot();
        assert_eq!(snap["current"]["name"], "Cafe Guest");
        assert_eq!(snap["history"][0]["result"], "done");
        // The password never reaches the snapshot.
        assert!(!snap.to_string().contains("guest-pass"));
        // Next time it is read back, not asked for.
        *fake.ip.lock().unwrap() = Some("192.168.1.10".into());
        switch(&mut rt, None, Some("Cafe Guest"), None).unwrap();
        assert_eq!(wait(&mut rt)["step"], "done");
        assert_eq!(
            fake.joins.lock().unwrap()[1].1.as_deref(),
            Some("guest-pass")
        );
    }

    #[test]
    fn a_refused_join_never_keeps_a_new_password_and_forgets_a_stored_one() {
        let fake = Arc::new(Fake::default());
        fake.results
            .lock()
            .unwrap()
            .push_back(Join::Refused("-3912".into()));
        let mut rt = runtime(fake.clone());
        switch(&mut rt, None, Some("Cafe Guest"), Some("wrong-pass")).unwrap();
        let op = wait(&mut rt);
        assert_eq!(op["step"], "wrong_password");
        assert!(fake.stored.lock().unwrap().is_empty());
        // A stored password that stopped working is forgotten, so the sheet asks again.
        fake.stored
            .lock()
            .unwrap()
            .insert("Home".into(), "old-pass".into());
        rt.shared
            .state
            .lock()
            .unwrap()
            .remembered
            .insert("Home".into());
        fake.results
            .lock()
            .unwrap()
            .push_back(Join::Refused("-3924".into()));
        switch(&mut rt, None, Some("Home"), None).unwrap();
        assert_eq!(wait(&mut rt)["step"], "wrong_password");
        assert_eq!(fake.forgotten.lock().unwrap().as_slice(), ["Home"]);
        assert!(!rt.shared.state.lock().unwrap().remembered.contains("Home"));
    }

    #[test]
    fn a_phone_is_woken_then_joined_with_the_password_it_gave_and_never_stored() {
        let fake = Arc::new(Fake::default());
        *fake.ip.lock().unwrap() = Some("172.20.10.12".into());
        fake.reachable.store(true, Ordering::Release);
        // The hotspot takes a moment to appear after it was woken.
        fake.results
            .lock()
            .unwrap()
            .extend([Join::NotFound, Join::Joined]);
        let mut rt = runtime(fake.clone());
        switch(&mut rt, Some("我的 iPhone"), None, None).unwrap();
        let op = wait(&mut rt);
        assert_eq!(op["step"], "done", "{op}");
        assert_eq!(fake.woken.load(Ordering::Acquire), 1);
        assert_eq!(fake.joins.lock().unwrap().len(), 2);
        assert_eq!(
            fake.joins.lock().unwrap()[1].1.as_deref(),
            Some("phone-secret")
        );
        assert!(fake.stored.lock().unwrap().is_empty());
        assert_eq!(rt.snapshot()["current"]["kind"], "hotspot");
    }

    #[test]
    fn the_gate_holds_new_requests_until_the_switch_ends_and_waits_for_those_in_flight() {
        let fake = Arc::new(Fake::default());
        *fake.ip.lock().unwrap() = Some("192.168.1.9".into());
        fake.reachable.store(true, Ordering::Release);
        let mut rt = runtime(fake.clone());
        assert_eq!(rt.mod_event(&event("s1", "begin", "r1")), ModReply::Go);
        rt.shared
            .state
            .lock()
            .unwrap()
            .remembered
            .insert("Home".into());
        fake.stored
            .lock()
            .unwrap()
            .insert("Home".into(), "home-pass".into());
        switch(&mut rt, None, Some("Home"), None).unwrap();
        // A new request of another session is held; the one in flight keeps the switch waiting.
        assert_eq!(rt.mod_event(&event("s2", "begin", "r2")), ModReply::Hold);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(rt.snapshot()["operation"]["step"], "waiting");
        assert!(fake.joins.lock().unwrap().is_empty());
        let snap = rt.snapshot();
        let states: Vec<&str> = snap["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["state"].as_str().unwrap())
            .collect();
        assert!(
            states.contains(&"live") && states.contains(&"held"),
            "{snap}"
        );
        // The reply ends: the switch goes on, and the held session may send again afterwards.
        assert_eq!(rt.mod_event(&event("s1", "end", "r1")), ModReply::Go);
        assert_eq!(wait(&mut rt)["step"], "done");
        assert_eq!(rt.mod_event(&event("s2", "begin", "r2")), ModReply::Go);
    }

    #[test]
    fn a_request_without_heartbeat_does_not_keep_the_switch_waiting_forever() {
        let fake = Arc::new(Fake::default());
        let rt = runtime(fake);
        rt.mod_event(&event("s1", "begin", "r1"));
        let later = Instant::now() + HEARTBEAT_STALE + Duration::from_secs(1);
        assert!(!quiet(&rt.shared.state.lock().unwrap(), Instant::now()));
        assert!(quiet(&rt.shared.state.lock().unwrap(), later));
    }

    #[test]
    fn waiting_gives_up_at_the_deadline_and_cancel_leaves_the_network_alone() {
        let fake = Arc::new(Fake::default());
        let mut rt = runtime(fake.clone());
        rt.mod_event(&event("s1", "begin", "r1"));
        rt.shared
            .state
            .lock()
            .unwrap()
            .remembered
            .insert("Home".into());
        switch(&mut rt, None, Some("Home"), None).unwrap();
        rt.shared
            .state
            .lock()
            .unwrap()
            .operation
            .as_mut()
            .unwrap()
            .deadline = Instant::now();
        let op = wait(&mut rt);
        assert_eq!(op["step"], "timeout");
        assert!(fake.joins.lock().unwrap().is_empty());
        // Gate open again.
        assert_eq!(rt.mod_event(&event("s2", "begin", "r2")), ModReply::Go);
        switch(&mut rt, None, Some("Home"), None).unwrap();
        rt.command(Command {
            action: "cancel",
            phone: None,
            ssid: None,
            password: None,
        })
        .unwrap();
        assert_eq!(wait(&mut rt)["step"], "cancelled");
        assert!(fake.joins.lock().unwrap().is_empty());
        assert_eq!(rt.snapshot()["history"][0]["result"], "cancelled");
    }

    #[test]
    fn without_a_traffic_reading_taken_after_the_start_nothing_is_switched() {
        let fake = Arc::new(Fake::default());
        let mut rt = runtime(fake.clone());
        rt.shared.monitored.store(true, Ordering::Release);
        // A reading from before the switch began does not count.
        rt.shared.state.lock().unwrap().sampled_at = Some(Instant::now());
        rt.shared
            .state
            .lock()
            .unwrap()
            .remembered
            .insert("Home".into());
        switch(&mut rt, None, Some("Home"), None).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(rt.snapshot()["operation"]["step"], "waiting");
        rt.shared
            .state
            .lock()
            .unwrap()
            .operation
            .as_mut()
            .unwrap()
            .deadline = Instant::now();
        assert_eq!(wait(&mut rt)["step"], "timeout");
        assert!(fake.joins.lock().unwrap().is_empty());
        // With a fresh quiet reading it goes ahead.
        *fake.ip.lock().unwrap() = Some("192.168.1.20".into());
        fake.stored
            .lock()
            .unwrap()
            .insert("Home".into(), "home-pass".into());
        switch(&mut rt, None, Some("Home"), None).unwrap();
        std::thread::sleep(Duration::from_millis(1000));
        rt.shared.state.lock().unwrap().sampled_at = Some(Instant::now());
        assert_eq!(wait(&mut rt)["step"], "unreachable");
        assert_eq!(fake.joins.lock().unwrap().len(), 1);
    }

    #[test]
    fn traffic_counts_only_ai_hosts_and_only_after_a_baseline() {
        let first = br#"{"connections":[
          {"id":"a","upload":100,"download":1000,"metadata":{"host":"api.anthropic.com","process":"Codex (Service)"}},
          {"id":"b","upload":10,"download":10,"metadata":{"host":"github.com","process":"Codex (Service)"}},
          {"id":"c","upload":5,"download":5,"metadata":{"host":"","sniffHost":"ws.chatgpt.com","process":"Claude Helper"}}]}"#;
        let second = br#"{"connections":[
          {"id":"a","upload":100,"download":9000,"metadata":{"host":"api.anthropic.com","process":"Codex (Service)"}},
          {"id":"b","upload":10,"download":99999,"metadata":{"host":"github.com","process":"Codex (Service)"}},
          {"id":"c","upload":5,"download":60,"metadata":{"host":"","sniffHost":"ws.chatgpt.com","process":"Claude Helper"}}]}"#;
        let mut previous = HashMap::new();
        deltas(&mut previous, parse_mihomo(first).unwrap());
        let sample = deltas(&mut previous, parse_mihomo(second).unwrap());
        assert_eq!(sample.get("Codex 桌面"), Some(&(0, 8000)));
        assert_eq!(sample.get("Claude 桌面"), Some(&(0, 55)));
        let mut state = new_state(Some("en0".into()));
        let now = Instant::now();
        apply_sample(&mut state, &sample, now);
        assert!(!quiet(&state, now));
        assert!(quiet(&state, now + QUIET));
        let names: Vec<String> = sources(&state, now)
            .into_iter()
            .filter(|s| s.1 == "live")
            .map(|s| s.0)
            .collect();
        assert_eq!(names, ["Codex 桌面"]);
    }

    #[test]
    fn nettop_lines_keep_only_the_ai_clients() {
        let text = ",bytes_in,bytes_out,\nlaunchd.1,0,0,\nclaude.15998,6590,24384,\nCodex (Service).46063,163928520,177357,\nGoogle Chrome Helper.9,5,5,\nclaude.helper.7,1,1,\n";
        let readings = parse_nettop(text);
        assert_eq!(readings.len(), 2);
        assert_eq!(
            readings[0],
            ("claude.15998".into(), "Claude Code".into(), 24384, 6590)
        );
        assert_eq!(readings[1].1, "Codex 桌面");
    }

    #[test]
    fn networksetup_output_is_read_the_way_it_prints_it() {
        assert_eq!(parse_join(""), Join::Joined);
        assert_eq!(
            parse_join("Could not find network Lab iPhone.\n"),
            Join::NotFound
        );
        assert_eq!(
            parse_join(
                "Failed to join network Lab iPhone.\nError: -3900  The operation couldn’t be completed. tmpErr\n"
            ),
            Join::Refused("-3900".into())
        );
        assert_eq!(
            parse_hardware_ports(
                "Hardware Port: Ethernet\nDevice: en1\n\nHardware Port: Wi-Fi\nDevice: en0\nEthernet Address: x\n"
            ),
            Some("en0".into())
        );
        assert_eq!(
            parse_preferred("Preferred networks on en0:\n\tStudio-5G\n\tHome 2.4G\n"),
            ["Studio-5G", "Home 2.4G"]
        );
        assert_eq!(
            parse_phones(
                r#"{"devices":[{"name":"我的 iPhone","battery":62,"network":"5G","signal":3},{"name":""}]}"#
            ),
            [Phone {
                name: "我的 iPhone".into(),
                battery: Some(62),
                network: Some("5G".into()),
                signal: Some(3)
            }]
        );
        assert_eq!(vault_id("Cafe"), "wifi-43616665");
        assert!(vault_id(&"x".repeat(32)).len() <= 80);
    }

    #[test]
    fn history_and_remembered_networks_survive_a_restart_without_secrets() {
        let root = std::env::temp_dir().join(format!(
            "network-switch-{}",
            crate::platform::random_id().unwrap()
        ));
        crate::platform::private_dir(&root).unwrap();
        let fake = Arc::new(Fake::default());
        *fake.ip.lock().unwrap() = Some("192.168.1.9".into());
        fake.reachable.store(true, Ordering::Release);
        let mut rt = Runtime::with_sys(Some(&root), fake.clone());
        switch(&mut rt, None, Some("Cafe Guest"), Some("guest-pass")).unwrap();
        assert_eq!(wait(&mut rt)["step"], "done");
        let again = Runtime::with_sys(Some(&root), fake);
        let snap = again.snapshot();
        assert_eq!(snap["history"][0]["to"], "Cafe Guest");
        assert!(
            again
                .shared
                .state
                .lock()
                .unwrap()
                .remembered
                .contains("Cafe Guest")
        );
        for file in ["network.json", "network-log.jsonl"] {
            let text = std::fs::read_to_string(root.join(file)).unwrap();
            assert!(!text.contains("guest-pass"), "{file}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn switch_requests_are_checked_before_anything_starts() {
        let fake = Arc::new(Fake::default());
        let mut rt = runtime(fake);
        assert_eq!(switch(&mut rt, None, None, None), Err("要切到的网络无效"));
        assert_eq!(
            switch(&mut rt, Some("A"), Some("B"), None),
            Err("要切到的网络无效")
        );
        assert_eq!(
            switch(&mut rt, None, Some("Home"), Some("short")),
            Err("Wi-Fi 密码应为 8 到 63 个字符")
        );
        assert!(rt.snapshot()["operation"].is_null());
    }
}
