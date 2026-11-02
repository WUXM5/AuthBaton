// 派生自 cc-same crates/core/src/retention.rs（v0.1.12, MIT, Songkeys）
//! How long Claude Code keeps the transcripts behind Claude Desktop's sessions, and changing it.
//!
//! Claude Code 2.1.248 and later keep the transcript of a session started or last continued in
//! Claude Desktop or Cowork at any age. `desktopSessionCleanupPeriodDays` can give them a limit
//! (a transcript goes once it is older than both that and `cleanupPeriodDays`), and a managed
//! `cleanupPeriodDays` applies to them as well. Earlier versions deleted them after
//! `cleanupPeriodDays`, 30 days by default, which also governs terminal sessions and other data.
//! A Desktop session whose transcript is gone still shows in the list but opens empty.
//!
//! AuthManager: a change keeps the rest of Claude Code's settings file as written (cc-same's
//! serde_json kept its keys in order; here a `Map` would sort them), the backup is named
//! `.authmanager-backup-<time>`, and cc-same's own earlier change (0.1.0's ten years) is not ours
//! to undo.

use crate::fsx;
use crate::jsonraw;
use crate::paths::Paths;
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const CLEANUP: &str = "cleanupPeriodDays";
pub const DESKTOP_CLEANUP: &str = "desktopSessionCleanupPeriodDays";
/// Claude Code's default for `cleanupPeriodDays`.
pub const DEFAULT_DAYS: f64 = 30.0;
/// The first Claude Code that keeps Desktop and Cowork transcripts at any age.
const KEEPS_DESKTOP: [u32; 3] = [2, 1, 248];
/// What `keep` sets `cleanupPeriodDays` to on an older Claude Code.
const TEN_YEARS: u64 = 3650;
const MAX_SETTINGS: u64 = 16 * 1024 * 1024;

/// Why the transcripts of Desktop sessions have an age limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// `desktopSessionCleanupPeriodDays` in the user's settings.
    DesktopSetting,
    /// The organization's managed settings.
    Organization,
    /// A Claude Code before 2.1.248, which applies `cleanupPeriodDays` to every transcript.
    OlderClaudeCode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Retention {
    /// Days after which Claude Code deletes a Desktop session's transcript; `None`: never.
    pub desktop_days: Option<f64>,
    pub limited_by: Option<Limit>,
    /// `cleanupPeriodDays` in effect, for terminal sessions and other data.
    pub cleanup_days: f64,
    /// The newest Claude Code that Claude Desktop has installed, if found.
    pub claude_code: Option<String>,
    /// A change we made that `undo` can put back.
    pub undo: Option<Undo>,
}

impl Default for Retention {
    fn default() -> Retention {
        Retention { desktop_days: None, limited_by: None, cleanup_days: DEFAULT_DAYS, claude_code: None, undo: None }
    }
}

impl Retention {
    /// Desktop sessions lose their transcripts within a year.
    pub fn is_short(&self) -> bool {
        self.desktop_days.is_some_and(|d| d < 365.0)
    }
}

/// A change we made to Claude Code's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Undo {
    pub key: String,
    /// What we wrote (`None`: the key was removed). Undo applies only while this is in place.
    pub set: Option<Value>,
    /// The value before (`None`: the key was not set).
    pub previous: Option<Value>,
}

/// What `keep` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    Already,
    AnyAge,
    TenYears,
}

pub fn read(paths: &Paths) -> Retention {
    let user = settings(&paths.claude_settings).unwrap_or_default();
    let managed = managed(&paths.managed_settings);
    let managed_cleanup = number(&managed, CLEANUP);
    let cleanup_days = managed_cleanup.or_else(|| number(&user, CLEANUP)).unwrap_or(DEFAULT_DAYS);
    let managed_desktop = number(&managed, DESKTOP_CLEANUP);
    let desktop_setting = managed_desktop.or_else(|| number(&user, DESKTOP_CLEANUP));
    let version = claude_code_version(paths);
    // A Desktop without a Claude Code of its own yet will install a current one.
    let keeps_desktop = version.as_ref().is_none_or(|(v, _)| *v >= KEEPS_DESKTOP);
    let (desktop_days, limited_by) = if let Some(days) = managed_cleanup {
        (Some(days), Some(Limit::Organization))
    } else if !keeps_desktop {
        (Some(cleanup_days), Some(Limit::OlderClaudeCode))
    } else if let Some(days) = desktop_setting.filter(|d| *d > 0.0) {
        let by = if managed_desktop.is_some() { Limit::Organization } else { Limit::DesktopSetting };
        (Some(days.max(cleanup_days)), Some(by))
    } else {
        (None, None)
    };
    Retention {
        desktop_days,
        limited_by,
        cleanup_days,
        claude_code: version.map(|(_, name)| name),
        undo: pending_undo(paths, &user),
    }
}

/// Keep the transcripts of Desktop sessions: lift the Desktop limit, or on a Claude Code before
/// 2.1.248 raise `cleanupPeriodDays` to ten years. The settings file is backed up and the change
/// remembered, so `undo` can put it back.
pub fn keep(paths: &Paths) -> Result<Kept> {
    match read(paths).limited_by {
        None => Ok(Kept::Already),
        Some(Limit::Organization) => bail!("your organization sets how long Claude Code keeps transcripts"),
        Some(Limit::DesktopSetting) => {
            change(paths, DESKTOP_CLEANUP, None)?;
            Ok(Kept::AnyAge)
        }
        Some(Limit::OlderClaudeCode) => {
            change(paths, CLEANUP, Some(Value::from(TEN_YEARS)))?;
            Ok(Kept::TenYears)
        }
    }
}

/// Put back what we changed, if it is still in place. Returns whether anything changed.
pub fn undo(paths: &Paths) -> Result<bool> {
    let user = settings(&paths.claude_settings).unwrap_or_default();
    let Some(u) = pending_undo(paths, &user) else { return Ok(false) };
    write_key(paths, &u.key, u.previous)?;
    let _ = fs::remove_file(paths.retention_undo_file());
    Ok(true)
}

/// Set `cleanupPeriodDays` (terminal sessions and other data). Claude Code rejects 0.
pub fn set_cleanup_days(paths: &Paths, days: u32) -> Result<Option<PathBuf>> {
    anyhow::ensure!(days >= 1, "cleanupPeriodDays must be at least 1 (Claude Code rejects 0)");
    write_key(paths, CLEANUP, Some(Value::from(days)))
}

fn change(paths: &Paths, key: &str, value: Option<Value>) -> Result<()> {
    let user = settings(&paths.claude_settings).unwrap_or_default();
    let record = Undo { key: key.into(), set: value.clone(), previous: user.get(key).cloned() };
    write_key(paths, key, value)?;
    fsx::create_private_dir_all(&paths.state_dir)?;
    fsx::atomic_write(&paths.retention_undo_file(), &serde_json::to_vec_pretty(&record)?, None, false)?;
    Ok(())
}

/// Write one key of Claude Code's user settings (`None` removes it), keeping every other key. The
/// previous file is kept next to it as `settings.json.authmanager-backup-<time>`; returns its path.
fn write_key(paths: &Paths, key: &str, value: Option<Value>) -> Result<Option<PathBuf>> {
    let path = &paths.claude_settings;
    let mut map = Map::new();
    let mut raw = None;
    let mut backup = None;
    if path.exists() {
        let bytes = fsx::read_limited(path, MAX_SETTINGS).with_context(|| format!("cannot read {}", path.display()))?;
        map = match serde_json::from_slice(&bytes).with_context(|| format!("cannot read {}", path.display()))? {
            Value::Object(m) => m,
            _ => bail!("{} is not a JSON object; left untouched", path.display()),
        };
        let mut name = path.file_name().context("settings path has no file name")?.to_os_string();
        name.push(format!(".authmanager-backup-{}", crate::snapshot::stamp()));
        let b = fsx::unique_path(path.with_file_name(name));
        fs::copy(path, &b)?;
        backup = Some(b);
        raw = Some(bytes);
    } else if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match &value {
        Some(v) => map.insert(key.into(), v.clone()),
        None => map.remove(key),
    };
    let spliced = raw.as_deref().and_then(|raw| spliced_settings(raw, key, value.as_ref(), &map));
    let body = match spliced {
        Some(body) => body,
        None => {
            let mut body = serde_json::to_vec_pretty(&Value::Object(map))?;
            body.push(b'\n');
            body
        }
    };
    fsx::atomic_write(path, &body, None, false)?;
    Ok(backup)
}

/// AuthManager: the settings file as written with only `key` changed (in its place, or added at
/// the end), when that reads back as `want`.
fn spliced_settings(raw: &[u8], key: &str, value: Option<&Value>, want: &Map<String, Value>) -> Option<Vec<u8>> {
    let obj = jsonraw::parse(raw).filter(|o| !o.members.is_empty())?;
    let new = value.map(|v| obj.member(key, v));
    let mut members: Vec<&[u8]> = Vec::new();
    let mut placed = false;
    for m in &obj.members {
        if m.key != key {
            members.push(m.raw);
        } else if let (Some(new), false) = (&new, placed) {
            members.push(new);
            placed = true;
        }
    }
    if let (Some(new), false) = (&new, placed) {
        members.push(new);
    }
    let out = obj.rebuild(members);
    jsonraw::reads_as(&out, want).then_some(out)
}

/// The change we can put back while it is still in place, from our own record.
fn pending_undo(paths: &Paths, user: &Map<String, Value>) -> Option<Undo> {
    let record = fsx::read_json(&paths.retention_undo_file(), 1024 * 1024).ok()?;
    let u = serde_json::from_value::<Undo>(record).ok()?;
    same(user.get(&u.key), u.set.as_ref()).then_some(u)
}

/// Two settings values, numbers compared by value (`3650` and `3650.0` are the same).
fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y || x.as_f64().is_some_and(|x| y.as_f64() == Some(x)),
        (None, None) => true,
        _ => false,
    }
}

fn settings(path: &Path) -> Option<Map<String, Value>> {
    match fsx::read_json(path, MAX_SETTINGS) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

/// File-based managed settings: `managed-settings.json` and `managed-settings.d/*.json`.
fn managed(dir: &Path) -> Map<String, Value> {
    let mut files = vec![dir.join("managed-settings.json")];
    if let Ok(entries) = fs::read_dir(dir.join("managed-settings.d")) {
        let mut drop_ins: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        drop_ins.sort();
        files.extend(drop_ins);
    }
    let mut out = Map::new();
    for f in files {
        if let Some(m) = settings(&f) {
            out.extend(m);
        }
    }
    out
}

fn number(map: &Map<String, Value>, key: &str) -> Option<f64> {
    map.get(key).and_then(Value::as_f64)
}

/// The newest Claude Code that Claude Desktop has installed, from `<user data>/claude-code/<version>`.
fn claude_code_version(paths: &Paths) -> Option<([u32; 3], String)> {
    fs::read_dir(paths.user_data.join("claude-code"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            parse_version(&name).map(|v| (v, name))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
}

fn parse_version(s: &str) -> Option<[u32; 3]> {
    let mut parts = s.split('.').map(|p| p.parse::<u32>().ok());
    let v = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Home {
        _tmp: crate::testing::TempDir,
        paths: Paths,
    }

    impl Home {
        fn new(claude_code: &[&str]) -> Home {
            let tmp = crate::testing::tempdir().unwrap();
            let root = tmp.path();
            let mut paths = Paths::new(root.join("Claude"), root.join("state"));
            paths.claude_settings = root.join("claude/settings.json");
            paths.managed_settings = root.join("managed");
            for v in claude_code {
                fs::create_dir_all(paths.user_data.join("claude-code").join(v)).unwrap();
            }
            Home { _tmp: tmp, paths }
        }

        fn settings(&self, v: Value) {
            fs::create_dir_all(self.paths.claude_settings.parent().unwrap()).unwrap();
            fs::write(&self.paths.claude_settings, v.to_string()).unwrap();
        }

        fn read_settings(&self) -> Value {
            serde_json::from_slice(&fs::read(&self.paths.claude_settings).unwrap()).unwrap()
        }
    }

    #[test]
    fn current_claude_code_keeps_desktop_transcripts() {
        let h = Home::new(&["2.1.273", "2.1.281"]);
        h.settings(json!({ "model": "opus" }));
        let r = read(&h.paths);
        assert_eq!(r.desktop_days, None);
        assert!(!r.is_short());
        assert_eq!(r.cleanup_days, 30.0);
        assert_eq!(r.claude_code.as_deref(), Some("2.1.281"));
        assert_eq!(keep(&h.paths).unwrap(), Kept::Already);
    }

    #[test]
    fn a_desktop_limit_counts_no_shorter_than_cleanup() {
        let h = Home::new(&["2.1.281"]);
        h.settings(json!({ "desktopSessionCleanupPeriodDays": 7 }));
        let r = read(&h.paths);
        assert_eq!((r.desktop_days, r.limited_by), (Some(30.0), Some(Limit::DesktopSetting)));
        assert!(r.is_short());
    }

    #[test]
    fn older_claude_code_applies_cleanup_to_desktop() {
        let h = Home::new(&["2.1.200"]);
        let r = read(&h.paths);
        assert_eq!((r.desktop_days, r.limited_by), (Some(30.0), Some(Limit::OlderClaudeCode)));
    }

    #[test]
    fn a_managed_cleanup_wins() {
        let h = Home::new(&["2.1.281"]);
        fs::create_dir_all(h.paths.managed_settings.join("managed-settings.d")).unwrap();
        fs::write(h.paths.managed_settings.join("managed-settings.d/retention.json"), r#"{"cleanupPeriodDays": 14}"#)
            .unwrap();
        let r = read(&h.paths);
        assert_eq!((r.desktop_days, r.limited_by), (Some(14.0), Some(Limit::Organization)));
        assert!(keep(&h.paths).is_err());
    }

    #[test]
    fn keep_lifts_the_desktop_limit_and_undo_puts_it_back() {
        let h = Home::new(&["2.1.281"]);
        h.settings(json!({ "desktopSessionCleanupPeriodDays": 60, "model": "opus" }));
        assert_eq!(keep(&h.paths).unwrap(), Kept::AnyAge);
        assert_eq!(h.read_settings(), json!({ "model": "opus" }));
        assert!(!read(&h.paths).is_short());
        assert!(read(&h.paths).undo.is_some());
        assert!(undo(&h.paths).unwrap());
        assert_eq!(h.read_settings(), json!({ "desktopSessionCleanupPeriodDays": 60, "model": "opus" }));
        assert_eq!(read(&h.paths).undo, None);
    }

    #[test]
    fn keep_on_older_claude_code_sets_ten_years_and_undo_removes_it() {
        let h = Home::new(&["2.1.200"]);
        h.settings(json!({}));
        assert_eq!(keep(&h.paths).unwrap(), Kept::TenYears);
        assert_eq!(h.read_settings(), json!({ "cleanupPeriodDays": 3650 }));
        assert!(undo(&h.paths).unwrap());
        assert_eq!(h.read_settings(), json!({}));
    }

    #[test]
    fn undo_is_not_offered_once_the_setting_changed_again() {
        let h = Home::new(&["2.1.200"]);
        h.settings(json!({ "cleanupPeriodDays": 20 }));
        keep(&h.paths).unwrap();
        h.settings(json!({ "cleanupPeriodDays": 90 }));
        assert_eq!(read(&h.paths).undo, None);
        assert!(!undo(&h.paths).unwrap());
        assert_eq!(h.read_settings(), json!({ "cleanupPeriodDays": 90 }));
    }

    /// Only the key changes; the rest of the file stays as the user wrote it.
    #[test]
    fn a_change_keeps_the_rest_of_the_settings_file_as_written() {
        let h = Home::new(&["2.1.281"]);
        let file = h.paths.claude_settings.clone();
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let text = "{\n  \"model\": \"opus\",\n  \"desktopSessionCleanupPeriodDays\": 60,\n  \"env\": {\"B\": \"1\", \"A\": \"2\"}\n}\n";
        fs::write(&file, text).unwrap();
        assert_eq!(keep(&h.paths).unwrap(), Kept::AnyAge);
        let kept = "{\n  \"model\": \"opus\",\n  \"env\": {\"B\": \"1\", \"A\": \"2\"}\n}\n";
        assert_eq!(fs::read_to_string(&file).unwrap(), kept);
        assert!(undo(&h.paths).unwrap());
        let back = "{\n  \"model\": \"opus\",\n  \"env\": {\"B\": \"1\", \"A\": \"2\"},\n  \"desktopSessionCleanupPeriodDays\": 60\n}\n";
        assert_eq!(fs::read_to_string(&file).unwrap(), back);
    }

    #[test]
    fn versions_parse_strictly() {
        assert_eq!(parse_version("2.1.248"), Some([2, 1, 248]));
        assert_eq!(parse_version("2.1"), None);
        assert_eq!(parse_version("2.1.248.1"), None);
        assert_eq!(parse_version("latest"), None);
    }
}
