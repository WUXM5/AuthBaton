//! Public request path with a deliberately blocked synthetic credential reader.
use super::*;
use std::time::{Duration, Instant};

#[test]
fn slow_auth_read_keeps_rpc_responsive_and_queues_other_accounts_in_order() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    manager.codex = Some(fixture.codex(false));
    let a = codex_bundle();
    let b = claude("b", "SYNTHETIC_B");
    let c = claude("c", "SYNTHETIC_C");
    for (id, bundle) in [("a", &a), ("b", &b), ("c", &c)] {
        fixture.platform.vault_write(id, &bundle.encoded()).unwrap();
        manager.db.accounts.push(account(id, bundle, false));
    }
    let helper = FAKE_STORE.replace(
        "operation = request['operation']",
        "operation = request['operation']\nif operation == 'read' and request.get('account') == 'a':\n    import time\n    (root/'reader-started').touch()\n    deadline = time.monotonic() + 3\n    while not (root/'release-reader').exists() and time.monotonic() < deadline:\n        time.sleep(0.01)",
    );
    fs::write(&fixture.platform.helper, helper).unwrap();
    let sources = manager.sources.clone();
    let mut accounts = Some(manager);
    let mut login = LoginManager::new();
    let mut shutdown = false;
    let mut request = |method: &str, params: Value| {
        crate::response(
            &serde_json::to_vec(&json!({"version":1,"id":"test","method":method,"params":params}))
                .unwrap(),
            &sources,
            None,
            &mut login,
            &mut shutdown,
            &mut accounts,
        )
    };
    let started = Instant::now();
    let response = request("accounts.check", json!({"id":"a","online":true}));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        response["result"]["directory"]["accounts"][0]["check_status"],
        "running"
    );
    assert_eq!(response["result"]["directory"]["checks_blocked"], false);
    let started = Instant::now();
    while !fixture.root.join("reader-started").exists() {
        assert!(started.elapsed() < Duration::from_secs(1));
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    assert!(request("ping", Value::Null)["result"].is_object());
    assert!(request("accounts.check", json!({"id":"b","online":false}))["error"].is_null());
    assert!(request("accounts.check", json!({"id":"c","online":false}))["error"].is_null());
    assert!(request("accounts.check", json!({"id":"b","online":false}))["error"].is_null());
    let listing = request("accounts.list", Value::Null);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        listing["result"]["directory"]["accounts"][1]["check_status"],
        "queued"
    );
    assert_eq!(
        listing["result"]["directory"]["accounts"][2]["check_status"],
        "queued"
    );
    assert!(request("accounts.update", json!({"id":"c","label":"changed"}))["error"].is_object());
    assert!(request("accounts.activate", json!({"id":"c","confirm":true}))["error"].is_object());
    assert!(!listing.to_string().contains("SYNTHETIC_"));
    let manager = accounts.as_mut().unwrap();
    assert_eq!(manager.check_queue.len(), 2);
    // Only IDs are queued: the worker must see credentials updated before its turn.
    let mut newer = b.clone();
    newer.credential["expiresAt"] = json!(9_000_000);
    fixture.platform.vault_write("b", &newer.encoded()).unwrap();
    fs::write(fixture.root.join("release-reader"), b"release").unwrap();
    manager.finish_job();
    assert_eq!(manager.db.accounts[0].state, "healthy");
    manager.start_next_check();
    assert_eq!(manager.job.as_ref().unwrap().id, "b");
    manager.finish_job();
    assert_eq!(
        manager.db.accounts[1].metadata.as_ref().unwrap().expires_at,
        Some(9000)
    );
    manager.start_next_check();
    assert_eq!(manager.job.as_ref().unwrap().id, "c");
    manager.finish_job();
    assert!(manager.check_queue.is_empty());
    assert_eq!(manager.snapshot()["busy"], false);
    assert!(
        manager.snapshot()["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["check_status"].is_null())
    );
}

#[test]
fn read_failure_preserves_identity_and_does_not_block_the_next_check() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    for id in ["a", "b"] {
        let bundle = claude(id, "SYNTHETIC_REFRESH");
        manager.db.accounts.push(account(id, &bundle, false));
        if id == "b" {
            fixture.platform.vault_write(id, &bundle.encoded()).unwrap();
        }
    }
    manager.local_check("a").unwrap();
    manager.local_check("b").unwrap();
    manager.finish_job();
    assert_eq!(manager.db.accounts[0].state, "attention");
    assert!(manager.db.accounts[0].metadata.is_some());
    manager.start_next_check();
    manager.finish_job();
    assert_eq!(manager.db.accounts[1].state, "saved");
    assert!(manager.db.accounts[1].last_local.is_some());
    assert!(!manager.snapshot()["busy"].as_bool().unwrap());
}

#[test]
fn async_native_observation_retires_the_old_default_when_identity_changed() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let native = fixture.native();
    let expected = claude("a", "SYNTHETIC_A");
    let other = claude("other", "SYNTHETIC_OTHER");
    native.initialize(&fixture.platform, &other).unwrap();
    manager.native_override = Some(native.clone());
    manager.db.accounts.push(account("a", &expected, true));
    manager.local_check("a").unwrap();
    manager.finish_job();
    assert_eq!(manager.db.accounts[0].state, "attention");
    assert!(!manager.db.accounts[0].native);
    assert_eq!(
        manager.db.accounts[0].metadata.as_ref().unwrap().owner,
        expected.metadata().unwrap().owner
    );
    assert!(native.read(&fixture.platform).unwrap() == other);
}

#[test]
fn stopping_discards_unstarted_checks_and_recovery_still_blocks_admission() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let bundle = claude("a", "SYNTHETIC_REFRESH");
    fixture
        .platform
        .vault_write("a", &bundle.encoded())
        .unwrap();
    for id in ["a", "b"] {
        manager.db.accounts.push(account(id, &bundle, false));
    }
    manager.local_check("a").unwrap();
    manager.local_check("b").unwrap();
    manager.stop();
    assert!(manager.check_queue.is_empty());
    assert!(manager.active_check.is_none());
    assert!(manager.db.accounts[1].last_local.is_none());
    let reopened = fixture.manager();
    assert!(reopened.check_queue.is_empty());
    assert!(reopened.active_check.is_none());
    manager.job = Some(Job {
        id: "a".into(),
        cancel: Arc::new(AtomicBool::new(false)),
        handle: std::thread::spawn(|| JobResult::Recovery(Err("synthetic recovery pending"))),
    });
    assert!(manager.local_check("b").is_err());
    manager.finish_job();
}
