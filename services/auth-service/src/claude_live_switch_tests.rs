//! Opt-in steps for scripts/claude-protocol/live-hot-switch.py: a real Claude CLI runs in its own
//! CLAUDE_CONFIG_DIR while these steps switch that store with the production locks and writer.
//! The default login and AuthManager's vault are only read; copies carry an unusable refresh
//! token so the test CLI can never rotate a real login. Without the driver's env each step skips.
use super::*;

const UNUSABLE_REFRESH: &str = "authmanager-live-test-unusable-refresh";

fn isolated() -> Option<(PathBuf, NativeStore, Platform)> {
    let Some(dir) = std::env::var_os("AUTHMANAGER_LIVE_CLAUDE_DIR") else {
        eprintln!("skipped: run through scripts/claude-protocol/live-hot-switch.py");
        return None;
    };
    let dir = PathBuf::from(dir);
    assert!(
        dir.starts_with("/private/tmp/"),
        "the live store must stay in a temporary directory"
    );
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let store = NativeStore::claude(&home, Some(&dir), None).unwrap();
    assert!(
        store.service.starts_with("Claude Code-credentials-") && store.config.starts_with(&dir)
    );
    let platform = Platform {
        // The pinned helper that owns the vault items, so reading them never prompts.
        helper: home.join("Library/Application Support/AuthManager/credential-store-v1"),
        recovery_wait: None,
        fixture_keychain: false,
    };
    Some((home, store, platform))
}
fn copy_of(home: &Path, platform: &Platform, source: &str) -> Bundle {
    let mut bundle = if source == "native" {
        NativeStore::claude(home, None, None)
            .unwrap()
            .read(platform)
            .unwrap()
    } else {
        Bundle::decode(platform.vault_read(source).unwrap()).unwrap()
    };
    assert_eq!(bundle.provider, "claude");
    let expires = bundle.metadata().unwrap().expires_at.unwrap();
    assert!(
        expires > now() + 900,
        "{source}: access token expires too soon for a live run"
    );
    bundle.credential["refreshToken"] = json!(UNUSABLE_REFRESH);
    bundle
}
fn source() -> String {
    std::env::var("AUTHMANAGER_LIVE_SOURCE").unwrap()
}

#[test]
#[ignore = "driven by scripts/claude-protocol/live-hot-switch.py"]
fn live_claude_prepare() {
    let Some((home, store, platform)) = isolated() else {
        return;
    };
    store
        .initialize(&platform, &copy_of(&home, &platform, &source()))
        .unwrap();
}

#[test]
#[ignore = "driven by scripts/claude-protocol/live-hot-switch.py"]
fn live_claude_switch() {
    let Some((home, store, platform)) = isolated() else {
        return;
    };
    let target = copy_of(&home, &platform, &source());
    let locks = crate::claude::SwitchLocks::acquire(&store).unwrap();
    let current = store.read(&platform).unwrap();
    store
        .apply_guarded(&platform, &current, &target, &|| locks.check())
        .unwrap();
    locks.check().unwrap();
}

#[test]
#[ignore = "driven by scripts/claude-protocol/live-hot-switch.py"]
fn live_claude_expect() {
    let Some((home, store, platform)) = isolated() else {
        return;
    };
    // The running CLI must neither write the previous login back nor rotate the copy.
    assert!(store.read(&platform).unwrap() == copy_of(&home, &platform, &source()));
}

#[test]
#[ignore = "driven by scripts/claude-protocol/live-hot-switch.py"]
fn live_claude_cleanup() {
    let Some((_, store, platform)) = isolated() else {
        return;
    };
    platform
        .native_delete_key(&store.service, &store.account)
        .unwrap();
}
