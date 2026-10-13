//! Synthetic failure tests. Include as a child module of `accounts`.
//! The helper below stores fixtures in this test's temporary directory; it never uses Keychain.
use super::*;
#[path = "automation_boundary_tests.rs"]
mod automation_boundary_tests;
#[path = "check_queue_tests.rs"]
mod check_queue_tests;
#[path = "claude_boundary_tests.rs"]
mod claude_boundary_tests;
#[path = "claude_guard_tests.rs"]
mod claude_guard_tests;
#[path = "cloud_boundary_tests.rs"]
mod cloud_boundary_tests;
#[path = "desktop_boundary_tests.rs"]
mod desktop_boundary_tests;
#[path = "interrupted_boundary_tests.rs"]
mod interrupted_boundary_tests;
#[path = "pi_boundary_tests.rs"]
mod pi_boundary_tests;
#[path = "reset_boundary_tests.rs"]
mod reset_boundary_tests;
#[path = "usage_clients_tests.rs"]
mod usage_clients_tests;
use crate::login::LoginState;
use crate::platform::{read_json, write_json};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::fs;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    root: PathBuf,
    platform: Platform,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("authmanager-boundary-{}", random_id().unwrap()));
        private_dir(&root).unwrap();
        let helper = root.join("fake-store.py");
        fs::write(&helper, FAKE_STORE).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            platform: Platform {
                helper,
                recovery_wait: None,
                fixture_keychain: true,
            },
        }
    }

    fn manager(&self) -> Accounts {
        let home = self.root.join("home");
        private_dir(&home).unwrap();
        let mut manager = Accounts::with_platform(
            &self.root,
            &home,
            Sources {
                codex_home: home.join(".codex"),
                pi_home: home.join(".pi"),
                claude_bin: None,
            },
            None,
            self.platform.clone(),
        )
        .unwrap();
        manager.client_running = |_| false;
        manager
    }

    fn native(&self) -> NativeStore {
        let directory = self.root.join("native");
        private_dir(&directory).unwrap();
        NativeStore {
            provider: "claude".into(),
            config: directory.join(".claude.json"),
            directory,
            service: "synthetic-native-only".into(),
            account: "synthetic".into(),
            mode: "file".into(),
        }
    }

    fn codex(&self, corrupt: bool) -> PathBuf {
        let binary = self.root.join("fake-codex.py");
        fs::write(&binary, FAKE_CODEX).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        if corrupt {
            fs::write(self.root.join("corrupt-auth"), b"synthetic").unwrap();
        }
        binary
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const FAKE_STORE: &str = r#"#!/usr/bin/python3
import base64, json, pathlib, sys
root = pathlib.Path(__file__).resolve().parent
request = json.load(sys.stdin)
operation = request['operation']
if operation.startswith('db.'):
    path = pathlib.Path(request['path']).resolve()
    if not path.is_relative_to(root):
        raise SystemExit(2)
else:
    name = base64.urlsafe_b64encode((request['service'] + '\0' + request['account']).encode()).decode()
    path = root / ('key-' + name)
if operation in ('read', 'db.read'):
    result = {'status': 'ok', 'value': path.read_text()} if path.exists() else {'status': 'missing'}
elif operation in ('write', 'db.write'):
    if (operation == 'write' and (root / 'fail-write').exists()) or (operation == 'db.write' and (root / 'fail-db').exists()):
        result = {'status': 'failed'}
    else:
        if not (operation == 'write' and (root / 'drop-write').exists()):
            path.write_text(request['value'])
        result = {'status': 'ok'}
elif operation == 'delete':
    if (root / 'fail-delete').exists():
        result = {'status': 'failed'}
    else:
        path.unlink(missing_ok=True)
        result = {'status': 'ok'}
else:
    result = {'status': 'invalid'}
print(json.dumps(result))
"#;

const FAKE_CODEX: &str = r#"#!/usr/bin/python3
import json, os, pathlib, sys
root = pathlib.Path(__file__).resolve().parent
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    result = {}
    if request.get('method') == 'account/read':
        revoked = (root / 'revoked-auth').exists() or (root / 'quota-revoked').exists()
        result = {'account': None if revoked else {'type': 'chatgpt'}, 'requiresOpenaiAuth': True}
    if request.get('method') == 'account/rateLimits/read':
        (root / 'quota-called').touch()
        if (root / 'revoke-during-quota').exists() or (root / 'quota-outage').exists():
            if (root / 'revoke-during-quota').exists():
                (root / 'quota-revoked').touch()
            print(json.dumps({'id': request['id'], 'error': {'code': -32603, 'message': 'failed to fetch codex rate limits: SYNTHETIC_SECRET HTTP 401 Unauthorized'}}), flush=True)
            continue
        path = pathlib.Path(os.environ['CODEX_HOME']) / 'auth.json'
        if not path.resolve().is_relative_to(root):
            raise SystemExit(2)
        if (root / 'corrupt-auth').exists():
            path.write_text('CORRUPT_SYNTHETIC_AUTH')
        else:
            auth = json.loads(path.read_text())
            auth['tokens']['refresh_token'] = 'SYNTHETIC_ROTATED_REFRESH'
            path.write_text(json.dumps(auth))
        result = {'rateLimits': {'primary': {'usedPercent': 10}}}
    print(json.dumps({'id': request['id'], 'result': result}), flush=True)
"#;

fn claude(owner: &str, refresh: &str) -> Bundle {
    Bundle {
        provider: "claude".into(),
        credential: json!({"accessToken":format!("SYNTHETIC_ACCESS_{owner}"),"refreshToken":refresh,
            "expiresAt":1000,"scopes":["user:inference","user:profile"],"subscriptionType":"pro"}),
        identity: json!({"accountUuid":owner,"organizationUuid":"synthetic-org","emailAddress":format!("{owner}@example.invalid")}),
    }
}

fn account(id: &str, bundle: &Bundle, native: bool) -> Account {
    Account {
        id: id.into(),
        provider: bundle.provider.clone(),
        label: id.into(),
        metadata: Some(bundle.metadata().unwrap()),
        native,
        automatic: true,
        state: "saved".into(),
        message: String::new(),
        last_local: None,
        last_online: None,
        last_refresh: None,
        next_check: 0,
        failures: 0,
        quota: vec![],
        pending_profile: None,
        warmup: None,
        warmup_off: None,
    }
}

fn finish_account_job(manager: &mut Accounts) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while manager
        .job
        .as_ref()
        .is_some_and(|j| !j.handle.is_finished())
    {
        assert!(
            std::time::Instant::now() < deadline,
            "account job did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    manager.finish_job();
}

fn hold_unsaved(manager: &mut Accounts, old: &Bundle, next: &Bundle) {
    let result = Outcome {
        bundle: old.clone(),
        online: false,
        refreshed: false,
        state: "unsaved".into(),
        message: "synthetic save failure".into(),
        quota: vec![],
        pending: Some((old.clone(), next.clone())),
    };
    manager.job = Some(Job {
        id: "a".into(),
        handle: std::thread::spawn(move || JobResult::Check(Box::new(result))),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    manager.finish_job();
}

#[test]
fn unsaved_rotation_survives_failed_retry_and_only_updates_metadata_after_save() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let old = claude("a", "SYNTHETIC_OLD_REFRESH");
    let mut next = claude("a", "SYNTHETIC_NEW_REFRESH");
    next.credential["expiresAt"] = json!(9000000);
    fixture.platform.vault_write("a", &old.encoded()).unwrap();
    manager.db.accounts.push(account("a", &old, false));
    hold_unsaved(&mut manager, &old, &next);
    assert!(manager.unsaved.contains_key("a"));
    assert_eq!(
        manager.db.accounts[0].metadata.as_ref().unwrap().expires_at,
        Some(1)
    );
    assert_eq!(manager.db.accounts[0].last_refresh, None);
    fs::write(fixture.root.join("fail-write"), b"synthetic").unwrap();
    assert!(manager.retry_save("a").is_err());
    assert!(manager.unsaved.contains_key("a"));
    fs::remove_file(fixture.root.join("fail-write")).unwrap();
    manager.retry_save("a").unwrap();
    assert!(!manager.unsaved.contains_key("a"));
    assert_eq!(fixture.platform.vault_read("a").unwrap(), next.encoded());
    assert_eq!(
        manager.db.accounts[0].metadata.as_ref().unwrap().expires_at,
        Some(9000)
    );
    assert!(manager.db.accounts[0].last_refresh.is_some());
}

#[test]
fn retry_save_never_overwrites_a_newer_credential() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let old = claude("a", "SYNTHETIC_OLD_REFRESH");
    let next = claude("a", "SYNTHETIC_PENDING_REFRESH");
    let newer = claude("a", "SYNTHETIC_OTHER_REFRESH");
    manager.db.accounts.push(account("a", &old, false));
    hold_unsaved(&mut manager, &old, &next);
    fixture.platform.vault_write("a", &newer.encoded()).unwrap();
    assert!(manager.retry_save("a").is_err());
    assert_eq!(fixture.platform.vault_read("a").unwrap(), newer.encoded());
    assert!(manager.unsaved.contains_key("a"));
}

#[test]
fn scheduler_writes_back_a_held_result_and_never_regrants_over_it() {
    for state in ["unsaved", "uncertain"] {
        let fixture = Fixture::new();
        let mut manager = fixture.manager();
        let old = claude("a", "SYNTHETIC_OLD_REFRESH");
        let next = claude("a", "SYNTHETIC_PENDING_REFRESH");
        fixture.platform.vault_write("a", &old.encoded()).unwrap();
        manager.db.accounts.push(account("a", &old, false));
        hold_unsaved(&mut manager, &old, &next);
        manager.db.accounts[0].state = state.into();
        manager.local_check("a").unwrap();
        manager.finish_job();
        assert_eq!(manager.db.accounts[0].state, state);
        assert_eq!(manager.db.accounts[0].message, "synthetic save failure");
        assert_eq!(
            manager.check("a", true),
            Err("请先重试保存未提交的凭据，或重新登录；不会重复提交旧刷新凭据")
        );
        assert!(manager.job.is_none());
        manager.db.accounts[0].next_check = 0;
        manager.last_scan = 0;
        manager.tick();
        assert!(manager.job.is_none(), "write-back must not start a check");
        assert_eq!(fixture.platform.vault_read("a").unwrap(), next.encoded());
        assert_eq!(manager.db.accounts[0].state, "saved");
        assert!(!manager.unsaved.contains_key("a"));
    }
}

#[test]
fn unanswered_claude_states_stay_scheduled_and_survive_restart_as_reconciliation() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    for (id, native, state) in [
        ("interrupted", true, "checking"),
        ("lost", false, "unsaved"),
        ("journal", true, "unsaved"),
        ("unknown", false, "uncertain"),
    ] {
        let mut a = account(id, &claude(id, "SYNTHETIC_REFRESH"), native);
        a.state = state.into();
        manager.db.accounts.push(a);
    }
    let mut codex = account("codex", &claude("codex", "SYNTHETIC_REFRESH"), false);
    codex.provider = "codex".into();
    codex.state = "unsaved".into();
    manager.db.accounts.push(codex);
    manager.save().unwrap();
    let reopened = fixture.manager();
    let states: Vec<_> = reopened
        .db
        .accounts
        .iter()
        .map(|a| a.state.as_str())
        .collect();
    assert_eq!(
        states,
        ["uncertain", "uncertain", "unsaved", "uncertain", "unsaved"]
    );
    let scheduled: Vec<_> = reopened
        .db
        .accounts
        .iter()
        .map(|a| due(a, u64::MAX))
        .collect();
    assert_eq!(scheduled, [true, true, true, true, false]);
}

#[test]
fn recovery_repairs_known_identity_without_overwriting_previous_account() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let old = claude("a", "SYNTHETIC_A_REFRESH");
    let target = claude("b", "SYNTHETIC_B_REFRESH");
    let native = fixture.native();
    write_json(
        &native.directory.join(".credentials.json"),
        &json!({"claudeAiOauth":target.credential}),
    )
    .unwrap();
    write_json(
        &native.config,
        &json!({"oauthAccount":old.identity,"projects":{"keep":true}}),
    )
    .unwrap();
    fixture.platform.vault_write("a", &old.encoded()).unwrap();
    fixture
        .platform
        .vault_write("b", &target.encoded())
        .unwrap();
    manager.db.accounts = vec![account("a", &old, true), account("b", &target, false)];
    manager.native_override = Some(native.clone());
    manager.db.switch = Some(Switch {
        desktop: false,
        provider: "claude".into(),
        target: "b".into(),
        previous: Some("a".into()),
    });
    manager.recover().unwrap();
    finish_account_job(&mut manager);
    assert_eq!(fixture.platform.vault_read("a").unwrap(), old.encoded());
    assert_eq!(
        native.read(&fixture.platform).unwrap().encoded(),
        target.encoded()
    );
    assert_eq!(
        read_json(&native.config).unwrap().unwrap()["projects"]["keep"],
        true
    );
    assert!(manager.db.switch.is_none());
    assert!(!manager.db.accounts[0].native);
    assert!(manager.db.accounts[1].native);
}

#[test]
fn recovery_rejects_unknown_credentials_without_clearing_journal() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let old = claude("a", "SYNTHETIC_A_REFRESH");
    let target = claude("b", "SYNTHETIC_B_REFRESH");
    let unknown = claude("c", "SYNTHETIC_UNKNOWN_REFRESH");
    let native = fixture.native();
    let original_config = json!({"oauthAccount":old.identity,"projects":{"keep":true}});
    write_json(
        &native.directory.join(".credentials.json"),
        &json!({"claudeAiOauth":unknown.credential}),
    )
    .unwrap();
    write_json(&native.config, &original_config).unwrap();
    fixture.platform.vault_write("a", &old.encoded()).unwrap();
    fixture
        .platform
        .vault_write("b", &target.encoded())
        .unwrap();
    manager.db.accounts = vec![account("a", &old, true), account("b", &target, false)];
    manager.native_override = Some(native.clone());
    manager.profile_transport = Some(|_, _, _, _| {
        Ok((
            200,
            json!({"account":{"uuid":"c"},"organization":{"uuid":"synthetic-org"}}),
        ))
    });
    manager.db.switch = Some(Switch {
        desktop: false,
        provider: "claude".into(),
        target: "b".into(),
        previous: Some("a".into()),
    });
    manager.recover().unwrap();
    finish_account_job(&mut manager);
    assert!(manager.notice.as_deref().unwrap().contains("未登记"));
    assert!(manager.db.switch.is_some());
    assert_eq!(read_json(&native.config).unwrap().unwrap(), original_config);
    assert_eq!(fixture.platform.vault_read("a").unwrap(), old.encoded());
    assert_eq!(fixture.platform.vault_read("b").unwrap(), target.encoded());
}

fn codex_bundle() -> Bundle {
    let id = format!(
        "x.{}.x",
        URL_SAFE_NO_PAD.encode(
            json!({"https://api.openai.com/auth":{
        "chatgpt_user_id":"synthetic-user","chatgpt_account_id":"synthetic-account"}})
            .to_string()
        )
    );
    let access = format!(
        "x.{}.x",
        URL_SAFE_NO_PAD.encode(json!({"exp":9999999999u64}).to_string())
    );
    Bundle {
        provider: "codex".into(),
        identity: Value::Null,
        credential: json!({"tokens":{
        "id_token":id,"access_token":access,"refresh_token":"SYNTHETIC_OLD_REFRESH","account_id":"synthetic-account"}}),
    }
}

#[test]
fn remove_clears_owned_credentials_and_profiles_but_preserves_source_and_default() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let bundle = codex_bundle();
    let source = fixture.root.join("import-source.json");
    write_json(&source, &bundle.credential).unwrap();
    manager.import_codex_file(&source, "synthetic").unwrap();
    let id = manager.db.accounts[0].id.clone();
    let stage = manager.staging("codex", "pending-profile").unwrap();
    write_json(&stage.directory.join("auth.json"), &bundle.credential).unwrap();
    manager.db.accounts[0].pending_profile = Some("pending-profile".into());
    manager
        .unsaved
        .insert(id.clone(), (bundle.clone(), bundle.clone()));
    let check = fixture.root.join("profiles").join(format!("check-{id}"));
    private_dir(&check).unwrap();
    write_json(&check.join("auth.json"), &bundle.credential).unwrap();
    private_dir(&manager.sources.codex_home).unwrap();
    let default = manager.sources.codex_home.join("auth.json");
    write_json(&default, &json!({"synthetic":"default-must-remain"})).unwrap();
    let original_default = fs::read(&default).unwrap();
    let original_source = fs::read(&source).unwrap();
    manager.save().unwrap();
    manager.remove(&id, true).unwrap();
    assert!(manager.db.accounts.is_empty());
    assert!(fixture.manager().db.accounts.is_empty());
    assert!(fixture.platform.vault_read(&id).is_err());
    assert!(!stage.directory.exists());
    assert!(!check.exists());
    assert!(!manager.unsaved.contains_key(&id));
    assert_eq!(fs::read(source).unwrap(), original_source);
    assert_eq!(fs::read(default).unwrap(), original_default);
    manager.remove(&id, true).unwrap();
}

#[test]
fn remove_requires_confirmation_and_rejects_current_or_bound_accounts() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let bundle = codex_bundle();
    fixture
        .platform
        .vault_write("a", &bundle.encoded())
        .unwrap();
    manager.db.accounts.push(account("a", &bundle, false));
    assert!(manager.remove("a", false).is_err());
    manager.db.accounts[0].native = true;
    assert!(manager.remove("a", true).is_err());
    manager.db.accounts[0].native = false;
    manager.db.desktop.selected = Some("a".into());
    assert!(manager.remove("a", true).is_err());
    manager.db.desktop.selected = None;
    manager.pending_login = Some("a".into());
    assert!(manager.remove("a", true).is_err());
    assert_eq!(fixture.platform.vault_read("a").unwrap(), bundle.encoded());
    assert_eq!(manager.db.accounts.len(), 1);
}

#[test]
fn remove_failure_keeps_durable_id_and_can_retry_after_service_reopen() {
    for failure in ["fail-delete", "fail-db"] {
        let fixture = Fixture::new();
        let mut manager = fixture.manager();
        let bundle = codex_bundle();
        fixture
            .platform
            .vault_write("a", &bundle.encoded())
            .unwrap();
        manager.db.accounts.push(account("a", &bundle, false));
        manager.save().unwrap();
        fs::write(fixture.root.join(failure), b"synthetic").unwrap();
        assert!(manager.remove("a", true).is_err());
        assert_eq!(manager.db.accounts.len(), 1);
        assert_eq!(fixture.manager().db.accounts.len(), 1);
        assert_eq!(
            fixture.platform.vault_read("a").is_ok(),
            failure == "fail-delete"
        );
        if failure == "fail-db" {
            assert!(manager.storage_fault.get());
        }
        fs::remove_file(fixture.root.join(failure)).unwrap();
        let mut reopened = fixture.manager();
        reopened.remove("a", true).unwrap();
        reopened.remove("a", true).unwrap();
        assert!(fixture.manager().db.accounts.is_empty());
        assert!(fixture.platform.vault_read("a").is_err());
    }
}

#[test]
fn remove_waits_for_its_quota_worker_then_forgets_completed_cache() {
    use std::sync::Barrier;
    static GATE: Barrier = Barrier::new(2);
    fn held_usage(
        _: &crate::codex_api::Access,
        request: &crate::codex_api::Request,
    ) -> Result<crate::codex_api::Response> {
        if matches!(request, crate::codex_api::Request::Usage) {
            GATE.wait();
            GATE.wait();
        }
        Ok(crate::codex_api::Response {
            status: 200,
            retry_after: None,
            body: json!({"rate_limit":{"primary_window":{"used_percent":40}}}),
        })
    }
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let bundle = codex_bundle();
    fixture
        .platform
        .vault_write("a", &bundle.encoded())
        .unwrap();
    manager.db.accounts.push(account("a", &bundle, false));
    manager.quota.transport = Some(held_usage);
    manager.quota_tick();
    GATE.wait();
    let attempted = manager.remove("a", true);
    GATE.wait();
    manager.quota.stop(now());
    assert!(attempted.is_err());
    assert_eq!(
        serde_json::to_value(manager.quota.snapshot("a", now(), true)).unwrap()["state"],
        "ready"
    );
    manager.remove("a", true).unwrap();
    assert_eq!(
        serde_json::to_value(manager.quota.snapshot("a", now(), true)).unwrap()["state"],
        "pending"
    );
    assert!(
        manager.snapshot()["accounts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn independent_quota_works_with_running_client_without_auth_writes_or_duplicate_requests() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn usage(
        _: &crate::codex_api::Access,
        request: &crate::codex_api::Request,
    ) -> Result<crate::codex_api::Response> {
        if matches!(request, crate::codex_api::Request::Usage) {
            CALLS.fetch_add(1, Ordering::SeqCst);
        }
        Ok(crate::codex_api::Response {
            status: 200,
            body: json!({"rate_limit":{"primary_window":{"used_percent":40,"limit_window_seconds":18000}}}),
            retry_after: None,
        })
    }
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let bundle = codex_bundle();
    private_dir(&manager.sources.codex_home).unwrap();
    let path = manager.sources.codex_home.join("auth.json");
    write_json(&path, &bundle.credential).unwrap();
    manager.db.accounts.push(account("native", &bundle, true));
    manager.db.accounts[0].automatic = false;
    manager.client_running = |_| true;
    manager.quota.transport = Some(usage);
    let before_account = serde_json::to_value(&manager.db.accounts[0]).unwrap();
    let before_file = fs::read(&path).unwrap();
    fs::write(fixture.root.join("fail-write"), b"synthetic").unwrap();
    manager.request_quota(Some("native")).unwrap();
    manager.quota_tick();
    manager.request_quota(Some("native")).unwrap();
    manager.quota_tick();
    manager.quota.stop(now());
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    assert!(manager.job.is_none());
    assert_eq!(fs::read(path).unwrap(), before_file);
    assert_eq!(
        serde_json::to_value(&manager.db.accounts[0]).unwrap(),
        before_account
    );
    let snapshot = manager.snapshot();
    assert_eq!(snapshot["accounts"][0]["usage"]["state"], "ready");
    assert_eq!(
        snapshot["accounts"][0]["usage"]["windows"][0]["used_percent"],
        40.0
    );
    assert!(!snapshot.to_string().contains("SYNTHETIC_OLD_REFRESH"));
    assert!(!snapshot.to_string().contains("synthetic-account"));
}

#[test]
fn requested_quota_reads_as_refreshing_from_request_until_result() {
    fn usage(
        _: &crate::codex_api::Access,
        _: &crate::codex_api::Request,
    ) -> Result<crate::codex_api::Response> {
        Ok(crate::codex_api::Response {
            status: 200,
            body: json!({"rate_limit":{"primary_window":{"used_percent":40,"limit_window_seconds":18000}}}),
            retry_after: None,
        })
    }
    let refreshing = |m: &Accounts| {
        m.snapshot()["accounts"][0]["usage"]["refreshing"]
            .as_bool()
            .unwrap()
    };
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let bundle = codex_bundle();
    private_dir(&manager.sources.codex_home).unwrap();
    write_json(
        &manager.sources.codex_home.join("auth.json"),
        &bundle.credential,
    )
    .unwrap();
    manager.db.accounts.push(account("native", &bundle, true));
    manager.quota.transport = Some(usage);
    // The IPC reply is built before the scheduler starts the job; it must not read as idle.
    manager.request_quota(Some("native")).unwrap();
    assert!(refreshing(&manager));
    manager.quota_tick();
    assert!(refreshing(&manager));
    manager.quota.stop(now());
    assert!(!refreshing(&manager));
    assert_eq!(manager.snapshot()["accounts"][0]["usage"]["state"], "ready");
    // A request from elsewhere (the Claude mod, a warm-up) for an account that then stops being
    // schedulable does not read as running until the account is back.
    manager.quota.request("native", now());
    manager.db.accounts[0].pending_profile = Some("synthetic-pending".into());
    assert!(!refreshing(&manager));
    manager.db.accounts[0].pending_profile = None;
    assert!(refreshing(&manager));
    manager.quota.invalidate("native");
    // An account the scheduler skips is refused instead of staying queued forever.
    manager.db.accounts[0].native = false;
    manager.db.accounts[0].state = "unsaved".into();
    assert!(manager.request_quota(Some("native")).is_err());
    assert!(!refreshing(&manager));
}

#[test]
fn failed_import_never_publishes_empty_accounts_and_retry_is_idempotent() {
    for failure in ["fail-write", "drop-write"] {
        let fixture = Fixture::new();
        let mut manager = fixture.manager();
        let bundle = codex_bundle();
        private_dir(&manager.sources.codex_home).unwrap();
        let native_file = manager.sources.codex_home.join("auth.json");
        write_json(&native_file, &bundle.credential).unwrap();
        let before = fs::read(&native_file).unwrap();
        fs::write(fixture.root.join(failure), b"synthetic").unwrap();
        for _ in 0..2 {
            assert!(manager.import("codex", "Codex current").is_err());
            assert!(
                manager.db.accounts.is_empty(),
                "a failed import must not publish an account"
            );
        }
        assert!(fixture.manager().db.accounts.is_empty());
        fs::remove_file(fixture.root.join(failure)).unwrap();
        manager.import("codex", "Codex current").unwrap();
        let id = manager.db.accounts[0].id.clone();
        manager.import("codex", "Codex current").unwrap();
        assert_eq!(manager.db.accounts.len(), 1);
        assert_eq!(manager.db.accounts[0].id, id);
        assert!(manager.db.accounts[0].metadata.is_some());
        assert!(manager.db.accounts[0].native);
        assert_eq!(fixture.manager().db.accounts.len(), 1);
        assert_eq!(fs::read(&native_file).unwrap(), before);
    }
}

#[test]
fn blank_import_name_defaults_to_login_identity_and_keeps_existing_name() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    private_dir(&manager.sources.codex_home).unwrap();
    write_json(
        &manager.sources.codex_home.join("auth.json"),
        &codex_bundle().credential,
    )
    .unwrap();
    manager.import("codex", "  ").unwrap();
    let identity = manager.db.accounts[0]
        .metadata
        .as_ref()
        .unwrap()
        .display
        .clone();
    assert!(!identity.is_empty());
    assert_eq!(manager.db.accounts[0].label, identity);
    manager
        .configure(&manager.db.accounts[0].id.clone(), Some("work"), None)
        .unwrap();
    manager.import("codex", "").unwrap();
    assert_eq!(manager.db.accounts[0].label, "work");
}

#[test]
fn scheduler_advances_between_accounts_without_refreshing_native_shadow() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    manager.codex = Some(fixture.codex(false));
    let bundle = codex_bundle();
    for id in ["first", "second"] {
        let mut bundle = bundle.clone();
        let mut claims =
            crate::credentials::claims(bundle.credential["tokens"]["id_token"].as_str().unwrap())
                .unwrap();
        claims["https://api.openai.com/auth"]["chatgpt_user_id"] = json!(id);
        bundle.credential["tokens"]["id_token"] = json!(format!(
            "x.{}.x",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        ));
        fixture.platform.vault_write(id, &bundle.encoded()).unwrap();
        manager.db.accounts.push(account(id, &bundle, false));
    }
    private_dir(&manager.sources.codex_home).unwrap();
    write_json(
        &manager.sources.codex_home.join("auth.json"),
        &bundle.credential,
    )
    .unwrap();
    manager.db.accounts.push(account("native", &bundle, true));
    for id in ["first", "second"] {
        manager.last_scan = 0;
        manager.tick();
        assert_eq!(manager.job.as_ref().unwrap().id, id);
        manager.finish_job();
        let index = manager.index(id).unwrap();
        assert!(manager.db.accounts[index].last_online.is_some());
        assert!(manager.db.accounts[index].last_refresh.is_some());
    }
    manager.last_scan = 0;
    manager.tick();
    assert_eq!(manager.job.as_ref().unwrap().id, "native");
    manager.finish_job();
    assert!(manager.job.is_none());
    assert_eq!(manager.db.accounts[2].state, "delegated");
    assert!(manager.db.accounts[2].last_online.is_none());
    assert_eq!(
        read_json(&manager.sources.codex_home.join("auth.json"))
            .unwrap()
            .unwrap(),
        bundle.credential
    );
}

#[test]
fn codex_save_failure_or_false_ack_never_marks_rotation_successful() {
    for failure in ["fail-write", "drop-write"] {
        let fixture = Fixture::new();
        let old = codex_bundle();
        fixture.platform.vault_write("c", &old.encoded()).unwrap();
        fs::write(fixture.root.join(failure), b"synthetic").unwrap();
        let result = Work {
            transport: crate::maintenance::http,
            id: "c".into(),
            bundle: old.clone(),
            platform: fixture.platform.clone(),
            profile: fixture.root.join("check"),
            codex: Some(fixture.codex(false)),
            native: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
        .run();
        assert_eq!(result.state, "unsaved");
        assert!(!result.online && !result.refreshed);
        assert!(result.pending.is_some());
        assert_eq!(result.bundle.encoded(), old.encoded());
        assert_eq!(fixture.platform.vault_read("c").unwrap(), old.encoded());
    }
}

#[test]
fn codex_quota_success_cannot_hide_corrupt_persisted_credentials() {
    let fixture = Fixture::new();
    let old = codex_bundle();
    fixture.platform.vault_write("c", &old.encoded()).unwrap();
    let result = Work {
        transport: crate::maintenance::http,
        id: "c".into(),
        bundle: old,
        platform: fixture.platform.clone(),
        profile: fixture.root.join("check"),
        codex: Some(fixture.codex(true)),
        native: None,
        cancel: Arc::new(AtomicBool::new(false)),
    }
    .run();
    assert!(!result.online && !result.refreshed);
    assert_eq!(result.state, "attention");
}

#[test]
fn cancelled_login_with_different_identity_never_overwrites_original() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let old = claude("a", "SYNTHETIC_A_REFRESH");
    let other = claude("b", "SYNTHETIC_B_REFRESH");
    fixture.platform.vault_write("a", &old.encoded()).unwrap();
    let mut entry = account("a", &old, false);
    entry.pending_profile = Some("synthetic-pending".into());
    manager.db.accounts.push(entry);
    let staging = manager.staging("claude", "synthetic-pending").unwrap();
    write_json(
        &staging.directory.join(".credentials.json"),
        &json!({"claudeAiOauth":other.credential}),
    )
    .unwrap();
    write_json(&staging.config, &json!({"oauthAccount":other.identity})).unwrap();
    manager.pending_login = Some("a".into());
    let mut login = LoginManager::new();
    login.state = Some(LoginState {
        attempt_id: "synthetic".into(),
        provider: "claude".into(),
        phase: "cancelled".into(),
        message: String::new(),
        verification_url: None,
        user_code: None,
    });
    manager.finish_login(&mut login);
    assert_eq!(fixture.platform.vault_read("a").unwrap(), old.encoded());
    // Another identity's login can never be saved here, so the account is left as it was.
    assert_eq!(login.state.unwrap().phase, "failed");
    assert_eq!(manager.db.accounts[0].state, "saved");
    assert!(manager.db.accounts[0].pending_profile.is_none());
}

static REFRESH_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn grant_transport(
    url: &str,
    _token: Option<&str>,
    body: Option<&Value>,
    _usage: bool,
) -> Result<(u16, Value)> {
    if url.ends_with("/v1/oauth/token") {
        REFRESH_CALLS.fetch_add(1, Ordering::Relaxed);
        let body = body.unwrap();
        assert_eq!(body["grant_type"], "refresh_token");
        assert!(body.get("expires_in").is_none());
        Ok((
            200,
            json!({"access_token":"SYNTHETIC_NEW_ACCESS","refresh_token":"SYNTHETIC_NEW_REFRESH","expires_in":3600}),
        ))
    } else {
        Err("synthetic profile outage")
    }
}

#[test]
fn claude_http_rotation_failed_save_is_retried_without_second_grant() {
    let fixture = Fixture::new();
    let old = claude("a", "SYNTHETIC_OLD_REFRESH");
    fixture.platform.vault_write("a", &old.encoded()).unwrap();
    fs::write(fixture.root.join("fail-write"), b"synthetic").unwrap();
    REFRESH_CALLS.store(0, Ordering::Relaxed);
    let outcome = Work {
        id: "a".into(),
        bundle: old.clone(),
        platform: fixture.platform.clone(),
        profile: fixture.root.join("unused"),
        codex: None,
        native: None,
        cancel: Arc::new(AtomicBool::new(false)),
        transport: grant_transport,
    }
    .run();
    assert_eq!(outcome.state, "unsaved");
    assert!(!outcome.refreshed);
    assert_eq!(REFRESH_CALLS.load(Ordering::Relaxed), 1);
    let mut manager = fixture.manager();
    manager.db.accounts.push(account("a", &old, false));
    manager.job = Some(Job {
        id: "a".into(),
        cancel: Arc::new(AtomicBool::new(false)),
        handle: std::thread::spawn(move || JobResult::Check(Box::new(outcome))),
    });
    manager.finish_job();
    assert!(manager.unsaved.contains_key("a"));
    assert!(manager.db.accounts[0].last_refresh.is_none());
    fs::remove_file(fixture.root.join("fail-write")).unwrap();
    manager.retry_save("a").unwrap();
    assert_eq!(REFRESH_CALLS.load(Ordering::Relaxed), 1);
    assert!(manager.db.accounts[0].last_refresh.is_some());
    assert_eq!(
        fixture.platform.vault_read("a").unwrap()["credential"]["refreshToken"],
        "SYNTHETIC_NEW_REFRESH"
    );
}

fn profile_outage_transport(
    url: &str,
    token: Option<&str>,
    _body: Option<&Value>,
    _usage: bool,
) -> Result<(u16, Value)> {
    if url.ends_with("/v1/oauth/token") {
        Ok((
            200,
            json!({"access_token":"SYNTHETIC_ROTATED","refresh_token":"SYNTHETIC_ROTATED_REFRESH","expires_in":3600}),
        ))
    } else {
        assert_eq!(token, Some("SYNTHETIC_ROTATED"));
        Err("synthetic profile outage")
    }
}
#[test]
fn claude_keeps_committed_refresh_when_later_profile_request_fails() {
    let fixture = Fixture::new();
    let old = claude("a", "SYNTHETIC_OLD_REFRESH");
    fixture.platform.vault_write("a", &old.encoded()).unwrap();
    let outcome = Work {
        id: "a".into(),
        bundle: old,
        platform: fixture.platform.clone(),
        profile: fixture.root.join("unused"),
        codex: None,
        native: None,
        cancel: Arc::new(AtomicBool::new(false)),
        transport: profile_outage_transport,
    }
    .run();
    assert!(outcome.refreshed);
    assert!(!outcome.online);
    assert_eq!(outcome.state, "retry");
    assert_eq!(
        fixture.platform.vault_read("a").unwrap()["credential"]["refreshToken"],
        "SYNTHETIC_ROTATED_REFRESH"
    );
}

static CANCEL_ON_PROFILE: std::sync::Mutex<Option<Arc<AtomicBool>>> = std::sync::Mutex::new(None);
fn cancelling_transport(
    url: &str,
    _token: Option<&str>,
    _body: Option<&Value>,
    _usage: bool,
) -> Result<(u16, Value)> {
    assert!(
        url.ends_with("/api/oauth/profile"),
        "no refresh request may begin after cancellation"
    );
    CANCEL_ON_PROFILE
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .store(true, Ordering::Release);
    Ok((401, Value::Null))
}
#[test]
fn cancellation_prevents_initial_refresh_and_401_followup_refresh() {
    let fixture = Fixture::new();
    for initially_cancelled in [true, false] {
        let mut bundle = claude("a", "SYNTHETIC_REFRESH");
        if !initially_cancelled {
            bundle.credential["expiresAt"] = json!((now() + 3600) * 1000);
        }
        let cancel = Arc::new(AtomicBool::new(initially_cancelled));
        *CANCEL_ON_PROFILE.lock().unwrap() = Some(cancel.clone());
        let result = Work {
            id: "a".into(),
            bundle,
            platform: fixture.platform.clone(),
            profile: fixture.root.join("unused"),
            codex: None,
            native: None,
            cancel,
            transport: cancelling_transport,
        }
        .run();
        assert!(!result.refreshed && !result.online);
        assert_eq!(result.message, "检查已停止");
        *CANCEL_ON_PROFILE.lock().unwrap() = None;
    }
}

#[test]
fn file_import_is_offline_paused_and_keeps_native_and_duplicate_credentials() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    private_dir(&manager.sources.codex_home).unwrap();
    let mut paths = Vec::new();
    for name in ["nova", "orbit", "current"] {
        let mut bundle = codex_bundle();
        let mut claims =
            crate::credentials::claims(bundle.credential["tokens"]["id_token"].as_str().unwrap())
                .unwrap();
        claims["https://api.openai.com/auth"]["chatgpt_user_id"] = json!(name);
        bundle.credential["tokens"]["id_token"] = json!(format!(
            "x.{}.x",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        ));
        let path = manager.sources.codex_home.join(if name == "current" {
            "auth.json".to_string()
        } else {
            format!("auth-{name}.json")
        });
        write_json(&path, &bundle.credential).unwrap();
        paths.push((
            path,
            fs::read(manager.sources.codex_home.join(if name == "current" {
                "auth.json".to_string()
            } else {
                format!("auth-{name}.json")
            }))
            .unwrap(),
        ));
    }
    for (path, _) in &paths {
        manager
            .import_codex_file(path, path.file_stem().unwrap().to_str().unwrap())
            .unwrap();
    }
    assert_eq!(manager.db.accounts.len(), 3);
    assert_eq!(manager.db.accounts.iter().filter(|a| a.native).count(), 1);
    assert!(manager.db.accounts[2].native);
    assert!(manager.db.accounts.iter().all(|a| !a.automatic
        && a.last_online.is_none()
        && a.last_refresh.is_none()
        && !due(a, u64::MAX)));
    manager.tick();
    assert!(manager.job.is_none());
    let id = manager.db.accounts[0].id.clone();
    let mut current = fixture.platform.vault_read(&id).unwrap();
    current["credential"]["tokens"]["refresh_token"] = json!("SYNTHETIC_NEWER_SAVED_TOKEN");
    fixture.platform.vault_write(&id, &current).unwrap();
    manager.configure(&id, None, Some(true)).unwrap();
    manager.import_codex_file(&paths[0].0, "duplicate").unwrap();
    assert_eq!(manager.db.accounts.len(), 3);
    assert_eq!(fixture.platform.vault_read(&id).unwrap(), current);
    assert!(!manager.db.accounts[0].automatic);
    assert!(fixture.manager().db.accounts.iter().all(|a| !a.automatic));
    for (path, before) in paths {
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[test]
fn codex_hidden_permanent_refresh_failure_is_not_treated_as_quota_outage() {
    for failure in ["revoked-auth", "revoke-during-quota", "quota-outage"] {
        let fixture = Fixture::new();
        let old = codex_bundle();
        fixture.platform.vault_write("c", &old.encoded()).unwrap();
        fs::write(fixture.root.join(failure), b"synthetic").unwrap();
        let result = Work {
            transport: crate::maintenance::http,
            id: "c".into(),
            bundle: old.clone(),
            platform: fixture.platform.clone(),
            profile: fixture.root.join("check"),
            codex: Some(fixture.codex(false)),
            native: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
        .run();
        assert!(!result.online && !result.refreshed);
        assert!(!result.message.contains("SYNTHETIC_SECRET"));
        assert_eq!(fixture.platform.vault_read("c").unwrap(), old.encoded());
        if failure == "quota-outage" {
            assert_eq!(result.state, "retry");
            assert_eq!(result.message, "Codex 额度查询暂不可用；未确认账号失效");
        } else {
            assert_eq!(result.state, "reauth");
            assert_eq!(result.message, "官方认证需要重新登录");
        }
        if failure == "revoked-auth" {
            assert!(!fixture.root.join("quota-called").exists());
        }
    }
}

#[test]
fn only_accounts_the_local_strategy_resets_are_paced_toward_their_line() {
    use crate::automation::{Config, Order, Rule};
    fn usage(
        _: &crate::codex_api::Access,
        _: &crate::codex_api::Request,
    ) -> Result<crate::codex_api::Response> {
        Ok(crate::codex_api::Response {
            status: 200,
            body: json!({"rate_limit":{"primary_window":{"used_percent":40,"limit_window_seconds":604800}}}),
            retry_after: None,
        })
    }
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let bundle = codex_bundle();
    private_dir(&manager.sources.codex_home).unwrap();
    write_json(
        &manager.sources.codex_home.join("auth.json"),
        &bundle.credential,
    )
    .unwrap();
    manager.db.accounts.push(account("native", &bundle, true));
    manager.quota.transport = Some(usage);
    let rule = |order, participate| Rule {
        id: "native".into(),
        order,
        participate,
        below: 5,
    };
    manager.db.automation.config = Config {
        enabled: true,
        accounts: vec![rule(Order::ResetFirst, true)],
    };
    manager.quota_tick();
    assert_eq!(manager.quota.line("native"), Some(5.0));
    manager.quota.stop(now());
    for (enabled, rule, mode) in [
        (false, rule(Order::ResetFirst, true), "local"),
        (true, rule(Order::ResetFirst, false), "local"),
        (true, rule(Order::SwitchOnly, true), "local"),
        (true, rule(Order::ResetFirst, true), "cloud"),
    ] {
        manager.db.automation.config = Config {
            enabled,
            accounts: vec![rule],
        };
        manager.cloud.config.mode = mode.into();
        assert_eq!(manager.quota_line("native"), None, "{enabled} {mode}");
    }
    manager.cloud.config.mode = "local".into();
    manager.db.automation.config = Config {
        enabled: true,
        accounts: vec![rule(Order::SwitchFirst, true)],
    };
    assert_eq!(manager.quota_line("native"), Some(5.0));
}
