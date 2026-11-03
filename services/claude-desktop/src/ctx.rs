// 派生自 cc-same crates/core/src/ctx.rs（v0.1.12, MIT, Songkeys）
use crate::config::Config;
use crate::fsx;
use crate::model::Record;
use crate::paths::Paths;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

/// A stand-in for Claude Desktop, for tests: whether it runs, and which `<account>[/<org>]` it
/// has loaded. AuthManager: set by the caller only (cc-same also read `CC_SAME_FAKE_*` from the
/// environment).
#[derive(Clone, Debug, Default)]
pub struct FakeDesktop {
    pub running: Option<bool>,
    pub active: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub enum LogSink {
    Stderr,
    File(PathBuf),
    Silent,
}

/// AuthManager: the log file is rotated past this size (one older file is kept), and only its
/// owner can read it. It holds what the engine did and why, never the contents of Claude's
/// sign-in files.
const LOG_MAX_BYTES: u64 = 512 * 1024;

pub struct Logger {
    sink: Mutex<LogSink>,
}

impl Logger {
    pub fn new(sink: LogSink) -> Logger {
        Logger { sink: Mutex::new(sink) }
    }

    pub fn set_sink(&self, sink: LogSink) {
        *self.sink.lock().unwrap() = sink;
    }

    pub fn log(&self, msg: impl AsRef<str>) {
        let msg = msg.as_ref();
        match &*self.sink.lock().unwrap() {
            LogSink::Stderr => eprintln!("{msg}"),
            LogSink::Silent => {}
            LogSink::File(path) => {
                if fs::metadata(path).map(|m| m.len() > LOG_MAX_BYTES).unwrap_or(false) {
                    let _ = fs::rename(path, path.with_extension("log.1"));
                }
                if let Some(parent) = path.parent() {
                    let _ = fsx::create_private_dir_all(parent);
                }
                if let Ok(mut f) = OpenOptions::new().create(true).append(true).mode(0o600).open(path) {
                    if f.metadata().is_ok_and(|m| m.permissions().mode() & 0o777 != 0o600) {
                        let _ = f.set_permissions(fs::Permissions::from_mode(0o600));
                    }
                    let now = crate::localtime::now(c"%Y-%m-%d %H:%M:%S");
                    let _ = writeln!(f, "{now} {msg}");
                }
            }
        }
    }
}

pub(crate) type RecordKey = (PathBuf, i128, u64);
/// When the transcript index was built, and the ids it found (None: no projects folder).
pub(crate) type TranscriptCache = Option<(Instant, Option<Arc<HashSet<String>>>)>;

#[derive(Default)]
pub(crate) struct LogScan {
    pub offset: u64,
    pub found: Option<(String, String)>,
    /// Byte offset of the line `found` came from.
    pub found_at: u64,
    /// When that line was written (Unix seconds), from its timestamp.
    pub found_time: Option<f64>,
}

#[derive(Default)]
pub(crate) struct Caches {
    pub records: Mutex<HashMap<RecordKey, Record>>,
    pub recent_accounts: Mutex<HashMap<String, Instant>>,
    pub transcripts: Mutex<TranscriptCache>,
    pub log_scan: Mutex<HashMap<PathBuf, LogScan>>,
    /// AuthManager: syncs asked for through [`crate::api::sync_once`] that are running now (the
    /// app knew this from its own busy state).
    pub syncing: AtomicUsize,
    /// AuthManager: since when a sync has had something to do, as [`crate::api::overview`] last
    /// saw it (the app kept this in its store).
    pub pending_since: Mutex<Option<Instant>>,
}

/// Everything a sync needs: locations, settings, test overrides, a logger and caches.
/// Shareable across threads.
pub struct Ctx {
    pub paths: Paths,
    config: RwLock<Config>,
    pub fake: FakeDesktop,
    pub logger: Logger,
    pub verbose: bool,
    pub(crate) caches: Caches,
}

impl Ctx {
    pub fn new(paths: Paths, config: Config, fake: FakeDesktop, sink: LogSink) -> Ctx {
        Ctx {
            paths,
            config: RwLock::new(config),
            fake,
            logger: Logger::new(sink),
            verbose: false,
            caches: Caches::default(),
        }
    }

    /// Default paths and the saved config.
    pub fn detect(sink: LogSink) -> anyhow::Result<Ctx> {
        let paths = Paths::detect();
        let config = Config::load(&paths)?;
        Ok(Ctx::new(paths, config, FakeDesktop::default(), sink))
    }

    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    pub fn set_config(&self, config: Config) {
        *self.config.write().unwrap() = config;
    }

    /// Re-read `config.json` (the background sync does this every pass).
    pub fn reload_config(&self) -> anyhow::Result<()> {
        let cfg = Config::load(&self.paths)?;
        self.set_config(cfg);
        Ok(())
    }

    pub fn log(&self, msg: impl AsRef<str>) {
        self.logger.log(msg);
    }

    pub fn debug(&self, msg: impl AsRef<str>) {
        if self.verbose {
            self.logger.log(msg);
        }
    }
}
