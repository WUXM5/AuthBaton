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
digest = lambda token: hashlib.sha256(token.encode()).hexdigest()
# The desktop runs on its own login from the default file until AuthManager logs it in.
state_file = root / 'desktop-state.json'
if state_file.exists():
    state = json.loads(state_file.read_text())
else:
    token = json.loads((root / 'home/.codex/auth.json').read_text())['tokens']['access_token']
    claims = json.loads(base64.urlsafe_b64decode(token.split('.')[1] + '=='))['https://api.openai.com/auth']
    state = {'owner':'codex:' + claims['chatgpt_user_id'] + ':' + claims['chatgpt_account_id'],'mode':'chatgpt','boundOwner':None,'hash':digest(token)}
seen = {'ok':True,'target':'synthetic-target','instance':'synthetic-instance','requests':[],**state}
if request['operation'] == 'preflight' and (root / 'fail-preflight').exists():
    refusal = {'ok':False,'code':(root / 'fail-preflight').read_text()}
    if (root / 'fail-preflight-tasks.json').exists(): refusal['tasks'] = json.loads((root / 'fail-preflight-tasks.json').read_text())
    print(json.dumps(refusal));sys.exit(0)
if request['operation'] == 'poll' and (root / 'poll.json').exists():
    print((root / 'poll.json').read_text());sys.exit(0)
if request['operation'] in ('preflight', 'probe', 'poll'):
    print(json.dumps(seen));sys.exit(0)
if request['operation'] == 'bind':
    if state['mode'] != 'chatgptAuthTokens' or state['owner'] != params['owner']:
        print(json.dumps({'ok':False,'code':'identity_changed'}));sys.exit(0)
    state['boundOwner'] = state['owner'];state_file.write_text(json.dumps(state))
    print(json.dumps({**seen,'boundOwner':state['owner'],'confirmed':True}));sys.exit(0)
if request['operation'] == 'apply' and (root / 'rotate-to.json').exists():
    (root / 'home/.codex/auth.json').write_text((root / 'rotate-to.json').read_text())
    (root / 'rotate-to.json').unlink()
if request['operation'] == 'reply':
    (root / 'reply-hash').write_text(digest(auth.get('accessToken','')))
owner = params.get('owner', 'codex:a:workspace-a')
# fail-completion: the login never lands. fail-after-login: it lands, then Codex stops answering.
landed = request['operation'] in ('apply', 'update') and not (root / 'fail-completion').exists()
if landed:
    state_file.write_text(json.dumps({'owner':owner,'mode':'chatgptAuthTokens','boundOwner':owner,'hash':digest(auth.get('accessToken',''))}))
if landed and (root / 'fail-after-login').exists():
    print(json.dumps({'ok':False,'code':'rpc_timeout'}));sys.exit(0)
response = {'ok':True,'confirmed':not (root / 'fail-completion').exists(),'owner':owner,'boundOwner':owner,'mode':'chatgptAuthTokens','target':'synthetic-target','instance':'synthetic-instance','hash':digest(auth.get('accessToken','')),'requests':[]}
print(json.dumps(response))
"#;

fn default_file(manager: &Accounts, fixture: &Fixture) -> Value {
    manager
        .native("codex")
        .unwrap()
        .read(&fixture.platform)
        .unwrap()
        .credential
}
// The next poll, as soon as the tick that asks for it runs.
fn poll(manager: &mut Accounts) {
    manager.desktop.last_poll = 0;
    manager.desktop_tick();
    finish(manager);
}
// What a failed switch leaves behind blocks nothing: checks, quota and the next switch all run.
fn unblocked(manager: &Accounts) {
    let snapshot = manager.snapshot();
    assert_eq!(snapshot["recovery_needed"], false);
    assert_eq!(snapshot["checks_blocked"], false);
    assert!(manager.quota_paused().is_none());
    assert!(manager.db.switch.is_none());
    assert!(manager.ensure_idle().is_ok());
}

#[test]
fn a_login_that_landed_before_the_switch_failed_is_finished_by_the_next_poll() {
    let fixture = Fixture::new();
    let (mut manager, a, b) = setup(&fixture);
    fs::write(fixture.root.join("fail-after-login"), b"fixture").unwrap();
    manager.activate("b", true).unwrap();
    finish(&mut manager);
    let action = manager.snapshot()["desktop"]["action"].clone();
    assert_eq!(action["state"], "failed");
    assert_eq!(
        action["message"],
        "调试端口已连接，但 Codex 账号服务响应超时；请稍后重试；切换没有完成"
    );
    assert_eq!(default_file(&manager, &fixture), a.credential);
    assert!(manager.db.desktop.selected.is_none());
    unblocked(&manager);
    assert!(fixture.manager().db.switch.is_none());
    // The desktop now shows b on AuthManager's login: the poll finishes the switch without
    // logging in again.
    fs::remove_file(fixture.root.join("fail-after-login")).unwrap();
    poll(&mut manager);
    assert_eq!(sent(&fixture), ["preflight", "apply", "poll", "bind"]);
    assert_eq!(default_file(&manager, &fixture), b.credential);
    assert_eq!(manager.db.desktop.selected.as_deref(), Some("b"));
    assert!(manager.db.accounts[1].native && !manager.db.accounts[0].native);
    assert_eq!(manager.snapshot()["desktop"]["action"]["state"], "complete");
    assert_eq!(manager.snapshot()["desktop"]["managed"], true);
    let ops: Vec<_> = desktop_log(&fixture)
        .iter()
        .map(|e| e["op"].as_str().unwrap().to_string())
        .collect();
    assert!(
        ops.ends_with(&["apply_failed".into(), "finish".into(), "apply_done".into()]),
        "{ops:?}"
    );
    // And back again, normally.
    manager.activate("a", true).unwrap();
    finish(&mut manager);
    assert_eq!(default_file(&manager, &fixture), a.credential);
    assert_eq!(manager.db.desktop.selected.as_deref(), Some("a"));
    assert!(!manager.snapshot().to_string().contains("SYNTHETIC-"));
}

#[test]
fn a_switch_whose_login_never_landed_changes_nothing_and_can_simply_be_asked_again() {
    let fixture = Fixture::new();
    let (mut manager, a, b) = setup(&fixture);
    fs::write(fixture.root.join("fail-completion"), b"fixture").unwrap();
    manager.activate("b", true).unwrap();
    finish(&mut manager);
    assert_eq!(manager.snapshot()["desktop"]["action"]["state"], "failed");
    unblocked(&manager);
    // The desktop is still on its own login: the poll leaves it and the default file alone.
    poll(&mut manager);
    assert_eq!(sent(&fixture), ["preflight", "apply", "poll"]);
    assert_eq!(default_file(&manager, &fixture), a.credential);
    assert!(manager.db.accounts[0].native);
    fs::remove_file(fixture.root.join("fail-completion")).unwrap();
    manager.activate("b", true).unwrap();
    finish(&mut manager);
    assert_eq!(default_file(&manager, &fixture), b.credential);
    assert_eq!(manager.db.desktop.selected.as_deref(), Some("b"));
}

#[test]
fn switching_to_the_account_the_desktop_is_already_on_only_syncs_the_default_file() {
    let fixture = Fixture::new();
    let (mut manager, _a, b) = setup(&fixture);
    // A crash or an older version left the desktop on b's login with nothing recorded, and a
    // task runs: no preflight, no login, so the task does not matter.
    fs::write(fixture.root.join("fail-preflight"), "tasks_active").unwrap();
    fs::write(
        fixture.root.join("desktop-state.json"),
        json!({"owner":"codex:b:workspace-b","mode":"chatgptAuthTokens","boundOwner":null,"hash":"held"}).to_string(),
    )
    .unwrap();
    manager.desktop.finish_after = now() + 60;
    poll(&mut manager);
    manager.activate("b", true).unwrap();
    finish(&mut manager);
    assert_eq!(sent(&fixture), ["poll", "bind"]);
    assert_eq!(default_file(&manager, &fixture), b.credential);
    assert_eq!(manager.db.desktop.selected.as_deref(), Some("b"));
    assert_eq!(manager.snapshot()["desktop"]["action"]["state"], "complete");
}

#[test]
fn the_poll_finishes_only_logins_that_auth_manager_made() {
    // The desktop's own login of another account, an unknown account, or a check that may write
    // the default file: nothing is finished yet.
    for (state, busy) in [
        (
            json!({"owner":"codex:b:workspace-b","mode":"chatgpt","boundOwner":null,"hash":"h"}),
            false,
        ),
        (
            json!({"owner":"codex:z:workspace-z","mode":"chatgptAuthTokens","boundOwner":null,"hash":"h"}),
            false,
        ),
        (
            json!({"owner":"codex:b:workspace-b","mode":"chatgptAuthTokens","boundOwner":null,"hash":"h"}),
            true,
        ),
    ] {
        let fixture = Fixture::new();
        let (mut manager, a, _) = setup(&fixture);
        fs::write(fixture.root.join("desktop-state.json"), state.to_string()).unwrap();
        if busy {
            manager.job = Some(Job {
                id: "a".into(),
                cancel: Arc::new(AtomicBool::new(false)),
                handle: std::thread::spawn(|| JobResult::ReadError("synthetic check")),
            });
        }
        poll(&mut manager);
        assert_eq!(sent(&fixture), ["poll"]);
        assert_eq!(default_file(&manager, &fixture), a.credential);
        assert!(manager.db.desktop.selected.is_none());
        if let Some(job) = manager.job.take() {
            let _ = job.handle.join();
        }
    }
}

#[test]
fn a_finish_that_keeps_failing_is_retried_every_30_seconds_not_every_poll() {
    let fixture = Fixture::new();
    let (mut manager, a, _) = setup(&fixture);
    fs::write(
        fixture.root.join("desktop-state.json"),
        json!({"owner":"codex:b:workspace-b","mode":"chatgptAuthTokens","boundOwner":null,"hash":"h"}).to_string(),
    )
    .unwrap();
    manager.desktop.other_clients = || true;
    poll(&mut manager);
    poll(&mut manager);
    assert_eq!(sent(&fixture), ["poll", "poll"]);
    assert!(manager.desktop.message.contains("默认文件还没同步"));
    let failures = desktop_log(&fixture)
        .iter()
        .filter(|e| e["op"] == "finish_failed")
        .count();
    assert_eq!(failures, 1);
    assert_eq!(default_file(&manager, &fixture), a.credential);
    manager.desktop.other_clients = || false;
    manager.desktop.finish_after = 0;
    poll(&mut manager);
    assert_eq!(manager.db.desktop.selected.as_deref(), Some("b"));
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
fn a_desktop_journal_left_by_an_older_version_is_dropped_on_load() {
    let fixture = Fixture::new();
    let (mut manager, a, _) = setup(&fixture);
    manager.db.switch = Some(Switch {
        provider: "codex".into(),
        target: "b".into(),
        previous: Some("a".into()),
        desktop: true,
    });
    manager.save().unwrap();
    let reopened = fixture.manager();
    assert!(reopened.db.switch.is_none());
    assert!(reopened.notice.is_none());
    assert!(reopened.ensure_idle().is_ok());
    assert!(reopened.db.accounts[0].native);
    assert_eq!(default_file(&reopened, &fixture), a.credential);
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

#[test]
fn a_switch_asked_for_during_a_poll_or_a_check_waits_for_it_and_then_starts() {
    for poll_running in [true, false] {
        let fixture = Fixture::new();
        let (mut manager, _, b) = setup(&fixture);
        manager.desktop.connected = true;
        let slow = std::thread::spawn(|| std::thread::sleep(std::time::Duration::from_millis(300)));
        if poll_running {
            manager.desktop.job = Some(std::thread::spawn(move || {
                let _ = slow.join();
                crate::desktop::Completion::Reply(Ok(json!({"ok":true})))
            }));
        } else {
            manager.job = Some(Job {
                id: "a".into(),
                cancel: Arc::new(AtomicBool::new(false)),
                handle: std::thread::spawn(move || {
                    let _ = slow.join();
                    JobResult::ReadError("synthetic check")
                }),
            });
        }
        manager.activate("b", true).unwrap();
        assert_eq!(manager.snapshot()["desktop"]["action"]["state"], "checking");
        assert_eq!(
            manager.activate("b", true).unwrap_err(),
            "正在切换账号，请等这次切换结束"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while manager.snapshot()["desktop"]["action"]["state"] != "complete" {
            assert!(
                std::time::Instant::now() < deadline,
                "{}",
                manager.snapshot()["desktop"]
            );
            manager.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(default_file(&manager, &fixture), b.credential);
        assert_eq!(sent(&fixture), ["preflight", "apply"]);
    }
}

#[test]
fn a_waiting_switch_gives_up_after_its_deadline_and_says_so() {
    let fixture = Fixture::new();
    let (mut manager, a, _) = setup(&fixture);
    manager.desktop.connected = true;
    manager.job = Some(Job {
        id: "a".into(),
        cancel: Arc::new(AtomicBool::new(false)),
        handle: std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(1));
            JobResult::ReadError("synthetic check")
        }),
    });
    manager.activate("b", true).unwrap();
    manager.desktop.pending_switch.as_mut().unwrap().1 = now();
    manager.desktop_tick();
    let action = manager.snapshot()["desktop"]["action"].clone();
    assert_eq!(action["state"], "failed");
    assert!(
        action["message"]
            .as_str()
            .unwrap()
            .ends_with("切换未开始，可以再试一次")
    );
    assert!(manager.desktop.pending_switch.is_none());
    assert_eq!(default_file(&manager, &fixture), a.credential);
    let _ = manager.job.take().unwrap().handle.join();
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

// Seconds since the desktop finished starting; u64::MAX while it is still starting.
static STARTED_AGO: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
fn started_ago(_: &std::path::Path, _: u32, _: u64) -> Option<u64> {
    let ago = STARTED_AGO.load(std::sync::atomic::Ordering::SeqCst);
    (ago != u64::MAX).then(|| now() - ago)
}

#[test]
fn a_desktop_is_restarted_only_once_its_startup_has_finished() {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    manager.desktop.relaunch = counted_restart;
    manager.desktop.undebugged = fresh_desktop;
    manager.desktop.startup_finished = started_ago;
    DESKTOP_AGE.store(1, SeqCst);
    STARTED_AGO.store(u64::MAX, SeqCst);
    let before = RESTARTS.load(SeqCst);
    manager.desktop_tick();
    assert!(manager.desktop.job.is_none());
    assert_eq!(manager.desktop.settled_pid, None);
    STARTED_AGO.store(0, SeqCst);
    manager.desktop.last_watch = 0;
    manager.desktop_tick();
    finish(&mut manager);
    assert_eq!(RESTARTS.load(SeqCst), before + 1);
}

#[test]
fn a_slow_startup_counts_as_just_started_from_when_it_finished() {
    use crate::desktop::{FRESH_DESKTOP_SECS, STARTUP_WAIT_SECS};
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    manager.desktop.relaunch = counted_restart;
    manager.desktop.undebugged = fresh_desktop;
    manager.desktop.startup_finished = started_ago;
    let before = RESTARTS.load(SeqCst);
    // Finished starting a while ago: the person may be using it.
    DESKTOP_AGE.store(200, SeqCst);
    STARTED_AGO.store(FRESH_DESKTOP_SECS + 1, SeqCst);
    manager.desktop_tick();
    assert!(manager.desktop.job.is_none());
    // Still starting long after launch: left alone for good.
    manager.desktop.settled_pid = None;
    manager.desktop.last_watch = 0;
    DESKTOP_AGE.store(STARTUP_WAIT_SECS + 1, SeqCst);
    STARTED_AGO.store(u64::MAX, SeqCst);
    manager.desktop_tick();
    assert_eq!(manager.desktop.settled_pid, Some(4242));
    // A slow startup that has just finished.
    manager.desktop.settled_pid = None;
    manager.desktop.last_watch = 0;
    DESKTOP_AGE.store(200, SeqCst);
    STARTED_AGO.store(5, SeqCst);
    manager.desktop_tick();
    finish(&mut manager);
    assert_eq!(RESTARTS.load(SeqCst), before + 1);
}

fn desktop_log(fixture: &Fixture) -> Vec<Value> {
    fs::read_to_string(fixture.root.join("desktop-log.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn sent(fixture: &Fixture) -> Vec<String> {
    fs::read_to_string(fixture.root.join("desktop-operations"))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

// The desktop's next poll: bound to `id`, still holding `held`, asking for a token after a 401.
fn refresh_request(fixture: &Fixture, id: &str, held: &Bundle) {
    use sha2::{Digest, Sha256};
    let token = held.credential["tokens"]["access_token"].as_str().unwrap();
    let owner = format!("codex:{id}:workspace-{id}");
    fs::write(
        fixture.root.join("poll.json"),
        json!({"ok":true,"owner":owner,"boundOwner":owner,"mode":"chatgptAuthTokens","target":"synthetic-target","instance":"synthetic-instance","hash":format!("{:x}",Sha256::digest(token.as_bytes())),"requests":[{"id":7,"epoch":1,"ageMs":100}]}).to_string(),
    )
    .unwrap();
}
fn renewed(manager: &mut Accounts, fixture: &Fixture, id: &str) -> Bundle {
    // A renewal of the current account writes the default file and the Vault.
    let next = bundle(id, 2, now() + 7200);
    fixture.platform.vault_write(id, &next.encoded()).unwrap();
    write_json(
        &manager.sources.codex_home.join("auth.json"),
        &next.credential,
    )
    .unwrap();
    let outcome = crate::maintenance::Outcome {
        bundle: next.clone(),
        online: true,
        refreshed: true,
        state: "healthy".into(),
        message: "synthetic renewal".into(),
        quota: vec![],
        pending: None,
    };
    manager.job = Some(Job {
        id: id.into(),
        cancel: Arc::new(AtomicBool::new(false)),
        handle: std::thread::spawn(move || JobResult::Check(Box::new(outcome))),
    });
    manager.finish_job();
    next
}

#[test]
fn a_switch_is_logged_without_credentials_and_reports_takeover() {
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    assert_eq!(manager.snapshot()["desktop"]["managed"], false);
    manager.activate("b", true).unwrap();
    finish(&mut manager);
    assert_eq!(manager.db.desktop.selected.as_deref(), Some("b"));
    assert_eq!(manager.snapshot()["desktop"]["managed"], true);
    // Connected, but logged in by the desktop itself: not taken over.
    manager.desktop.mode = Some("chatgpt".into());
    assert_eq!(manager.snapshot()["desktop"]["managed"], false);
    let ops: Vec<_> = desktop_log(&fixture)
        .iter()
        .map(|e| e["op"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        ops,
        ["preflight", "quit_confirmation", "apply", "apply_done"]
    );
    let text = fs::read_to_string(fixture.root.join("desktop-log.jsonl")).unwrap();
    for secret in ["SYNTHETIC-", "x.ey", "example.invalid", "workspace-"] {
        assert!(!text.contains(secret), "{secret}");
    }
}

#[test]
fn a_renewal_is_never_pushed_and_the_desktops_next_request_gets_it_at_once() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let (mut manager, _, b) = setup(&fixture);
    manager.activate("b", true).unwrap();
    finish(&mut manager);
    let next = renewed(&mut manager, &fixture, "b");
    // No push: the desktop keeps working with what it holds.
    manager.desktop_tick();
    finish(&mut manager);
    assert_eq!(sent(&fixture), ["preflight", "apply"]);
    // After a 401 it asks; it gets the renewed token at once, without going online, even while
    // another account's check holds the job slot.
    manager.job = Some(Job {
        id: "a".into(),
        cancel: Arc::new(AtomicBool::new(false)),
        handle: std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(2));
            JobResult::ReadError("synthetic check still running")
        }),
    });
    refresh_request(&fixture, "b", &b);
    manager.desktop.last_poll = 0;
    manager.desktop_tick();
    finish(&mut manager);
    assert_eq!(sent(&fixture), ["preflight", "apply", "poll", "reply"]);
    let token = next.credential["tokens"]["access_token"].as_str().unwrap();
    assert_eq!(
        fs::read_to_string(fixture.root.join("reply-hash")).unwrap(),
        format!("{:x}", Sha256::digest(token.as_bytes()))
    );
    let log = desktop_log(&fixture);
    let ops: Vec<_> = log.iter().map(|e| e["op"].as_str().unwrap()).collect();
    assert!(
        ops.ends_with(&["renewed", "refresh_requested", "refresh_reply"]),
        "{ops:?}"
    );
    assert_eq!(log.last().unwrap()["reason"], "回复新令牌");
    assert!(!ops.contains(&"refresh_start"));
}

static SESSION_SWITCH: std::sync::Mutex<Vec<bool>> = std::sync::Mutex::new(Vec::new());
fn recorded_switch(skip: bool) -> Result<()> {
    SESSION_SWITCH.lock().unwrap().push(skip);
    Ok(())
}

#[test]
fn quit_confirmation_is_skipped_only_while_hot_switch_is_on() {
    let told = || SESSION_SWITCH.lock().unwrap().clone();
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    SESSION_SWITCH.lock().unwrap().clear();
    manager.desktop.skip_quit_confirmation = recorded_switch;
    manager.desktop_tick();
    manager.desktop_tick();
    assert_eq!(told(), [true]);
    // Turning hot switch off takes the switch back on the next tick.
    manager.configure_desktop(Some(false), None).unwrap();
    manager.desktop.skip_quit_confirmation = recorded_switch;
    manager.desktop_tick();
    assert_eq!(told(), [true, false]);
    manager.stop();
    assert_eq!(told(), [true, false]);
    // Stopping while hot switch is on takes it back too.
    let fixture = Fixture::new();
    let (mut manager, _, _) = setup(&fixture);
    SESSION_SWITCH.lock().unwrap().clear();
    manager.desktop.skip_quit_confirmation = recorded_switch;
    manager.desktop_tick();
    manager.stop();
    assert_eq!(told(), [true, false]);
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
