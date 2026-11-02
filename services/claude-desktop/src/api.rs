// AuthManager 新增（cc-same 里没有这个文件）：给 AuthManager 服务和界面用的门面。
//! The engine as AuthManager's service and window use it.
//!
//! Every answer is a `Serialize` struct (or enum) with snake_case fields. None holds a credential
//! or anything read from Claude's sign-in files: account IDs and emails at most. What cc-same's
//! command line and app did with the engine is here: [`overview`] is everything the app's window
//! showed, [`plan`] / [`sync_once`] / [`run_watch`] sync, [`switch`] / [`add`] trade Claude's
//! sign-in while [`recover`] finishes a switch cut short, and the rest edit the account list,
//! snapshots, transcript retention and settings. Messages are English; the window words the
//! `reason`s and `kind`s itself.

use crate::accounts::{self, NotFound, Roster, Slot, Strategy};
use crate::apply::{self, Busy, Outcome};
use crate::config::Config;
use crate::ctx::{Ctx, FakeDesktop, LogSink};
use crate::desktop::{self, NotReady, Quitting, Step};
use crate::fsx;
use crate::logins;
use crate::model::{Action, ActionKind, Surface, short};
use crate::paths::Paths;
use crate::report::{self, PartitionView, Warning};
use crate::retention::{self, Kept, Limit};
use crate::scan;
use crate::snapshot;
use crate::usage::{FIVE_HOURS, FRESH, Usage};
use crate::watch::{self, Pass, WatchOptions};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------------ the engine

/// The engine with its default locations: Claude Desktop's data, our state in
/// `~/Library/Application Support/AuthManager/claude-desktop`, and the log `engine.log` there.
/// Keep one for the life of the service: it caches what it has read.
pub fn context() -> Result<Ctx, Failure> {
    context_in(crate::paths::default_state_dir())
}

/// The engine with Claude Desktop's default locations and our state in `state_dir` (AuthManager
/// passes the folder inside its own runtime root).
pub fn context_in(state_dir: std::path::PathBuf) -> Result<Ctx, Failure> {
    let paths = Paths::new(crate::paths::default_user_data(), state_dir);
    let config = Config::load(&paths)?;
    let log = LogSink::File(paths.log_file.clone());
    Ok(Ctx::new(paths, config, FakeDesktop::default(), log))
}

// ------------------------------------------------------------------------ failures

/// Why something was not done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    /// Claude has to be closed first (restoring a snapshot, fixing symlinks).
    ClaudeRunning,
    /// Another sync or account switch is in progress.
    Busy,
    /// No account goes by that name.
    NotFound,
    /// The name fits several accounts (`candidates`).
    Ambiguous,
    /// The account is only known from its session folders: sign in to it once in Claude.
    NotInList,
    /// No other account in the rotation has a sign-in kept.
    NoOther,
    /// No sign-in is kept for that account: sign in to it once in Claude ([`add`]).
    NothingSaved,
    /// Switching is not available on this system.
    Unsupported,
    /// Claude did not quit, so nothing was changed.
    StillOpen,
    /// Claude's updater is replacing the app; try again in a minute.
    Updating,
    /// The wait for Claude to quit was stopped; nothing was changed.
    Stopped,
    /// Claude is signed in to that account, so it cannot be taken off the list.
    SignedIn,
    /// Not an alias: up to 24 letters, digits, `-`, `_` or `.`, not only digits, not starting
    /// with `-`.
    BadAlias,
    /// Another account goes by that alias.
    AliasTaken,
    /// Numbers start at 1.
    BadNumber,
    /// The organization's managed settings decide how long transcripts are kept.
    Organization,
    NoSuchSnapshot,
    /// Anything else; `message` says what.
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub reason: FailureReason,
    /// What went wrong, in English (for a detail line or the log).
    pub message: String,
    /// The accounts an ambiguous name could mean, as the list names them.
    pub candidates: Vec<String>,
}

impl Failure {
    fn new(reason: FailureReason, message: impl Into<String>) -> Failure {
        Failure { reason, message: message.into(), candidates: Vec::new() }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

impl From<anyhow::Error> for Failure {
    fn from(e: anyhow::Error) -> Failure {
        use FailureReason as R;
        let reason = if e.downcast_ref::<Busy>().is_some() {
            R::Busy
        } else if let Some(refused) = e.downcast_ref::<accounts::Refused>() {
            match refused {
                accounts::Refused::SignedIn => R::SignedIn,
                accounts::Refused::NoOther => R::NoOther,
                accounts::Refused::BadAlias(_) => R::BadAlias,
                accounts::Refused::AliasTaken { .. } => R::AliasTaken,
                accounts::Refused::BadNumber => R::BadNumber,
            }
        } else if let Some(refused) = e.downcast_ref::<logins::Refused>() {
            match refused {
                logins::Refused::Unsupported => R::Unsupported,
                logins::Refused::NothingSaved => R::NothingSaved,
            }
        } else if let Some(not_ready) = e.downcast_ref::<NotReady>() {
            match not_ready {
                NotReady::StillOpen => R::StillOpen,
                NotReady::Updating => R::Updating,
                NotReady::Stopped => R::Stopped,
            }
        } else if let Some(not_found) = e.downcast_ref::<NotFound>() {
            match not_found {
                NotFound::None(_) => R::NotFound,
                NotFound::Several(_) => R::Ambiguous,
            }
        } else {
            R::Error
        };
        let candidates = match e.downcast_ref::<NotFound>() {
            Some(NotFound::Several(names)) => names.clone(),
            _ => Vec::new(),
        };
        Failure { reason, message: format!("{e:#}"), candidates }
    }
}

fn claude_running(ctx: &Ctx) -> Result<(), Failure> {
    if desktop::is_running(ctx) {
        return Err(Failure::new(FailureReason::ClaudeRunning, "quit Claude first, then try again"));
    }
    Ok(())
}

// ------------------------------------------------------------------------ the overview

/// Everything the window shows at once. Reading it changes nothing of Claude's; our list of
/// accounts is brought up to date (an account Claude is signed in to joins it).
#[derive(Clone, Debug, Serialize)]
pub struct Overview {
    /// When this was read (Unix seconds).
    pub at: f64,
    pub engine_version: String,
    pub desktop: DesktopStatus,
    /// Every account: the list by number, then the ones only seen in Claude's session folders.
    pub accounts: Vec<AccountView>,
    /// The account "Next account" switches to: the next one in the list after the one Claude is
    /// signed in to that takes part and has a sign-in kept.
    pub next_account: Option<String>,
    /// Who each way of picking would switch to right now (AuthManager offers all three).
    pub picks: Picks,
    pub sync: SyncStatus,
    pub config: ConfigView,
    /// Oldest first.
    pub snapshots: Vec<SnapshotInfo>,
    /// How long Claude Code keeps the transcripts behind Desktop's sessions.
    pub retention: RetentionStatus,
    /// Session folders another tool turned into symlinks, relative to Claude's data folder:
    /// Claude silently stops saving there. [`fix_symlinks`] makes them real (Claude closed).
    pub symlinked_folders: Vec<String>,
    /// Sessions whose transcript is no longer on disk: they open empty.
    pub missing_transcripts: Option<MissingTranscripts>,
    /// Backup folders other tools left beside the session folders (safe to archive by hand).
    pub leftover_folders: Vec<String>,
    /// Organizations the synced session lists belong to; with more than one, synced sessions
    /// appear in all of them.
    pub organizations: usize,
}

/// The account each [`Strategy`] picks for "Next account".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Picks {
    pub next: Option<String>,
    pub next_available: Option<String>,
    pub best: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DesktopStatus {
    /// Claude.app is in `/Applications` or `~/Applications`.
    pub installed: bool,
    /// Claude's data folder exists.
    pub data_found: bool,
    pub running: bool,
    pub version: Option<String>,
    /// Switching accounts works here.
    pub can_switch: bool,
    /// The account Claude is signed in to (its `lastKnownAccountUuid`), running or not.
    pub signed_in: Option<String>,
    /// While Claude runs: the account it shows, and that account's organization when its log
    /// names it.
    pub open_account: Option<String>,
    pub open_org: Option<String>,
}

/// Where an account stands for switching Claude to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginState {
    /// Claude is signed in to it.
    SignedIn,
    /// Its sign-in is set aside: switching to it takes one step.
    Saved,
    /// No sign-in is kept for it (signed in to before sign-ins were kept, or removed): sign in to
    /// it once more in Claude to switch to it later.
    Unknown,
}

/// One account, the way people know it: Claude keeps a session list for every organization an
/// account was used in, and they are kept the same, so the row reports the list Claude shows.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AccountView {
    pub account_id: String,
    /// Its number in the list; none for an account only seen in Claude's session folders.
    pub number: Option<u32>,
    pub alias: Option<String>,
    pub email: Option<String>,
    /// In the numbered list (Claude was seen signed in to it, or a sign-in is kept for it).
    pub in_list: bool,
    /// Takes part in "Next account": in the list and not sitting out.
    pub in_rotation: bool,
    pub login: LoginState,
    /// When its kept sign-in was last in use (Unix seconds).
    pub login_saved_at: Option<f64>,
    /// Kept for about four weeks unused: Claude may ask to sign in again.
    pub login_stale: bool,
    /// Claude is running signed in to it.
    pub open: bool,
    /// Claude keeps session lists for it (one that just joined may have none yet).
    pub has_lists: bool,
    pub sessions: usize,
    /// Sessions the other accounts have and this one does not, yet.
    pub missing: usize,
    /// Kept out of the sync.
    pub excluded: bool,
    /// One of its session folders cannot be read or is a symlink: Claude cannot save there.
    pub broken: bool,
    /// What it last used of its plan, as Claude last read it.
    pub usage: Option<UsageView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fullness {
    Roomy,
    /// 80% or more of a window.
    Nearly,
    /// A window is used up.
    Full,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UsageView {
    /// Percent of the 5-hour window; 0 once the reading is older than the window, unknown when
    /// Claude is signed in to the account (it is filling a window nobody has read).
    pub five_hour: Option<u32>,
    /// Percent of the weekly window, while the reading is less than a week old.
    pub weekly: Option<u32>,
    /// When Claude read it (Unix seconds).
    pub at: f64,
    /// Older than Claude's checks of the open account (30 minutes): show its age.
    pub stale: bool,
    /// Claude is signed in to the account and running, so the plan is in use.
    pub in_use: bool,
    pub fullness: Fullness,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SyncStatus {
    /// The background sync is switched on (`background` in the engine's `config.json`).
    pub background: bool,
    /// The background sync wrote its heartbeat within the last minute.
    pub watch_alive: bool,
    pub watch_heartbeat_at: Option<f64>,
    /// A sync asked for with [`sync_once`] is running.
    pub syncing: bool,
    pub last_sync: Option<LastSyncView>,
    pub pending: PendingView,
    /// Sessions were copied into the account Claude has open: Claude shows them after it is
    /// quit and opened again.
    pub restart_needed: Option<RestartNeeded>,
    /// cc-same's one-line status, as its reason and numbers.
    pub headline: Headline,
    /// The snapshot taken before the very first change (kept forever).
    pub baseline_snapshot: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LastSyncView {
    pub at: f64,
    /// `watch` (the background sync), `manual`, …
    pub reason: String,
    pub applied: u64,
    /// Writes that failed (the log has them).
    pub errors: u64,
    pub deferred: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PendingView {
    /// Changes a sync would make now.
    pub changes: usize,
    /// Sessions it would copy or update.
    pub sessions: usize,
    /// Changes waiting until Claude lets go of the account it has open (a switch or a quit).
    pub deferred: usize,
    /// How long changes have been waiting for a sync, as overviews saw it.
    pub waiting_seconds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RestartNeeded {
    pub sessions: u64,
    /// When they were copied (Unix seconds).
    pub at: f64,
    /// The account Claude has open.
    pub account_id: Option<String>,
}

/// What the one-line status says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadlineKind {
    /// No Claude data on this Mac.
    Missing,
    /// Fewer than two session lists take part: nothing to keep in step.
    Single,
    /// Another tool linked `folders` session folders; Claude stops saving there.
    Linked,
    /// A sync is copying `sessions` sessions (0: some changes).
    Syncing,
    /// `sessions` sessions to copy, or `other` other updates, wait for a sync.
    Pending,
    /// `updates` updates land when Claude switches accounts or quits.
    Waiting,
    /// Every account has the same `sessions` sessions; `accounts` take part.
    InSync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Headline {
    pub kind: HeadlineKind,
    pub sessions: usize,
    pub other: usize,
    pub updates: usize,
    pub folders: usize,
    pub accounts: usize,
}

impl Headline {
    fn of(kind: HeadlineKind) -> Headline {
        Headline { kind, sessions: 0, other: 0, updates: 0, folders: 0, accounts: 0 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MissingTranscripts {
    pub missing: usize,
    pub total: usize,
}

/// How long a pending change may wait for the background sync before the status says so.
const SYNC_PATIENCE: Duration = Duration::from_secs(20);
/// With the background sync running, this few sessions in flight is routine mirroring (the
/// session in use right now, say), not worth a status of its own.
const ROUTINE_SESSIONS: usize = 3;

pub fn overview(ctx: &Ctx) -> Overview {
    let ov = report::overview(ctx);
    let now = fsx::now_secs();
    let accounts = account_rows(&ov, now);
    let pick = |strategy: Strategy| {
        let can_switch = |s: &Slot| ov.logins.saved(&s.account).is_some();
        let current = ov.logins.signed_in.as_deref();
        accounts::pick(&ov.roster, current, strategy, can_switch, &ov.usage, now).map(|s| s.account.clone())
    };
    let next_account = pick(Strategy::Next);
    let picks =
        Picks { next: next_account.clone(), next_available: pick(Strategy::NextAvailable), best: pick(Strategy::Best) };
    let pending_for = {
        let mut since = ctx.caches.pending_since.lock().unwrap();
        if ov.plan.total > 0 {
            Some(since.get_or_insert_with(Instant::now).elapsed())
        } else {
            *since = None;
            None
        }
    };
    let heartbeat = ov.heartbeat.as_ref();
    let watch_alive = heartbeat.is_some_and(|h| h.is_fresh());
    let syncing = ctx.caches.syncing.load(Ordering::Relaxed) > 0;
    let headline = headline(&ov, &accounts, syncing, pending_for, ov.config.background && watch_alive);
    let code: &[PartitionView] = ov.surface(Surface::Code).map(|s| s.partitions.as_slice()).unwrap_or_default();
    let installed = desktop::app_bundle().is_some();
    let sync = SyncStatus {
        background: ov.config.background,
        watch_alive,
        watch_heartbeat_at: heartbeat.map(|h| h.heartbeat_at),
        syncing,
        last_sync: ov.last_sync.as_ref().map(|l| LastSyncView {
            at: l.at,
            reason: l.reason.clone(),
            applied: l.applied,
            errors: l.errors,
            deferred: l.deferred,
        }),
        pending: PendingView {
            changes: ov.plan.total,
            sessions: ov.plan.sessions,
            deferred: ov.plan.deferred.values().sum(),
            waiting_seconds: pending_for.map(|d| d.as_secs_f64()),
        },
        restart_needed: ov.restart_hint().map(|hint| RestartNeeded {
            sessions: hint.sessions,
            at: hint.at,
            account_id: ov.app.open_account.clone(),
        }),
        headline,
        baseline_snapshot: ov.baseline.clone(),
    };
    Overview {
        at: now,
        engine_version: crate::VERSION.into(),
        desktop: DesktopStatus {
            installed,
            data_found: ctx.paths.user_data.is_dir(),
            running: ov.app.running,
            version: ov.desktop_version.clone(),
            can_switch: logins::supported() && installed,
            signed_in: ov.logins.signed_in.clone(),
            open_account: ov.app.open_account.clone(),
            open_org: ov.app.open_org.clone(),
        },
        next_account,
        picks,
        sync,
        config: config_view(&ov.config),
        snapshots: snapshot_infos(ctx, ov.baseline.as_deref()),
        retention: retention_status(&ov.retention),
        symlinked_folders: symlinked_folders(ctx),
        missing_transcripts: ov.warnings.iter().find_map(|w| match w {
            Warning::MissingTranscripts { missing, total } => {
                Some(MissingTranscripts { missing: *missing, total: *total })
            }
            _ => None,
        }),
        leftover_folders: ov
            .warnings
            .iter()
            .flat_map(|w| match w {
                Warning::LeftoverFolders { dirs, .. } => dirs.clone(),
                _ => Vec::new(),
            })
            .collect(),
        organizations: code.iter().filter(|p| !p.excluded).map(|p| p.part.org.as_str()).collect::<BTreeSet<_>>().len(),
        accounts,
    }
}

fn usage_view(u: &Usage, now: f64, in_use: bool) -> Option<UsageView> {
    let percent = |p: f64| p.clamp(0., 999.).round() as u32;
    let age = now - u.at;
    let five_hour = if in_use && age > FIVE_HOURS { None } else { u.five_hour_at(now).map(percent) };
    let weekly = u.weekly_at(now).map(percent);
    let fullness = match five_hour.max(weekly).unwrap_or(0) {
        100.. => Fullness::Full,
        80.. => Fullness::Nearly,
        _ => Fullness::Roomy,
    };
    (five_hour.is_some() || weekly.is_some()).then_some(UsageView {
        five_hour,
        weekly,
        at: u.at,
        stale: age > FRESH,
        in_use,
        fullness,
    })
}

/// One row per account (cc-same app's `model::accounts`): the accounts in the list by number,
/// then the ones only seen in Claude's session folders.
fn account_rows(ov: &report::Overview, now: f64) -> Vec<AccountView> {
    let logins = &ov.logins;
    let mut groups: Vec<Vec<&PartitionView>> = Vec::new();
    for p in ov.surface(Surface::Code).map(|c| c.partitions.as_slice()).unwrap_or_default() {
        match groups.iter_mut().find(|g| g[0].part.acct == p.part.acct) {
            Some(group) => group.push(p),
            None => groups.push(vec![p]),
        }
    }
    let row = |acct: &str, lists: &[&PartitionView]| {
        let slot = ov.roster.slot(acct);
        // The list Claude shows for the account: the one it has open, else the fullest.
        let shown = lists.iter().copied().max_by_key(|p| {
            let open = ov.app.showing(&p.part);
            (!p.excluded, open, p.sessions, Reverse(p.missing), Reverse(p.part.org.as_str()))
        });
        let saved = logins.saved(acct);
        let email = lists
            .iter()
            .find_map(|p| p.email.clone())
            .or_else(|| slot.and_then(|s| s.email.clone()))
            .or_else(|| saved.and_then(|s| s.email.clone()));
        let open = ov.app.open_account.as_deref() == Some(acct);
        let login = if logins.signed_in.as_deref() == Some(acct) {
            LoginState::SignedIn
        } else if saved.is_some() {
            LoginState::Saved
        } else {
            LoginState::Unknown
        };
        AccountView {
            account_id: acct.to_string(),
            number: slot.map(|s| s.number),
            alias: slot.and_then(|s| s.alias.clone()),
            email,
            in_list: slot.is_some(),
            in_rotation: slot.is_some_and(|s| !s.disabled),
            login,
            login_saved_at: saved.map(|s| s.set_aside_at),
            login_stale: saved.is_some_and(|s| s.stale()),
            open,
            has_lists: !lists.is_empty(),
            sessions: shown.map_or(0, |p| p.sessions),
            missing: shown.filter(|p| !p.excluded).map_or(0, |p| p.missing),
            excluded: !lists.is_empty() && lists.iter().all(|p| p.excluded),
            broken: lists.iter().any(|p| p.error.is_some() || p.part.is_link),
            usage: ov.usage.get(acct).and_then(|u| usage_view(u, now, open)),
        }
    };
    let mut rows: Vec<AccountView> = groups.iter().map(|lists| row(&lists[0].part.acct, lists)).collect();
    for slot in &ov.roster.slots {
        if !rows.iter().any(|r| r.account_id == slot.account) {
            rows.push(row(&slot.account, &[]));
        }
    }
    // The list by number; then named accounts, the ones only known by ID, and excluded ones.
    let key = |r: &AccountView| {
        let named = r.alias.is_some() || r.email.is_some();
        let name =
            r.alias.clone().or_else(|| r.email.clone()).unwrap_or_else(|| format!("Account {}", short(&r.account_id)));
        (r.number.is_none(), r.number, r.excluded, !named, name.to_lowercase(), r.account_id.clone())
    };
    rows.sort_by_key(key);
    rows
}

/// cc-same app's `model::mood`. `pending_for`: how long the current plan has waited;
/// `sync_alive`: the background sync is on and running.
fn headline(
    ov: &report::Overview,
    rows: &[AccountView],
    syncing: bool,
    pending_for: Option<Duration>,
    sync_alive: bool,
) -> Headline {
    use HeadlineKind as K;
    if syncing {
        return Headline { sessions: ov.plan.sessions, ..Headline::of(K::Syncing) };
    }
    if ov.warnings.iter().any(|w| matches!(w, Warning::DesktopDataMissing { .. })) {
        return Headline::of(K::Missing);
    }
    if let Some(n) = ov.warnings.iter().find_map(|w| match w {
        Warning::SymlinkedFolders { count, .. } => Some(*count),
        _ => None,
    }) {
        return Headline { folders: n, ..Headline::of(K::Linked) };
    }
    let taking_part = rows.iter().filter(|a| a.has_lists && !a.excluded && !a.broken).count();
    // Two lists are enough to keep in step, even two organizations of one account.
    let lists = ov.surface(Surface::Code).map_or(0, |code| {
        code.partitions.iter().filter(|p| !p.excluded && p.error.is_none() && !p.part.is_link).count()
    });
    if lists < 2 {
        return Headline::of(K::Single);
    }
    let union = ov.surface(Surface::Code).map(|s| s.union).unwrap_or(0);
    let in_sync = Headline { sessions: union, accounts: taking_part, ..Headline::of(K::InSync) };
    if ov.plan.total > 0 {
        let sessions = ov.plan.sessions;
        let fresh = pending_for.is_none_or(|t| t < SYNC_PATIENCE);
        if sync_alive && fresh {
            // The background sync is on it; a handful in flight is just mirroring.
            return if sessions <= ROUTINE_SESSIONS {
                in_sync
            } else {
                Headline { sessions, ..Headline::of(K::Syncing) }
            };
        }
        let writes: usize = ov
            .plan
            .per_partition
            .iter()
            .flat_map(|(_, kinds)| kinds.iter())
            .filter(|(k, _)| k.ends_with("sessions"))
            .map(|(_, n)| *n)
            .sum();
        return Headline { sessions, other: ov.plan.total.saturating_sub(writes), ..Headline::of(K::Pending) };
    }
    let deferred: usize = ov.plan.deferred.values().sum();
    if deferred > 0 { Headline { updates: deferred, ..Headline::of(K::Waiting) } } else { in_sync }
}

fn symlinked_folders(ctx: &Ctx) -> Vec<String> {
    snapshot::symlinked_folders(ctx)
        .iter()
        .map(|p| p.strip_prefix(&ctx.paths.user_data).unwrap_or(p).display().to_string())
        .collect()
}

// ------------------------------------------------------------------------ syncing

/// What kind of change a sync makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    NewSessions,
    UpdatedSessions,
    DeletedSessions,
    DeletionMarkers,
    StaleDeletionMarkers,
    MergedTaskLists,
    ArchiveHint,
}

fn change_kind(a: &Action) -> ChangeKind {
    match a.kind {
        ActionKind::WriteRecord if a.additive => ChangeKind::NewSessions,
        ActionKind::WriteRecord => ChangeKind::UpdatedSessions,
        ActionKind::TrashRecord => ChangeKind::DeletedSessions,
        ActionKind::WriteTomb => ChangeKind::DeletionMarkers,
        ActionKind::TrashTomb => ChangeKind::StaleDeletionMarkers,
        ActionKind::WriteCollection => ChangeKind::MergedTaskLists,
        ActionKind::WriteArchiveIdx => ChangeKind::ArchiveHint,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ChangeCount {
    pub kind: ChangeKind,
    pub count: usize,
}

/// The changes a sync would make in one session list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlanTarget {
    pub account_id: String,
    pub org_id: String,
    pub changes: Vec<ChangeCount>,
}

/// Changes waiting for Claude to let go of one session list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeferredChanges {
    pub account_id: String,
    pub org_id: String,
    pub count: usize,
}

/// What a sync would do now (a dry run).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlanSummary {
    /// Changes to the account Claude has open wait until it switches accounts or quits.
    pub claude_running: bool,
    pub changes: usize,
    /// Sessions that would be copied or updated somewhere.
    pub sessions: usize,
    pub targets: Vec<PlanTarget>,
    pub deferred: Vec<DeferredChanges>,
    pub deferred_total: usize,
    /// Things left alone and why, in English.
    pub notes: Vec<String>,
}

pub fn plan(ctx: &Ctx) -> PlanSummary {
    let (plan, app, _) = crate::plan::build_plan(ctx, None, None);
    let mut targets: Vec<PlanTarget> = Vec::new();
    let mut counts: Vec<BTreeMap<ChangeKind, usize>> = Vec::new();
    for a in &plan.actions {
        let i = match targets.iter().position(|t| t.account_id == a.part.acct && t.org_id == a.part.org) {
            Some(i) => i,
            None => {
                let target =
                    PlanTarget { account_id: a.part.acct.clone(), org_id: a.part.org.clone(), changes: Vec::new() };
                targets.push(target);
                counts.push(BTreeMap::new());
                targets.len() - 1
            }
        };
        *counts[i].entry(change_kind(a)).or_default() += 1;
    }
    for (target, counts) in targets.iter_mut().zip(counts) {
        target.changes = counts.into_iter().map(|(kind, count)| ChangeCount { kind, count }).collect();
    }
    let sessions: BTreeSet<&str> =
        plan.actions.iter().filter(|a| a.kind == ActionKind::WriteRecord).map(|a| a.name.as_str()).collect();
    let deferred = plan
        .deferred
        .iter()
        .map(|(key, count)| {
            let (acct, org) = key.split_once('/').unwrap_or((key.as_str(), ""));
            DeferredChanges { account_id: acct.into(), org_id: org.into(), count: *count }
        })
        .collect();
    PlanSummary {
        claude_running: app.running,
        changes: plan.actions.len(),
        sessions: sessions.len(),
        targets,
        deferred,
        deferred_total: plan.deferred_total(),
        notes: plan.notes.clone(),
    }
}

/// What a sync did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SyncResult {
    pub applied: usize,
    /// Session records written (copied or updated).
    pub copied_sessions: usize,
    /// Writes that failed, in English (also in the log).
    pub errors: Vec<String>,
    /// Changes waiting until Claude lets go of the account it has open.
    pub deferred: usize,
    /// Sessions were copied into the account Claude has open: they show after a restart.
    pub restart_needed: bool,
}

impl SyncResult {
    fn of(out: &Outcome) -> SyncResult {
        SyncResult {
            applied: out.applied_total(),
            copied_sessions: out.applied.get(&ActionKind::WriteRecord).copied().unwrap_or(0),
            errors: out.errors.clone(),
            deferred: out.deferred,
            restart_needed: !out.seeded_while_loaded.is_empty(),
        }
    }
}

/// Marks a sync asked for by hand as running, for the overview.
struct Syncing<'a>(&'a Ctx);

impl<'a> Syncing<'a> {
    fn start(ctx: &'a Ctx) -> Syncing<'a> {
        ctx.caches.syncing.fetch_add(1, Ordering::Relaxed);
        Syncing(ctx)
    }
}

impl Drop for Syncing<'_> {
    fn drop(&mut self) {
        self.0.caches.syncing.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Sync once, now ("Sync now"). Runs even with nothing to change: it records which session lists
/// take part and what each task list looked like, which later merges depend on.
pub fn sync_once(ctx: &Ctx) -> Result<SyncResult, Failure> {
    let _syncing = Syncing::start(ctx);
    let (_, out) = apply::run_sync(ctx, "manual")?;
    Ok(SyncResult::of(&out))
}

/// Sessions were just copied into the account Claude has open, for the first time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RestartNotice {
    pub sessions: usize,
    pub account_ids: Vec<String>,
}

/// One sync the background sync ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassSummary {
    pub at: i64,
    pub claude_running: bool,
    /// The sync ran; `error` says why it did not otherwise.
    pub ok: bool,
    pub error: Option<String>,
    pub result: Option<SyncResult>,
    /// Tell the user to quit and reopen Claude to see the sessions copied into the account it
    /// has open (when `notify` is on). Told once per session list.
    pub restart_notice: Option<RestartNotice>,
}

fn pass_summary(pass: &Pass) -> PassSummary {
    let restart_notice = (!pass.restart_notices.is_empty()).then(|| {
        let mut account_ids: Vec<String> = Vec::new();
        for key in pass.restart_notices.keys() {
            let acct = key.split_once('/').map_or(key.as_str(), |(a, _)| a).to_string();
            if !account_ids.contains(&acct) {
                account_ids.push(acct);
            }
        }
        RestartNotice { sessions: pass.restart_notices.values().sum(), account_ids }
    });
    PassSummary {
        at: fsx::now_secs() as i64,
        claude_running: pass.running,
        ok: pass.result.is_ok(),
        error: pass.result.as_ref().err().cloned(),
        result: pass.result.as_ref().ok().map(SyncResult::of),
        restart_notice,
    }
}

/// The background sync, on the calling thread until `stop` is set: it looks every 2 seconds,
/// lets Claude finish a burst of writes, syncs when something changed and at least every minute,
/// and writes a heartbeat every 15 seconds. `on_pass` hears every sync it runs. Run it while
/// `background` is on ([`set_background`]).
pub fn run_watch(ctx: &Ctx, stop: &AtomicBool, mut on_pass: impl FnMut(&PassSummary)) {
    watch::run(ctx, &WatchOptions::default(), stop, &mut |pass: &Pass| on_pass(&pass_summary(pass)));
}

// ------------------------------------------------------------------------ settings

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConfigView {
    /// Keep syncing in the background while AuthManager's service runs.
    pub background: bool,
    /// Tell the user when sessions were copied into the account Claude has open.
    pub notify: bool,
    /// Accounts and organizations that appear later join the sync by themselves.
    pub auto_join_new: bool,
    /// `<account>` or `<account>/<org>` kept out of the sync.
    pub exclude: Vec<String>,
    /// Rolling snapshots kept (the first one is kept forever).
    pub keep_snapshots: usize,
    /// Days the trash keeps what a sync removed or replaced.
    pub trash_days: f64,
}

fn config_view(cfg: &Config) -> ConfigView {
    ConfigView {
        background: cfg.background,
        notify: cfg.notify,
        auto_join_new: cfg.auto_join_new,
        exclude: cfg.exclude.clone(),
        keep_snapshots: cfg.keep_snapshots,
        trash_days: cfg.trash_days,
    }
}

/// A change to the settings: what is left out stays as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ConfigChange {
    pub background: Option<bool>,
    pub notify: Option<bool>,
    pub auto_join_new: Option<bool>,
    /// Keep these `<account>` or `<account>/<org>` out of the sync.
    pub exclude: Vec<String>,
    /// Take these out of `exclude` again.
    pub include: Vec<String>,
}

pub fn config(ctx: &Ctx) -> ConfigView {
    config_view(&ctx.config())
}

/// Change the settings in the engine's `config.json`; the background sync picks them up on its
/// next look.
pub fn set_config(ctx: &Ctx, change: &ConfigChange) -> Result<ConfigView, Failure> {
    let mut cfg = Config::load(&ctx.paths)?;
    let before = cfg.clone();
    if let Some(on) = change.background {
        cfg.background = on;
    }
    if let Some(on) = change.notify {
        cfg.notify = on;
    }
    if let Some(on) = change.auto_join_new {
        cfg.auto_join_new = on;
    }
    for id in &change.exclude {
        if !cfg.exclude.contains(id) {
            cfg.exclude.push(id.clone());
        }
    }
    cfg.exclude.retain(|e| !change.include.contains(e));
    if cfg != before || !ctx.paths.config_file().exists() {
        cfg.save(&ctx.paths)?;
    }
    ctx.set_config(cfg.clone());
    Ok(config_view(&cfg))
}

/// Switch the background sync on or off (the service starts or stops [`run_watch`]).
pub fn set_background(ctx: &Ctx, on: bool) -> Result<ConfigView, Failure> {
    set_config(ctx, &ConfigChange { background: Some(on), ..ConfigChange::default() })
}

// ------------------------------------------------------------------------ snapshots

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SnapshotInfo {
    /// `YYYYMMDD-HHMMSS-<reason>` in local time.
    pub id: String,
    /// `baseline`, `watch`, `manual`, `pre-restore`, …
    pub reason: String,
    /// Unix seconds.
    pub created: f64,
    /// Session folders it holds.
    pub folders: usize,
    /// Taken before the very first change; kept forever.
    pub baseline: bool,
}

fn snapshot_infos(ctx: &Ctx, baseline: Option<&str>) -> Vec<SnapshotInfo> {
    snapshot::list(ctx)
        .into_iter()
        .map(|m| SnapshotInfo {
            baseline: baseline == Some(m.id.as_str()),
            folders: m.partitions.len(),
            id: m.id,
            reason: m.reason,
            created: m.created,
        })
        .collect()
}

/// Oldest first.
pub fn snapshots(ctx: &Ctx) -> Vec<SnapshotInfo> {
    let state = crate::config::State::load(&ctx.paths);
    snapshot_infos(ctx, state.baseline.as_deref())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Restored {
    pub snapshot: String,
    /// The snapshot of how things were just before, to go back to.
    pub pre_restore_snapshot: String,
}

/// Put every session list back as it was in snapshot `id` (Claude closed). The current ones go
/// to the trash first, after a `pre-restore` snapshot.
pub fn restore_snapshot(ctx: &Ctx, id: &str) -> Result<Restored, Failure> {
    if !snapshot::list(ctx).iter().any(|m| m.id == id) {
        return Err(Failure::new(FailureReason::NoSuchSnapshot, format!("no such snapshot: {id}")));
    }
    claude_running(ctx)?;
    let pre = snapshot::restore(ctx, id)?;
    Ok(Restored { snapshot: id.to_string(), pre_restore_snapshot: pre })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SymlinksFixed {
    /// The folders made real again, relative to Claude's data folder.
    pub folders: Vec<String>,
}

/// Turn session folders another tool symlinked back into real folders holding a copy of what
/// they pointed to (Claude closed). A sync merges them afterwards.
pub fn fix_symlinks(ctx: &Ctx) -> Result<SymlinksFixed, Failure> {
    claude_running(ctx)?;
    let fixed = snapshot::fix_symlinks(ctx)?;
    let folders =
        fixed.iter().map(|p| p.strip_prefix(&ctx.paths.user_data).unwrap_or(p).display().to_string()).collect();
    Ok(SymlinksFixed { folders })
}

// ------------------------------------------------------------------------ transcript retention

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionLimit {
    /// `desktopSessionCleanupPeriodDays` in the user's Claude Code settings.
    DesktopSetting,
    /// The organization's managed settings.
    Organization,
    /// A Claude Code before 2.1.248, which applies `cleanupPeriodDays` to every transcript.
    OlderClaudeCode,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RetentionStatus {
    /// Days after which Claude Code deletes a Desktop session's transcript; none: never.
    pub desktop_days: Option<f64>,
    pub limited_by: Option<RetentionLimit>,
    /// `cleanupPeriodDays` in effect, for terminal sessions and other data.
    pub cleanup_days: f64,
    /// The newest Claude Code Claude Desktop has installed.
    pub claude_code: Option<String>,
    /// Desktop sessions lose their transcripts within a year: worth a reminder.
    pub short: bool,
    /// [`keep_transcripts`] can lift the limit (not one the organization sets).
    pub can_keep: bool,
    /// Keeping them sets `cleanupPeriodDays` to ten years (an older Claude Code).
    pub keep_sets_ten_years: bool,
    /// [`undo_retention`] can put back what we changed.
    pub can_undo: bool,
}

fn retention_status(r: &retention::Retention) -> RetentionStatus {
    RetentionStatus {
        desktop_days: r.desktop_days,
        limited_by: r.limited_by.map(|l| match l {
            Limit::DesktopSetting => RetentionLimit::DesktopSetting,
            Limit::Organization => RetentionLimit::Organization,
            Limit::OlderClaudeCode => RetentionLimit::OlderClaudeCode,
        }),
        cleanup_days: r.cleanup_days,
        claude_code: r.claude_code.clone(),
        short: r.is_short(),
        can_keep: r.limited_by.is_some_and(|l| l != Limit::Organization),
        keep_sets_ten_years: r.limited_by == Some(Limit::OlderClaudeCode),
        can_undo: r.undo.is_some(),
    }
}

pub fn retention(ctx: &Ctx) -> RetentionStatus {
    retention_status(&retention::read(&ctx.paths))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptsKept {
    /// Nothing to do: they are kept at any age already.
    Already,
    /// The Desktop limit is gone: kept at any age.
    AnyAge,
    /// An older Claude Code: `cleanupPeriodDays` is ten years now.
    TenYears,
}

/// Keep the transcripts of Desktop sessions. Claude Code's settings file is backed up beside
/// itself first, and [`undo_retention`] puts the change back.
pub fn keep_transcripts(ctx: &Ctx) -> Result<TranscriptsKept, Failure> {
    if retention::read(&ctx.paths).limited_by == Some(Limit::Organization) {
        return Err(Failure::new(
            FailureReason::Organization,
            "your organization sets how long Claude Code keeps transcripts",
        ));
    }
    Ok(match retention::keep(&ctx.paths)? {
        Kept::Already => TranscriptsKept::Already,
        Kept::AnyAge => TranscriptsKept::AnyAge,
        Kept::TenYears => TranscriptsKept::TenYears,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RetentionUndone {
    /// Something was put back (false: nothing of ours was in place any more).
    pub changed: bool,
}

pub fn undo_retention(ctx: &Ctx) -> Result<RetentionUndone, Failure> {
    Ok(RetentionUndone { changed: retention::undo(&ctx.paths)? })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct CleanupDaysSet {
    pub days: u32,
    /// The settings file existed and was backed up beside itself.
    pub backed_up: bool,
}

/// Set `cleanupPeriodDays` (terminal sessions and other data); at least 1.
pub fn set_cleanup_days(ctx: &Ctx, days: u32) -> Result<CleanupDaysSet, Failure> {
    let backup = retention::set_cleanup_days(&ctx.paths, days)?;
    Ok(CleanupDaysSet { days, backed_up: backup.is_some() })
}

// ------------------------------------------------------------------------ the list of accounts

/// The account `query` names in the list: its number, alias, email, or the start of its ID (4
/// characters or more). One only seen in Claude's session folders is not in the list yet.
fn named(ctx: &Ctx, roster: &Roster, query: &str) -> Result<String, Failure> {
    match roster.find(query) {
        Ok(slot) => Ok(slot.account.clone()),
        Err(e @ NotFound::Several(_)) => Err(anyhow::Error::from(e).into()),
        Err(NotFound::None(_)) => {
            let query = query.trim();
            let lower = query.to_lowercase();
            let labels = scan::account_labels(ctx);
            let mut others: Vec<(String, String)> = Vec::new();
            for part in scan::discover(ctx, Surface::Code) {
                if roster.slot(&part.acct).is_some() || others.iter().any(|(id, _)| *id == part.acct) {
                    continue;
                }
                let email = labels.get(&part.acct);
                let fits = email.is_some_and(|e| e.eq_ignore_ascii_case(query))
                    || (lower.len() >= 4 && part.acct.starts_with(&lower));
                if fits {
                    let name = email.cloned().unwrap_or_else(|| format!("Account {}", short(&part.acct)));
                    others.push((part.acct, name));
                }
            }
            match others.as_slice() {
                [(_, name)] => Err(Failure::new(
                    FailureReason::NotInList,
                    format!("{name} is not in the list yet: sign in to it once in Claude"),
                )),
                [] => Err(Failure::new(FailureReason::NotFound, format!("no account called {query}"))),
                many => {
                    let names: Vec<String> = many.iter().map(|(_, n)| n.clone()).collect();
                    let message = format!("{query} could be {}", names.join(" or "));
                    Err(Failure { reason: FailureReason::Ambiguous, message, candidates: names })
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AccountRef {
    pub account_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AliasSet {
    pub account_id: String,
    /// As kept: lowercase. None: taken away.
    pub alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AccountMoved {
    pub account_id: String,
    pub number: u32,
    /// The account that had that number and took the old one.
    pub swapped_with: Option<String>,
}

/// Give an account an alias; `None` or blank takes it away.
pub fn set_alias(ctx: &Ctx, query: &str, alias: Option<&str>) -> Result<AliasSet, Failure> {
    let roster = accounts::observe(ctx);
    let id = named(ctx, &roster, query)?;
    let alias = alias.filter(|a| !a.trim().is_empty());
    let roster = accounts::set_alias(ctx, &id, alias)?;
    let alias = roster.slot(&id).and_then(|s| s.alias.clone());
    Ok(AliasSet { account_id: id, alias })
}

/// Leave an account out of "Next account" (switching to it by name still works).
pub fn disable_account(ctx: &Ctx, query: &str) -> Result<AccountRef, Failure> {
    sit_out(ctx, query, true)
}

/// Put an account back in "Next account".
pub fn enable_account(ctx: &Ctx, query: &str) -> Result<AccountRef, Failure> {
    sit_out(ctx, query, false)
}

fn sit_out(ctx: &Ctx, query: &str, disabled: bool) -> Result<AccountRef, Failure> {
    let roster = accounts::observe(ctx);
    let id = named(ctx, &roster, query)?;
    accounts::set_disabled(ctx, &id, disabled)?;
    Ok(AccountRef { account_id: id })
}

/// Give an account another number; the account that had it takes the old one.
pub fn move_account(ctx: &Ctx, query: &str, number: u32) -> Result<AccountMoved, Failure> {
    let roster = accounts::observe(ctx);
    let id = named(ctx, &roster, query)?;
    let swapped_with = accounts::move_to(ctx, &id, number)?;
    Ok(AccountMoved { account_id: id, number, swapped_with })
}

/// Take an account off the list and forget the sign-in kept for it. Its sessions stay, and it
/// joins again the next time Claude is signed in to it. Not the account Claude is signed in to.
pub fn remove_account(ctx: &Ctx, query: &str) -> Result<AccountRef, Failure> {
    let roster = accounts::observe(ctx);
    let id = named(ctx, &roster, query)?;
    accounts::remove(ctx, &id)?;
    Ok(AccountRef { account_id: id })
}

// ------------------------------------------------------------------------ switching

/// Who to switch Claude to.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SwitchTarget {
    /// The next account in the list (skipping ones that sit out or have no sign-in kept), picked
    /// by `strategy`.
    Next {
        #[serde(default)]
        strategy: Strategy,
    },
    /// The account `query` names: its number, alias, email, or the start of its ID.
    Account { query: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchStage {
    /// Under way; `target` is known.
    Starting,
    /// Claude was asked to quit and has said nothing yet.
    WaitingForQuit,
    /// Claude asks whether to stop its work in progress ("Claude is still working"): the answer
    /// is given in Claude. Choosing Cancel there leaves no trace, so offer to stop waiting.
    Confirming,
    /// The answer was "Wait for Claude": it quits once that work is done, however long it takes.
    AfterWork,
    /// Claude's updater is replacing the app (up to 3 minutes).
    WaitingForUpdater,
    /// Claude has quit; its sign-in is being set aside and the target's put in its place.
    MovingSignIn,
    /// Claude is being started again.
    Launching,
    Done,
    Failed,
    /// Stopped waiting for Claude to quit; nothing was changed.
    Stopped,
}

/// How a switch, or adding an account, goes; the last one has stage `done`, `failed` or
/// `stopped`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SwitchProgress {
    pub stage: SwitchStage,
    /// Adding an account: Claude restarts signed out, on its sign-in page; signing in there makes
    /// the account join the list.
    pub adding: bool,
    /// The account the switch heads for (none when adding).
    pub target: Option<String>,
    /// Stopping now changes nothing (Claude is being asked to quit).
    pub can_stop: bool,
    /// Done: the account set aside (Claude was signed in to it) and the one Claude is signed in
    /// to now (none after adding).
    pub from: Option<String>,
    pub to: Option<String>,
    /// Done: Claude was started again (it was running, or it is needed to sign in).
    pub launched: bool,
    /// Done: Claude was signed in to the target already; nothing was changed.
    pub already: bool,
    /// Failed: why.
    pub failure: Option<Failure>,
    /// Stopped while Claude waited for its work: it still quits once that is done.
    pub after_work: bool,
}

impl SwitchProgress {
    fn at(stage: SwitchStage, adding: bool, target: Option<String>) -> SwitchProgress {
        use SwitchStage as S;
        SwitchProgress {
            stage,
            adding,
            target,
            can_stop: matches!(stage, S::WaitingForQuit | S::Confirming | S::AfterWork),
            from: None,
            to: None,
            launched: false,
            already: false,
            failure: None,
            after_work: false,
        }
    }
}

/// Switch Claude to `target` without signing out: Claude quits (asked the way the Dock does),
/// its sign-in is set aside and the target's put back, and Claude starts again if it was
/// running. `progress` hears each stage, the last one included, which is also returned. Setting
/// `stop` while Claude is asked to quit ends the switch with nothing changed. Blocks until done.
pub fn switch(
    ctx: &Ctx,
    target: &SwitchTarget,
    mut progress: impl FnMut(SwitchProgress),
    stop: &AtomicBool,
) -> SwitchProgress {
    let resolved = (|| -> Result<(String, Option<String>), Failure> {
        let roster = accounts::observe(ctx);
        let found = logins::list(ctx);
        let account = match target {
            SwitchTarget::Account { query } => named(ctx, &roster, query)?,
            SwitchTarget::Next { strategy } => {
                let usage = accounts::usage(ctx, &roster);
                let can_switch = |s: &Slot| found.saved(&s.account).is_some();
                let current = found.signed_in.as_deref();
                match accounts::pick(&roster, current, *strategy, can_switch, &usage, fsx::now_secs()) {
                    Some(slot) => slot.account.clone(),
                    None => return Err(anyhow::Error::from(accounts::Refused::NoOther).into()),
                }
            }
        };
        Ok((account, found.signed_in))
    })();
    let (account, signed_in) = match resolved {
        Ok(found) => found,
        Err(failure) => {
            let end = SwitchProgress { failure: Some(failure), ..SwitchProgress::at(SwitchStage::Failed, false, None) };
            progress(end.clone());
            return end;
        }
    };
    if signed_in.as_deref() == Some(account.as_str()) {
        let end = SwitchProgress {
            from: Some(account.clone()),
            to: Some(account.clone()),
            already: true,
            ..SwitchProgress::at(SwitchStage::Done, false, Some(account))
        };
        progress(end.clone());
        return end;
    }
    run_switch(ctx, Some(account), progress, stop)
}

/// Restart Claude signed out, on its sign-in page, to add an account: the one in use is set
/// aside (and stays in the list, ready to switch back to). Signing in happens in Claude, through
/// Anthropic's own flow; the overview shows the new account once Claude is signed in to it.
pub fn add(ctx: &Ctx, progress: impl FnMut(SwitchProgress), stop: &AtomicBool) -> SwitchProgress {
    run_switch(ctx, None, progress, stop)
}

fn run_switch(
    ctx: &Ctx,
    to: Option<String>,
    progress: impl FnMut(SwitchProgress),
    stop: &AtomicBool,
) -> SwitchProgress {
    use SwitchStage as S;
    let adding = to.is_none();
    let at = |stage: SwitchStage| SwitchProgress::at(stage, adding, to.clone());
    let progress = RefCell::new(progress);
    let send = |p: SwitchProgress| (progress.borrow_mut())(p);
    send(at(S::Starting));
    let quitting = Cell::new(None::<Quitting>);
    let on_quit = |q: Quitting| {
        quitting.set(Some(q));
        send(at(match q {
            Quitting::Asked => S::WaitingForQuit,
            Quitting::Confirming => S::Confirming,
            Quitting::AfterWork => S::AfterWork,
        }));
    };
    let on_step = |step: Step| {
        send(at(match step {
            Step::WaitingForUpdate => S::WaitingForUpdater,
            Step::MovingSignIn => S::MovingSignIn,
            Step::Launching => S::Launching,
        }))
    };
    let watch = desktop::Watch { progress: Some(&on_quit), stop: Some(stop), step: Some(&on_step) };
    let result = match &to {
        Some(account) => accounts::switch(ctx, account, &watch),
        None => accounts::add(ctx, &watch),
    };
    let end = match result {
        Ok(done) => SwitchProgress { from: done.from, to: done.to, launched: done.launched, ..at(S::Done) },
        Err(e) if e.downcast_ref::<NotReady>() == Some(&NotReady::Stopped) => {
            SwitchProgress { after_work: quitting.get() == Some(Quitting::AfterWork), ..at(S::Stopped) }
        }
        Err(e) => {
            ctx.log(format!("switch: {e:#}"));
            SwitchProgress { failure: Some(e.into()), ..at(S::Failed) }
        }
    };
    send(end.clone());
    end
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    /// No switch was cut short.
    Nothing,
    /// A switch cut short (a crash, a power cut) was undone or finished.
    Recovered,
    /// One waits, and Claude is running: try again once it has quit.
    ClaudeRunning,
    /// One waits, and another sync or switch holds the lock: try again later.
    Busy,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Recovery {
    pub status: RecoveryStatus,
    pub message: Option<String>,
}

/// Finish an account switch that was cut short, before Claude is next opened: call at startup.
/// Only while Claude is not running and nothing else holds the lock.
pub fn recover(ctx: &Ctx) -> Recovery {
    let status = |status: RecoveryStatus| Recovery { status, message: None };
    if !logins::interrupted(&ctx.paths) {
        return status(RecoveryStatus::Nothing);
    }
    if desktop::is_running(ctx) {
        return status(RecoveryStatus::ClaudeRunning);
    }
    let Ok(_lock) = apply::Lock::acquire(ctx, Duration::ZERO) else {
        return status(RecoveryStatus::Busy);
    };
    match logins::recover(&ctx.paths) {
        Ok(()) => {
            ctx.log("finished an interrupted account switch");
            status(RecoveryStatus::Recovered)
        }
        Err(e) => {
            ctx.log(format!("finishing an interrupted account switch: {e:#}"));
            Recovery { status: RecoveryStatus::Failed, message: Some(format!("{e:#}")) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, FakeDesktop, LogSink};
    use std::fs;

    const ADA: &str = "aaaaaaaa-0000-4000-8000-000000000001";
    const BOB: &str = "bbbbbbbb-0000-4000-8000-000000000002";
    const ADA_ORG: &str = "0a0a0a0a-0000-4000-8000-000000000001";
    const BOB_ORG: &str = "0b0b0b0b-0000-4000-8000-000000000002";

    fn ctx(tmp: &crate::testing::TempDir) -> Ctx {
        let mut paths = Paths::new(tmp.path().join("Claude"), tmp.path().join("state"));
        paths.claude_json = tmp.path().join(".claude.json");
        paths.claude_settings = tmp.path().join(".claude/settings.json");
        paths.managed_settings = tmp.path().join("managed");
        paths.projects = vec![tmp.path().join(".claude/projects")];
        paths.desktop_logs = tmp.path().join("Logs");
        fs::create_dir_all(&paths.user_data).unwrap();
        let fake = FakeDesktop { running: Some(false), active: None };
        Ctx::new(paths, Config::default(), fake, LogSink::Silent)
    }

    #[test]
    fn the_overview_reads_two_accounts_without_credentials() {
        let tmp = crate::testing::tempdir().unwrap();
        let ctx = ctx(&tmp);
        let sessions = ctx.paths.surface_root(Surface::Code);
        fs::create_dir_all(sessions.join(ADA).join(ADA_ORG)).unwrap();
        fs::create_dir_all(sessions.join(BOB).join(BOB_ORG)).unwrap();
        let config = serde_json::json!({ "lastKnownAccountUuid": ADA, "oauth:tokenCacheV2": "secret-token" });
        fs::write(ctx.paths.desktop_config(), config.to_string()).unwrap();
        let ov = overview(&ctx);
        assert_eq!(ov.desktop.signed_in.as_deref(), Some(ADA));
        assert_eq!(ov.accounts.iter().map(|a| a.account_id.as_str()).collect::<Vec<_>>(), [ADA, BOB]);
        assert_eq!((ov.accounts[0].number, ov.accounts[0].login), (Some(1), LoginState::SignedIn));
        assert_eq!((ov.accounts[1].in_list, ov.accounts[1].login), (false, LoginState::Unknown));
        assert_eq!(ov.sync.headline.kind, HeadlineKind::InSync);
        let json = serde_json::to_string(&ov).unwrap();
        assert!(!json.contains("secret-token"));
        assert!(json.contains("\"next_account\""));
    }

    #[test]
    fn names_resolve_to_accounts_or_say_why_not() {
        let tmp = crate::testing::tempdir().unwrap();
        let ctx = ctx(&tmp);
        let sessions = ctx.paths.surface_root(Surface::Code);
        fs::create_dir_all(sessions.join(BOB).join(BOB_ORG)).unwrap();
        fs::write(ctx.paths.desktop_config(), serde_json::json!({ "lastKnownAccountUuid": ADA }).to_string()).unwrap();
        let roster = accounts::observe(&ctx);
        assert_eq!(named(&ctx, &roster, "1").unwrap(), ADA);
        assert_eq!(named(&ctx, &roster, "bbbb").unwrap_err().reason, FailureReason::NotInList);
        assert_eq!(named(&ctx, &roster, "zzzz").unwrap_err().reason, FailureReason::NotFound);
        let aliased = set_alias(&ctx, "1", Some("Work")).unwrap();
        assert_eq!(aliased, AliasSet { account_id: ADA.into(), alias: Some("work".into()) });
        assert_eq!(remove_account(&ctx, "work").unwrap_err().reason, FailureReason::SignedIn);
        let stop = AtomicBool::new(false);
        let mut heard = Vec::new();
        let end = switch(&ctx, &SwitchTarget::Next { strategy: Strategy::Next }, |p| heard.push(p.stage), &stop);
        assert_eq!(end.failure.map(|f| f.reason), Some(FailureReason::NoOther));
        assert_eq!(heard, [SwitchStage::Failed]);
    }
}
