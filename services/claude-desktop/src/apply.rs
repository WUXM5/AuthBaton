// 派生自 cc-same crates/core/src/apply.rs（v0.1.12, MIT, Songkeys）
//! Carrying out a plan.
//!
//! AuthManager: macOS only; a record rewrite keeps the bytes Claude wrote (see
//! `spliced_record`); a busy lock is a typed error ([`Busy`]).

use crate::config::{LastSync, State};
use crate::ctx::Ctx;
use crate::desktop;
use crate::fsx;
use crate::jsonraw;
use crate::model::*;
use crate::plan::build_plan;
use crate::snapshot;
use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Exclusive lock on `<state>/lock`, shared by everything that changes Claude's session
/// folders or its sign-in.
pub struct Lock {
    file: File,
}

impl Lock {
    pub fn acquire(ctx: &Ctx, timeout: Duration) -> Result<Lock> {
        Lock::acquire_at(&ctx.paths.lock_file(), timeout)
    }

    /// The same kind of lock on another file in the state folder.
    pub fn acquire_at(path: &std::path::Path, timeout: Duration) -> Result<Lock> {
        if let Some(dir) = path.parent() {
            fsx::create_private_dir_all(dir)?;
        }
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        let deadline = Instant::now() + timeout;
        loop {
            if os_lock::try_lock(&file)? {
                return Ok(Lock { file });
            }
            if Instant::now() >= deadline {
                return Err(Busy.into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Another run holds the lock: a sync, a switch, a restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("another sync or account switch is in progress")
    }
}

impl std::error::Error for Busy {}

impl Drop for Lock {
    fn drop(&mut self) {
        os_lock::unlock(&self.file);
    }
}

mod os_lock {
    use std::fs::File;
    use std::io;
    use std::os::unix::io::AsRawFd;

    pub fn try_lock(f: &File) -> io::Result<bool> {
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::EWOULDBLOCK) { Ok(false) } else { Err(e) }
    }

    pub fn unlock(f: &File) {
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// Counts of what one run did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub applied: BTreeMap<ActionKind, usize>,
    pub skipped_loaded: usize,
    pub errors: Vec<String>,
    /// Sessions copied into an index Claude has loaded (visible after a restart).
    pub seeded_while_loaded: BTreeMap<String, usize>,
    pub deferred: usize,
}

impl Outcome {
    pub fn applied_total(&self) -> usize {
        self.applied.values().sum()
    }

    fn absorb(&mut self, other: Outcome) {
        for (k, v) in other.applied {
            *self.applied.entry(k).or_default() += v;
        }
        for (k, v) in other.seeded_while_loaded {
            *self.seeded_while_loaded.entry(k).or_default() += v;
        }
        self.skipped_loaded += other.skipped_loaded;
        self.errors.extend(other.errors);
        self.deferred = other.deferred;
    }
}

fn trash_target(ctx: &Ctx, p: &Partition, name: &str, stamp: &str) -> PathBuf {
    let base = ctx.paths.trash_dir().join(stamp).join(p.surface.as_str()).join(&p.acct).join(&p.org).join(name);
    fsx::unique_path(base)
}

fn execute(ctx: &Ctx, a: &Action, stamp: &str) -> Result<bool> {
    let base = &a.part.path;
    let target = base.join(&a.name);
    match a.kind {
        ActionKind::WriteRecord => {
            let ActionExtra::Record { portable, expect_mtime_ns } = &a.extra else {
                bail!("record action without data")
            };
            let mut existing = None;
            let mut existing_raw = None;
            if !a.additive {
                let md = match fs::symlink_metadata(&target) {
                    Ok(md) => md,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                    Err(e) => return Err(e.into()),
                };
                if !md.is_file() || Some(fsx::mtime_ns(&md)) != *expect_mtime_ns {
                    return Ok(false); // changed since planning; the next pass decides again
                }
                let raw = fsx::read_limited(&target, MAX_RECORD_BYTES)?;
                match serde_json::from_slice(&raw)? {
                    Value::Object(m) => existing = Some(m),
                    _ => return Ok(false),
                }
                existing_raw = Some(raw);
            }
            let out = desired_record(portable, existing.as_ref());
            let source =
                a.src.as_ref().and_then(|src| fsx::read_limited(&src.path.join(&a.name), MAX_RECORD_BYTES).ok());
            let body = match source.and_then(|src| spliced_record(&src, existing_raw.as_deref(), &out)) {
                Some(body) => body,
                None => serde_json::to_vec(&Value::Object(out))?,
            };
            Ok(fsx::atomic_write(&target, &body, a.mtime_ns, a.additive)?)
        }
        ActionKind::WriteTomb | ActionKind::WriteCollection | ActionKind::WriteArchiveIdx => {
            if let Some(parent) = target.parent() {
                fsx::ensure_real_dir(parent, base)?;
            }
            let payload = a.payload.as_deref().map(Vec::as_slice).unwrap_or_default();
            Ok(fsx::atomic_write(&target, payload, a.mtime_ns, a.additive)?)
        }
        ActionKind::TrashRecord | ActionKind::TrashTomb => {
            let md = match fs::symlink_metadata(&target) {
                Ok(md) => md,
                Err(_) => return Ok(false),
            };
            if !md.is_file() {
                bail!("unexpected file type: {}", target.display());
            }
            fsx::move_path(&target, &trash_target(ctx, &a.part, &a.name, stamp))?;
            Ok(true)
        }
    }
}

/// AuthManager: the winning record as Claude wrote it, with the target's own account-bound
/// members in place of the source's (none for a new copy), when that reads back as `want`.
/// cc-same's serde_json kept a record's keys in order; here a `Map` sorts them, so writing `want`
/// itself would reorder every record it touches. Falls back to that when the source changed since
/// planning or cannot be read this way.
fn spliced_record(src: &[u8], existing: Option<&[u8]>, want: &Json) -> Option<Vec<u8>> {
    let source = jsonraw::parse(src)?;
    let target = match existing {
        Some(raw) => Some(jsonraw::parse(raw)?),
        None => None,
    };
    let own = source.members.iter().filter(|m| !is_account_local(&m.key)).map(|m| m.raw);
    let bound = target.iter().flat_map(|t| t.members.iter()).filter(|m| is_account_local(&m.key)).map(|m| m.raw);
    let out = source.rebuild(own.chain(bound));
    jsonraw::reads_as(&out, want).then_some(out)
}

/// Apply `plan`, then record bookkeeping in `state` and save it.
pub fn apply_plan(ctx: &Ctx, mut plan: Plan, state: &mut State, reason: &str) -> Result<Outcome> {
    let cfg = ctx.config();
    let mut out = Outcome { deferred: plan.deferred_total(), ..Outcome::default() };
    if !plan.actions.is_empty() {
        if state.baseline.is_none() {
            let id = snapshot::take(ctx, "baseline")?;
            ctx.log(format!("baseline snapshot: {id}"));
            state.baseline = Some(id);
            state.last_snapshot_at = fsx::now_secs();
        } else if fsx::now_secs() - state.last_snapshot_at > cfg.snapshot_every_seconds {
            let id = snapshot::take(ctx, reason)?;
            ctx.debug(format!("snapshot: {id}"));
            state.last_snapshot_at = fsx::now_secs();
        }
        let known: BTreeMap<Surface, Vec<String>> = Surface::ALL.iter().map(|s| (*s, state.known(*s))).collect();
        let stamp = snapshot::stamp();
        let mut fresh = desktop::detect(ctx);
        let mut checked = Instant::now();
        for a in &plan.actions {
            // Desktop may launch or switch accounts mid-run.
            if checked.elapsed() > Duration::from_millis(250) {
                fresh = desktop::detect(ctx);
                checked = Instant::now();
            }
            let first_join = !known[&a.part.surface].contains(&a.part.key());
            let mut done = false;
            if fresh.loaded(&a.part) && !(a.additive && first_join) {
                out.skipped_loaded += 1;
            } else {
                match execute(ctx, a, &stamp) {
                    Ok(true) => {
                        done = true;
                        *out.applied.entry(a.kind).or_default() += 1;
                        if a.kind == ActionKind::WriteRecord && fresh.loaded(&a.part) {
                            *out.seeded_while_loaded.entry(a.part.key()).or_default() += 1;
                        }
                    }
                    Ok(false) => {}
                    Err(e) => {
                        let msg = format!("{:?} {}/{}: {e}", a.kind, a.part.label(), a.name);
                        ctx.log(format!("error: {msg}"));
                        out.errors.push(msg);
                    }
                }
            }
            if let (false, ActionExtra::Collection { rel, prev_base }) = (done, &a.extra) {
                // The member did not receive the merge: keep its old base, or its stale
                // content would read as its own edit next time.
                if let Some(rb) = plan.bases.get_mut(&a.part.surface).and_then(|m| m.get_mut(rel)) {
                    match prev_base {
                        Some(v) => rb.insert(a.part.key(), v.clone()),
                        None => rb.remove(&a.part.key()),
                    };
                }
            }
        }
    }
    // Bookkeeping, committed only after the writes.
    for (surface, rels) in std::mem::take(&mut plan.bases) {
        state.bases.insert(surface.as_str().to_string(), rels);
    }
    for (surface, parts) in &plan.members {
        let entry = state.known.entry(surface.as_str().to_string()).or_default();
        for p in parts {
            if !entry.contains(&p.key()) {
                entry.push(p.key());
            }
        }
        entry.sort();
    }
    state.last_sync = Some(LastSync {
        at: fsx::now_secs(),
        reason: reason.to_string(),
        applied: out.applied_total() as u64,
        errors: out.errors.len() as u64,
        deferred: out.deferred as u64,
    });
    state.save(&ctx.paths)?;
    if !plan.actions.is_empty() {
        snapshot::prune(ctx, state);
    }
    Ok(out)
}

/// Plan and apply until nothing is left (a few rounds at most), under the lock.
pub fn run_sync(ctx: &Ctx, reason: &str) -> Result<(Plan, Outcome)> {
    let _lock = Lock::acquire(ctx, Duration::from_secs(30))?;
    clear_stale_restart_hint(ctx)?;
    let mut total = Outcome::default();
    let mut last = Plan::default();
    for _ in 0..3 {
        let mut state = State::load(&ctx.paths);
        let (plan, _, _) = build_plan(ctx, None, Some(&state));
        let had_actions = !plan.actions.is_empty();
        last = plan.clone();
        let outcome = apply_plan(ctx, plan, &mut state, reason)?;
        let failed = !outcome.errors.is_empty();
        total.absorb(outcome);
        if !had_actions || failed {
            break;
        }
    }
    if !total.seeded_while_loaded.is_empty() {
        let app = desktop::detect(ctx);
        let mut state = State::load(&ctx.paths);
        let hint = state.pending_restart.get_or_insert_with(Default::default);
        for key in total.seeded_while_loaded.keys() {
            if !hint.keys.contains(key) {
                hint.keys.push(key.clone());
            }
        }
        hint.sessions += total.seeded_while_loaded.values().sum::<usize>() as u64;
        hint.init_marker = app.init_marker;
        hint.at = fsx::now_secs();
        state.save(&ctx.paths)?;
    }
    Ok((last, total))
}

/// Copied sessions show up once Desktop reloads its list: drop the hint after a quit or a
/// reload (a new "Initialization succeeded" line).
fn clear_stale_restart_hint(ctx: &Ctx) -> Result<()> {
    let mut state = State::load(&ctx.paths);
    let Some(hint) = &state.pending_restart else { return Ok(()) };
    let app = desktop::detect(ctx);
    if !app.running || app.init_marker != hint.init_marker {
        state.pending_restart = None;
        state.save(&ctx.paths)?;
    }
    Ok(())
}
