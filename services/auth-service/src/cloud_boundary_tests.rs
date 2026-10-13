use super::*;

fn bundle(owner: &str, refresh: &str, expires: u64) -> Bundle {
    let mut b = claude(owner, refresh);
    b.credential["expiresAt"] = json!(expires * 1000);
    b
}

#[test]
fn cloud_default_is_local_and_mode_changes_do_not_read_credentials_or_rewrite_accounts() {
    let f = Fixture::new();
    let mut m = f.manager();
    let before = m.snapshot();
    assert_eq!(m.cloud.config.mode, "local");
    m.cloud_command(
        "configure",
        Some("cloud"),
        Some("https://auth.example.invalid"),
        Some("Synthetic Mac"),
        None,
        false,
        None,
    )
    .unwrap();
    assert_eq!(m.snapshot()["accounts"], before["accounts"]);
    assert!(!m.cloud.config.paired);
    assert!(m.cloud.config.bindings.is_empty());
    assert!(
        f.platform
            .vault_read_optional("cloud-device-v1")
            .unwrap()
            .is_none()
    );
    let reopened = f.manager();
    assert_eq!(reopened.cloud.config.mode, "cloud");
}

#[test]
fn cloud_access_projection_cannot_become_a_local_refresh_bundle() {
    let original = bundle("alice", "SYNTHETIC-refresh", now() + 3600);
    let projected = crate::cloud::projection(&original);
    assert!(projected.credential.get("refreshToken").is_none());
    assert!(projected.metadata().is_err());
    assert_eq!(
        projected.cloud_metadata().unwrap().owner,
        original.metadata().unwrap().owner
    );
    assert!(original.cloud_metadata().is_err());
}

#[test]
fn cloud_native_cas_preserves_unrelated_fields_and_never_overwrites_a_newer_login() {
    let f = Fixture::new();
    let n = f.native();
    let a = bundle("alice", "SYNTHETIC-refresh", now() + 3600);
    n.initialize(&f.platform, &a).unwrap();
    let mut payload = read_json(&n.directory.join(".credentials.json"))
        .unwrap()
        .unwrap();
    payload["synthetic_other"] = json!({"keep":true});
    write_json(&n.directory.join(".credentials.json"), &payload).unwrap();
    let p = crate::cloud::projection(&a);
    n.cloud_publish(&f.platform, &a, &p).unwrap();
    assert!(n.cloud_read(&f.platform).unwrap() == p);
    assert_eq!(
        read_json(&n.directory.join(".credentials.json"))
            .unwrap()
            .unwrap()["synthetic_other"]["keep"],
        true
    );
    let other = bundle("bob", "SYNTHETIC-other", now() + 3600);
    write_json(
        &n.directory.join(".credentials.json"),
        &json!({"claudeAiOauth":other.credential}),
    )
    .unwrap();
    assert!(n.cloud_publish(&f.platform, &p, &p).is_err());
    assert_eq!(
        read_json(&n.directory.join(".credentials.json"))
            .unwrap()
            .unwrap()["claudeAiOauth"],
        other.credential
    );
}

#[test]
fn cloud_native_initialize_and_aborted_transfer_restore_are_distinct_paths() {
    let f = Fixture::new();
    let n = f.native();
    let a = bundle("alice", "SYNTHETIC-refresh", now() + 3600);
    let p = crate::cloud::projection(&a);
    n.cloud_initialize(&f.platform, &p).unwrap();
    assert!(n.cloud_read(&f.platform).unwrap() == p);
    assert!(n.cloud_initialize(&f.platform, &p).is_err());
    n.cloud_restore(&f.platform, &p, &a).unwrap();
    assert!(n.read(&f.platform).unwrap() == a);
}

#[test]
fn transferred_accounts_are_fenced_from_all_local_refresh_and_activation_entries_after_restart() {
    let f = Fixture::new();
    let mut m = f.manager();
    let a = bundle("alice", "SYNTHETIC-refresh", now() + 3600);
    f.platform.vault_write("alice", &a.encoded()).unwrap();
    m.db.accounts.push(account("alice", &a, false));
    m.save().unwrap();
    m.cloud.config.bindings.insert(
        "alice".into(),
        crate::cloud::Binding {
            operation: "synthetic-op".into(),
            owner: a.metadata().unwrap().owner,
            provider: "claude".into(),
            phase: "prepared".into(),
            ..Default::default()
        },
    );
    m.cloud.save().unwrap();
    assert!(m.request_check("alice", true).is_err());
    assert!(m.activate("alice", true).is_err());
    assert!(m.configure("alice", None, Some(true)).is_err());
    let mut reopened = f.manager();
    assert!(reopened.request_check("alice", true).is_err());
    reopened.cloud_tick();
    assert!(!reopened.db.accounts[0].automatic);
    assert_eq!(f.platform.vault_read("alice").unwrap(), a.encoded());
}

#[test]
fn cloud_selected_broker_accounts_do_not_retarget_explicit_local_sessions() {
    let f = Fixture::new();
    let mut m = f.manager();
    m.cloud.config.mode = "cloud".into();
    m.cloud.config.accounts = vec![
        json!({"id":"synthetic-cloud","provider":"codex","label":"Cloud","state":"ready","automatic":true}),
    ];
    m.cloud
        .config
        .selected
        .insert("codex_broker".into(), "synthetic-cloud".into());
    assert_eq!(
        m.pi_accounts()["accounts"][0]["id"],
        "cloud-synthetic-cloud"
    );
    assert_eq!(m.pi_accounts()["accounts"][0]["current"], true);
    assert!(m.pi_access(Some("unknown-local-account")).is_err());
    assert!(m.cloud.job.is_none());
}

fn cloud_config(f: &Fixture) -> (crate::cloud::Config, PathBuf) {
    let cfg = crate::cloud::Config {
        endpoint: "https://synthetic.example.invalid".into(),
        paired: true,
        ..Default::default()
    };
    f.platform
        .vault_write(
            "cloud-device-v1",
            &json!({"endpoint":cfg.endpoint,"token":"SYNTHETIC-device"}),
        )
        .unwrap();
    let path = f.root.join("cloud-config.json");
    write_json(&path, &serde_json::to_value(&cfg).unwrap()).unwrap();
    (cfg, path)
}

fn cloud_lease(target: &Bundle, version: u64) -> Value {
    let metadata = target.cloud_metadata().unwrap();
    json!({"id":"synthetic-cloud","provider":target.provider,"label":"Synthetic cloud","owner":metadata.owner,"version":version,"expires_at":metadata.expires_at,"access_token":target.credential["accessToken"],"projection":target.encoded()})
}

#[test]
fn cloud_cancelled_reauthorization_does_not_remain_queued_for_automatic_reimport() {
    let f = Fixture::new();
    let (mut cfg, path) = cloud_config(&f);
    let a = bundle("alice", "SYNTHETIC-new-authorization", now() + 3600);
    f.platform.vault_write("alice-new", &a.encoded()).unwrap();
    f.platform
        .vault_write("cloud-import-alice-new", &a.encoded())
        .unwrap();
    cfg.reauthorizations
        .insert("alice-new".into(), "synthetic-cloud".into());
    cfg.bindings.insert(
        "alice-new".into(),
        crate::cloud::Binding {
            operation: "synthetic-reauthorization".into(),
            cloud_id: Some("synthetic-cloud".into()),
            owner: a.metadata().unwrap().owner,
            provider: "claude".into(),
            phase: "imported".into(),
            replace_id: Some("synthetic-cloud".into()),
            ..Default::default()
        },
    );
    write_json(&path, &serde_json::to_value(&cfg).unwrap()).unwrap();
    crate::cloud::test_transport(Box::new(|_, action, _, _| match action {
        "import.abort" => Ok(json!({"aborted":true})),
        "accounts" => Ok(json!({"accounts":[],"devices":[]})),
        _ => panic!("unexpected synthetic cloud action"),
    }));
    let result =
        crate::cloud::cancel_transfer(cfg, f.platform.clone(), path, "alice-new".into(), None)
            .unwrap();
    assert!(!result.config.bindings.contains_key("alice-new"));
    assert!(
        !result.config.reauthorizations.contains_key("alice-new"),
        "cancel must remove the cloud_tick trigger, otherwise saved login is automatically reimported"
    );
    assert_eq!(f.platform.vault_read("alice-new").unwrap(), a.encoded());
}

#[test]
fn cloud_remote_commit_with_lost_ack_cannot_be_cancelled_into_local_refresh() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let f = Fixture::new();
    let (mut cfg, path) = cloud_config(&f);
    let a = bundle("alice", "SYNTHETIC-original-refresh", now() + 3600);
    f.platform.vault_write("alice", &a.encoded()).unwrap();
    cfg.bindings.insert(
        "alice".into(),
        crate::cloud::Binding {
            operation: "synthetic-import".into(),
            owner: a.metadata().unwrap().owner,
            provider: "claude".into(),
            phase: "prepared".into(),
            ..Default::default()
        },
    );
    write_json(&path, &serde_json::to_value(&cfg).unwrap()).unwrap();
    let committed = Rc::new(RefCell::new(false));
    let state = committed.clone();
    crate::cloud::test_transport(Box::new(move |_, action, _, _| match action {
        "import" => Ok(
            json!({"id":"synthetic-cloud","state":if *state.borrow(){"ready"}else{"awaiting_handoff"}}),
        ),
        "import.commit" => {
            *state.borrow_mut() = true;
            Err("synthetic response acknowledgement lost")
        }
        "import.abort" => {
            assert!(*state.borrow());
            Err("synthetic handoff already committed")
        }
        _ => panic!("unexpected synthetic cloud action"),
    }));
    assert!(
        crate::cloud::transfer(
            cfg,
            f.platform.clone(),
            path.clone(),
            "alice".into(),
            "Synthetic".into(),
            None,
        )
        .is_err()
    );
    let pending: crate::cloud::Config =
        serde_json::from_value(read_json(&path).unwrap().unwrap()).unwrap();
    assert_eq!(pending.bindings["alice"].phase, "commit_pending");
    assert!(
        crate::cloud::cancel_transfer(
            pending,
            f.platform.clone(),
            path.clone(),
            "alice".into(),
            None,
        )
        .is_err()
    );
    let persisted: crate::cloud::Config =
        serde_json::from_value(read_json(&path).unwrap().unwrap()).unwrap();
    assert!(persisted.bindings.contains_key("alice"));
    assert!(
        f.platform
            .vault_read_optional("cloud-import-alice")
            .unwrap()
            .is_some()
    );
}

#[test]
fn cloud_native_sync_recovers_after_native_write_succeeds_but_projection_cache_write_fails() {
    assert_cloud_sync_failure_recovery(false, false);
}

#[test]
fn cloud_native_sync_recovers_partial_publish_before_adopting_a_later_cloud_generation() {
    assert_cloud_sync_failure_recovery(true, false);
}

#[test]
fn cloud_partial_publish_recovery_refuses_to_overwrite_an_external_identity_change() {
    assert_cloud_sync_failure_recovery(false, true);
}

fn assert_cloud_sync_failure_recovery(later_generation: bool, external_change: bool) {
    let f = Fixture::new();
    // Fail only the final projection cache. The recovery journal must remain
    // writable so this models native success followed by cache commit failure.
    fs::write(
        &f.platform.helper,
        FAKE_STORE.replace(
            "operation == 'write' and (root / 'fail-write').exists()",
            "operation == 'write' and request['account'] == 'cloud-native-claude' and (root / 'fail-write').exists()",
        ),
    )
    .unwrap();
    let native = f.native();
    let (mut cfg, path) = cloud_config(&f);
    let old = crate::cloud::projection(&bundle("alice", "SYNTHETIC-unused", now() + 3600));
    let mut next = old.clone();
    next.credential["accessToken"] = json!("SYNTHETIC-next-access");
    native.cloud_initialize(&f.platform, &old).unwrap();
    f.platform
        .vault_write("cloud-native-claude", &old.encoded())
        .unwrap();
    cfg.selected
        .insert("claude".into(), "synthetic-cloud".into());
    let lease = cloud_lease(&next, 2);
    crate::cloud::test_transport(Box::new(move |_, action, _, _| {
        assert_eq!(action, "lease");
        Ok(lease.clone())
    }));
    fs::write(f.root.join("fail-write"), b"synthetic").unwrap();
    assert!(
        crate::cloud::native_sync(
            cfg.clone(),
            f.platform.clone(),
            path.clone(),
            "synthetic-cloud".into(),
            native.clone(),
            false,
            None,
        )
        .is_err()
    );
    assert!(native.cloud_read(&f.platform).unwrap() == next);
    assert_eq!(
        f.platform.vault_read("cloud-native-claude").unwrap(),
        old.encoded()
    );
    fs::remove_file(f.root.join("fail-write")).unwrap();
    if later_generation {
        next.credential["accessToken"] = json!("SYNTHETIC-later-access");
        let lease = cloud_lease(&next, 3);
        crate::cloud::test_transport(Box::new(move |_, action, _, _| {
            assert_eq!(action, "lease");
            Ok(lease.clone())
        }));
    }
    let external = if external_change {
        let changed = bundle("external-owner", "SYNTHETIC-external-refresh", now() + 3600);
        write_json(
            &native.directory.join(".credentials.json"),
            &json!({"claudeAiOauth":changed.credential}),
        )
        .unwrap();
        write_json(&native.config, &json!({"oauthAccount":changed.identity})).unwrap();
        Some(changed)
    } else {
        None
    };
    let recovered = crate::cloud::native_sync(
        cfg,
        f.platform.clone(),
        path,
        "synthetic-cloud".into(),
        native.clone(),
        false,
        None,
    );
    if let Some(external) = external {
        assert!(recovered.is_err());
        assert!(native.read(&f.platform).unwrap() == external);
        assert_eq!(
            f.platform.vault_read("cloud-native-claude").unwrap(),
            old.encoded()
        );
        return;
    }
    assert!(
        recovered.is_ok(),
        "retry must reconcile an already committed identical native projection instead of failing stale CAS: {:?}",
        recovered.err()
    );
    assert_eq!(
        f.platform.vault_read("cloud-native-claude").unwrap(),
        next.encoded()
    );
    assert!(native.cloud_read(&f.platform).unwrap() == next);
}

fn fixture_contains(directory: &Path, needle: &[u8]) -> bool {
    fs::read_dir(directory).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() {
            fixture_contains(&path, needle)
        } else {
            let bytes = fs::read(path).unwrap();
            bytes.windows(needle.len()).any(|window| window == needle)
                || base64::engine::general_purpose::STANDARD
                    .decode(&bytes)
                    .is_ok_and(|decoded| {
                        decoded.windows(needle.len()).any(|window| window == needle)
                    })
        }
    })
}

#[test]
fn cloud_activation_cannot_destroy_the_only_copy_of_an_unregistered_native_login() {
    let f = Fixture::new();
    let native = f.native();
    let (cfg, path) = cloud_config(&f);
    let marker = "SYNTHETIC-UNIQUE-UNREGISTERED-refresh";
    let original = bundle("unregistered", marker, now() + 3600);
    native.initialize(&f.platform, &original).unwrap();
    assert!(fixture_contains(&f.root, marker.as_bytes()));
    let target = crate::cloud::projection(&bundle(
        "cloud-owner",
        "SYNTHETIC-remote-only",
        now() + 3600,
    ));
    let lease = cloud_lease(&target, 2);
    crate::cloud::test_transport(Box::new(move |_, action, _, _| {
        assert_eq!(action, "lease");
        Ok(lease.clone())
    }));
    let outcome = crate::cloud::native_sync(
        cfg,
        f.platform.clone(),
        path,
        "synthetic-cloud".into(),
        native.clone(),
        true,
        None,
    );
    assert!(
        fixture_contains(&f.root, marker.as_bytes()),
        "activating cloud credentials must preserve or refuse to overwrite an unregistered native refresh token"
    );
    if outcome.is_err() {
        assert!(native.read(&f.platform).unwrap() == original);
    }
}
