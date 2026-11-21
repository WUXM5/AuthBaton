//! Quota for a Claude desktop sign-in that has no AuthBaton login of its own.
//!
//! The person turns this on, and confirms. We then read the "Claude Safe Storage" keychain item
//! once, keep only the derived AES key in this process, and decrypt the desktop's OAuth token
//! cache (never cookies, never `sessionKey`). A still-valid Claude Code access token is used for
//! one read of the usage endpoint AuthBaton already uses. Nothing is refreshed, claimed, or
//! written back. Turning the switch off, or stopping the service, drops the key.
use crate::claude_resets::USAGE_URL;
use crate::quota::{self, Window};
use base64::Engine;
use claude_desktop::api::LoginState;
use claude_desktop::{Ctx, Paths};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CLAUDE_CODE_CLIENT: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const BASE_SCOPE: &str = "user:inference user:file_upload user:profile";
/// The Code tab's Claude Code login adds this to the base scope. A new account gets it from its first
/// Code-mode conversation; the base-scope one may never come, and Claude 桌面 itself falls back to it
/// for the same usage read.
const SESSIONS_SCOPE: &str = "user:sessions:claude_code";
/// Do not send an access token that is about to expire; the query does not refresh it.
const MIN_REMAINING: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(300);
const LIMITED_WAIT: Duration = Duration::from_secs(900);
const UNAUTHORIZED_WAIT: Duration = Duration::from_secs(6 * 3600);
const STAGGER: Duration = Duration::from_secs(20);
const FILE_LIMIT: usize = 1024 * 1024;
const SERVICE_NAME: &str = "Claude Safe Storage";

/// The only request this module is allowed to send.
const USAGE_GET: Call = Call {
    method: "GET",
    url: USAGE_URL,
    body: false,
};

#[derive(Clone, Copy)]
struct Call {
    method: &'static str,
    url: &'static str,
    body: bool,
}

fn usage_call_allowed(call: Call) -> bool {
    call.method == "GET"
        && !call.body
        && call.url == USAGE_URL
        && !call.url.contains("reset_rate_limits")
        && !call.url.contains("/v1/messages")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Off,
    WaitingKey,
    KeyDenied,
    Ready,
    Error,
}

struct KeyMaterial([u8; 16]);

impl Drop for KeyMaterial {
    fn drop(&mut self) {
        zero(&mut self.0);
    }
}

impl KeyMaterial {
    fn copy(&self) -> KeyMaterial {
        KeyMaterial(self.0)
    }
}

struct AccountRow {
    state: &'static str,
    message: &'static str,
    checked_at: Option<u64>,
    windows: Vec<Window>,
    /// Ciphertext hash at the last 401/403, so a desktop refresh can be tried again.
    blocked_hash: Option<[u8; 32]>,
    retry_at: Option<Instant>,
    unauthorized: bool,
}

struct State {
    enabled: bool,
    phase: Phase,
    message: &'static str,
    key: Option<KeyMaterial>,
    generation: u64,
    rows: BTreeMap<String, AccountRow>,
    due: BTreeMap<String, Instant>,
}

struct Shared {
    state: Mutex<State>,
    cancel: AtomicBool,
    fetches: AtomicU64,
}

pub struct Quota {
    shared: Arc<Shared>,
    job: Option<JoinHandle<()>>,
    reader: Reader,
    fetch: Fetch,
}

#[derive(Clone, Copy)]
enum Reader {
    /// The real keychain. Never used by tests.
    System,
    /// Tests: do not spawn `security`.
    Inert,
}

#[derive(Clone, Copy)]
enum Fetch {
    Production,
    #[cfg(test)]
    Test,
}

enum KeyFail {
    Denied,
    Missing,
    Stopped,
    Error,
}

enum Job {
    Key {
        generation: u64,
    },
    Query {
        generation: u64,
        account: String,
        file: PathBuf,
    },
}

impl Quota {
    pub fn system() -> Quota {
        Quota::new(Reader::System, Fetch::Production)
    }

    /// Tests and a service that never started the desktop engine. Does not touch the keychain.
    pub fn inert() -> Quota {
        Quota::new(Reader::Inert, Fetch::Production)
    }

    fn new(reader: Reader, fetch: Fetch) -> Quota {
        Quota {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    enabled: false,
                    phase: Phase::Off,
                    message: OFF_MESSAGE,
                    key: None,
                    generation: 0,
                    rows: BTreeMap::new(),
                    due: BTreeMap::new(),
                }),
                cancel: AtomicBool::new(false),
                fetches: AtomicU64::new(0),
            }),
            job: None,
            reader,
            fetch,
        }
    }

    #[cfg(test)]
    fn testing() -> Quota {
        Quota::new(Reader::Inert, Fetch::Test)
    }

    #[cfg(test)]
    fn install_key(&mut self, key: [u8; 16]) {
        let mut state = self.shared.state.lock().unwrap();
        state.key = Some(KeyMaterial(key));
        state.phase = Phase::Ready;
        state.message = READY_MESSAGE;
        state.generation = state.generation.wrapping_add(1);
    }

    pub fn shutdown(&mut self) {
        self.shared.cancel.store(true, Ordering::Release);
        self.clear_secret();
    }

    /// Apply the persisted switch. Turning it off drops the key and every reading.
    pub fn set_enabled(&mut self, on: bool) {
        {
            let mut state = self.shared.state.lock().unwrap();
            if state.enabled == on && (!on || state.phase != Phase::Off) {
                return;
            }
            state.enabled = on;
            if !on {
                state.generation = state.generation.wrapping_add(1);
                state.phase = Phase::Off;
                state.message = OFF_MESSAGE;
                state.rows.clear();
                state.due.clear();
                if let Some(key) = state.key.take() {
                    drop(key);
                }
            }
        }
        if on {
            self.shared.cancel.store(false, Ordering::Release);
            self.begin_key_read();
        }
    }

    /// The person asked again after a refusal, or after a token the usage endpoint rejected.
    pub fn retry(&mut self) {
        let enabled = {
            let mut state = self.shared.state.lock().unwrap();
            if !state.enabled {
                return;
            }
            if state.phase == Phase::KeyDenied || state.key.is_none() {
                state.phase = Phase::Off;
                true
            } else {
                for row in state.rows.values_mut() {
                    row.unauthorized = false;
                    row.blocked_hash = None;
                    row.retry_at = None;
                }
                state.due.clear();
                false
            }
        };
        if enabled {
            self.shared.cancel.store(false, Ordering::Release);
            self.begin_key_read();
        }
    }

    pub fn poll(&mut self, ctx: &Ctx, accounts: &[Candidate], linked: impl Fn(&str) -> bool) {
        self.join_finished();
        if !self.shared.state.lock().unwrap().enabled {
            return;
        }
        if self.job.is_some() {
            return;
        }
        let ready = self.shared.state.lock().unwrap().phase == Phase::Ready;
        if !ready {
            if self.shared.state.lock().unwrap().phase == Phase::Off {
                self.begin_key_read();
            }
            return;
        }
        let now = Instant::now();
        let mut chosen: Option<(String, PathBuf)> = None;
        for (index, account) in accounts.iter().enumerate() {
            if !is_eligible(account.known, account.login, linked(&account.id)) {
                continue;
            }
            let Some(file) = cache_file(&ctx.paths, &account.id, account.login) else {
                continue;
            };
            // A rejected token is tried again when Desktop writes a new cache, without
            // waiting out the six hours.
            if self.cache_changed(&account.id, &file) {
                chosen = Some((account.id.clone(), file));
                break;
            }
            let due = self
                .shared
                .state
                .lock()
                .unwrap()
                .due
                .get(&account.id)
                .copied();
            let wait = due.unwrap_or_else(|| now + STAGGER * index as u32);
            if now < wait {
                if due.is_none() {
                    self.shared
                        .state
                        .lock()
                        .unwrap()
                        .due
                        .insert(account.id.clone(), wait);
                }
                continue;
            }
            if !self.may_query(&account.id, &file, now) {
                continue;
            }
            chosen = Some((account.id.clone(), file));
            break;
        }
        if let Some((account, file)) = chosen {
            let generation = self.shared.state.lock().unwrap().generation;
            self.spawn(Job::Query {
                generation,
                account,
                file,
            });
        }
    }

    pub fn top(&self) -> Value {
        let state = self.shared.state.lock().unwrap();
        json!({
            "enabled": state.enabled,
            "state": phase_name(state.phase),
            "message": state.message,
        })
    }

    pub fn account_field(&self, account: &Candidate, linked: bool) -> Option<Value> {
        let state = self.shared.state.lock().unwrap();
        if !state.enabled || !is_eligible(account.known, account.login, linked) {
            return None;
        }
        let row = state.rows.get(&account.id);
        let (name, message, checked_at, windows) = match (&state.phase, row) {
            (Phase::WaitingKey, _) => ("checking", WAITING_ACCOUNT, None, Vec::new()),
            (Phase::KeyDenied, _) => ("error", DENIED_ACCOUNT, None, Vec::new()),
            (Phase::Error, _) => ("error", state.message, None, Vec::new()),
            (Phase::Ready, Some(row)) => {
                (row.state, row.message, row.checked_at, row.windows.clone())
            }
            (Phase::Ready, None) => ("checking", "正在查询", None, Vec::new()),
            _ => return None,
        };
        Some(json!({
            "state": name,
            "message": message,
            "checked_at": checked_at,
            "windows": windows,
        }))
    }

    fn begin_key_read(&mut self) {
        self.join_finished();
        if self.job.is_some() {
            return;
        }
        let generation = {
            let mut state = self.shared.state.lock().unwrap();
            if !state.enabled || state.key.is_some() || state.phase == Phase::KeyDenied {
                return;
            }
            state.phase = Phase::WaitingKey;
            state.message = WAITING_MESSAGE;
            state.generation
        };
        match self.reader {
            Reader::Inert => {
                let mut state = self.shared.state.lock().unwrap();
                if state.generation == generation && state.key.is_none() {
                    state.phase = Phase::KeyDenied;
                    state.message = DENIED_MESSAGE;
                }
            }
            Reader::System => self.spawn(Job::Key { generation }),
        }
    }

    fn spawn(&mut self, job: Job) {
        if let Job::Query { ref account, .. } = job {
            let mut state = self.shared.state.lock().unwrap();
            let row = state
                .rows
                .entry(account.clone())
                .or_insert_with(AccountRow::checking);
            row.state = "checking";
            row.message = "正在查询";
        }
        let shared = self.shared.clone();
        let fetch = self.fetch;
        self.job = Some(std::thread::spawn(move || run_job(job, &shared, fetch)));
    }

    fn cache_changed(&self, account: &str, file: &Path) -> bool {
        let state = self.shared.state.lock().unwrap();
        let Some(row) = state.rows.get(account) else {
            return false;
        };
        row.unauthorized && cipher_hash(file).is_some_and(|hash| Some(hash) != row.blocked_hash)
    }

    fn may_query(&self, account: &str, file: &Path, now: Instant) -> bool {
        let state = self.shared.state.lock().unwrap();
        let Some(row) = state.rows.get(account) else {
            return true;
        };
        if row.unauthorized {
            let hash = cipher_hash(file);
            if hash.is_some() && hash == row.blocked_hash {
                return row.retry_at.is_none_or(|at| now >= at);
            }
        }
        row.retry_at.is_none_or(|at| now >= at)
    }

    fn clear_secret(&mut self) {
        self.shared.cancel.store(true, Ordering::Release);
        if let Some(handle) = self.job.take() {
            let _ = handle.join();
        }
        let mut state = self.shared.state.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        if let Some(key) = state.key.take() {
            drop(key);
        }
        state.rows.clear();
        state.due.clear();
    }
}

impl Quota {
    fn reap_job(&mut self) {
        if self.job.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(handle) = self.job.take()
        {
            let _ = handle.join();
        }
    }
}

// The first reap is a no-op so snapshot can call it on &self. Finished work is joined from tick,
// which has &mut. The worker writes its result before exiting, so a snapshot taken after tick
// sees it. A snapshot taken while the worker is still in HTTP shows "checking".
impl Quota {
    pub fn join_finished(&mut self) {
        self.reap_job();
    }

    #[cfg(test)]
    fn wait_for_job(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.job.is_some() {
            self.join_finished();
            if self.job.is_none() {
                break;
            }
            assert!(Instant::now() < deadline, "quota job did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[derive(Clone)]
pub struct Candidate {
    pub id: String,
    pub known: bool,
    pub login: LoginState,
}

pub fn is_eligible(known: bool, login: LoginState, linked: bool) -> bool {
    known && !linked && matches!(login, LoginState::SignedIn | LoginState::Saved)
}

fn cache_file(paths: &Paths, account: &str, login: LoginState) -> Option<PathBuf> {
    match login {
        // The open sign-in lives in Claude's own config. A set-aside sign-in was moved, still
        // encrypted, to the engine's logins folder. Same layout as `logins::saved_dir`.
        LoginState::SignedIn => Some(paths.desktop_config()),
        LoginState::Saved => {
            let path = paths
                .state_dir
                .join("logins")
                .join(account)
                .join("config.json");
            path.is_file().then_some(path)
        }
        LoginState::Unknown => None,
    }
}

fn run_job(job: Job, shared: &Shared, fetch: Fetch) {
    match job {
        Job::Key { generation } => {
            if shared.cancel.load(Ordering::Acquire) {
                return;
            }
            match read_keychain(&shared.cancel) {
                Ok(key) => {
                    let mut state = shared.state.lock().unwrap();
                    if state.enabled
                        && state.generation == generation
                        && !shared.cancel.load(Ordering::Acquire)
                    {
                        state.key = Some(key);
                        state.phase = Phase::Ready;
                        state.message = READY_MESSAGE;
                    }
                }
                Err(KeyFail::Denied) | Err(KeyFail::Stopped) => {
                    let mut state = shared.state.lock().unwrap();
                    if state.enabled && state.generation == generation {
                        state.phase = Phase::KeyDenied;
                        state.message = DENIED_MESSAGE;
                    }
                }
                Err(KeyFail::Missing) => {
                    fail_phase(shared, generation, "这台 Mac 上没有 Claude 的钥匙串项")
                }
                Err(KeyFail::Error) => fail_phase(shared, generation, "读取钥匙串失败"),
            }
        }
        Job::Query {
            generation,
            account,
            file,
        } => {
            let key = {
                let state = shared.state.lock().unwrap();
                if !state.enabled || state.generation != generation {
                    return;
                }
                state.key.as_ref().map(KeyMaterial::copy)
            };
            let Some(key) = key else { return };
            let hash = cipher_hash(&file);
            let outcome = query_file(&key, &file, &account, fetch, &shared.fetches);
            drop(key);
            let mut state = shared.state.lock().unwrap();
            if !state.enabled || state.generation != generation {
                return;
            }
            let now = Instant::now();
            let row = state
                .rows
                .entry(account.clone())
                .or_insert_with(AccountRow::checking);
            row.checked_at = Some(unix_secs());
            match outcome {
                Query::Ready(windows) => {
                    row.state = "ready";
                    row.message = "已从桌面登录读到额度";
                    row.windows = windows;
                    row.unauthorized = false;
                    row.blocked_hash = None;
                    row.retry_at = None;
                    state.due.insert(account, now + POLL);
                }
                Query::NoToken => {
                    row.state = "no_token";
                    row.message = "这个账号还没在 Claude 桌面用过 Code 模式";
                    row.windows.clear();
                    row.unauthorized = false;
                    row.retry_at = None;
                    state.due.insert(account, now + POLL);
                }
                Query::Unauthorized => {
                    row.state = "unauthorized";
                    row.message = "额度接口没接受桌面登录里的令牌，AuthBaton 不会替桌面续期";
                    row.windows.clear();
                    row.unauthorized = true;
                    row.blocked_hash = hash;
                    row.retry_at = Some(now + UNAUTHORIZED_WAIT);
                    state.due.insert(account, now + UNAUTHORIZED_WAIT);
                }
                Query::Limited => {
                    row.state = "limited";
                    row.message = "额度查询被限流，15 分钟后再试";
                    row.retry_at = Some(now + LIMITED_WAIT);
                    state.due.insert(account, now + LIMITED_WAIT);
                }
                Query::Error(message) => {
                    row.state = "error";
                    row.message = message;
                    row.retry_at = Some(now + POLL);
                    state.due.insert(account, now + POLL);
                }
            }
        }
    }
}

fn fail_phase(shared: &Shared, generation: u64, message: &'static str) {
    let mut state = shared.state.lock().unwrap();
    if state.enabled && state.generation == generation {
        state.phase = Phase::Error;
        state.message = message;
    }
}

enum Query {
    Ready(Vec<Window>),
    NoToken,
    Unauthorized,
    Limited,
    Error(&'static str),
}

fn query_file(
    key: &KeyMaterial,
    file: &Path,
    account: &str,
    fetch: Fetch,
    fetches: &AtomicU64,
) -> Query {
    let Some(token) = access_from_file(key, file, account) else {
        return Query::NoToken;
    };
    if !usage_call_allowed(USAGE_GET) {
        return Query::Error("不允许的请求");
    }
    fetches.fetch_add(1, Ordering::Relaxed);
    let result = match fetch {
        Fetch::Production => send_usage(&token),
        #[cfg(test)]
        Fetch::Test => test_fetch(&token),
    };
    let mut bytes = token.into_bytes();
    zero(&mut bytes);
    match result {
        Ok((200, body)) => match quota::parse_claude_usage(&body) {
            Ok(windows) => Query::Ready(windows),
            Err(_) => Query::Error("没有读到额度窗口"),
        },
        Ok((401 | 403, _)) => Query::Unauthorized,
        Ok((429, _)) => Query::Limited,
        Ok(_) => Query::Error("额度暂时读不到"),
        Err(message) => Query::Error(message),
    }
}

fn send_usage(token: &str) -> Result<(u16, Value), &'static str> {
    // Body stays absent, so this is a GET. The guard above is the regression check.
    crate::maintenance::http(USAGE_GET.url, Some(token), None, true).map_err(|message| {
        if crate::network::not_sent(message) {
            "网络没有连上，稍后再试"
        } else {
            "额度暂时读不到"
        }
    })
}

fn access_from_file(key: &KeyMaterial, file: &Path, account: &str) -> Option<String> {
    let bytes = read_limited(file)?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    if let Some(owner) = value.get("lastKnownAccountUuid").and_then(Value::as_str)
        && owner != account
    {
        return None;
    }
    let now = unix_millis();
    let mut best: Option<(Rank, String)> = None;
    for name in ["oauth:tokenCacheV2", "oauth:tokenCache"] {
        let Some(encoded) = value.get(name).and_then(Value::as_str) else {
            continue;
        };
        let Ok(blob) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
            continue;
        };
        let Ok(plain) = decrypt_v10(&key.0, &blob) else {
            continue;
        };
        let parsed = serde_json::from_slice::<Value>(&plain).ok();
        let mut plain = plain;
        zero(&mut plain);
        let Some(object) = parsed.and_then(|v| v.as_object().cloned()) else {
            continue;
        };
        for (cache_key, entry) in object {
            let Some(rank) = candidate(&cache_key, &entry, account, now) else {
                continue;
            };
            let Some(token) = entry.get("token").and_then(Value::as_str) else {
                continue;
            };
            if best.as_ref().is_none_or(|(have, _)| rank > *have) {
                best = Some((rank, token.to_string()));
            }
        }
    }
    best.map(|(_, token)| token)
}

/// How a usable access compares: a base-scope one before a Code tab one, as Claude 桌面 picks
/// for its own usage read, then the later expiry.
type Rank = (bool, u64);

fn candidate(key: &str, entry: &Value, account: &str, now: u64) -> Option<Rank> {
    let (client, scope, owner) = parse_cache_key(key)?;
    if client != CLAUDE_CODE_CLIENT {
        return None;
    }
    let base = code_scope(scope)?;
    if let Some(owner) = owner
        && owner != account
    {
        return None;
    }
    let exp = as_millis(entry.get("expiresAt").or_else(|| entry.get("expires_at"))?)?;
    (exp > now.saturating_add(MIN_REMAINING.as_millis() as u64)).then_some((base, exp))
}

/// Claude Code's base scope (true) or the Code tab's, base plus sessions (false). Compared as sets:
/// Claude 桌面 writes the same words in different orders for different uses.
fn code_scope(scope: &str) -> Option<bool> {
    let words = |text: &str| {
        let mut list: Vec<String> = text.split(' ').filter(|w| !w.is_empty()).map(str::to_owned).collect();
        list.sort_unstable();
        list.dedup();
        list
    };
    let have = words(scope);
    if have == words(BASE_SCOPE) {
        return Some(true);
    }
    (have == words(&format!("{BASE_SCOPE} {SESSIONS_SCOPE}"))).then_some(false)
}

fn parse_cache_key(key: &str) -> Option<(&str, &str, Option<&str>)> {
    let (owner, rest) = if let Some(rest) = key.strip_prefix("acct:") {
        let (owner, rest) = rest.split_once('|')?;
        (Some(owner), rest)
    } else {
        (None, key)
    };
    let (client, rest) = rest.split_once(':')?;
    let (_org, rest) = rest.split_once(':')?;
    let scope = rest
        .split_once(':')?
        .1
        .split_once(':')
        .map(|(_, scope)| scope)
        .or_else(|| {
            // `https://host:scope` — the host contains one colon in the scheme.
            let host = rest
                .strip_prefix("https://")
                .or_else(|| rest.strip_prefix("http://"))?;
            Some(host.split_once(':')?.1)
        })?;
    Some((client, scope, owner))
}

fn cipher_hash(file: &Path) -> Option<[u8; 32]> {
    let bytes = read_limited(file)?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let mut hasher = Sha256::new();
    let mut any = false;
    for name in ["oauth:tokenCacheV2", "oauth:tokenCache"] {
        if let Some(text) = value.get(name).and_then(Value::as_str) {
            hasher.update(name.as_bytes());
            hasher.update([0]);
            hasher.update(text.as_bytes());
            any = true;
        }
    }
    any.then(|| hasher.finalize().into())
}

fn read_limited(path: &Path) -> Option<Vec<u8>> {
    let mut file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(FILE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= FILE_LIMIT).then_some(bytes)
}

fn read_keychain(cancel: &AtomicBool) -> Result<KeyMaterial, KeyFail> {
    // Drop the service environment so a token in it cannot reach `security`. HOME and USER
    // stay: without them the login keychain cannot be found and the allow dialog never appears.
    let mut command = Command::new("/usr/bin/security");
    command
        .env_clear()
        .args(["find-generic-password", "-s", SERVICE_NAME, "-w"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    if let Some(user) = std::env::var_os("USER") {
        command.env("USER", user);
    }
    let mut child = command.spawn().map_err(|_| KeyFail::Error)?;
    // No timeout: killing `security` leaves its two keychain dialogs on screen, and an "Allow" the
    // person clicks there later reaches no one. Wait for their answer; stop only when turned off.
    let status = loop {
        if cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(KeyFail::Stopped);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(KeyFail::Error);
            }
        }
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_end(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_end(&mut stderr);
    }
    if status.code() != Some(0) {
        zero(&mut stdout);
        let kind = classify_key_failure(status.code(), &stderr);
        zero(&mut stderr);
        return Err(kind);
    }
    zero(&mut stderr);
    if stdout.last() == Some(&b'\n') {
        stdout.pop();
    }
    if stdout.last() == Some(&b'\r') {
        stdout.pop();
    }
    if stdout.is_empty() {
        return Err(KeyFail::Error);
    }
    let derived = derive_key(&stdout);
    zero(&mut stdout);
    derived.map(KeyMaterial).map_err(|_| KeyFail::Error)
}

fn classify_key_failure(code: Option<i32>, stderr: &[u8]) -> KeyFail {
    if code == Some(44) {
        return KeyFail::Missing;
    }
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if text.contains("cancel") || text.contains("denied") || text.contains("interaction") {
        KeyFail::Denied
    } else {
        KeyFail::Error
    }
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Off => "off",
        Phase::WaitingKey => "waiting_key",
        Phase::KeyDenied => "key_denied",
        Phase::Ready => "ready",
        Phase::Error => "error",
    }
}

impl AccountRow {
    fn checking() -> AccountRow {
        AccountRow {
            state: "checking",
            message: "正在查询",
            checked_at: None,
            windows: Vec::new(),
            blocked_hash: None,
            retry_at: None,
            unauthorized: false,
        }
    }
}

const OFF_MESSAGE: &str = "关闭。打开后，只给没有 AuthBaton 登录的桌面账号查额度";
const WAITING_MESSAGE: &str = "正在等你允许 AuthBaton 读取 Claude 的钥匙串";
const DENIED_MESSAGE: &str = "没有允许读取钥匙串。点重试再问一次";
const READY_MESSAGE: &str = "已打开。只查没有 AuthBaton 登录的桌面账号";
const WAITING_ACCOUNT: &str = "正在等钥匙串允许";
const DENIED_ACCOUNT: &str = "还没有允许读取钥匙串";

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn as_millis(value: &Value) -> Option<u64> {
    let n = value
        .as_u64()
        .or_else(|| value.as_f64().map(|n| n as u64))?;
    if n == 0 {
        return None;
    }
    if n > 1_000_000_000_000 {
        Some(n)
    } else if n > 1_000_000_000 {
        Some(n.saturating_mul(1000))
    } else {
        None
    }
}

fn zero(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(Ordering::SeqCst);
}

fn derive_key(password: &[u8]) -> Result<[u8; 16], ()> {
    let mut key = [0u8; 16];
    let status = unsafe {
        CCKeyDerivationPBKDF(
            K_CC_PBKDF2,
            password.as_ptr(),
            password.len(),
            b"saltysalt".as_ptr(),
            b"saltysalt".len(),
            K_CC_PRF_HMAC_SHA1,
            1003,
            key.as_mut_ptr(),
            key.len(),
        )
    };
    if status != 0 {
        zero(&mut key);
        return Err(());
    }
    Ok(key)
}

fn decrypt_v10(key: &[u8; 16], blob: &[u8]) -> Result<Vec<u8>, ()> {
    if blob.len() < 3 + 16 || &blob[..3] != b"v10" || !(blob.len() - 3).is_multiple_of(16) {
        return Err(());
    }
    let cipher = &blob[3..];
    let mut out = vec![0u8; cipher.len()];
    let mut moved = 0usize;
    let iv = [b' '; 16];
    let status = unsafe {
        CCCrypt(
            K_CC_DECRYPT,
            K_CC_ALGORITHM_AES,
            K_CC_OPTION_PKCS7,
            key.as_ptr().cast(),
            key.len(),
            iv.as_ptr().cast(),
            cipher.as_ptr().cast(),
            cipher.len(),
            out.as_mut_ptr().cast(),
            out.len(),
            &mut moved,
        )
    };
    if status != 0 || moved > out.len() {
        zero(&mut out);
        return Err(());
    }
    out.truncate(moved);
    Ok(out)
}

#[cfg(test)]
fn encrypt_v10(key: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; plain.len() + 16];
    let mut moved = 0usize;
    let iv = [b' '; 16];
    let status = unsafe {
        CCCrypt(
            K_CC_ENCRYPT,
            K_CC_ALGORITHM_AES,
            K_CC_OPTION_PKCS7,
            key.as_ptr().cast(),
            key.len(),
            iv.as_ptr().cast(),
            plain.as_ptr().cast(),
            plain.len(),
            out.as_mut_ptr().cast(),
            out.len(),
            &mut moved,
        )
    };
    assert_eq!(status, 0);
    out.truncate(moved);
    let mut blob = b"v10".to_vec();
    blob.extend_from_slice(&out);
    blob
}

// CommonCrypto, via libSystem. Constants are the macOS headers' values:
// kCCDecrypt = 1, kCCAlgorithmAES = 0, kCCOptionPKCS7Padding = 1,
// kCCPBKDF2 = 2, kCCPRFHmacAlgSHA1 = 1.
#[cfg(test)]
const K_CC_ENCRYPT: u32 = 0;
const K_CC_DECRYPT: u32 = 1;
const K_CC_ALGORITHM_AES: u32 = 0;
const K_CC_OPTION_PKCS7: u32 = 1;
const K_CC_PBKDF2: u32 = 2;
const K_CC_PRF_HMAC_SHA1: u32 = 1;

#[link(name = "System")]
unsafe extern "C" {
    fn CCKeyDerivationPBKDF(
        algorithm: u32,
        password: *const u8,
        password_len: usize,
        salt: *const u8,
        salt_len: usize,
        prf: u32,
        rounds: u32,
        derived: *mut u8,
        derived_len: usize,
    ) -> i32;
    fn CCCrypt(
        op: u32,
        alg: u32,
        options: u32,
        key: *const u8,
        key_len: usize,
        iv: *const u8,
        data_in: *const u8,
        data_in_len: usize,
        data_out: *mut u8,
        data_out_len: usize,
        data_out_moved: *mut usize,
    ) -> i32;
}

#[cfg(test)]
fn test_fetch(_token: &str) -> Result<(u16, Value), &'static str> {
    TEST_CALLS.fetch_add(1, Ordering::SeqCst);
    let url = TEST_URL.lock().unwrap().clone();
    if url != USAGE_URL {
        return Err("拒绝的地址");
    }
    Ok((
        200,
        json!({
            "five_hour": {"utilization": 3, "resets_at": "2099-01-01T00:00:00Z"},
            "seven_day": {"utilization": 9, "resets_at": "2099-01-02T00:00:00Z"}
        }),
    ))
}

#[cfg(test)]
static TEST_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static TEST_URL: Mutex<String> = Mutex::new(String::new());

/// One real pass over this Mac's desktop sign-ins. Ignores the "already linked" rule so a
/// linked account's cached access can be checked once. Prints no secret.
#[cfg(test)]
pub fn live_probe() -> Result<(), &'static str> {
    eprintln!("waiting for keychain allow; click Allow, not Always Allow");
    let cancel = AtomicBool::new(false);
    let key = match read_keychain(&cancel) {
        Ok(key) => key,
        Err(KeyFail::Denied) | Err(KeyFail::Stopped) => {
            return Err("钥匙串被拒绝");
        }
        Err(KeyFail::Missing) => return Err("没有找到钥匙串项"),
        Err(KeyFail::Error) => return Err("读取钥匙串失败"),
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("没有 HOME")?;
    let claude = home.join("Library/Application Support/Claude/config.json");
    let logins = home.join("Library/Application Support/AuthManager/claude-desktop/logins");
    let mut accounts = Vec::new();
    if let Some(id) = owner_of(&claude) {
        accounts.push((id, claude.clone()));
    }
    if let Ok(entries) = fs::read_dir(&logins) {
        for entry in entries.flatten() {
            let path = entry.path().join("config.json");
            if !path.is_file() {
                continue;
            }
            let Some(id) = owner_of(&path).or_else(|| entry.file_name().into_string().ok()) else {
                continue;
            };
            if accounts.iter().any(|(have, _)| have == &id) {
                continue;
            }
            accounts.push((id, path));
        }
    }
    let directory = authbaton_directory();
    for (id, file) in accounts {
        let handle = crate::claude_desktop::handle_of(&id);
        let Some(token) = access_from_file(&key, &file, &id) else {
            println!("handle={handle} status=no_token");
            continue;
        };
        let result = send_usage(&token);
        let mut bytes = token.into_bytes();
        zero(&mut bytes);
        match result {
            Ok((status, body)) => {
                let windows = quota::parse_claude_usage(&body).unwrap_or_default();
                let summary = windows.iter().map(remaining).collect::<Vec<_>>().join(",");
                let compared = compare_authbaton(&directory, &handle);
                println!(
                    "handle={handle} status={status} remaining={summary} authbaton={compared}"
                );
            }
            Err(_) => println!("handle={handle} status=network"),
        }
    }
    Ok(())
}

#[cfg(test)]
fn owner_of(path: &Path) -> Option<String> {
    let bytes = read_limited(path)?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("lastKnownAccountUuid")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
fn remaining(window: &Window) -> String {
    let value = serde_json::to_value(window).unwrap_or(Value::Null);
    let label = value.get("label").and_then(Value::as_str).unwrap_or("?");
    let used = value
        .get("used_percent")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    format!("{label}:{}", (100.0 - used).clamp(0.0, 100.0))
}

#[cfg(test)]
fn authbaton_directory() -> Value {
    let sock = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/AuthManager/service.sock");
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&sock) else {
        return Value::Null;
    };
    let _ = std::io::Write::write_all(
        &mut stream,
        b"{\"version\":1,\"id\":\"desktop-quota\",\"method\":\"accounts.list\"}\n",
    );
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(&mut stream, &mut buf);
    serde_json::from_str(buf.lines().next().unwrap_or("")).unwrap_or(Value::Null)
}

#[cfg(test)]
fn compare_authbaton(directory: &Value, handle: &str) -> String {
    let dir = &directory["result"]["directory"];
    let rows = dir["claude_desktop"]["accounts"].as_array();
    let Some(row) = rows.and_then(|rows| rows.iter().find(|row| row["handle"] == handle)) else {
        return "unavailable".into();
    };
    if row["linked_account"].is_null() {
        return "not_linked".into();
    }
    let id = row["linked_account"].as_str().unwrap_or("");
    let account = dir["accounts"].as_array().and_then(|rows| {
        rows.iter()
            .find(|a| a["id"] == id && a["provider"] == "claude")
    });
    let Some(account) = account else {
        return "not_linked".into();
    };
    account["usage"]["windows"]
        .as_array()
        .map(|windows| {
            windows
                .iter()
                .filter(|w| w["scope"].is_null() && w["kind"].is_null())
                .map(|w| {
                    let used = w["used_percent"].as_f64().unwrap_or(0.0);
                    format!(
                        "{}:{}",
                        w["label"].as_str().unwrap_or("?"),
                        (100.0 - used).clamp(0.0, 100.0)
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| "no_windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v10_round_trip_matches_the_known_vector() {
        let password = b"synthetic-safe-storage";
        let key = derive_key(password).unwrap();
        let blob = base64::engine::general_purpose::STANDARD
            .decode("djEwpQ+HopiAaIUhVSZ5vAzoPO6Fy4DiuMVY2vrWF+900KTQ73mr0mYohm1BaF0K+mCn3q4XepkWyw10dlCuN0gDpw==")
            .unwrap();
        let plain = decrypt_v10(&key, &blob).unwrap();
        assert_eq!(
            plain,
            br#"{"token":"synthetic-access","expiresAt":2000000000000}"#
        );
        let again = encrypt_v10(&key, &plain);
        assert_eq!(decrypt_v10(&key, &again).unwrap(), plain);
    }

    #[test]
    fn selection_keeps_a_fresh_claude_code_access_base_scope_first() {
        let account = "aaaaaaaa-0000-4000-8000-000000000001";
        let now = 1_700_000_000_000u64;
        let fresh = now + 120_000;
        let soon = now + 30_000;
        let key = format!(
            "acct:{account}|{CLAUDE_CODE_CLIENT}:org:https://api.anthropic.com:{BASE_SCOPE}"
        );
        let sessions = format!("{key} {SESSIONS_SCOPE}");
        let desktop = "acct:aaaaaaaa-0000-4000-8000-000000000001|89355bc3-cbfd-4382-905b-976645cad410:org:https://api.anthropic.com:user:inference";
        let body = json!({
            &key: {"token": "keep-me", "expiresAt": fresh},
            &sessions: {"token": "sessions", "expiresAt": fresh + 1},
            "acct:other-account|9d1c250a-e61b-44d9-88ed-5944d1962f5e:org:https://api.anthropic.com:user:inference user:file_upload user:profile": {
                "token": "other", "expiresAt": fresh
            },
            desktop: {"token": "desktop", "expiresAt": fresh}
        });
        // The base scope wins over a later-expiring Code tab one; another account's and Claude 桌面's own never count.
        assert_eq!(chosen(&body, account, now).as_deref(), Some("keep-me"));
        let expiring = json!({&key: {"token": "soon", "expiresAt": soon}});
        assert!(chosen(&expiring, account, now).is_none());
        // A new account after its first Code-mode conversation: only the Code tab's, in any word order.
        let only_sessions = json!({&sessions: {"token": "sessions", "expiresAt": fresh}});
        assert_eq!(chosen(&only_sessions, account, now).as_deref(), Some("sessions"));
        let reordered = format!(
            "acct:{account}|{CLAUDE_CODE_CLIENT}:org:https://api.anthropic.com:{SESSIONS_SCOPE} user:profile user:inference user:file_upload"
        );
        let shuffled = json!({&reordered: {"token": "shuffled", "expiresAt": fresh}});
        assert_eq!(chosen(&shuffled, account, now).as_deref(), Some("shuffled"));
        // A chat-only account: only Claude 桌面's own login, so nothing to read with.
        let chat_only = json!({desktop: {"token": "desktop", "expiresAt": fresh}});
        assert!(chosen(&chat_only, account, now).is_none());
        let wider = format!("{key} {SESSIONS_SCOPE} user:office");
        assert!(chosen(&json!({&wider: {"token": "wider", "expiresAt": fresh}}), account, now).is_none());
    }

    fn chosen(body: &Value, account: &str, now: u64) -> Option<String> {
        let mut best = None;
        for (key, entry) in body.as_object().unwrap() {
            let Some(rank) = candidate(key, entry, account, now) else {
                continue;
            };
            let token = entry["token"].as_str().unwrap().to_string();
            if best.as_ref().is_none_or(|(have, _)| rank > *have) {
                best = Some((rank, token));
            }
        }
        best.map(|(_, token)| token)
    }

    #[test]
    fn a_linked_account_is_not_eligible() {
        assert!(is_eligible(true, LoginState::Saved, false));
        assert!(is_eligible(true, LoginState::SignedIn, false));
        assert!(!is_eligible(true, LoginState::Saved, true));
        assert!(!is_eligible(false, LoginState::Saved, false));
        assert!(!is_eligible(true, LoginState::Unknown, false));
    }

    #[test]
    fn the_only_call_is_a_get_of_the_usage_url() {
        assert!(usage_call_allowed(USAGE_GET));
        assert_eq!(
            USAGE_GET.url,
            "https://api.anthropic.com/api/oauth/usage?at_wall=1&skip_spend=1"
        );
        assert!(!USAGE_GET.url.contains("reset_rate_limits"));
        assert!(!USAGE_GET.url.contains("/v1/messages"));
    }

    /// Real keychain prompt and one usage GET per desktop sign-in, including linked ones.
    /// `cargo test --offline -- --ignored live_desktop_token_quota_once --test-threads=1`
    #[test]
    #[ignore]
    fn live_desktop_token_quota_once() {
        live_probe().unwrap();
    }

    #[test]
    fn opening_requires_confirm_and_a_linked_account_is_not_queried() {
        let mut quota = Quota::testing();
        quota.set_enabled(true);
        assert_eq!(quota.top()["state"], "key_denied");
        let key = derive_key(b"synthetic-safe-storage").unwrap();
        quota.install_key(key);
        assert_eq!(quota.top()["enabled"], true);
        assert_eq!(quota.top()["state"], "ready");

        let root =
            std::env::temp_dir().join(format!("authmanager-desktop-quota-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let user = root.join("Claude");
        fs::create_dir_all(&user).unwrap();
        let account = "aaaaaaaa-0000-4000-8000-000000000001";
        let cache_key = format!(
            "acct:{account}|{CLAUDE_CODE_CLIENT}:bbbbbbbb-0000-4000-8000-000000000002:https://api.anthropic.com:{BASE_SCOPE}"
        );
        let body = json!({
            &cache_key: {"token": "synthetic-access-token-not-a-secret", "expiresAt": 4_000_000_000_000u64}
        });
        let blob = encrypt_v10(&key, body.to_string().as_bytes());
        let encoded = base64::engine::general_purpose::STANDARD.encode(blob);
        fs::write(
            user.join("config.json"),
            json!({"lastKnownAccountUuid": account, "oauth:tokenCacheV2": encoded}).to_string(),
        )
        .unwrap();
        let paths = Paths::new(user, root.join("state"));
        let ctx = Ctx::new(
            paths,
            claude_desktop::Config::default(),
            claude_desktop::FakeDesktop::default(),
            claude_desktop::LogSink::Silent,
        );
        let candidate = Candidate {
            id: account.into(),
            known: true,
            login: LoginState::SignedIn,
        };
        *TEST_URL.lock().unwrap() = USAGE_URL.into();
        TEST_CALLS.store(0, Ordering::SeqCst);
        quota.poll(&ctx, std::slice::from_ref(&candidate), |_| true);
        quota.wait_for_job();
        assert_eq!(TEST_CALLS.load(Ordering::SeqCst), 0);
        assert!(quota.account_field(&candidate, true).is_none());

        quota.poll(&ctx, std::slice::from_ref(&candidate), |_| false);
        quota.wait_for_job();
        let field = quota.account_field(&candidate, false).unwrap();
        assert_eq!(field["state"], "ready");
        assert_eq!(field["windows"][0]["label"], "5 小时窗口");
        assert_eq!(field["windows"][0]["used_percent"], 3.0);
        assert!(field["checked_at"].is_number());
        let text = field.to_string();
        assert!(!text.contains("synthetic-access-token-not-a-secret"));
        assert_eq!(TEST_CALLS.load(Ordering::SeqCst), 1);
        let _ = fs::remove_dir_all(&root);
    }
}
