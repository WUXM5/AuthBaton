//! Independent syscall/Keychain interleaving regressions; synthetic helper only.
use super::*;
use crate::claude_locks::LOST;
use std::sync::Mutex;
static SCENARIO: Mutex<Option<(PathBuf, &'static str)>> = Mutex::new(None);
static GRANTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn transport(
    url: &str,
    _token: Option<&str>,
    _body: Option<&Value>,
    _usage: bool,
) -> Result<(u16, Value)> {
    let (root, scenario) = SCENARIO.lock().unwrap().clone().unwrap();
    if url.ends_with("/v1/oauth/token") {
        GRANTS.fetch_add(1, Ordering::Relaxed);
        fs::write(root.join("armed"), scenario).unwrap();
        Ok((
            200,
            json!({"access_token":"SYNTHETIC_NEW_ACCESS","refresh_token":"SYNTHETIC_NEW_REFRESH","expires_in":3600}),
        ))
    } else if url.ends_with("/api/oauth/profile") {
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
    let mut native = fixture.native();
    native.mode = "auto".into();
    let bundle = claude("a", "SYNTHETIC_OLD_REFRESH");
    fixture
        .platform
        .write_key(
            &native.service,
            &native.account,
            &json!({"claudeAiOauth":bundle.credential}),
        )
        .unwrap();
    write_json(&native.config, &json!({"oauthAccount":bundle.identity})).unwrap();
    fixture
        .platform
        .vault_write("a", &bundle.encoded())
        .unwrap();
    let helper = FAKE_STORE.replace(
        "if operation in ('read', 'db.read'):",
        r#"
armed = root / 'armed'
scenario = armed.read_text() if armed.exists() else ''
native = request.get('service') == 'synthetic-native-only'
if scenario.startswith('late-') and native and operation == 'read':
    counter = root / 'native-reads'
    count = int(counter.read_text()) + 1 if counter.exists() else 1
    counter.write_text(str(count))
    if count == 3:
        name = '.oauth_refresh.lock' if scenario == 'late-refresh' else '.storage-write.lock'
        lock = root / 'native' / name
        lock.rename(root / 'native' / ('retired-' + name))
        lock.mkdir()
        (root / 'replacement-created').touch()
if native and operation == 'write' and (root / 'replacement-created').exists():
    (root / 'write-after-replacement').touch()
if scenario == 'native-failure' and native and operation == 'write':
    print(json.dumps({'status':'failed'})); raise SystemExit(0)
if scenario == 'native-and-next-journal-failure' and operation == 'write':
    print(json.dumps({'status':'failed'})); raise SystemExit(0)
if scenario == 'mirror-failure' and operation == 'write' and request.get('account') == 'a':
    print(json.dumps({'status':'failed'})); raise SystemExit(0)
if operation in ('read', 'db.read'):
"#,
    );
    fs::write(&fixture.platform.helper, helper).unwrap();
    *SCENARIO.lock().unwrap() = Some((fixture.root.clone(), scenario));
    GRANTS.store(0, Ordering::Relaxed);
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
fn late_lock_replacement_after_precommit_check_blocks_native_write() {
    for (scenario, lockname) in [
        ("late-refresh", ".oauth_refresh.lock"),
        ("late-storage", ".storage-write.lock"),
    ] {
        let (fixture, native, work) = setup(scenario);
        let result = work.run();
        assert!(fixture.root.join("replacement-created").exists());
        assert!(
            !fixture.root.join("write-after-replacement").exists(),
            "guard must stop before native write"
        );
        assert_eq!(
            native.read(&fixture.platform).unwrap().credential["refreshToken"],
            "SYNTHETIC_OLD_REFRESH"
        );
        assert_eq!(result.state, "unsaved");
        assert!(result.message.ends_with(LOST));
        assert!(
            native.directory.join(lockname).is_dir(),
            "replacement lock must survive Drop"
        );
        assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
    }
}
#[test]
fn post_success_native_failure_recovers_durable_rotation_without_grant() {
    let (fixture, native, work) = setup("native-failure");
    let owner = work.bundle.clone();
    let first = work.clone().run();
    assert_eq!(first.state, "unsaved");
    assert!(native.read(&fixture.platform).unwrap() == owner);
    let second = work.clone().run();
    assert_eq!(second.state, "unsaved");
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
    fs::remove_file(fixture.root.join("armed")).unwrap();
    let (recovered, refreshed) =
        crate::claude::retry(&native, &fixture.platform, "a", &owner, None).unwrap();
    assert!(refreshed);
    assert_eq!(
        recovered.credential["refreshToken"],
        "SYNTHETIC_NEW_REFRESH"
    );
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
}
#[test]
fn post_success_native_and_journal_failure_recovers_from_memory_without_regrant() {
    let (fixture, native, work) = setup("native-and-next-journal-failure");
    let owner = work.bundle.clone();
    let first = work.clone().run();
    assert_eq!(first.state, "unsaved");
    assert!(native.read(&fixture.platform).unwrap() == owner);
    fs::remove_file(fixture.root.join("armed")).unwrap();
    assert!(crate::claude::retry(&native, &fixture.platform, "a", &owner, None).is_err());
    let (recovered, refreshed) = crate::claude::retry(
        &native,
        &fixture.platform,
        "a",
        &owner,
        first.pending.as_ref(),
    )
    .unwrap();
    assert!(refreshed);
    assert_eq!(
        recovered.credential["refreshToken"],
        "SYNTHETIC_NEW_REFRESH"
    );
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
}
#[test]
fn post_success_native_and_journal_failure_regrants_the_unchanged_token_after_restart() {
    let (fixture, native, work) = setup("native-and-next-journal-failure");
    let owner = work.bundle.clone();
    drop(work.clone().run());
    fs::remove_file(fixture.root.join("armed")).unwrap();
    // The rotated result died with the process; only the token that was sent remains.
    SCENARIO.lock().unwrap().as_mut().unwrap().1 = "disarmed";
    let second = work.run();
    assert_eq!(second.state, "healthy");
    assert!(second.refreshed);
    assert_eq!(GRANTS.load(Ordering::Relaxed), 2);
    let current = native.read(&fixture.platform).unwrap();
    assert!(owner.same_owner(&current).unwrap());
    assert_eq!(current.credential["refreshToken"], "SYNTHETIC_NEW_REFRESH");
    assert!(
        fixture
            .platform
            .vault_read_optional(&crate::claude::journal_id("a"))
            .unwrap()
            .is_none()
    );
}
#[test]
fn post_success_native_committed_mirror_failed_does_not_regrant() {
    let (fixture, native, work) = setup("mirror-failure");
    let owner = work.bundle.clone();
    let first = work.clone().run();
    assert_eq!(first.state, "backup_pending");
    assert_eq!(
        native.read(&fixture.platform).unwrap().credential["refreshToken"],
        "SYNTHETIC_NEW_REFRESH"
    );
    let second = work.run();
    assert_eq!(second.state, "unsaved");
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
    fs::remove_file(fixture.root.join("armed")).unwrap();
    let (recovered, _) =
        crate::claude::retry(&native, &fixture.platform, "a", &owner, None).unwrap();
    assert_eq!(
        fixture.platform.vault_read("a").unwrap(),
        recovered.encoded()
    );
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
}

#[test]
fn manual_recovery_restores_cleared_slot_and_preserves_other_native_data() {
    let (fixture, native, work) = setup("native-failure");
    let owner = work.bundle.clone();
    assert_eq!(work.clone().run().state, "unsaved");
    fs::remove_file(fixture.root.join("armed")).unwrap();
    let mut cleared = owner.credential.clone();
    cleared["accessToken"] = json!("");
    cleared["refreshToken"] = json!("");
    cleared["expiresAt"] = json!(0);
    let payload = json!({"claudeAiOauth":cleared,"mcpOAuth":{"synthetic":"x".repeat(8192)}});
    fixture
        .platform
        .write_key(&native.service, &native.account, &payload)
        .unwrap();
    let config = fs::read(&native.config).unwrap();
    // A background pass cannot undo an official cleared login, or regrant.
    assert_eq!(work.run().state, "unsaved");
    assert_eq!(
        fixture
            .platform
            .read_key(&native.service, &native.account)
            .unwrap()
            .unwrap(),
        payload
    );
    // Use the UI's account entry after restart: it must not require valid native tokens first.
    let mut manager = fixture.manager();
    let binary = fixture.root.join("fake-claude");
    fs::write(
        &binary,
        b"#!/bin/sh\ntouch \"$0.invoked\"\necho '2.1.280 (Claude Code)'\n",
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    manager.sources.claude_bin = Some(binary);
    manager.native_override = Some(native.clone());
    manager.db.accounts.push(account("a", &owner, true));
    manager.db.accounts[0].state = "unsaved".into();
    assert!(manager.unsaved.is_empty());
    let helper = fs::read_to_string(&fixture.platform.helper).unwrap().replace(
        "operation = request['operation']",
        "operation = request['operation']\nif operation == 'read' and request.get('account') == 'a-claude-refresh' and not (root/'slow-read').exists():\n    (root/'slow-read').touch()\n    import time\n    time.sleep(5.2)",
    );
    fs::write(&fixture.platform.helper, helper).unwrap();
    let started = std::time::Instant::now();
    manager.retry_save("a").unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(manager.job.is_some());
    assert_eq!(manager.snapshot()["busy"], true);
    manager.finish_job();
    assert_eq!(manager.db.accounts[0].state, "saved");
    assert!(!fixture.root.join("fake-claude.invoked").exists());
    assert!(manager.db.accounts[0].last_refresh.is_some());
    let recovered = native.read(&fixture.platform).unwrap();
    assert_eq!(
        recovered.credential["refreshToken"],
        "SYNTHETIC_NEW_REFRESH"
    );
    assert_eq!(
        fixture
            .platform
            .read_key(&native.service, &native.account)
            .unwrap()
            .unwrap()["mcpOAuth"],
        payload["mcpOAuth"]
    );
    assert_eq!(fs::read(&native.config).unwrap(), config);
    assert_eq!(
        fixture.platform.vault_read("a").unwrap(),
        recovered.encoded()
    );
    assert!(
        fixture
            .platform
            .vault_read_optional(&crate::claude::journal_id("a"))
            .unwrap()
            .is_none()
    );
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
}

#[test]
fn cleared_recovery_requires_completed_journal_and_matching_identity() {
    for scenario in [
        "unknown-grant",
        "logout",
        "different-owner",
        "partial-clear",
    ] {
        let (fixture, native, work) = setup("native-failure");
        let owner = work.bundle.clone();
        assert_eq!(work.run().state, "unsaved");
        fs::remove_file(fixture.root.join("armed")).unwrap();
        let mut credential = owner.credential.clone();
        credential["accessToken"] = json!("");
        credential["refreshToken"] = json!("");
        credential["expiresAt"] = json!(0);
        if scenario == "partial-clear" {
            credential["expiresAt"] = json!(1000);
        }
        let payload = if scenario == "logout" {
            json!({"keep":true})
        } else {
            json!({"claudeAiOauth":credential,"keep":true})
        };
        fixture
            .platform
            .write_key(&native.service, &native.account, &payload)
            .unwrap();
        if scenario == "different-owner" {
            write_json(
                &native.config,
                &json!({"oauthAccount":claude("b", "synthetic").identity}),
            )
            .unwrap();
        }
        if scenario == "unknown-grant" {
            fixture
                .platform
                .vault_write(
                    &crate::claude::journal_id("a"),
                    &json!({"expected":owner.encoded(),"next":null}),
                )
                .unwrap();
        }
        assert!(
            crate::claude::retry(&native, &fixture.platform, "a", &owner, None).is_err(),
            "{scenario}"
        );
        assert_eq!(
            fixture
                .platform
                .read_key(&native.service, &native.account)
                .unwrap()
                .unwrap(),
            payload
        );
        assert!(
            fixture
                .platform
                .vault_read_optional(&crate::claude::journal_id("a"))
                .unwrap()
                .is_some()
        );
        assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn recovery_adopts_a_newer_official_login_without_overwriting_it() {
    let (fixture, native, work) = setup("native-failure");
    let owner = work.bundle.clone();
    assert_eq!(work.run().state, "unsaved");
    fs::remove_file(fixture.root.join("armed")).unwrap();
    let mut newer = owner.clone();
    newer.credential["accessToken"] = json!("SYNTHETIC_RELOGIN_ACCESS");
    newer.credential["refreshToken"] = json!("SYNTHETIC_RELOGIN_REFRESH");
    newer.credential["expiresAt"] = json!((now() + 7200) * 1000);
    fixture
        .platform
        .write_key(
            &native.service,
            &native.account,
            &json!({"claudeAiOauth":newer.credential}),
        )
        .unwrap();
    let (recovered, refreshed) =
        crate::claude::retry(&native, &fixture.platform, "a", &owner, None).unwrap();
    assert!(!refreshed);
    assert!(recovered == newer);
    assert!(native.read(&fixture.platform).unwrap() == newer);
    assert_eq!(fixture.platform.vault_read("a").unwrap(), newer.encoded());
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
}

#[test]
fn known_unwritable_native_payload_blocks_grant_before_rotation() {
    let (fixture, native, work) = setup("success");
    let payload = json!({"claudeAiOauth":work.bundle.credential,"mcpOAuth":"x".repeat(32768)});
    fixture
        .platform
        .write_key(&native.service, &native.account, &payload)
        .unwrap();
    let result = work.run();
    assert_eq!(result.state, "retry");
    assert!(result.message.contains("32 KB"));
    assert_eq!(GRANTS.load(Ordering::Relaxed), 0);
    assert!(
        fixture
            .platform
            .vault_read_optional(&crate::claude::journal_id("a"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .platform
            .read_key(&native.service, &native.account)
            .unwrap()
            .unwrap(),
        payload
    );
}

#[test]
fn cancelled_recovery_retains_identity_pause_and_durable_journal() {
    let (fixture, native, work) = setup("native-failure");
    let owner = work.bundle.clone();
    assert_eq!(work.run().state, "unsaved");
    let mut manager = fixture.manager();
    let binary = fixture.root.join("fake-claude");
    fs::write(&binary, b"#!/bin/sh\necho '2.1.278 (Claude Code)'\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    manager.sources.claude_bin = Some(binary);
    manager.native_override = Some(native.clone());
    manager.db.accounts.push(account("a", &owner, true));
    manager.db.accounts[0].state = "unsaved".into();
    manager.db.accounts[0].automatic = false;
    manager.retry_save("a").unwrap();
    manager
        .job
        .as_ref()
        .unwrap()
        .cancel
        .store(true, Ordering::Release);
    manager.finish_job();
    let account = &manager.db.accounts[0];
    assert_eq!(account.state, "unsaved");
    assert!(!account.automatic);
    assert_eq!(
        account.metadata.as_ref().unwrap().owner,
        owner.metadata().unwrap().owner
    );
    assert!(native.read(&fixture.platform).unwrap() == owner);
    assert!(
        fixture
            .platform
            .vault_read_optional(&crate::claude::journal_id("a"))
            .unwrap()
            .is_some()
    );
    assert_eq!(GRANTS.load(Ordering::Relaxed), 1);
}
