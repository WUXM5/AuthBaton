// 派生自 cc-same crates/core/src/lib.rs（v0.1.12, MIT, Songkeys）
//! Keep Claude Desktop's local Code sessions identical across all your accounts, and switch the
//! account Claude Desktop is signed in to without signing out. macOS only.
//!
//! Derived from cc-same 0.1.12 (commit 3d5183b, <https://github.com/songkeys/cc-same>, MIT,
//! Copyright (c) 2026 Songkeys; the license is in `LICENSE-cc-same.txt`). Each module ported from
//! its engine (`crates/core`) keeps the upstream file's name and structure so later upstream
//! changes can be merged file by file. What changed for AuthManager:
//! * macOS only: the Windows and Linux branches are gone.
//! * No login agent (`service.rs`) and no notifications (`notify.rs`): AuthManager's service runs
//!   [`watch`] on a thread of its own ([`api::run_watch`]), and its window tells the user.
//! * Our state lives in `~/Library/Application Support/AuthManager/claude-desktop`; nothing of
//!   cc-same's own state is read.
//! * No chrono, dirs, reflink-copy or tempfile: [`localtime`] and [`fsx`] use libc directly.
//! * serde_json runs without `preserve_order` (it is shared with the rest of the workspace), so
//!   a `Map` sorts its keys. Files Claude or the user wrote are edited member by member instead
//!   ([`jsonraw`]), and the places that depended on key order were changed (see the notes in
//!   `merge`, `plan`, `apply`, `logins` and `retention`).
//! * [`api`] is new: the engine as AuthManager's service and window use it, every answer a
//!   `Serialize` struct without credentials.
//!
//! How every index is kept identical (upstream):
//! * Every linked index stays a real folder. Desktop 2.x opens its storage folder with
//!   `O_DIRECTORY|O_NOFOLLOW` and silently stops saving sessions through a symlink.
//! * One writer at a time. Desktop writes only the loaded account's index and reads it only
//!   when that account loads. We never modify an index Desktop has loaded; we mirror it to
//!   the others and catch it up after an account switch or a quit.
//! * Per session the newest file wins; mtimes travel with copies, so a copy never looks newer
//!   than its source. Deletions travel as Desktop's own `deleted_<id>` markers.
//! * Account-bound fields (org connectors, Remote Control mirrors, published artifacts, pins)
//!   stay with the account that owns them.
//! * Nothing is hard-deleted: copy-on-write snapshots before changes, a trash folder for
//!   anything removed or replaced.

// Upstream is edition 2021; its nested `if let`s stay as they are so upstream changes merge
// cleanly.
#![allow(clippy::collapsible_if)]

#[cfg(not(target_os = "macos"))]
compile_error!("claude-desktop supports macOS only");

pub mod accounts;
pub mod api;
pub mod apply;
pub mod config;
pub mod desktop;
pub mod fsx;
pub mod jsonraw;
pub mod localtime;
pub mod logins;
pub mod merge;
pub mod model;
pub mod paths;
pub mod plan;
pub mod report;
pub mod retention;
pub mod scan;
pub mod snapshot;
pub mod usage;
pub mod watch;

mod ctx;

pub use config::{Config, State};
pub use ctx::{Ctx, FakeDesktop, LogSink, Logger};
pub use model::*;
pub use paths::Paths;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Temporary folders for unit tests (cc-same used the tempfile crate).
#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn tempdir() -> std::io::Result<TempDir> {
        static N: AtomicU64 = AtomicU64::new(0);
        loop {
            let n = N.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!("claude-desktop-test-{}-{n}-{nanos}", std::process::id()));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(TempDir(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }
}
