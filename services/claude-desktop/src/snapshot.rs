// 派生自 cc-same crates/core/src/snapshot.rs（v0.1.12, MIT, Songkeys）
//! Snapshots (copy-on-write clones of every index folder), restore, and turning symlinked
//! folders left by other tools back into real ones.

use crate::apply::Lock;
use crate::config::State;
use crate::ctx::Ctx;
use crate::desktop;
use crate::fsx;
use crate::model::{Surface, is_uuid};
use crate::scan;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotPart {
    pub surface: String,
    pub acct: String,
    pub org: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub reason: String,
    /// Seconds since the Unix epoch.
    pub created: f64,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub partitions: Vec<SnapshotPart>,
}

/// Local time, `YYYYMMDD-HHMMSS`.
pub fn stamp() -> String {
    crate::localtime::now(c"%Y%m%d-%H%M%S")
}

/// Clone every index folder into a new snapshot. Returns its id.
pub fn take(ctx: &Ctx, reason: &str) -> Result<String> {
    let dir = ctx.paths.snapshots_dir();
    fsx::create_private_dir_all(&dir)?;
    let mut id = format!("{}-{reason}", stamp());
    let mut n = 1;
    while dir.join(&id).exists() {
        n += 1;
        id = format!("{}-{reason}-{n}", stamp());
    }
    let root = dir.join(&id);
    fsx::create_private_dir_all(&root)?;
    let mut saved = Vec::new();
    for surface in Surface::ALL {
        for p in scan::discover(ctx, surface) {
            if p.is_link {
                continue;
            }
            let dst = root.join(surface.as_str()).join(&p.acct).join(&p.org);
            fsx::create_private_dir_all(dst.parent().unwrap())?;
            fsx::copy_tree(&p.path, &dst).with_context(|| format!("snapshot of {}", p.path.display()))?;
            saved.push(SnapshotPart { surface: surface.as_str().into(), acct: p.acct, org: p.org });
        }
    }
    let manifest = Manifest {
        id: id.clone(),
        reason: reason.to_string(),
        created: fsx::now_secs(),
        version: crate::VERSION.into(),
        partitions: saved,
    };
    fs::write(root.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    Ok(id)
}

pub fn list(ctx: &Ctx) -> Vec<Manifest> {
    let Ok(entries) = fs::read_dir(ctx.paths.snapshots_dir()) else { return Vec::new() };
    let mut names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|n| {
            let raw = fs::read(ctx.paths.snapshots_dir().join(&n).join("manifest.json")).ok()?;
            serde_json::from_slice(&raw).ok()
        })
        .collect()
}

/// Keep the baseline plus the newest `keepSnapshots`; empty trash older than `trashDays`.
pub fn prune(ctx: &Ctx, state: &State) {
    let cfg = ctx.config();
    let snaps: Vec<Manifest> = list(ctx).into_iter().filter(|s| Some(&s.id) != state.baseline.as_ref()).collect();
    let excess = snaps.len().saturating_sub(cfg.keep_snapshots);
    for s in snaps.iter().take(excess) {
        let _ = fs::remove_dir_all(ctx.paths.snapshots_dir().join(&s.id));
    }
    let max_age = Duration::from_secs_f64(cfg.trash_days.max(0.0) * 86400.0);
    if let Ok(entries) = fs::read_dir(ctx.paths.trash_dir()) {
        for e in entries.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > max_age);
            if old {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

fn require_quit(ctx: &Ctx) -> Result<()> {
    if desktop::is_running(ctx) {
        bail!("quit Claude first, then try again");
    }
    Ok(())
}

/// Put every index folder in the snapshot back. The current folders go to the trash first,
/// after a `pre-restore` snapshot. Returns that snapshot's id.
pub fn restore(ctx: &Ctx, id: &str) -> Result<String> {
    require_quit(ctx)?;
    let root = ctx.paths.snapshots_dir().join(id);
    let raw = fs::read(root.join("manifest.json")).with_context(|| format!("no such snapshot: {id}"))?;
    let manifest: Manifest = serde_json::from_slice(&raw)?;
    let _lock = Lock::acquire(ctx, Duration::from_secs(30))?;
    let pre = take(ctx, "pre-restore")?;
    let stamp = stamp();
    for part in &manifest.partitions {
        let Some(surface) = Surface::parse(&part.surface) else { continue };
        if !is_uuid(&part.acct) || !is_uuid(&part.org) {
            continue;
        }
        let current = ctx.paths.surface_root(surface).join(&part.acct).join(&part.org);
        let src = root.join(surface.as_str()).join(&part.acct).join(&part.org);
        if fs::symlink_metadata(&current).is_ok() {
            let old = ctx
                .paths
                .trash_dir()
                .join(&stamp)
                .join("replaced-by-restore")
                .join(surface.as_str())
                .join(&part.acct)
                .join(&part.org);
            fsx::move_path(&current, &fsx::unique_path(old))?;
        }
        fsx::ensure_real_dir(current.parent().unwrap(), &ctx.paths.user_data)?;
        fsx::copy_tree(&src, &current)?;
    }
    let mut state = State::load(&ctx.paths);
    state.bases.clear(); // re-learn collection bases from the restored files
    state.save(&ctx.paths)?;
    Ok(pre)
}

/// Index folders that are symlinks (left by "merge + symlink" tools).
pub fn symlinked_folders(ctx: &Ctx) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for surface in Surface::ALL {
        let Ok(accts) = fs::read_dir(ctx.paths.surface_root(surface)) else { continue };
        for a in accts.flatten() {
            let name = a.file_name().to_string_lossy().into_owned();
            if !is_uuid(&name) {
                continue;
            }
            if fsx::is_symlink(&a.path()) {
                out.push(a.path());
                continue;
            }
            let Ok(orgs) = fs::read_dir(a.path()) else { continue };
            for o in orgs.flatten() {
                if is_uuid(&o.file_name().to_string_lossy()) && fsx::is_symlink(&o.path()) {
                    out.push(o.path());
                }
            }
        }
    }
    out.sort();
    out
}

/// Replace each symlinked index folder with a real folder holding a copy of its target.
pub fn fix_symlinks(ctx: &Ctx) -> Result<Vec<PathBuf>> {
    require_quit(ctx)?;
    let todo = symlinked_folders(ctx);
    if todo.is_empty() {
        return Ok(todo);
    }
    let _lock = Lock::acquire(ctx, Duration::from_secs(30))?;
    let stamp = stamp();
    let mut fixed = Vec::new();
    for link in todo {
        let Ok(real) = fs::canonicalize(&link) else { continue };
        if !real.is_dir() {
            continue;
        }
        let mut tmp = link.clone().into_os_string();
        tmp.push(".authmanager-materialize");
        let tmp = PathBuf::from(tmp);
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fsx::copy_tree(&real, &tmp)?;
        let rel = link.strip_prefix(&ctx.paths.user_data).unwrap_or(&link);
        let trash = fsx::unique_path(ctx.paths.trash_dir().join(&stamp).join("symlinks").join(rel));
        fsx::move_path(&link, &trash)?;
        fs::rename(&tmp, &link)?;
        fixed.push(link);
    }
    Ok(fixed)
}
