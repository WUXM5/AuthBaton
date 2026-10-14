use super::*;

fn bundle(user: &str, generation: u64, expiry: u64) -> Bundle {
    let claims = json!({"exp":expiry,"generation":generation,"email":format!("{user}@example.invalid"),"https://api.openai.com/auth":{"chatgpt_user_id":user,"chatgpt_account_id":format!("workspace-{user}"),"chatgpt_plan_type":"pro"}});
    let token = format!("x.{}.x", URL_SAFE_NO_PAD.encode(claims.to_string()));
    Bundle {
        provider: "codex".into(),
        identity: Value::Null,
        credential: json!({"auth_mode":"chatgpt","tokens":{"id_token":token,"access_token":token,"refresh_token":format!("SYNTHETIC-{user}-{generation}"),"account_id":format!("workspace-{user}")}}),
    }
}
pub(super) fn setup(fixture: &Fixture) -> (Accounts, Bundle, Bundle) {
    let mut manager = fixture.manager();
    private_dir(&manager.sources.codex_home).unwrap();
    let a = bundle("a", 1, now() + 3600);
    let b = bundle("b", 1, now() + 3600);
    write_json(&manager.sources.codex_home.join("auth.json"), &a.credential).unwrap();
    fixture.platform.vault_write("a", &a.encoded()).unwrap();
    fixture.platform.vault_write("b", &b.encoded()).unwrap();
    manager.db.accounts = vec![account("a", &a, true), account("b", &b, false)];
    manager.db.desktop.enabled = true;
    manager.desktop.other_clients = || false;
    let helper = fixture.root.join("desktop-fixture.py");
    fs::write(&helper, FAKE_DESKTOP).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    manager.desktop.helper = Some(helper);
    manager.desktop.last_poll = now() + 1000;
    (manager, a, b)
}
fn finish(manager: &mut Accounts) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while let Some(job) = manager.desktop.job.as_ref() {
        assert!(std::time::Instant::now() < deadline);
        if job.is_finished() {
            manager.desktop_tick();
        } else {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
pub(super) const FAKE_DESKTOP: &str = r#"#!/usr/bin/python3
import base64, hashlib, json, pathlib, sys
root = pathlib.Path(__file__).resolve().parent
request = json.load(sys.stdin)
params = request.get('params') or {}
auth = params.get('auth') or {}
with (root / 'desktop-operations').open('a') as log:
    log.write(request['operation'] + '\n')
if request['operation'] == 'preflight' and (root / 'fail-preflight').exists():
    refusal = {'ok':False,'code':(root / 'fail-preflight').read_text()}
    if (root / 'fail-preflight-tasks.json').exists(): refusal['tasks'] = json.loads((root / 'fail-preflight-tasks.json').read_text())
    print(json.dumps(refusal));sys.exit(0)
if request['operation'] in ('preflight', 'probe'):
    token = json.loads((root / 'home/.codex/auth.json').read_text())['tokens']['access_token']
    claims = json.loads(base64.urlsafe_b64decode(token.split('.')[1] + '=='))['https://api.openai.com/auth']
    owner = 'codex:' + claims['chatgpt_user_id'] + ':' + claims['chatgpt_account_id']
    response = {'ok':True,'owner':owner,'mode':'chatgpt','boundOwner':None,'target':'synthetic-target','instance':'synthetic-instance','hash':hashlib.sha256(token.encode()).hexdigest()}
    if (root / 'probe-override.json').exists(): response.update(json.loads((root / 'probe-override.json').read_text()))
    print(json.dumps(response));sys.exit(0)
if request['operation'] == 'apply':
    assert json.loads((root / 'accounts.sqlite3').read_text())['switch']['desktop']
if request['operation'] == 'apply' and (root / 'rotate-to.json').exists():
    (root / 'home/.codex/auth.json').write_text((root / 'rotate-to.json').read_text())
    (root / 'rotate-to.json').unlink()
owner = params.get('owner', 'codex:a:workspace-a')
print(json.dumps({'ok':True,'confirmed':not (root / 'fail-completion').exists(),'owner':owner,'boundOwner':owner,'mode':'chatgptAuthTokens','target':'synthetic-target','instance':'synthetic-instance','hash':hashlib.sha256(auth.get('accessToken','').encode()).hexdigest(),'requests':[]}))
"#;

#[test]
fn desktop_partial_commit_preserves_default_and_recovers_forward() {
    let fixture = Fixture::new();
    let (mut manager, a, b) = setup(&fixture);
    fs::write(fixture.root.join("fail-completion"), b"fixture").unwrap();
    manager.activate("b", true).unwrap();
    finish(&mut manager);
    assert_eq!(
        manager
            .native("codex")
            .unwrap()
            .read(&fixture.platform)
            .unwrap()
            .credential,
        a.credential
    );
    assert!(manager.db.accounts[0].native);
    assert!(manager.db.switch.as_ref().unwrap().desktop);
    assert!(manager.db.desktop.selected.is_none());
    fs::remove_file(fixture.root.join("fail-completion")).unwrap();
    manager.recover().unwrap();
    finish(&mut manager);
    assert_eq!(
        manager
            .native("codex")
            .unwrap()
            .read(&fixture.platform)
            .unwrap()
            .credential,
        b.credential
    );
    assert!(manager.db.switch.is_none());
    assert_eq!(manager.db.desktop.selected.as_deref(), Some("b"));
    assert!(manager.db.accounts[1].native);
    manager.activate("a", true).unwrap();
    finish(&mut manager);
    assert_eq!(
        manager
            .native("codex")
            .unwrap()
            .read(&fixture.platform)
            .unwrap()
            .credential,
        a.credential
    );
    assert_eq!(manager.db.desktop.selected.as_deref(), Some("a"));
    assert!(!manager.snapshot().to_string().contains("SYNTHETIC-"));
}
#[test]
fn same_account_adoption_keeps_generation_rotated_during_preflight() {
    let fixture = Fixture::new();
    let (mut manager, _a, _b) = setup(&fixture);
    let rotated = bundle("a", 2, now() + 7200);
    write_json(&fixture.root.join("rotate-to.json"), &rotated.credential).unwrap();
    manager.activate("a", true).unwrap();
    finish(&mut manager);
    assert!(manager.db.switch.is_none());
    assert_eq!(
        manager
            .native("codex")
            .unwrap()
            .read(&fixture.platform)
            .unwrap()
            .credential,
        rotated.credential
    );
    assert_eq!(
        Bundle::decode(fixture.platform.vault_read("a").unwrap())
            .unwrap()
            .credential,
        rotated.credential
    );
    assert_eq!(
        manager.db.accounts[0].metadata.as_ref().unwrap().expires_at,
        rotated.metadata().unwrap().expires_at
    );
}
#[test]
fn expired_target_journal_has_a_non_destructive_native_recovery_exit() {
    let fixture = Fixture::new();
    let (mut manager, a, _b) = setup(&fixture);
    let expired = bundle("b", 1, 1);
    fixture
        .platform
        .vault_write("b", &expired.encoded())
        .unwrap();
    manager.db.switch = Some(Switch {
        provider: "codex".into(),
        target: "b".into(),
        previous: Some("a".into()),
        desktop: true,
    });
    manager.save().unwrap();
    assert!(manager.recover().is_err());
    manager.recover_desktop_native(true).unwrap();
    assert!(manager.db.switch.is_none());
    assert!(!manager.db.desktop.enabled);
    assert!(manager.db.accounts[0].native);
    assert_eq!(
        manager
            .native("codex")
            .unwrap()
            .read(&fixture.platform)
            .unwrap()
            .credential,
        a.credential
    );
    assert!(manager.ensure_idle().is_ok());
}
#[test]
fn version_one_metadata_migrates_without_import_or_desktop_enablement() {
    let fixture = Fixture::new();
    fixture
        .platform
        .save_db(
            &fixture.root.join("accounts.sqlite3"),
            &json!({"version":1,"accounts":[],"switch":null}),
        )
        .unwrap();
    let manager = fixture.manager();
    assert_eq!(manager.db.version, 8);
    assert!(!manager.db.desktop.enabled);
    assert!(manager.db.accounts.is_empty());
}

#[test]
fn completed_slow_poll_leaves_controls_available_between_attempts() {
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    manager.desktop.last_poll = now().saturating_sub(30);
    manager.desktop.job = Some(std::thread::spawn(|| {
        crate::desktop::Completion::Observe(Ok(
            json!({"ok":true,"owner":"codex:a:workspace-a","mode":"chatgptAuthTokens","boundOwner":"codex:a:workspace-a","target":"fixture-target","instance":"fixture-instance","hash":"fixture-hash","requests":[]}),
        ))
    }));
    finish(&mut manager);
    let busy = manager.desktop.job.is_some();
    if let Some(job) = manager.desktop.job.take() {
        let _ = job.join();
    }
    assert!(
        !busy,
        "completed slow observation must allow connection controls before retrying"
    );
}

#[test]
fn failed_preflight_does_not_journal_or_apply_and_durable_intent_precedes_apply() {
    for code in [
        "debug_port_unavailable",
        "port_owner_mismatch",
        "target_unavailable",
        "bridge_install_failed",
        "rpc_timeout",
        "unsupported_auth",
        "tasks_active",
    ] {
        let fixture = Fixture::new();
        let (mut manager, a, _) = setup(&fixture);
        manager.save().unwrap();
        let before = fs::read(fixture.root.join("accounts.sqlite3")).unwrap();
        fs::write(fixture.root.join("fail-preflight"), code).unwrap();
        manager.activate("b", true).unwrap();
        assert_eq!(manager.snapshot()["desktop"]["action"]["state"], "checking");
        assert!(manager.db.switch.is_none());
        finish(&mut manager);
        let feedback = manager.snapshot()["desktop"]["action"].clone();
        assert_eq!(feedback["id"], "b");
        assert_eq!(feedback["state"], "failed");
        assert!(feedback["message"].as_str().unwrap().contains("切换未开始"));
        manager.desktop.message = "后续连接轮询状态".into();
        assert_eq!(manager.snapshot()["desktop"]["action"], feedback);
        assert!(manager.db.switch.is_none());
        assert_eq!(
            fs::read(fixture.root.join("accounts.sqlite3")).unwrap(),
            before
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("desktop-operations")).unwrap(),
            "preflight\n"
        );
        assert_eq!(
            manager
                .native("codex")
                .unwrap()
                .read(&fixture.platform)
                .unwrap()
                .credential,
            a.credential
        );
        fs::remove_file(fixture.root.join("fail-preflight")).unwrap();
        manager.activate("b", true).unwrap();
        finish(&mut manager);
        assert!(manager.db.switch.is_none());
        assert!(manager.db.accounts[1].native);
        assert_eq!(manager.snapshot()["desktop"]["action"]["state"], "complete");
    }
}

#[test]
fn busy_thread_refusal_names_the_thread_and_leaves_no_standing_notice() {
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    manager.save().unwrap();
    manager.desktop.connected = true;
    fs::write(fixture.root.join("fail-preflight"), "tasks_active").unwrap();
    fs::write(
        fixture.root.join("fail-preflight-tasks.json"),
        r#"[{"name":"合成任务","flags":["waitingOnApproval"]},{"name":"另一个合成任务","flags":[]}]"#,
    )
    .unwrap();
    manager.activate("b", true).unwrap();
    finish(&mut manager);
    let snapshot = manager.snapshot();
    let feedback = &snapshot["desktop"]["action"];
    assert_eq!(feedback["state"], "failed");
    let message = feedback["message"].as_str().unwrap();
    assert!(
        message.contains("「合成任务」等待审批") && message.contains("「另一个合成任务」运行中"),
        "{message}"
    );
    assert!(message.ends_with("切换未开始"));
    assert!(feedback["at"].as_u64().is_some());
    assert!(
        snapshot["notice"].is_null(),
        "a switch that never started leaves no banner"
    );
    assert!(
        manager.desktop.connected,
        "the bridge answered, so the connection stands"
    );
    assert!(manager.db.switch.is_none());
}

fn pending(manager: &mut Accounts) {
    manager.db.switch = Some(Switch {
        provider: "codex".into(),
        target: "b".into(),
        previous: Some("a".into()),
        desktop: true,
    });
    manager.save().unwrap();
}

#[test]
fn reconcile_only_clears_a_proven_unapplied_managed_switch_without_auth_writes() {
    let fixture = Fixture::new();
    let (mut manager, a, b) = setup(&fixture);
    pending(&mut manager);
    manager.reconcile_desktop().unwrap();
    finish(&mut manager);
    assert!(manager.db.switch.is_none());
    assert!(manager.db.accounts[0].native);
    assert_eq!(
        manager
            .native("codex")
            .unwrap()
            .read(&fixture.platform)
            .unwrap()
            .credential,
        a.credential
    );
    assert_eq!(fixture.platform.vault_read("a").unwrap(), a.encoded());
    assert_eq!(fixture.platform.vault_read("b").unwrap(), b.encoded());
    assert_eq!(
        fs::read_to_string(fixture.root.join("desktop-operations")).unwrap(),
        "probe\n"
    );
}

#[test]
fn reconcile_retains_pending_on_external_changed_unknown_or_failed_evidence() {
    for mismatch in [
        json!({"mode":"chatgptAuthTokens"}),
        json!({"owner":"codex:b:workspace-b"}),
        json!({"hash":"different"}),
        json!({"boundOwner":"codex:a:workspace-a"}),
        json!({"ok":false,"code":"target_unavailable"}),
    ] {
        let fixture = Fixture::new();
        let (mut manager, a, _) = setup(&fixture);
        pending(&mut manager);
        write_json(&fixture.root.join("probe-override.json"), &mismatch).unwrap();
        manager.reconcile_desktop().unwrap();
        finish(&mut manager);
        assert!(manager.db.switch.is_some());
        assert!(fixture.manager().db.switch.is_some());
        assert_eq!(
            manager
                .native("codex")
                .unwrap()
                .read(&fixture.platform)
                .unwrap()
                .credential,
            a.credential
        );
    }
}

#[test]
fn reconcile_save_failure_keeps_journal_and_busy_binding_blocks_reconcile() {
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    pending(&mut manager);
    manager.db.desktop.selected = Some("a".into());
    assert!(manager.reconcile_desktop().is_err());
    manager.db.desktop.selected = None;
    fs::write(fixture.root.join("fail-db"), b"fixture").unwrap();
    manager.reconcile_desktop().unwrap();
    assert!(manager.reconcile_desktop().is_err());
    finish(&mut manager);
    assert!(manager.db.switch.is_some());
    assert!(fixture.manager().db.switch.is_some());
    assert!(manager.storage_fault.get());
}

static RELAUNCHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn counted_relaunch(_: u16) -> Result<()> {
    RELAUNCHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[test]
fn relaunch_switch_refuses_running_turns_and_switches_once_the_desktop_connects() {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = Fixture::new();
    let (mut manager, _a, b) = setup(&fixture);
    manager.desktop.relaunch = counted_relaunch;
    let before = RELAUNCHES.load(SeqCst);
    manager.desktop.turns_running = |_| Ok(true);
    assert_eq!(
        manager.relaunch_desktop_and_activate("b").unwrap_err(),
        "Codex 里还有任务在运行；等它结束后再切换，桌面不会被重启"
    );
    assert_eq!(RELAUNCHES.load(SeqCst), before);
    assert!(manager.desktop.job.is_none());

    manager.desktop.turns_running = |_| Ok(false);
    manager.desktop.target = Some("page-of-the-old-instance".into());
    manager.relaunch_desktop_and_activate("b").unwrap();
    assert_eq!(
        manager.desktop.action.as_ref().unwrap()["state"],
        "relaunching"
    );
    // The tick that completes the relaunch polls at once; connecting then starts the switch.
    finish(&mut manager);
    assert_eq!(RELAUNCHES.load(SeqCst), before + 1);
    let operations = fs::read_to_string(fixture.root.join("desktop-operations")).unwrap();
    assert_eq!(
        operations.lines().collect::<Vec<_>>(),
        ["poll", "preflight", "apply"]
    );
    assert!(manager.desktop.pending_switch.is_none());
    assert_eq!(manager.db.desktop.selected.as_deref(), Some("b"));
    assert_eq!(
        manager
            .native("codex")
            .unwrap()
            .read(&fixture.platform)
            .unwrap()
            .credential,
        b.credential
    );
    assert_eq!(
        manager.desktop.action.as_ref().unwrap()["state"],
        "complete"
    );
}

#[test]
fn failed_relaunch_reports_on_the_attempt_and_leaves_nothing_pending() {
    let fixture = Fixture::new();
    let (mut manager, a, _b) = setup(&fixture);
    manager.desktop.turns_running = |_| Ok(false);
    manager.desktop.relaunch = |_| Err("Codex 桌面没有退出，可能在等待确认；请手动退出后再切换");
    manager.relaunch_desktop_and_activate("b").unwrap();
    finish(&mut manager);
    let action = manager.desktop.action.clone().unwrap();
    assert_eq!(action["state"], "failed");
    assert!(action["message"].as_str().unwrap().ends_with("切换未开始"));
    assert!(manager.desktop.pending_switch.is_none());
    assert!(manager.db.switch.is_none());
    assert_eq!(
        manager
            .native("codex")
            .unwrap()
            .read(&fixture.platform)
            .unwrap()
            .credential,
        a.credential
    );
}

static RESTARTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static DESKTOP_AGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(3);
fn counted_restart(_: u16) -> Result<()> {
    RESTARTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}
fn fresh_desktop(_: u16) -> Result<crate::desktop::Undebugged> {
    Ok(Some((
        4242,
        DESKTOP_AGE.load(std::sync::atomic::Ordering::SeqCst),
    )))
}
fn iso(secs: u64) -> String {
    let time = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::gmtime_r(&time, &mut tm) };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}
fn open_turn(manager: &Accounts, name: &str, started: u64) {
    let day = manager.sources.codex_home.join("sessions/2026/10/14");
    fs::create_dir_all(&day).unwrap();
    let line = format!(
        "{{\"timestamp\":\"{}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"task_started\"}}}}\n",
        iso(started)
    );
    fs::write(day.join(name), line).unwrap();
}

#[test]
fn a_desktop_that_just_started_without_the_debug_port_is_restarted_once() {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    manager.desktop.relaunch = counted_restart;
    manager.desktop.undebugged = fresh_desktop;
    DESKTOP_AGE.store(3, SeqCst);
    let before = RESTARTS.load(SeqCst);
    manager.desktop_tick();
    assert!(manager.desktop.job.is_some());
    finish(&mut manager);
    assert_eq!(RESTARTS.load(SeqCst), before + 1);
    // The restarted desktop is polled at once.
    let operations = fs::read_to_string(fixture.root.join("desktop-operations")).unwrap();
    assert_eq!(operations.lines().collect::<Vec<_>>(), ["poll"]);
    assert!(manager.desktop.connected);
    // The same process seen again is not restarted a second time.
    manager.desktop.last_watch = 0;
    manager.desktop_tick();
    assert!(manager.desktop.job.is_none());
    assert_eq!(RESTARTS.load(SeqCst), before + 1);
}

#[test]
fn a_switch_waiting_for_recovery_still_gets_its_desktop_restarted_in_debug_mode() {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    pending(&mut manager);
    manager.desktop.relaunch = counted_restart;
    manager.desktop.undebugged = fresh_desktop;
    DESKTOP_AGE.store(3, SeqCst);
    let before = RESTARTS.load(SeqCst);
    manager.desktop_tick();
    finish(&mut manager);
    assert_eq!(RESTARTS.load(SeqCst), before + 1);
    assert!(manager.db.switch.is_some());
}

#[test]
fn a_desktop_in_use_or_with_hot_switch_off_is_left_running() {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    manager.desktop.relaunch = counted_restart;
    manager.desktop.undebugged = fresh_desktop;
    let before = RESTARTS.load(SeqCst);
    DESKTOP_AGE.store(crate::desktop::FRESH_DESKTOP_SECS + 1, SeqCst);
    manager.desktop_tick();
    assert!(manager.desktop.job.is_none());
    manager.desktop = crate::desktop::Runtime::default();
    manager.desktop.relaunch = counted_restart;
    manager.desktop.undebugged = fresh_desktop;
    manager.db.desktop.enabled = false;
    DESKTOP_AGE.store(3, SeqCst);
    manager.desktop_tick();
    assert!(manager.desktop.job.is_none());
    assert_eq!(RESTARTS.load(SeqCst), before);
}

#[test]
fn only_a_turn_started_since_the_desktop_launched_keeps_it_from_restarting() {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    manager.desktop.relaunch = counted_restart;
    manager.desktop.undebugged = fresh_desktop;
    DESKTOP_AGE.store(30, SeqCst);
    let before = RESTARTS.load(SeqCst);
    open_turn(&manager, "rollout-new.jsonl", now() - 10);
    manager.desktop_tick();
    assert!(manager.desktop.job.is_none());
    // A turn that was already running elsewhere, such as in the CLI, does not count.
    fs::remove_file(
        manager
            .sources
            .codex_home
            .join("sessions/2026/10/14/rollout-new.jsonl"),
    )
    .unwrap();
    open_turn(&manager, "rollout-cli.jsonl", now() - 600);
    manager.desktop.settled_pid = None;
    manager.desktop.last_watch = 0;
    manager.desktop_tick();
    finish(&mut manager);
    assert_eq!(RESTARTS.load(SeqCst), before + 1);
}

#[test]
fn a_failed_restart_is_not_retried() {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    manager.desktop.relaunch = |_| Err("Codex 桌面没有退出，可能在等待确认；请手动退出后再切换");
    manager.desktop.undebugged = fresh_desktop;
    DESKTOP_AGE.store(3, SeqCst);
    manager.desktop_tick();
    finish(&mut manager);
    assert!(manager.desktop.restart_failed);
    assert!(
        manager
            .desktop
            .message
            .starts_with("Codex 桌面未能以调试模式重新启动")
    );
    manager.desktop.undebugged = |_| Ok(Some((4343, 2)));
    manager.desktop.last_watch = 0;
    manager.desktop_tick();
    assert!(manager.desktop.job.is_none());
}

#[test]
fn running_turns_come_from_the_latest_turn_of_recent_rollouts() {
    let fixture = Fixture::new();
    let home = fixture.root.join("codex-home");
    assert!(!crate::desktop::turns_running(&home).unwrap());
    let day = home.join("sessions/2026/09/30");
    fs::create_dir_all(&day).unwrap();
    let event =
        |kind: &str| format!("{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\"}}}}\n");
    let write = |name: &str, kinds: &[&str]| {
        let body: String =
            std::iter::once("{\"type\":\"session_meta\",\"payload\":{}}\n".to_string())
                .chain(kinds.iter().map(|k| event(k)))
                .collect();
        fs::write(day.join(name), body).unwrap();
    };
    write(
        "rollout-finished.jsonl",
        &["task_started", "token_count", "task_complete"],
    );
    write(
        "rollout-interrupted.jsonl",
        &["task_started", "turn_aborted"],
    );
    write("notes.jsonl", &["task_started"]);
    assert!(!crate::desktop::turns_running(&home).unwrap());
    write("rollout-old.jsonl", &["task_complete", "task_started"]);
    fs::File::options()
        .write(true)
        .open(day.join("rollout-old.jsonl"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400))
        .unwrap();
    assert!(!crate::desktop::turns_running(&home).unwrap());
    write(
        "rollout-live.jsonl",
        &[
            "task_started",
            "task_complete",
            "task_started",
            "token_count",
        ],
    );
    assert!(crate::desktop::turns_running(&home).unwrap());
}
