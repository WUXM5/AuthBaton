// AuthManager 新增（cc-same 里没有这个文件）：换号时把「允许 Bypass 权限模式」带给换上的账号。
//! Bypass permissions mode, which Claude Desktop allows per account.
//!
//! Desktop keeps the switch (Settings → Claude Code) in `claude_desktop_config.json`, under
//! `preferences.bypassPermissionsGateByAccount`, one entry per account. A session started while
//! the signed-in account has it off drops from bypass to "accept edits", and Desktop saves that
//! in the session's record, which the sync then copies to every account. Sessions in different
//! modes cannot message each other in Desktop: the message is held for an approval nobody can
//! give there, and expires.
//!
//! So a switch away from an account that allows bypass turns it on for the account switched to.
//! Desktop reads the file once, when it starts, and writes it whole from memory: it is only
//! changed while Claude is not running, which a switch makes sure of.

use crate::ctx::Ctx;
use crate::fsx;
use crate::jsonraw;
use anyhow::{Context as _, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

/// What a switch did about bypass permissions mode for the account switched to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Carried {
    /// Nothing to do: the account left does not allow it, or the one switched to already does.
    #[default]
    Nothing,
    TurnedOn,
    /// It should have been turned on and could not be (the reason is in the log).
    Failed,
}

/// The accounts Desktop allows bypass permissions mode for.
pub fn allowed(ctx: &Ctx) -> BTreeSet<String> {
    let settings = fsx::read_json(&ctx.paths.desktop_settings(), 16 * 1024 * 1024).unwrap_or_default();
    let gate = settings.pointer("/preferences/bypassPermissionsGateByAccount").and_then(Value::as_object);
    gate.into_iter().flatten().filter(|(_, on)| **on == Value::Bool(true)).map(|(account, _)| account.clone()).collect()
}

/// Allow bypass permissions mode for `to` when `from` allows it. Only while Claude is not running.
pub fn carry(ctx: &Ctx, from: &str, to: &str) -> Carried {
    let allowed = allowed(ctx);
    if !allowed.contains(from) || allowed.contains(to) {
        return Carried::Nothing;
    }
    match turn_on(ctx, to) {
        Ok(()) => Carried::TurnedOn,
        Err(e) => {
            ctx.log(format!("allowing bypass permissions mode for the account switched to: {e:#}"));
            Carried::Failed
        }
    }
}

fn turn_on(ctx: &Ctx, account: &str) -> Result<()> {
    let path = ctx.paths.desktop_settings();
    let before = fsx::read_limited(&path, 16 * 1024 * 1024).context("reading Claude's settings")?;
    let after =
        jsonraw::with_member(&before, &["preferences", "bypassPermissionsGateByAccount", account], &Value::Bool(true))
            .context("Claude's settings are not written the way they were expected to be")?;
    fsx::atomic_write(&path, &after, None, false).context("writing Claude's settings")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::{Config, FakeDesktop, LogSink};
    use std::fs;

    const ADA: &str = "5a1d3c07-8f2e-4b6a-9c1d-2e3f4a5b6c7d";
    const GRACE: &str = "9e31ea7e-1c2d-4e5f-8a9b-0c1d2e3f4a5b";

    fn mac(settings: Option<&str>) -> (crate::testing::TempDir, Ctx) {
        let tmp = crate::testing::tempdir().unwrap();
        let paths = Paths::new(tmp.path().join("Claude"), tmp.path().join("state"));
        fs::create_dir_all(&paths.user_data).unwrap();
        if let Some(text) = settings {
            fs::write(paths.desktop_settings(), text).unwrap();
        }
        let fake = FakeDesktop { running: Some(false), active: None };
        (tmp, Ctx::new(paths, Config::default(), fake, LogSink::Silent))
    }

    fn settings(gate: &str) -> String {
        format!(
            "{{\n  \"mcpServers\": {{\"z\": 1,   \"a\": 2}},\n  \"preferences\": {{\n    \"sidebarMode\": \"code\",\n    \"bypassPermissionsGateByAccount\": {gate},\n    \"keepAwakeEnabled\": false\n  }}\n}}"
        )
    }

    #[test]
    fn the_account_switched_to_is_allowed_bypass_when_the_one_left_is() {
        let before = settings(&format!("{{\n      \"{ADA}\": true\n    }}"));
        let (_tmp, ctx) = mac(Some(&before));
        assert_eq!(allowed(&ctx), BTreeSet::from([ADA.to_string()]));
        assert_eq!(carry(&ctx, ADA, GRACE), Carried::TurnedOn);
        assert_eq!(allowed(&ctx), BTreeSet::from([ADA.to_string(), GRACE.to_string()]));
        // Everything else in Claude's settings is as it was, byte for byte.
        let after = fs::read_to_string(ctx.paths.desktop_settings()).unwrap();
        assert_eq!(after, before.replace("true\n", &format!("true,\n      \"{GRACE}\": true\n")));
        // Done once: the next switch finds nothing to do, and leaves the file alone.
        assert_eq!(carry(&ctx, ADA, GRACE), Carried::Nothing);
        assert_eq!(fs::read_to_string(ctx.paths.desktop_settings()).unwrap(), after);
    }

    #[test]
    fn bypass_is_not_turned_on_when_the_account_left_does_not_allow_it() {
        for gate in ["{}".to_string(), format!("{{\"{ADA}\": false}}"), format!("{{\"{GRACE}\": true}}")] {
            let before = settings(&gate);
            let (_tmp, ctx) = mac(Some(&before));
            assert_eq!(carry(&ctx, ADA, GRACE), Carried::Nothing, "{gate}");
            assert_eq!(fs::read_to_string(ctx.paths.desktop_settings()).unwrap(), before);
        }
        // No settings at all: Claude was never opened, or keeps none.
        let (_tmp, ctx) = mac(None);
        assert_eq!(carry(&ctx, ADA, GRACE), Carried::Nothing);
        assert!(!ctx.paths.desktop_settings().exists());
    }

    #[test]
    fn a_settings_file_that_cannot_be_written_is_reported() {
        let (_tmp, ctx) = mac(Some(&settings(&format!("{{\"{ADA}\": true}}"))));
        // Claude's settings folder is read-only: the new file cannot be put in place.
        let dir = ctx.paths.user_data.clone();
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&dir, perms.clone()).unwrap();
        let carried = carry(&ctx, ADA, GRACE);
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&dir, perms).unwrap();
        assert_eq!(carried, Carried::Failed);
        assert_eq!(allowed(&ctx), BTreeSet::from([ADA.to_string()]));
    }
}
