// 派生自 cc-same crates/core/src/paths.rs（v0.1.12, MIT, Songkeys）
//! Where Claude Desktop keeps its data, and where we keep ours (macOS).
//!
//! | | |
//! |---|---|
//! | Claude Desktop data | `~/Library/Application Support/Claude` |
//! | Desktop logs | `~/Library/Logs/Claude` |
//! | Our state | `~/Library/Application Support/AuthManager/claude-desktop` |
//!
//! AuthManager: macOS only, and no `CC_SAME_*` environment overrides: a caller that needs other
//! locations (tests) passes them to [`Paths::new`] or sets the fields. cc-same's own state folder
//! is never read. `CLAUDE_CONFIG_DIR` is Claude Code's own variable and still counts.

use crate::model::Surface;
use std::env;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    /// Claude Desktop's user data directory.
    pub user_data: PathBuf,
    /// Claude Desktop's log directory (holds `main.log`).
    pub desktop_logs: PathBuf,
    /// Our state: config, snapshots, trash, lock.
    pub state_dir: PathBuf,
    /// Our log file.
    pub log_file: PathBuf,
    /// Claude Code transcript roots (`~/.claude/projects`, `$CLAUDE_CONFIG_DIR/projects`).
    pub projects: Vec<PathBuf>,
    /// Claude Code user settings (`cleanupPeriodDays` lives here).
    pub claude_settings: PathBuf,
    /// Where Claude Code reads file-based managed settings (`managed-settings.json` and
    /// `managed-settings.d/`), which an organization uses to override user settings.
    pub managed_settings: PathBuf,
    /// Claude Code CLI state (its `oauthAccount` labels an account with an email).
    pub claude_json: PathBuf,
}

impl Paths {
    /// The default locations.
    pub fn detect() -> Paths {
        Paths::new(default_user_data(), default_state_dir())
    }

    pub fn new(user_data: PathBuf, state_dir: PathBuf) -> Paths {
        let desktop_logs = default_desktop_logs();
        let log_file = default_log_file(&state_dir);
        let claude_home = env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home().join(".claude"));
        let mut projects = vec![home().join(".claude").join("projects")];
        if let Some(dir) = env_path("CLAUDE_CONFIG_DIR") {
            projects.push(dir.join("projects"));
        }
        Paths {
            user_data,
            desktop_logs,
            state_dir,
            log_file,
            projects,
            claude_settings: claude_home.join("settings.json"),
            managed_settings: default_managed_settings(),
            claude_json: home().join(".claude.json"),
        }
    }

    pub fn surface_root(&self, surface: Surface) -> PathBuf {
        self.user_data.join(surface.dir_name())
    }

    /// Desktop's local Cowork sessions. Not synced; their records name the account's email.
    pub fn cowork_sessions(&self) -> PathBuf {
        self.user_data.join("local-agent-mode-sessions")
    }

    pub fn desktop_config(&self) -> PathBuf {
        self.user_data.join("config.json")
    }

    pub fn config_file(&self) -> PathBuf {
        self.state_dir.join("config.json")
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    pub fn snapshots_dir(&self) -> PathBuf {
        self.state_dir.join("snapshots")
    }

    pub fn trash_dir(&self) -> PathBuf {
        self.state_dir.join("trash")
    }

    pub fn lock_file(&self) -> PathBuf {
        self.state_dir.join("lock")
    }

    /// Written by the background sync ([`crate::watch`]) so front-ends can tell it is alive.
    pub fn heartbeat_file(&self) -> PathBuf {
        self.state_dir.join("agent.json")
    }

    /// What we last changed in Claude Code's settings, so it can be put back.
    pub fn retention_undo_file(&self) -> PathBuf {
        self.state_dir.join("retention-undo.json")
    }

    /// The numbered list of accounts to switch between.
    pub fn roster_file(&self) -> PathBuf {
        self.state_dir.join("accounts.json")
    }

    /// Claude Desktop's record of plan usage, per organization.
    pub fn usage_history(&self) -> PathBuf {
        self.user_data.join("plan-usage-history.json")
    }
}

/// Claude Code's system directory for managed settings.
fn default_managed_settings() -> PathBuf {
    PathBuf::from("/Library/Application Support/ClaudeCode")
}

/// `$HOME` (cc-same asked the dirs crate).
pub fn home() -> PathBuf {
    env_path("HOME").unwrap_or_else(|| PathBuf::from("."))
}

fn env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn application_support() -> PathBuf {
    home().join("Library").join("Application Support")
}

/// Electron's `userData` for an app named "Claude".
pub fn default_user_data() -> PathBuf {
    application_support().join("Claude")
}

fn default_desktop_logs() -> PathBuf {
    home().join("Library").join("Logs").join("Claude")
}

/// AuthManager's data folder, then this engine's.
pub fn default_state_dir() -> PathBuf {
    application_support().join("AuthManager").join("claude-desktop")
}

/// AuthManager: the log lives in the state folder (cc-same used `~/Library/Logs/cc-same.log`).
fn default_log_file(state_dir: &Path) -> PathBuf {
    state_dir.join("engine.log")
}
