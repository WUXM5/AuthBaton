use super::*;

fn bundle(owner: &str, refresh: &str, expires: u64) -> Bundle {
    let mut b = claude(owner, refresh);
    b.credential["expiresAt"] = json!(expires * 1000);
    b
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

// Each account picks its renewer: this Mac or the cloud (contract §2 and §3).

const VAULT_SERVICE: &str = "io.github.wuxm5.authbaton.vault";

fn codex(user: &str, refresh: &str, generation: u64) -> Bundle {
    let claims = json!({"exp":now()+3600,"generation":generation,"email":format!("{user}@example.invalid"),
        "https://api.openai.com/auth":{"chatgpt_user_id":user,"chatgpt_account_id":format!("workspace-{user}"),"chatgpt_plan_type":"team"}});
    let token = format!("x.{}.x", URL_SAFE_NO_PAD.encode(claims.to_string()));
    Bundle {
        provider: "codex".into(),
        identity: Value::Null,
        credential: json!({"auth_mode":"chatgpt","tokens":{"id_token":token,"access_token":token,"refresh_token":refresh,"account_id":format!("workspace-{user}")}}),
    }
}
/// What the Worker hands out for a cloud account: its access only.
fn lease_of(id: &str, full: &Bundle) -> Value {
    let p = crate::cloud::projection(full);
    let m = p.cloud_metadata().unwrap();
    json!({"id":id,"provider":p.provider,"label":"Synthetic cloud","owner":m.owner,"version":1,
        "expires_at":m.expires_at,"access_token":access_of(full),"projection":p.encoded()})
}
fn access_of(full: &Bundle) -> String {
    if full.provider == "codex" {
        full.credential["tokens"]["access_token"]
            .as_str()
            .unwrap()
            .into()
    } else {
        full.credential["accessToken"].as_str().unwrap().into()
    }
}
fn directory_row(id: &str, full: &Bundle, label: &str) -> Value {
    json!({"id":id,"provider":full.provider,"label":label,"state":"ready","expires_at":full.metadata().unwrap().expires_at,
        "last_refresh":null,"error":null,"used_by":[{"device_id":"device-this","label":"nova Mac","at":now()},{"device_id":"device-orbit","label":"orbit Mac","at":now()-300}]})
}
fn paired(f: &Fixture) -> Accounts {
    cloud_config(f);
    let mut m = f.manager();
    m.cloud.config.device_id = Some("device-this".into());
    m
}
/// A row the cloud renews; this Mac holds only its leased access.
fn add_cloud_row(m: &mut Accounts, id: &str, cloud_id: &str, full: &Bundle, origin: &str) {
    let mut a = account(id, full, false);
    a.metadata = Some(crate::cloud::projection(full).cloud_metadata().unwrap());
    a.automatic = false;
    a.state = "cloud_managed".into();
    m.db.accounts.push(a);
    let owner = full.metadata().unwrap().owner;
    m.cloud.config.bindings.insert(
        id.into(),
        crate::cloud::Binding {
            operation: format!("synthetic-{id}"),
            cloud_id: Some(cloud_id.into()),
            owner: owner.clone(),
            provider: full.provider.clone(),
            phase: "cloud_managed".into(),
            origin: origin.into(),
            ..Default::default()
        },
    );
    m.cloud
        .config
        .accounts
        .push(directory_row(cloud_id, full, id));
    m.cloud.config.owners.insert(cloud_id.into(), owner);
    m.cloud.set_lease(cloud_id, lease_of(cloud_id, full));
    m.save().unwrap();
    m.cloud.save().unwrap();
}
fn finish_cloud(m: &mut Accounts) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while m.cloud.job.as_ref().is_some_and(|j| !j.is_finished()) {
        assert!(
            std::time::Instant::now() < deadline,
            "cloud job did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    m.cloud.poll();
    m.reconcile_cloud_rows();
}
/// A synthetic Worker that records the actions it was sent.
fn worker(
    mut answer: impl FnMut(&str, &Value, usize) -> crate::platform::Result<Value> + 'static,
) -> std::rc::Rc<std::cell::RefCell<Vec<(String, Value)>>> {
    let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::<(String, Value)>::new()));
    let log = calls.clone();
    crate::cloud::test_transport(Box::new(move |_, action, _, body| {
        let seen = log.borrow().iter().filter(|(a, _)| a == action).count();
        log.borrow_mut().push((action.into(), body.clone()));
        answer(action, &body, seen)
    }));
    calls
}
fn actions(calls: &std::rc::Rc<std::cell::RefCell<Vec<(String, Value)>>>) -> Vec<String> {
    calls.borrow().iter().map(|(a, _)| a.clone()).collect()
}
/// Vault items whose stored value contains `needle`.
fn vault_items_with(f: &Fixture, needle: &str) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(&f.root)
        .unwrap()
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let decoded = base64::engine::general_purpose::URL_SAFE
                .decode(name.strip_prefix("key-")?)
                .ok()?;
            let (service, item) = std::str::from_utf8(&decoded).ok()?.split_once('\0')?;
            let value = f.platform.vault_read_optional(item).ok()??;
            (service == VAULT_SERVICE && value.to_string().contains(needle))
                .then(|| item.to_string())
        })
        .collect();
    found.sort();
    found
}
fn row<'a>(m: &'a Accounts, id: &str) -> &'a Account {
    m.db.accounts.iter().find(|a| a.id == id).unwrap()
}
fn view(m: &Accounts, id: &str) -> Value {
    m.snapshot()["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == id)
        .unwrap()["cloud"]
        .clone()
}

#[test]
fn cloud_directory_holds_no_global_mode_and_configuring_reads_no_credentials() {
    let f = Fixture::new();
    let mut m = f.manager();
    let before = m.snapshot();
    m.cloud_command(
        "configure",
        Some("https://auth.example.invalid"),
        Some("Synthetic Mac"),
        None,
        false,
    )
    .unwrap();
    assert_eq!(m.snapshot()["accounts"], before["accounts"]);
    assert!(!m.cloud.config.paired);
    assert!(
        f.platform
            .vault_read_optional("cloud-device-v1")
            .unwrap()
            .is_none()
    );
    let directory = m.cloud_snapshot();
    for gone in [
        "mode",
        "accounts",
        "selected",
        "pending_activation",
        "transfers",
    ] {
        assert!(directory.get(gone).is_none(), "{gone}");
    }
    assert_eq!(directory["admin_url"], "https://auth.example.invalid/admin");
    assert_eq!(directory["label"], "Synthetic Mac");
}

#[test]
fn cloud_directory_sync_gives_each_cloud_account_exactly_one_row() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let mine = bundle("nova", "SYNTHETIC-nova-cloud", now() + 3600);
    let other = bundle("orbit", "SYNTHETIC-orbit-cloud", now() + 3600);
    let twin = bundle("lab", "SYNTHETIC-lab-cloud", now() + 3600);
    let local_twin = bundle("lab", "SYNTHETIC-lab-local", now() + 3600);
    add_cloud_row(&mut m, "nova", "cloud-nova", &mine, "");
    f.platform
        .vault_write("lab", &local_twin.encoded())
        .unwrap();
    m.db.accounts.push(account("lab", &local_twin, false));
    // A lease from earlier said whose cloud-lab is: the same account this Mac has its own login of.
    m.cloud
        .config
        .owners
        .insert("cloud-lab".into(), twin.metadata().unwrap().owner);
    m.save().unwrap();
    let listed = std::rc::Rc::new(std::cell::RefCell::new(vec![
        directory_row("cloud-nova", &mine, "nova"),
        directory_row("cloud-orbit", &other, "o***@example.invalid"),
        directory_row("cloud-lab", &twin, "lab"),
    ]));
    let rows = listed.clone();
    worker(move |action, _, _| {
        assert_eq!(action, "accounts");
        Ok(json!({"accounts":*rows.borrow(),"devices":[]}))
    });
    m.cloud_command("refresh", None, None, None, false).unwrap();
    finish_cloud(&mut m);
    let bound = |m: &Accounts, cloud_id: &str| -> Vec<String> {
        m.cloud
            .config
            .bindings
            .iter()
            .filter(|(_, b)| b.cloud_id.as_deref() == Some(cloud_id))
            .map(|(id, _)| id.clone())
            .collect()
    };
    assert_eq!(bound(&m, "cloud-nova"), ["nova"]);
    let shadow = bound(&m, "cloud-orbit");
    assert_eq!(shadow.len(), 1, "an account from another Mac gets one row");
    assert!(
        bound(&m, "cloud-lab").is_empty(),
        "two separate logins of lab: this Mac's own stays alone"
    );
    assert_eq!(m.db.accounts.len(), 3);
    let s = row(&m, &shadow[0]);
    assert_eq!(
        (s.state.as_str(), s.automatic, s.native),
        ("cloud_managed", false, false)
    );
    assert!(
        f.platform
            .vault_read_optional(&shadow[0])
            .unwrap()
            .is_none()
    );
    let v = view(&m, &shadow[0]);
    assert_eq!(v["origin"], "other");
    assert_eq!(v["maintainer"], "cloud");
    assert_eq!(v["fresh"], true);
    assert_eq!(v["state"], "ready");
    assert_eq!(v["used_by"][0]["this"], true);
    assert_eq!(v["used_by"][1]["this"], false);
    assert_eq!(view(&m, "nova")["origin"], "this");
    assert!(view(&m, "lab").is_null());
    // Syncing again adds nothing.
    m.cloud_command("refresh", None, None, None, false).unwrap();
    finish_cloud(&mut m);
    assert_eq!(m.db.accounts.len(), 3);
    // The first lease fills in the full email.
    m.cloud
        .set_lease("cloud-orbit", lease_of("cloud-orbit", &other));
    m.reconcile_cloud_rows();
    let s = row(&m, &shadow[0]);
    assert_eq!(
        s.metadata.as_ref().unwrap().display,
        "orbit@example.invalid"
    );
    assert_eq!(s.label, "orbit@example.invalid");
    // Moved back on another Mac. A directory read before this Mac's takeover does not count.
    listed.borrow_mut().retain(|r| r["id"] == "cloud-lab");
    m.cloud.config.bindings.get_mut("nova").unwrap().since = now() + 100;
    m.cloud.config.bindings.get_mut(&shadow[0]).unwrap().since = now() + 100;
    m.cloud_command("refresh", None, None, None, false).unwrap();
    finish_cloud(&mut m);
    assert_eq!(
        m.db.accounts.len(),
        3,
        "a read from before the takeover removes nothing"
    );
    for b in m.cloud.config.bindings.values_mut() {
        b.since = 0;
    }
    m.cloud_command("refresh", None, None, None, false).unwrap();
    finish_cloud(&mut m);
    assert_eq!(
        m.db.accounts.len(),
        2,
        "the other Mac's row goes with its account"
    );
    let nova = row(&m, "nova");
    assert_eq!(nova.state, "reauth");
    assert_eq!(
        nova.message,
        "已在另一台 Mac 转回本机续期，这台需要重新登录"
    );
    assert!(m.cloud.config.bindings.is_empty());
    // Negative control: this Mac's own login of lab is untouched throughout.
    assert_eq!(row(&m, "lab").state, "saved");
    assert_eq!(f.platform.vault_read("lab").unwrap(), local_twin.encoded());
}

#[test]
fn cloud_rows_never_enter_a_renewal_path() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let cloud = bundle("nova", "SYNTHETIC-nova-cloud", now() + 3600);
    add_cloud_row(&mut m, "nova", "cloud-nova", &cloud, "");
    let local = bundle("orbit", "SYNTHETIC-orbit-local", now() + 3600);
    f.platform.vault_write("orbit", &local.encoded()).unwrap();
    m.db.accounts.push(account("orbit", &local, false));
    // Both look due to a scheduler that forgets which renews what.
    for a in &mut m.db.accounts {
        a.next_check = 0;
    }
    m.db.accounts[0].automatic = true;
    m.save().unwrap();
    assert!(m.local_check("nova").is_err());
    assert!(m.request_check("nova", false).is_err());
    assert!(m.check("nova", true).is_err());
    assert!(m.retry_save("nova").is_err());
    assert!(m.configure("nova", None, Some(true)).is_err());
    assert!(m.job.is_none());
    m.reconcile_cloud_rows();
    assert!(
        !row(&m, "nova").automatic,
        "a cloud row never keeps local maintenance on"
    );
    m.last_scan = 0;
    m.cloud.next_directory = now() + 600;
    m.tick();
    // Negative control: the same scheduler pass does check the local row.
    assert_eq!(m.job.as_ref().map(|j| j.id.as_str()), Some("orbit"));
    finish_account_job(&mut m);
    // 认证检查 of a cloud row asks the cloud; nothing renews here.
    let calls = worker(|action, body, _| match action {
        "check" => {
            assert_eq!(body["id"], "cloud-nova");
            Ok(json!({}))
        }
        "accounts" => Ok(
            json!({"accounts":[directory_row("cloud-nova", &bundle("nova", "x", now() + 3600), "nova")],"devices":[]}),
        ),
        _ => panic!("unexpected synthetic cloud action {action}"),
    });
    m.request_check("nova", true).unwrap();
    finish_cloud(&mut m);
    assert_eq!(actions(&calls), ["check", "accounts"]);
    assert!(m.job.is_none());
    assert!(f.platform.vault_read_optional("nova").unwrap().is_none());
    assert!(!fixture_contains(&f.root, b"SYNTHETIC-nova-cloud"));
}

#[test]
fn switching_claude_to_a_cloud_row_writes_access_only_and_switching_away_keeps_no_copy() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let native = f.native();
    m.native_override = Some(native.clone());
    let nova = bundle("nova", "SYNTHETIC-nova-refresh", now() + 3600);
    let renewed = bundle("nova", "SYNTHETIC-nova-renewed", now() + 7200);
    let lab = bundle("lab", "SYNTHETIC-lab-refresh", now() + 3600);
    let cloud = bundle("orbit", "SYNTHETIC-orbit-cloud-refresh", now() + 3600);
    native.initialize(&f.platform, &renewed).unwrap();
    f.platform.vault_write("nova", &nova.encoded()).unwrap();
    f.platform.vault_write("lab", &lab.encoded()).unwrap();
    m.db.accounts.push(account("nova", &nova, true));
    m.db.accounts.push(account("lab", &lab, false));
    add_cloud_row(&mut m, "orbit", "cloud-orbit", &cloud, "other");
    m.activate("orbit", true).unwrap();
    let held = native.cloud_read(&f.platform).unwrap();
    assert!(held.credential.get("refreshToken").is_none());
    assert_eq!(held.credential, crate::cloud::projection(&cloud).credential);
    assert_eq!(held.identity, cloud.identity);
    // The local login switched away from is kept with the generation its client renewed.
    assert_eq!(f.platform.vault_read("nova").unwrap(), renewed.encoded());
    assert_eq!(m.cloud.config.selected["claude"], "cloud-orbit");
    assert!(row(&m, "orbit").native && !row(&m, "nova").native);
    assert!(m.cloud.config.bindings["orbit"].used);
    assert_eq!(view(&m, "orbit")["fresh"], false);
    let access = access_of(&cloud);
    assert_eq!(vault_items_with(&f, &access), ["cloud-native-claude"]);
    assert!(!fixture_contains(&f.root, b"SYNTHETIC-orbit-cloud-refresh"));
    assert!(!m.snapshot().to_string().contains("SYNTHETIC"));
    // Switching away: the access-only login is replaced and saved nowhere.
    m.activate("lab", true).unwrap();
    assert!(native.read(&f.platform).unwrap() == lab);
    assert!(vault_items_with(&f, &access).is_empty());
    assert!(f.platform.vault_read_optional("orbit").unwrap().is_none());
    assert_eq!(f.platform.vault_read("nova").unwrap(), renewed.encoded());
    assert!(!m.cloud.config.selected.contains_key("claude"));
    assert!(row(&m, "lab").native && !row(&m, "orbit").native);
    assert!(m.db.switch.is_none());
    // Negative control: switching away from a local login does keep its renewed generation.
    let lab_renewed = bundle("lab", "SYNTHETIC-lab-renewed", now() + 7200);
    native.refresh(&f.platform, &lab, &lab_renewed).unwrap();
    m.activate("nova", true).unwrap();
    assert_eq!(f.platform.vault_read("lab").unwrap(), lab_renewed.encoded());
}

#[test]
fn switching_codex_to_a_cloud_row_without_hot_switch_writes_access_only() {
    let f = Fixture::new();
    let mut m = paired(&f);
    private_dir(&m.sources.codex_home).unwrap();
    let file = m.sources.codex_home.join("auth.json");
    let nova = codex("nova", "SYNTHETIC-nova-refresh", 1);
    let cloud = codex("orbit", "SYNTHETIC-orbit-cloud-refresh", 1);
    write_json(&file, &nova.credential).unwrap();
    f.platform.vault_write("nova", &nova.encoded()).unwrap();
    m.db.accounts.push(account("nova", &nova, true));
    add_cloud_row(&mut m, "orbit", "cloud-orbit", &cloud, "");
    m.activate("orbit", true).unwrap();
    let written = read_json(&file).unwrap().unwrap();
    assert!(written["tokens"].get("refresh_token").is_none());
    assert_eq!(written["auth_mode"], "chatgptAuthTokens");
    assert_eq!(written, crate::cloud::projection(&cloud).credential);
    assert_eq!(f.platform.vault_read("nova").unwrap(), nova.encoded());
    m.activate("nova", true).unwrap();
    assert_eq!(read_json(&file).unwrap().unwrap(), nova.credential);
    assert!(vault_items_with(&f, &access_of(&cloud)).is_empty());
    assert!(!fixture_contains(&f.root, b"SYNTHETIC-orbit-cloud-refresh"));
}

#[test]
fn codex_hot_switch_logs_the_desktop_in_with_leased_access_and_back() {
    let f = Fixture::new();
    cloud_config(&f);
    let (mut m, a, _) = super::desktop_boundary_tests::setup(&f);
    let cloud = codex("orbit", "SYNTHETIC-orbit-cloud-refresh", 1);
    add_cloud_row(&mut m, "orbit", "cloud-orbit", &cloud, "other");
    let finish = |m: &mut Accounts| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while let Some(job) = m.desktop.job.as_ref() {
            assert!(std::time::Instant::now() < deadline);
            if job.is_finished() {
                m.desktop_tick();
            } else {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    };
    m.activate("orbit", true).unwrap();
    finish(&mut m);
    let state = read_json(&f.root.join("desktop-state.json"))
        .unwrap()
        .unwrap();
    assert_eq!(state["owner"], "codex:orbit:workspace-orbit");
    assert_eq!(state["mode"], "chatgptAuthTokens");
    let file = m.sources.codex_home.join("auth.json");
    assert_eq!(
        read_json(&file).unwrap().unwrap(),
        crate::cloud::projection(&cloud).credential
    );
    assert_eq!(m.db.desktop.selected.as_deref(), Some("orbit"));
    assert_eq!(m.cloud.config.selected["codex"], "cloud-orbit");
    let orbit = row(&m, "orbit");
    assert!(orbit.native && !orbit.automatic);
    assert_eq!(f.platform.vault_read("a").unwrap(), a.encoded());
    assert_eq!(
        vault_items_with(&f, &access_of(&cloud)),
        ["cloud-native-codex"]
    );
    // Back to the local account: the desktop and the default file get its full login.
    m.activate("a", true).unwrap();
    finish(&mut m);
    assert_eq!(read_json(&file).unwrap().unwrap(), a.credential);
    assert_eq!(m.db.desktop.selected.as_deref(), Some("a"));
    assert!(!m.cloud.config.selected.contains_key("codex"));
    assert!(vault_items_with(&f, &access_of(&cloud)).is_empty());
    assert!(!fixture_contains(&f.root, b"SYNTHETIC-orbit-cloud-refresh"));
}

#[test]
fn activating_a_cloud_row_keeps_an_unregistered_native_login_as_its_own_row() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let native = f.native();
    m.native_override = Some(native.clone());
    let marker = "SYNTHETIC-UNIQUE-UNREGISTERED-refresh";
    let original = bundle("unregistered", marker, now() + 3600);
    native.initialize(&f.platform, &original).unwrap();
    let cloud = bundle("orbit", "SYNTHETIC-orbit-cloud", now() + 3600);
    add_cloud_row(&mut m, "orbit", "cloud-orbit", &cloud, "other");
    m.activate("orbit", true).unwrap();
    let kept =
        m.db.accounts
            .iter()
            .find(|a| {
                a.metadata
                    .as_ref()
                    .is_some_and(|m| m.owner == original.metadata().unwrap().owner)
            })
            .unwrap();
    assert!(!kept.native);
    assert_eq!(f.platform.vault_read(&kept.id).unwrap(), original.encoded());
}

#[test]
fn strategy_rules_may_name_cloud_rows() {
    use crate::automation::{Config, Order, Rule};
    let f = Fixture::new();
    let mut m = paired(&f);
    let nova = bundle("nova", "SYNTHETIC-nova", now() + 3600);
    f.platform.vault_write("nova", &nova.encoded()).unwrap();
    m.db.accounts.push(account("nova", &nova, true));
    add_cloud_row(
        &mut m,
        "orbit",
        "cloud-orbit",
        &bundle("orbit", "SYNTHETIC-orbit", now() + 3600),
        "other",
    );
    let rule = |id: &str| Rule {
        id: id.into(),
        order: Order::SwitchFirst,
        participate: true,
        below: 5,
    };
    m.configure_strategy(
        Config {
            enabled: false,
            accounts: vec![rule("nova"), rule("orbit")],
        },
        false,
    )
    .unwrap();
    assert_eq!(m.db.automation.config.accounts.len(), 2);
    assert!(m.strategy_account_ready(row(&m, "orbit")));
    assert_eq!(
        view(&m, "orbit")["fresh"],
        false,
        "a row with a rule is no longer new"
    );
    // Negative control: a rule naming no account still fails the whole save.
    assert!(
        m.configure_strategy(
            Config {
                enabled: false,
                accounts: vec![rule("nova"), rule("missing")],
            },
            false,
        )
        .is_err()
    );
    // A cloud account the cloud cannot renew is no switch target.
    m.cloud.config.accounts[0]["state"] = json!("reauthorization_required");
    m.cloud.config.accounts[0]["error"] = json!("refresh_rejected");
    assert!(!m.strategy_account_ready(row(&m, "orbit")));
    assert_eq!(
        view(&m, "orbit")["error"],
        "续期被拒（刷新凭据已失效），需重新登录以恢复云端续期。"
    );
}

#[test]
fn moving_to_the_cloud_keeps_the_row_and_its_errors_on_the_card() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let nova = bundle("nova", "SYNTHETIC-nova-refresh", now() + 3600);
    f.platform.vault_write("nova", &nova.encoded()).unwrap();
    m.db.accounts.push(account("nova", &nova, false));
    m.save().unwrap();
    let listed = directory_row("cloud-nova", &nova, "nova");
    let calls = worker(move |action, _, seen| match action {
        "import" => Ok(json!({"id":"cloud-nova","state":"awaiting_handoff"})),
        "import.commit" if seen == 0 => Err("synthetic reply lost"),
        "import.commit" => Ok(json!({"id":"cloud-nova","state":"ready"})),
        "accounts" => Ok(json!({"accounts":[listed.clone()],"devices":[]})),
        _ => panic!("unexpected synthetic cloud action {action}"),
    });
    m.cloud_command("import", None, None, Some("nova"), true)
        .unwrap();
    assert_eq!(view(&m, "nova")["transfer"]["running"], true);
    finish_cloud(&mut m);
    let v = view(&m, "nova");
    assert_eq!(v["transfer"]["direction"], "to_cloud");
    assert_eq!(v["transfer"]["running"], false);
    assert_eq!(v["transfer"]["error"], "synthetic reply lost");
    assert_eq!(v["maintainer"], "local");
    // The cloud may have taken it over already: only finishing the move is safe.
    assert!(m.activate("nova", true).is_err());
    assert!(m.request_check("nova", false).is_err());
    m.cloud_command("resume_import", None, None, Some("nova"), true)
        .unwrap();
    finish_cloud(&mut m);
    assert_eq!(
        actions(&calls),
        [
            "import",
            "import.commit",
            "import",
            "import.commit",
            "accounts"
        ]
    );
    assert_eq!(
        m.db.accounts.len(),
        1,
        "the row stays and is not duplicated"
    );
    let v = view(&m, "nova");
    assert_eq!(
        (v["maintainer"].as_str(), v["origin"].as_str()),
        (Some("cloud"), Some("this"))
    );
    assert!(v["transfer"].is_null());
    assert_eq!(row(&m, "nova").state, "cloud_managed");
    assert!(!row(&m, "nova").automatic);
    // No full copy stays here to go stale while the cloud renews it.
    assert!(f.platform.vault_read_optional("nova").unwrap().is_none());
    assert!(
        f.platform
            .vault_read_optional("cloud-import-nova")
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_move_to_the_cloud_that_stopped_before_the_cloud_took_it_can_still_be_switched_to() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let native = f.native();
    m.native_override = Some(native.clone());
    let nova = bundle("nova", "SYNTHETIC-nova-refresh", now() + 3600);
    f.platform.vault_write("nova", &nova.encoded()).unwrap();
    m.db.accounts.push(account("nova", &nova, false));
    m.save().unwrap();
    let calls = worker(|action, _, _| match action {
        "import" => Err("synthetic outage"),
        "import.abort" => Ok(json!({"aborted":true})),
        "accounts" => Ok(json!({"accounts":[],"devices":[]})),
        _ => panic!("unexpected synthetic cloud action {action}"),
    });
    m.cloud_command("import", None, None, Some("nova"), true)
        .unwrap();
    finish_cloud(&mut m);
    assert_eq!(view(&m, "nova")["transfer"]["error"], "synthetic outage");
    // Its login is still in its Vault item: 换上 works as for any local row.
    m.activate("nova", true).unwrap();
    assert!(native.read(&f.platform).unwrap() == nova);
    m.cloud_command("cancel_import", None, None, Some("nova"), true)
        .unwrap();
    finish_cloud(&mut m);
    assert_eq!(actions(&calls), ["import", "import.abort", "accounts"]);
    assert!(view(&m, "nova").is_null());
    assert_eq!(row(&m, "nova").state, "saved");
}

fn exported(cloud: &Bundle) -> Value {
    json!({"id":"cloud-orbit","owner":cloud.metadata().unwrap().owner,"version":3,"bundle":cloud.encoded()})
}

#[test]
fn moving_a_cloud_row_back_saves_the_full_login_before_the_cloud_drops_its_copy() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let native = f.native();
    m.native_override = Some(native.clone());
    let cloud = bundle("orbit", "SYNTHETIC-orbit-exported-refresh", now() + 3600);
    add_cloud_row(&mut m, "orbit", "cloud-orbit", &cloud, "other");
    m.activate("orbit", true).unwrap();
    assert!(native.read(&f.platform).is_err(), "in use with access only");
    let answer = exported(&cloud);
    let calls = worker(move |action, body, _| match action {
        "export.begin" => {
            assert_eq!(body["id"], "cloud-orbit");
            Ok(answer.clone())
        }
        "export.commit" => Ok(json!({"removed":true})),
        _ => panic!("unexpected synthetic cloud action {action}"),
    });
    m.cloud_command("export", None, None, Some("orbit"), true)
        .unwrap();
    finish_cloud(&mut m);
    assert_eq!(actions(&calls), ["export.begin", "export.commit"]);
    let operations: Vec<Value> = calls
        .borrow()
        .iter()
        .map(|(_, b)| b["operation"].clone())
        .collect();
    assert_eq!(operations[0], operations[1]);
    assert_eq!(f.platform.vault_read("orbit").unwrap(), cloud.encoded());
    assert!(native.read(&f.platform).unwrap() == cloud);
    let orbit = row(&m, "orbit");
    assert_eq!(
        (orbit.state.as_str(), orbit.automatic, orbit.native),
        ("saved", true, true)
    );
    assert!(m.cloud.config.bindings.is_empty());
    assert!(m.cloud.config.export_commits.is_empty());
    assert!(!m.cloud.config.selected.contains_key("claude"));
    assert!(view(&m, "orbit").is_null());
    assert!(
        f.platform
            .vault_read_optional("cloud-native-claude")
            .unwrap()
            .is_none()
    );
    // Negative control of the fence: a local row takes local checks.
    m.request_check("orbit", false).unwrap();
    finish_account_job(&mut m);
}

#[test]
fn a_lost_export_begin_reply_is_replayed_with_the_same_operation() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let cloud = bundle("orbit", "SYNTHETIC-orbit-exported-refresh", now() + 3600);
    add_cloud_row(&mut m, "orbit", "cloud-orbit", &cloud, "");
    let answer = exported(&cloud);
    let calls = worker(move |action, _, seen| match action {
        "export.begin" if seen == 0 => Err("synthetic reply lost"),
        "export.begin" => Ok(answer.clone()),
        "export.commit" => Ok(json!({"removed":true})),
        _ => panic!("unexpected synthetic cloud action {action}"),
    });
    m.cloud_command("export", None, None, Some("orbit"), true)
        .unwrap();
    finish_cloud(&mut m);
    let v = view(&m, "orbit");
    assert_eq!(v["transfer"]["direction"], "to_local");
    assert_eq!(v["transfer"]["error"], "synthetic reply lost");
    assert_eq!(v["state"], "exporting");
    assert!(f.platform.vault_read_optional("orbit").unwrap().is_none());
    assert!(
        m.check("orbit", true).is_err(),
        "still the cloud's to renew"
    );
    m.cloud_command("export", None, None, Some("orbit"), true)
        .unwrap();
    finish_cloud(&mut m);
    assert_eq!(
        actions(&calls),
        ["export.begin", "export.begin", "export.commit"]
    );
    let operations: Vec<Value> = calls
        .borrow()
        .iter()
        .map(|(_, b)| b["operation"].clone())
        .collect();
    assert!(
        operations
            .iter()
            .all(|o| *o == operations[0] && o.is_string())
    );
    assert_eq!(f.platform.vault_read("orbit").unwrap(), cloud.encoded());
    assert_eq!(row(&m, "orbit").state, "saved");
}

#[test]
fn a_failed_vault_write_leaves_the_move_resumable_and_giving_up_hands_renewal_back() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let cloud = bundle("orbit", "SYNTHETIC-orbit-exported-refresh", now() + 3600);
    add_cloud_row(&mut m, "orbit", "cloud-orbit", &cloud, "");
    let answer = exported(&cloud);
    let listed = directory_row("cloud-orbit", &cloud, "orbit");
    let calls = worker(move |action, _, _| match action {
        "export.begin" => Ok(answer.clone()),
        "export.abort" => Ok(json!({"aborted":true})),
        "accounts" => Ok(json!({"accounts":[listed.clone()],"devices":[]})),
        _ => panic!("unexpected synthetic cloud action {action}"),
    });
    fs::write(f.root.join("fail-write"), b"synthetic").unwrap();
    m.cloud_command("export", None, None, Some("orbit"), true)
        .unwrap();
    finish_cloud(&mut m);
    fs::remove_file(f.root.join("fail-write")).unwrap();
    assert_eq!(m.cloud.config.bindings["orbit"].phase, "exporting");
    assert!(view(&m, "orbit")["transfer"]["error"].is_string());
    assert!(f.platform.vault_read_optional("orbit").unwrap().is_none());
    assert!(m.cloud.config.export_commits.is_empty());
    m.cloud_command("cancel_export", None, None, Some("orbit"), true)
        .unwrap();
    finish_cloud(&mut m);
    assert_eq!(
        actions(&calls),
        ["export.begin", "export.abort", "accounts"]
    );
    let operations: Vec<Value> = calls
        .borrow()
        .iter()
        .take(2)
        .map(|(_, b)| b["operation"].clone())
        .collect();
    assert_eq!(operations[0], operations[1]);
    let b = &m.cloud.config.bindings["orbit"];
    assert_eq!(
        (b.phase.as_str(), b.export.is_none()),
        ("cloud_managed", true)
    );
    assert!(f.platform.vault_read_optional("orbit").unwrap().is_none());
    assert_eq!(view(&m, "orbit")["maintainer"], "cloud");
    assert!(!row(&m, "orbit").automatic);
}

#[test]
fn a_move_back_never_overwrites_a_native_login_changed_meanwhile() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let native = f.native();
    m.native_override = Some(native.clone());
    let cloud = bundle("orbit", "SYNTHETIC-orbit-exported-refresh", now() + 3600);
    add_cloud_row(&mut m, "orbit", "cloud-orbit", &cloud, "");
    m.activate("orbit", true).unwrap();
    // Someone signs in to another account with the official client meanwhile.
    let outside = bundle("outside", "SYNTHETIC-outside-refresh", now() + 3600);
    write_json(
        &native.directory.join(".credentials.json"),
        &json!({"claudeAiOauth":outside.credential}),
    )
    .unwrap();
    write_json(&native.config, &json!({"oauthAccount":outside.identity})).unwrap();
    let answer = exported(&cloud);
    let calls = worker(move |action, _, _| match action {
        "export.begin" => Ok(answer.clone()),
        _ => panic!("unexpected synthetic cloud action {action}"),
    });
    m.cloud_command("export", None, None, Some("orbit"), true)
        .unwrap();
    finish_cloud(&mut m);
    assert_eq!(actions(&calls), ["export.begin"], "nothing is committed");
    assert!(native.read(&f.platform).unwrap() == outside);
    let b = &m.cloud.config.bindings["orbit"];
    assert_eq!(b.phase, "exporting");
    assert_eq!(b.error.as_deref(), Some("原生登录已变化，未覆盖本机凭据"));
}

#[test]
fn a_lost_export_commit_is_retried_and_a_commit_after_cloud_renewal_needs_a_new_login() {
    for spent in [false, true] {
        let f = Fixture::new();
        let mut m = paired(&f);
        let cloud = bundle("orbit", "SYNTHETIC-orbit-exported-refresh", now() + 3600);
        add_cloud_row(&mut m, "orbit", "cloud-orbit", &cloud, "");
        let answer = exported(&cloud);
        let mut exporting = directory_row("cloud-orbit", &cloud, "orbit");
        exporting["state"] = json!("exporting");
        let calls = worker(move |action, _, seen| match action {
            "export.begin" => Ok(answer.clone()),
            "export.commit" if seen == 0 => Err("synthetic reply lost"),
            "export.commit" if spent => Err(crate::cloud::EXPORT_NOT_ACTIVE),
            "export.commit" => Ok(json!({"removed":true})),
            "accounts" => Ok(json!({"accounts":[exporting.clone()],"devices":[]})),
            _ => panic!("unexpected synthetic cloud action {action}"),
        });
        m.cloud_command("export", None, None, Some("orbit"), true)
            .unwrap();
        finish_cloud(&mut m);
        // The login is this Mac's already; only the cloud's copy is owed.
        assert_eq!(row(&m, "orbit").state, "saved");
        assert!(m.cloud.config.export_commits.contains_key("cloud-orbit"));
        // While the commit is owed, the cloud still lists the account: it gets no second row.
        m.cloud_command("refresh", None, None, None, false).unwrap();
        finish_cloud(&mut m);
        assert_eq!(m.db.accounts.len(), 1);
        m.cloud.next_commit = 0;
        m.cloud_tick();
        finish_cloud(&mut m);
        assert_eq!(
            actions(&calls),
            ["export.begin", "export.commit", "accounts", "export.commit"]
        );
        assert!(m.cloud.config.export_commits.is_empty());
        assert!(m.cloud.config.bindings.is_empty());
        let orbit = row(&m, "orbit");
        if spent {
            // Terminal: the cloud renewed the login after giving up on the move.
            assert_eq!(orbit.state, "reauth");
            assert_eq!(orbit.message, crate::cloud::EXPORT_NOT_ACTIVE);
            assert!(f.platform.vault_read_optional("orbit").unwrap().is_none());
        } else {
            assert_eq!(orbit.state, "saved");
            assert_eq!(f.platform.vault_read("orbit").unwrap(), cloud.encoded());
        }
        // Either way nothing retries the commit again.
        m.cloud.next_commit = 0;
        m.cloud.next_directory = now() + 60;
        m.cloud_tick();
        finish_cloud(&mut m);
        assert_eq!(
            actions(&calls)
                .iter()
                .filter(|a| *a == "export.commit")
                .count(),
            2
        );
    }
}

#[test]
fn removing_a_cloud_row_goes_through_the_cloud_and_never_while_in_use() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let native = f.native();
    m.native_override = Some(native.clone());
    let orbit = bundle("orbit", "SYNTHETIC-orbit", now() + 3600);
    let lab = bundle("lab", "SYNTHETIC-lab", now() + 3600);
    add_cloud_row(&mut m, "orbit", "cloud-orbit", &orbit, "other");
    add_cloud_row(&mut m, "lab", "cloud-lab", &lab, "");
    m.activate("lab", true).unwrap();
    let listed = directory_row("cloud-lab", &lab, "lab");
    let calls = worker(move |action, body, seen| match action {
        "remove" if seen == 0 => Err("synthetic outage"),
        "remove" => {
            assert_eq!(body["id"], "cloud-orbit");
            Ok(json!({"removed":true}))
        }
        "accounts" => Ok(json!({"accounts":[listed.clone()],"devices":[]})),
        _ => panic!("unexpected synthetic cloud action {action}"),
    });
    assert!(m.remove("lab", true).is_err(), "in use");
    assert!(actions(&calls).is_empty());
    m.remove("orbit", true).unwrap();
    finish_cloud(&mut m);
    assert!(
        m.db.accounts.iter().any(|a| a.id == "orbit"),
        "a refused removal keeps the row"
    );
    m.remove("orbit", true).unwrap();
    finish_cloud(&mut m);
    assert_eq!(actions(&calls), ["remove", "remove", "accounts"]);
    assert!(!m.db.accounts.iter().any(|a| a.id == "orbit"));
    assert!(!m.cloud.config.bindings.contains_key("orbit"));
    assert!(row(&m, "lab").native);
}

#[test]
fn leaving_the_cloud_drops_other_macs_rows_and_asks_again_for_rows_moved_up_here() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let native = f.native();
    m.native_override = Some(native.clone());
    let nova = bundle("nova", "SYNTHETIC-nova", now() + 3600);
    add_cloud_row(&mut m, "nova", "cloud-nova", &nova, "");
    add_cloud_row(
        &mut m,
        "orbit",
        "cloud-orbit",
        &bundle("orbit", "SYNTHETIC-orbit", now() + 3600),
        "other",
    );
    let lab = bundle("lab", "SYNTHETIC-lab-local", now() + 3600);
    f.platform.vault_write("lab", &lab.encoded()).unwrap();
    m.db.accounts.push(account("lab", &lab, false));
    m.save().unwrap();
    let calls = worker(|action, _, _| match action {
        "leave" => Ok(json!({"revoked":true})),
        _ => panic!("unexpected synthetic cloud action {action}"),
    });
    m.activate("nova", true).unwrap();
    assert!(
        m.cloud_command("leave", None, None, None, true).is_err(),
        "a cloud row is in use"
    );
    assert!(actions(&calls).is_empty());
    m.activate("lab", true).unwrap();
    m.cloud_command("leave", None, None, None, true).unwrap();
    finish_cloud(&mut m);
    assert_eq!(actions(&calls), ["leave"]);
    assert!(!m.cloud.config.paired);
    assert!(
        f.platform
            .vault_read_optional("cloud-device-v1")
            .unwrap()
            .is_none()
    );
    assert!(m.cloud.config.bindings.is_empty());
    assert!(!m.db.accounts.iter().any(|a| a.id == "orbit"));
    assert_eq!(row(&m, "nova").state, "reauth");
    assert_eq!(row(&m, "nova").message, "已断开云端配对，这台需要重新登录");
    assert_eq!(row(&m, "lab").state, "saved");
    assert!(row(&m, "lab").native);
}

#[test]
fn pi_lists_cloud_rows_under_their_own_ids_and_maps_old_cloud_names() {
    let f = Fixture::new();
    let mut m = paired(&f);
    let nova = codex("nova", "SYNTHETIC-nova-refresh", 1);
    let orbit = codex("orbit", "SYNTHETIC-orbit-cloud", 1);
    f.platform.vault_write("nova", &nova.encoded()).unwrap();
    m.db.accounts.push(account("nova", &nova, false));
    add_cloud_row(&mut m, "orbit", "orbit-cloud-id", &orbit, "other");
    let listed = m.pi_accounts();
    let ids: Vec<&str> = listed["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["nova", "orbit"]);
    let ready = m.pi_access(Some("orbit")).unwrap();
    assert_eq!(ready["account_id"], "orbit");
    assert_eq!(ready["access_token"], access_of(&orbit));
    let legacy = m.pi_access(Some("cloud-orbit-cloud-id")).unwrap();
    assert_eq!(legacy["account_id"], "cloud-orbit-cloud-id");
    assert_eq!(legacy["access_token"], access_of(&orbit));
    assert!(m.pi_access(Some("cloud-unknown")).is_err());
    assert!(!listed.to_string().contains("SYNTHETIC"));
    // A cloud account the cloud cannot renew reads to pi as needing a new login.
    m.cloud.config.accounts[0]["state"] = json!("reauthorization_required");
    assert_eq!(m.pi_accounts()["accounts"][1]["state"], "reauth");
}

#[test]
fn transferred_accounts_are_fenced_from_all_local_refresh_entries_after_restart() {
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
            phase: "commit_pending".into(),
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
