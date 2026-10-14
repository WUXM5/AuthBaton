use super::*;
use crate::claude_locks::{Lease, RefreshLocks};
use std::sync::Mutex;

static SCENARIO: Mutex<Option<(PathBuf, &'static str)>> = Mutex::new(None);
static GRANTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SENT: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn transport(
    url: &str,
    token: Option<&str>,
    body: Option<&Value>,
    _usage: bool,
) -> Result<(u16, Value)> {
    let scenario = SCENARIO.lock().unwrap().clone().unwrap();
    if url.ends_with("/v1/oauth/token") {
        GRANTS.fetch_add(1, Ordering::Relaxed);
        SENT.lock()
            .unwrap()
            .push(body.unwrap()["refresh_token"].as_str().unwrap().into());
        let native = scenario.0.join("native");
        assert!(native.join(".oauth_refresh.lock").is_dir());
        let mut legacy = fs::canonicalize(&native).unwrap().into_os_string();
        legacy.push(".lock");
        assert!(PathBuf::from(legacy).is_dir());
        assert!(
            !native.join(".storage-write.lock").exists(),
            "never hold writer lock over HTTP"
        );
        match scenario.1 {
            "unknown" => return Err("synthetic unknown grant"),
            "not_sent" => return Err(crate::network::NOT_SENT),
            "rejected" => return Ok((400, json!({"error":"invalid_grant"}))),
            "backup" => {
                fs::write(scenario.0.join("fail-write"), b"synthetic").unwrap();
            }
            "lost" => {
                fs::rename(native.join(".oauth_refresh.lock"), native.join("old-lock")).unwrap();
                fs::create_dir(native.join(".oauth_refresh.lock")).unwrap();
            }
            "writer_busy" => {
                fs::create_dir(native.join(".storage-write.lock")).unwrap();
            }
            "sibling" => {
                let storage = Lease::storage(&native).unwrap();
                let mut sibling = claude("a", "SYNTHETIC_SIBLING_REFRESH");
                sibling.credential["accessToken"] = json!("SYNTHETIC_SIBLING_ACCESS");
                sibling.credential["expiresAt"] = json!((now() + 3600) * 1000);
                write_json(
                    &native.join(".credentials.json"),
                    &json!({"claudeAiOauth":sibling.credential}),
                )
                .unwrap();
                drop(storage);
            }
            _ => {}
        }
        Ok((
            200,
            json!({"access_token":"SYNTHETIC_NEW_ACCESS","refresh_token":"SYNTHETIC_NEW_REFRESH","expires_in":3600}),
        ))
    } else if url.ends_with("/api/oauth/profile") {
        assert!(token.is_some());
        if scenario.1 == "profile_sibling" && token == Some("SYNTHETIC_ACCESS_a") {
            let mut sibling = claude("a", "SYNTHETIC_SIBLING_REFRESH");
            sibling.credential["accessToken"] = json!("SYNTHETIC_SIBLING_ACCESS");
            sibling.credential["expiresAt"] = json!((now() + 3600) * 1000);
            write_json(
                &scenario.0.join("native/.credentials.json"),
                &json!({"claudeAiOauth":sibling.credential}),
            )
            .unwrap();
            return Ok((401, Value::Null));
        }
        Ok((
            200,
            json!({"account":{"uuid":"a"},"organization":{"uuid":"synthetic-org"}}),
        ))
    } else {
        Ok((200, json!({"five_hour":{"utilization":12}})))
    }
}
fn setup(scenario: &'static str) -> (Fixture, NativeStore, Work) {
    let fixture = Fixture::new();
    let native = fixture.native();
    let mut bundle = claude("a", "SYNTHETIC_OLD_REFRESH");
    if scenario == "profile_sibling" {
        bundle.credential["expiresAt"] = json!((now() + 3600) * 1000);
    }
    write_json(
        &native.directory.join(".credentials.json"),
        &json!({"claudeAiOauth":bundle.credential,"keep":"untouched"}),
    )
    .unwrap();
    write_json(
        &native.config,
        &json!({"oauthAccount":bundle.identity,"projects":{"keep":true}}),
    )
    .unwrap();
    fixture
        .platform
        .vault_write("a", &bundle.encoded())
        .unwrap();
    GRANTS.store(0, Ordering::Relaxed);
    SENT.lock().unwrap().clear();
    *SCENARIO.lock().unwrap() = Some((fixture.root.clone(), scenario));
    let work = Work {
        id: "a".into(),
        bundle,
        platform: fixture.platform.clone(),
        native: Some(native.clone()),
        profile: fixture.root.join("unused"),
        codex: None,
        cancel: Arc::new(AtomicBool::new(false)),
        transport,
    };
    (fixture, native, work)
}
#[test]
fn native_refresh_commits_then_mirrors_without_login_or_config_changes() {
    let (fixture, native, work) = setup("success");
    let config = fs::read(&native.config).unwrap();
    let result = work.run();
    assert_eq!(result.state, "healthy");
    assert!(result.refreshed);
    assert!(result.online);
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
    assert!(native.read(&fixture.platform).unwrap() == result.bundle);
    assert_eq!(
        fixture.platform.vault_read("a").unwrap(),
        result.bundle.encoded()
    );
    assert!(
        fixture
            .platform
            .vault_read_optional(&crate::claude::journal_id("a"))
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read(&native.config).unwrap(), config);
    assert_eq!(
        read_json(&native.directory.join(".credentials.json"))
            .unwrap()
            .unwrap()["keep"],
        "untouched"
    );
}
#[test]
fn locked_native_store_never_posts_and_adopts_sibling_on_next_attempt() {
    let (fixture, native, work) = setup("success");
    let locks = RefreshLocks::acquire(&native.directory).unwrap();
    let result = work.clone().run();
    assert_eq!(result.state, "retry");
    assert_eq!(GRANTS.load(Ordering::Relaxed), 0);
    let mut sibling = work.bundle.clone();
    sibling.credential["expiresAt"] = json!((now() + 3600) * 1000);
    sibling.credential["accessToken"] = json!("SYNTHETIC_SIBLING_ACCESS");
    native
        .refresh(&fixture.platform, &work.bundle, &sibling)
        .unwrap();
    drop(locks);
    let result = work.run();
    assert_eq!(result.state, "healthy");
    assert!(!result.refreshed);
    assert!(result.bundle == sibling);
    assert_eq!(GRANTS.load(Ordering::Relaxed), 0);
}
#[test]
fn newer_native_write_wins_compare_and_swap() {
    let (fixture, native, work) = setup("sibling");
    let result = work.run();
    assert_eq!(result.state, "healthy");
    assert!(!result.refreshed);
    assert_eq!(
        result.bundle.credential["refreshToken"],
        "SYNTHETIC_SIBLING_REFRESH"
    );
    assert_eq!(
        fixture.platform.vault_read("a").unwrap(),
        native.read(&fixture.platform).unwrap().encoded()
    );
}
#[test]
fn profile_401_rereads_native_and_does_not_refresh_a_siblings_new_token() {
    let (_fixture, _native, work) = setup("profile_sibling");
    let result = work.run();
    assert_eq!(result.state, "healthy");
    assert_eq!(
        result.bundle.credential["accessToken"],
        "SYNTHETIC_SIBLING_ACCESS"
    );
    assert_eq!(GRANTS.load(Ordering::Relaxed), 0);
}
#[test]
fn lost_refresh_lease_preserves_new_token_without_committing_or_deleting_successor() {
    let (fixture, native, work) = setup("lost");
    let old = work.bundle.clone();
    let result = work.run();
    assert_eq!(result.state, "unsaved");
    assert!(!result.refreshed);
    assert!(native.read(&fixture.platform).unwrap() == old);
    assert!(native.directory.join(".oauth_refresh.lock").exists());
    fs::remove_dir(native.directory.join(".oauth_refresh.lock")).unwrap();
    let (recovered, refreshed) =
        crate::claude::retry(&native, &fixture.platform, "a", &old, None).unwrap();
    assert!(refreshed);
    assert_eq!(
        recovered.credential["refreshToken"],
        "SYNTHETIC_NEW_REFRESH"
    );
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
}
#[test]
fn durable_rotation_recovers_after_restart_without_second_grant() {
    let (fixture, native, work) = setup("writer_busy");
    let old = work.bundle.clone();
    let result = work.run();
    assert_eq!(result.state, "unsaved");
    assert!(native.read(&fixture.platform).unwrap() == old);
    drop(result);
    fs::remove_dir(native.directory.join(".storage-write.lock")).unwrap();
    let (recovered, refreshed) =
        crate::claude::retry(&native, &fixture.platform, "a", &old, None).unwrap();
    assert!(refreshed);
    assert_eq!(
        recovered.credential["refreshToken"],
        "SYNTHETIC_NEW_REFRESH"
    );
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
}
#[test]
fn failed_vault_mirror_never_reuses_old_refresh_and_recovers_from_native() {
    let (fixture, native, work) = setup("backup");
    let old = work.bundle.clone();
    let result = work.clone().run();
    assert_eq!(result.state, "backup_pending");
    assert!(result.refreshed);
    assert_eq!(
        native.read(&fixture.platform).unwrap().credential["refreshToken"],
        "SYNTHETIC_NEW_REFRESH"
    );
    let again = work.run();
    assert_eq!(again.state, "unsaved");
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
    fs::remove_file(fixture.root.join("fail-write")).unwrap();
    let (bundle, _) = crate::claude::retry(&native, &fixture.platform, "a", &old, None).unwrap();
    assert_eq!(fixture.platform.vault_read("a").unwrap(), bundle.encoded());
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
}
fn rescenario(scenario: &'static str) {
    SCENARIO.lock().unwrap().as_mut().unwrap().1 = scenario;
}
fn journal(fixture: &Fixture) -> Option<Value> {
    fixture
        .platform
        .vault_read_optional(&crate::claude::journal_id("a"))
        .unwrap()
}
#[test]
fn unknown_grant_is_reconciled_by_regranting_the_same_refresh_token() {
    let (fixture, native, work) = setup("unknown");
    assert_eq!(work.clone().run().state, "uncertain");
    assert!(journal(&fixture).is_some());
    assert_eq!(work.clone().run().state, "uncertain");
    rescenario("success");
    let result = work.run();
    assert_eq!(result.state, "healthy");
    assert!(result.refreshed);
    assert_eq!(*SENT.lock().unwrap(), ["SYNTHETIC_OLD_REFRESH"; 3]);
    assert_eq!(
        native.read(&fixture.platform).unwrap().credential["refreshToken"],
        "SYNTHETIC_NEW_REFRESH"
    );
    assert!(journal(&fixture).is_none());
}
#[test]
fn unknown_grant_adopts_an_official_renewal_without_another_grant() {
    let (fixture, native, work) = setup("unknown");
    assert_eq!(work.clone().run().state, "uncertain");
    let mut sibling = work.bundle.clone();
    sibling.credential["refreshToken"] = json!("SYNTHETIC_OFFICIAL_REFRESH");
    native
        .refresh(&fixture.platform, &work.bundle, &sibling)
        .unwrap();
    rescenario("success");
    let result = work.run();
    assert_eq!(result.state, "healthy");
    assert!(result.bundle == sibling);
    assert!(!result.refreshed);
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
    assert!(journal(&fixture).is_none());
}
#[test]
fn reconciliation_rejected_by_the_issuer_requires_login() {
    let (fixture, native, work) = setup("unknown");
    assert_eq!(work.clone().run().state, "uncertain");
    rescenario("rejected");
    assert_eq!(work.clone().run().state, "reauth");
    assert_eq!(GRANTS.load(Ordering::Relaxed), 2);
    assert!(native.read(&fixture.platform).unwrap() == work.bundle);
    assert!(journal(&fixture).is_none());
}
#[test]
fn a_request_that_never_left_is_an_ordinary_retry_without_a_journal() {
    let (fixture, native, work) = setup("not_sent");
    let result = work.clone().run();
    assert_eq!(result.state, "retry");
    assert_eq!(result.message, crate::network::NOT_SENT);
    assert!(native.read(&fixture.platform).unwrap() == work.bundle);
    assert!(journal(&fixture).is_none());
}
#[test]
fn keychain_is_authoritative_and_fallback_is_not_rewritten_on_renewal() {
    let (fixture, mut native, work) = setup("success");
    native.mode = "auto".into();
    fixture
        .platform
        .write_key(
            &native.service,
            &native.account,
            &json!({"claudeAiOauth":work.bundle.credential,"keychainOnly":true}),
        )
        .unwrap();
    fs::write(
        native.directory.join(".credentials.json"),
        b"INVALID_OBSOLETE_FALLBACK",
    )
    .unwrap();
    assert!(native.read(&fixture.platform).unwrap() == work.bundle);
    let work = Work {
        native: Some(native.clone()),
        ..work
    };
    let result = work.run();
    assert_eq!(result.state, "healthy");
    assert!(result.refreshed);
    assert!(native.read(&fixture.platform).unwrap() == result.bundle);
    assert_eq!(
        fs::read(native.directory.join(".credentials.json")).unwrap(),
        b"INVALID_OBSOLETE_FALLBACK"
    );
}
#[test]
fn native_current_auto_check_is_queued_even_with_running_official_client() {
    let (fixture, native, work) = setup("success");
    let mut manager = fixture.manager();
    let binary = fixture.root.join("fake-claude");
    fs::write(
        &binary,
        b"#!/bin/sh\ntouch \"$0.invoked\"\necho '99.0.0 (Claude Code)'\n",
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    manager.sources.claude_bin = Some(binary);
    manager.native_override = Some(native.clone());
    manager.client_running = |_| true;
    manager.db.accounts.push(account("a", &work.bundle, true));
    // Hold the official lock so the worker cannot reach the real transport.
    let locks = RefreshLocks::acquire(&native.directory).unwrap();
    manager.check("a", false).unwrap();
    assert_eq!(manager.db.accounts[0].state, "checking");
    manager.finish_job();
    assert_eq!(manager.db.accounts[0].state, "retry");
    assert!(!fixture.root.join("fake-claude.invoked").exists());
    assert_eq!(
        manager.snapshot()["accounts"][0]["renewal_mode"],
        "claude_coordinated"
    );
    drop(locks);
}
#[test]
fn claude_login_starts_without_probing_cli_version() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let binary = fixture.root.join("fake-claude");
    fs::write(
        &binary,
        b"#!/bin/sh\nif [ \"$1\" = '--version' ]; then\n touch \"$0.version\"\n echo '99.0.0 (Claude Code)'\n exit 0\nfi\nprintf '%s\\n' \"$@\" > \"$0.partial\"\nmv \"$0.partial\" \"$0.args\"\n",
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    manager.sources.claude_bin = Some(binary);
    let mut login = LoginManager::new();
    manager
        .begin_login("claude", "Synthetic login", None, &mut login)
        .unwrap();
    // Starting a shell can take seconds on a loaded machine; the fake renames its finished file.
    let started = std::time::Instant::now();
    while !fixture.root.join("fake-claude.args").exists() {
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        fs::read_to_string(fixture.root.join("fake-claude.args")).unwrap(),
        "auth\nlogin\n--claudeai\n"
    );
    assert!(!fixture.root.join("fake-claude.version").exists());
    assert!(login.state.as_ref().unwrap().active());
}
#[test]
fn claude_account_switch_uses_storage_without_probing_cli_version() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let binary = fixture.root.join("fake-claude");
    fs::write(
        &binary,
        b"#!/bin/sh\ntouch \"$0.invoked\"\necho '99.0.0 (Claude Code)'\n",
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    manager.sources.claude_bin = Some(binary);
    let native = fixture.native();
    let original = claude("a", "SYNTHETIC_A");
    let target = claude("b", "SYNTHETIC_B");
    native.initialize(&fixture.platform, &original).unwrap();
    fixture
        .platform
        .vault_write("a", &original.encoded())
        .unwrap();
    fixture
        .platform
        .vault_write("b", &target.encoded())
        .unwrap();
    manager.native_override = Some(native.clone());
    manager.db.accounts.push(account("a", &original, true));
    manager.db.accounts.push(account("b", &target, false));
    manager.activate("b", true).unwrap();
    assert!(native.read(&fixture.platform).unwrap() == target);
    assert!(!manager.db.accounts[0].native && manager.db.accounts[1].native);
    assert!(manager.db.switch.is_none());
    assert!(!fixture.root.join("fake-claude.invoked").exists());
}
#[test]
fn secure_storage_override_controls_locks_and_fallback_but_not_identity_config() {
    let fixture = Fixture::new();
    let home = fixture.root.join("home");
    let config = fixture.root.join("config");
    let secure = fixture.root.join("secure");
    let store = NativeStore::claude(&home, Some(&config), Some(secure.to_str().unwrap())).unwrap();
    assert_eq!(store.directory, secure);
    assert_eq!(store.config, config.join(".claude.json"));
    let empty = NativeStore::claude(&home, Some(&config), Some("")).unwrap();
    assert_eq!(empty.directory, home.join(".claude"));
    assert_eq!(empty.service, "Claude Code-credentials");
    assert!(NativeStore::claude(&home, Some(&config), Some("relative")).is_err());
}
#[test]
fn claude_switch_runs_beside_official_clients_under_their_locks() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    manager.client_running = |_| true;
    let native = fixture.native();
    let original = claude("a", "SYNTHETIC_A");
    let target = claude("b", "SYNTHETIC_B");
    native.initialize(&fixture.platform, &original).unwrap();
    for (id, bundle) in [("a", &original), ("b", &target)] {
        fixture.platform.vault_write(id, &bundle.encoded()).unwrap();
    }
    manager.native_override = Some(native.clone());
    manager.db.accounts.push(account("a", &original, true));
    manager.db.accounts.push(account("b", &target, false));
    // Any official refresh, credential write or config save in progress defers the switch
    // before anything is written.
    let holds: [&dyn Fn() -> Box<dyn std::any::Any>; 3] = [
        &|| Box::new(RefreshLocks::acquire(&native.directory).unwrap()),
        &|| Box::new(Lease::storage(&native.directory).unwrap()),
        &|| Box::new(Lease::config(&native.config).unwrap()),
    ];
    for hold in holds {
        let lease = hold();
        assert_eq!(
            manager.activate("b", true).unwrap_err(),
            crate::claude_locks::BUSY
        );
        assert!(native.read(&fixture.platform).unwrap() == original);
        assert!(manager.db.switch.is_none() && manager.db.accounts[0].native);
        drop(lease);
    }
    manager.activate("b", true).unwrap();
    assert!(native.read(&fixture.platform).unwrap() == target);
    assert!(!manager.db.accounts[0].native && manager.db.accounts[1].native);
    assert!(manager.db.accounts[1].message.contains("30 秒内"));
    let mut config_lock = native.config.as_os_str().to_owned();
    config_lock.push(".lock");
    for lock in [
        native.directory.join(".oauth_refresh.lock"),
        native.directory.join(".storage-write.lock"),
        PathBuf::from(config_lock),
    ] {
        assert!(!lock.exists(), "{lock:?} must be released");
    }
}
