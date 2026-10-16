use super::*;
use crate::automation::{Config, LatchKind, Order, Phase, Rule};
use crate::codex_api::{Access, Request, Response, Transport};

fn response(request: &Request, used: u64) -> Response {
    Response {
        status: 200,
        retry_after: None,
        body: match request {
            Request::Usage => {
                json!({"rate_limit":{"primary_window":{"used_percent":used,"reset_at":now()+3600,"limit_window_seconds":18000}},"code_review_rate_limit":{"primary_window":{"used_percent":100,"reset_at":now()+3600}}})
            }
            Request::ResetCredits => {
                json!({"available_count":2,"credits":[{"id":"credit","reset_type":"codex_rate_limits","status":"available","expires_at":"2099-01-01T00:00:00Z"}]})
            }
            Request::Consume { .. } => panic!("quota polling must never POST"),
        },
    }
}
fn low(_: &Access, r: &Request) -> Result<Response> {
    Ok(response(r, 99))
}
fn high(_: &Access, r: &Request) -> Result<Response> {
    Ok(response(r, 10))
}
fn quota(manager: &mut Accounts, id: &str, transport: Transport, time: u64) {
    manager.quota.transport = Some(transport);
    manager
        .quota
        .start(id, Ok(Access::synthetic(now() + 3600)), None, time);
    manager.quota.stop(time);
}
fn setup(f: &Fixture) -> Accounts {
    let (mut m, _, _) = super::desktop_boundary_tests::setup(f);
    let helper = super::desktop_boundary_tests::FAKE_DESKTOP.replace(
        "if request['operation'] == 'preflight'",
        &format!("{AUTO_HELPER}\nif request['operation'] == 'preflight'"),
    );
    fs::write(m.desktop.helper.as_ref().unwrap(), helper).unwrap();
    m.desktop.observed(&json!({"target":"synthetic-target","instance":"synthetic-instance","owner":"codex:a:workspace-a","hash":"synthetic","mode":"chatgptAuthTokens","boundOwner":"codex:a:workspace-a"})).unwrap();
    quota(&mut m, "a", low, now());
    quota(&mut m, "b", high, now());
    m.configure_strategy(
        Config {
            enabled: true,
            accounts: vec![
                Rule {
                    id: "a".into(),
                    order: Order::SwitchOnly,
                    participate: true,
                    below: 2,
                },
                Rule {
                    id: "b".into(),
                    order: Order::SwitchOnly,
                    participate: true,
                    below: 2,
                },
            ],
        },
        true,
    )
    .unwrap();
    m
}
fn settle(m: &mut Accounts) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "automation did not settle"
        );
        if m.desktop.job.as_ref().is_some_and(|j| j.is_finished()) {
            m.desktop_tick();
        }
        m.automation_tick();
        if m.automation.job.is_none()
            && m.desktop.job.is_none()
            && m.db
                .automation
                .run
                .as_ref()
                .is_none_or(|r| r.phase == Phase::Attention)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
const AUTO_HELPER: &str = r#"
if request['operation'].startswith('auto_'):
    op = request['operation']
    db = json.loads((root / 'accounts.sqlite3').read_text())
    run = db['automation']['run']
    statefile = root / 'auto-tasks.json'
    tasks = json.loads(statefile.read_text()) if statefile.exists() else []
    owner = 'codex:a:workspace-a'
    if op == 'auto_prepare':
        assert run['id'] == params['batchId'] and run['phase'] == 'preparing'
        tasks = [{'thread_id':i, 'turn_id':'turn-'+i, 'resume_key':'resume-'+i, 'state':'planned', 'resumed_turn_id':None} for i in ['one','two']]
    elif op == 'auto_pause':
        assert run['phase'] == 'pausing'
        current = next(t for t in tasks if t['thread_id']==params['threadId'])
        assert next(t for t in run['tasks'] if t['thread_id']==params['threadId'])['state']=='interrupt_sent'
        current['state']='paused'
    elif op == 'auto_resume':
        assert run['phase'] == 'resuming' and db['switch'] is None
        assert json.loads((root / 'home/.codex/auth.json').read_text())['tokens']['account_id']=='workspace-b'
        current = next(t for t in tasks if t['thread_id']==params['threadId'])
        assert next(t for t in run['tasks'] if t['thread_id']==params['threadId'])['state']=='resume_sent'
        current['state']='resumed';current['resumed_turn_id']='new-'+params['threadId'];owner='codex:b:workspace-b'
    elif op == 'auto_cancel':
        if (root/'fail-cancel').exists():
            print(json.dumps({'ok':False,'code':'target_unavailable'}));sys.exit(0)
        print(json.dumps({'ok':True,'instance':'synthetic-instance','cancelled':True}));sys.exit(0)
    statefile.write_text(json.dumps(tasks))
    if (root/('fail-'+op)).exists():
        code = (root/('fail-'+op)).read_text()
        if code != 'tasks_waiting': code = 'resume_unconfirmed' if op=='auto_resume' else 'task_probe_failed'
        print(json.dumps({'ok':False,'code':code}));sys.exit(0)
    print(json.dumps({'ok':True,'tasks':tasks,'owner':owner,'target':'synthetic-target','instance':'synthetic-instance','hash':'synthetic','mode':'chatgptAuthTokens','boundOwner':owner}));sys.exit(0)
"#;

#[test]
fn two_sessions_pause_before_auth_and_resume_only_after_native_commit() {
    let f = Fixture::new();
    let mut m = setup(&f);
    settle(&mut m);
    assert!(m.db.automation.run.is_none());
    assert!(m.db.accounts[1].native);
    let operations = fs::read_to_string(f.root.join("desktop-operations")).unwrap();
    assert_eq!(
        operations.lines().collect::<Vec<_>>(),
        vec![
            "auto_prepare",
            "auto_pause",
            "auto_pause",
            "auto_verify",
            "apply",
            "auto_resume",
            "auto_resume",
            "auto_cancel"
        ]
    );
    assert!(
        m.snapshot()["strategy"]["message"]
            .as_str()
            .unwrap()
            .contains("2 个")
    );
    let snapshot = m.snapshot().to_string();
    assert!(!snapshot.contains("codex:a:workspace-a"));
    assert!(!snapshot.contains("SYNTHETIC-"));
}
#[test]
fn lost_prepare_reply_keeps_batch_id_and_can_be_cancelled() {
    let f = Fixture::new();
    let mut m = setup(&f);
    fs::write(f.root.join("fail-auto_prepare"), b"1").unwrap();
    settle(&mut m);
    assert_eq!(
        m.db.automation.run.as_ref().unwrap().phase,
        Phase::Attention
    );
    assert!(m.db.accounts[0].native);
    m.stop_strategy(true).unwrap();
    settle(&mut m);
    assert!(m.db.automation.run.is_none());
    assert_eq!(
        fs::read_to_string(f.root.join("desktop-operations"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        vec!["auto_prepare", "auto_cancel"]
    );
}
#[test]
fn tasks_awaiting_input_delay_the_switch_without_stopping_anything_else() {
    let f = Fixture::new();
    let mut m = setup(&f);
    fs::write(f.root.join("fail-auto_prepare"), b"tasks_waiting").unwrap();
    settle(&mut m);
    // The bridge refused before it made a batch: nothing was paused, so nothing waits on the user.
    assert!(m.db.automation.run.is_none());
    assert!(m.db.automation.config.enabled);
    assert!(m.automation.message.contains("延后"));
    assert!(m.quota_paused().is_none());
    assert!(m.request_quota(Some("b")).is_ok());
    // Once the tasks are answered, the strategy switches on its own.
    fs::remove_file(f.root.join("fail-auto_prepare")).unwrap();
    m.automation.next_check = 0;
    settle(&mut m);
    assert!(m.db.automation.run.is_none());
    assert!(m.db.accounts[1].native);
}
#[test]
fn lost_resume_reply_restart_and_missing_renderer_never_replay() {
    let f = Fixture::new();
    let mut m = setup(&f);
    fs::write(f.root.join("fail-auto_resume"), b"1").unwrap();
    settle(&mut m);
    assert_eq!(
        m.db.automation.run.as_ref().unwrap().phase,
        Phase::Attention
    );
    assert_eq!(
        m.db.automation.run.as_ref().unwrap().tasks[0].state,
        "resume_sent"
    );
    let mut reopened = f.manager();
    reopened.desktop.helper = m.desktop.helper.clone();
    reopened.desktop.other_clients = || false;
    for _ in 0..3 {
        reopened.automation_tick();
    }
    let count = fs::read_to_string(f.root.join("desktop-operations"))
        .unwrap()
        .lines()
        .filter(|s| *s == "auto_resume")
        .count();
    assert_eq!(count, 1);
    fs::write(f.root.join("fail-cancel"), b"1").unwrap();
    reopened.stop_strategy(true).unwrap();
    settle(&mut reopened);
    for _ in 0..3 {
        reopened.automation_tick();
    }
    assert_eq!(
        fs::read_to_string(f.root.join("desktop-operations"))
            .unwrap()
            .lines()
            .filter(|s| *s == "auto_cancel")
            .count(),
        1
    );
    assert!(
        !reopened
            .db
            .automation
            .run
            .as_ref()
            .unwrap()
            .cancel_requested
    );
    assert!(reopened.forget_strategy(false).is_err());
    reopened.forget_strategy(true).unwrap();
    assert!(reopened.ensure_idle().is_ok());
    assert!(!reopened.db.automation.config.enabled);
    assert!(!reopened.db.accounts[0].native);
    assert_eq!(reopened.db.automation.latches["a"].kind, LatchKind::Switch);
}
#[test]
fn auth_partial_failure_never_resumes_and_leaves_no_auth_journal() {
    let f = Fixture::new();
    let mut m = setup(&f);
    fs::write(f.root.join("fail-completion"), b"1").unwrap();
    settle(&mut m);
    assert!(m.db.switch.is_none());
    assert_eq!(
        m.db.automation.run.as_ref().unwrap().phase,
        Phase::Attention
    );
    assert!(
        !fs::read_to_string(f.root.join("desktop-operations"))
            .unwrap()
            .contains("auto_resume")
    );
    m.forget_strategy(true).unwrap();
    assert!(m.db.switch.is_none());
    // The switch latch stands exactly when the source stopped being current.
    assert_eq!(
        m.db.automation.latches.contains_key("a"),
        !m.db.accounts[0].native
    );
}
#[test]
fn priority_requires_fresh_core_quota_and_ignores_depleted_review_window() {
    let f = Fixture::new();
    let mut m = setup(&f);
    assert_eq!(
        m.quota
            .automation_observation("b", now())
            .unwrap()
            .remaining,
        90.
    );
    quota(&mut m, "b", high, now() - 301);
    m.automation_tick();
    assert!(m.automation.job.is_none());
    assert!(m.db.automation.run.is_none());
    assert!(!f.root.join("desktop-operations").exists());
    quota(&mut m, "b", high, now());
    m.db.automation.config.accounts[1].participate = false;
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(m.automation.job.is_none());
    m.db.automation.config.accounts[1].participate = true;
    m.db.accounts[1].state = "reauth".into();
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(m.automation.job.is_none());
}
#[test]
fn pending_intent_save_failure_prevents_any_desktop_action() {
    let f = Fixture::new();
    let mut m = setup(&f);
    fs::write(f.root.join("fail-db"), b"1").unwrap();
    m.automation_tick();
    assert!(m.automation.job.is_none());
    assert!(m.storage_fault.get());
    assert!(!f.root.join("desktop-operations").exists());
}
#[test]
fn automatic_reset_latch_survives_low_cache_and_restart_without_second_credit() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static POSTS: AtomicUsize = AtomicUsize::new(0);
    fn transport(_: &Access, r: &Request) -> Result<Response> {
        if let Request::Consume { .. } = r {
            POSTS.fetch_add(1, Ordering::SeqCst);
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: json!({"code":"reset"}),
            });
        }
        Ok(response(r, 99))
    }
    POSTS.store(0, Ordering::SeqCst);
    let f = Fixture::new();
    let mut m = setup(&f);
    m.db.automation.config.accounts[0].order = Order::ResetFirst;
    m.db.automation.config.accounts[1].participate = false;
    m.reset_transport = Some(transport);
    m.automation_tick();
    m.finish_reset();
    assert_eq!(POSTS.load(Ordering::SeqCst), 1);
    for _ in 0..3 {
        quota(&mut m, "a", low, now());
        m.automation.next_check = 0;
        m.automation_tick();
        assert!(m.reset_job.is_none());
    }
    let mut reopened = f.manager();
    reopened.desktop.helper = m.desktop.helper.clone();
    reopened.desktop.other_clients = || false;
    reopened.desktop.observed(&json!({"target":"synthetic-target","instance":"synthetic-instance","owner":"codex:a:workspace-a","hash":"synthetic","mode":"chatgptAuthTokens","boundOwner":"codex:a:workspace-a"})).unwrap();
    reopened.reset_transport = Some(transport);
    quota(&mut reopened, "a", low, now());
    reopened.automation_tick();
    assert!(reopened.reset_job.is_none());
    assert_eq!(POSTS.load(Ordering::SeqCst), 1);
    quota(&mut reopened, "a", high, now());
    reopened.automation.next_check = 0;
    reopened.automation_tick();
    assert!(!reopened.db.automation.latches.contains_key("a"));
}
#[test]
fn strategy_confirmation_and_validation_fail_before_any_side_effect() {
    let f = Fixture::new();
    let mut m = setup(&f);
    let mut config = m.db.automation.config.clone();
    assert!(m.configure_strategy(config.clone(), false).is_err());
    config.accounts.push(config.accounts[0].clone());
    assert!(m.configure_strategy(config, true).is_err());
    assert!(!f.root.join("desktop-operations").exists());
    assert_eq!(crate::resets::parse_expiry("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        crate::resets::parse_expiry("1970-01-01T08:00:00.000+08:00"),
        Some(0)
    );
    assert!(crate::resets::parse_expiry("2026-02-29T00:00:00Z").is_none());
    assert!(crate::resets::parse_expiry("2026-01-01T-1:00:00Z").is_none());
}

#[test]
fn account_group_uses_source_order_and_selects_candidates_across_groups() {
    use crate::automation::Action;
    let f = Fixture::new();
    let mut m = setup(&f);
    m.db.automation.config.accounts[0].order = Order::ResetFirst;
    m.db.automation.config.accounts[1].order = Order::SwitchFirst;
    let usage = m.quota.automation_observation("a", now()).unwrap();
    assert_eq!(
        m.strategy_action(&m.db.accounts[0], &usage),
        Action::Reset("credit".into())
    );
    // Source policy wins, even when the target is in the other group.
    quota(&mut m, "a", high, now());
    quota(&mut m, "b", low, now());
    m.db.accounts[0].native = false;
    m.db.accounts[1].native = true;
    m.desktop.owner = Some("codex:b:workspace-b".into());
    let usage = m.quota.automation_observation("b", now()).unwrap();
    assert_eq!(
        m.strategy_action(&m.db.accounts[1], &usage),
        Action::Switch("a".into())
    );
    m.db.automation.config.accounts[0].participate = false;
    assert_eq!(
        m.strategy_action(&m.db.accounts[1], &usage),
        Action::Reset("credit".into())
    );
    assert!(!f.root.join("desktop-operations").exists());
    assert!(m.reset_job.is_none());
}

#[test]
fn account_group_changes_do_not_reorder_global_candidate_priority() {
    use crate::automation::Action;
    let f = Fixture::new();
    let mut m = setup(&f);
    let mut third = m.db.accounts[1].clone();
    third.id = "c".into();
    third.native = false;
    m.db.accounts.push(third);
    let mut rule = m.db.automation.config.accounts[1].clone();
    rule.id = "c".into();
    rule.order = Order::ResetFirst;
    m.db.automation.config.accounts.insert(1, rule);
    quota(&mut m, "c", high, now());
    let usage = m.quota.automation_observation("a", now()).unwrap();
    assert_eq!(
        m.strategy_action(&m.db.accounts[0], &usage),
        Action::Switch("c".into())
    );
    m.db.automation.config.accounts[1].order = Order::SwitchFirst;
    assert_eq!(
        m.strategy_action(&m.db.accounts[0], &usage),
        Action::Switch("c".into())
    );
    m.db.automation.config.accounts.swap(1, 2);
    assert_eq!(
        m.strategy_action(&m.db.accounts[0], &usage),
        Action::Switch("b".into())
    );
}

#[test]
fn duplicate_account_in_different_groups_is_rejected_without_saving() {
    let f = Fixture::new();
    let mut m = setup(&f);
    let before = fs::read(f.root.join("accounts.sqlite3")).unwrap();
    let mut config = m.db.automation.config.clone();
    let mut duplicate = config.accounts[0].clone();
    duplicate.order = Order::ResetFirst;
    config.accounts.push(duplicate);
    assert!(m.configure_strategy(config, true).is_err());
    assert_eq!(fs::read(f.root.join("accounts.sqlite3")).unwrap(), before);
    assert!(!f.root.join("desktop-operations").exists());
    let mut wire = serde_json::to_value(&m.db.automation.config).unwrap();
    wire["accounts"][0]["order"] = json!("both");
    assert!(serde_json::from_value::<Config>(wire.clone()).is_err());
    wire["accounts"][0].as_object_mut().unwrap().remove("order");
    assert!(serde_json::from_value::<Config>(wire).is_err());
}

/// Rewrites the saved current-format database into the pre-v8 two-threshold shape.
fn legacy_db(m: &Accounts, version: u64, switch_below: u8, resets: &[Option<u8>]) -> Value {
    let mut old = serde_json::to_value(&m.db).unwrap();
    old["version"] = json!(version);
    let latches = old["automation"]["latches"].take();
    let automation = old["automation"].as_object_mut().unwrap();
    automation.remove("latches");
    automation.insert("reset_latches".into(), latches);
    let config = &mut old["automation"]["config"];
    config["switch_below"] = json!(switch_below);
    for (rule, reset) in config["accounts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(resets)
    {
        let rule = rule.as_object_mut().unwrap();
        rule.remove("below");
        rule.insert("reset_below".into(), json!(reset));
    }
    old
}

#[test]
fn v4_strategy_migration_preserves_order_thresholds_and_reset_recovery() {
    for (legacy, migrated_order) in [
        ("reset_first", Order::ResetFirst),
        ("switch_first", Order::SwitchFirst),
    ] {
        let f = Fixture::new();
        let mut m = setup(&f);
        m.db.automation.config.accounts[1].participate = false;
        m.db.automation.latches.insert(
            "a".into(),
            crate::automation::Latch {
                request_id: "unchanged-reset-request".into(),
                created_at: now(),
                threshold: 2,
                kind: LatchKind::Reset,
            },
        );
        m.db.reset_attempts.push(crate::resets::Attempt {
            executions: vec![],
            account_id: "a".into(),
            owner: "codex:a:workspace-a".into(),
            request_id: "unchanged-reset-request".into(),
            credit_id: "credit".into(),
            expires_at: None,
            created_at: now(),
            outcome: crate::resets::Outcome::Reset,
        });
        let mut old = legacy_db(&m, 4, 2, &[Some(2), None]);
        old["automation"]["config"]["order"] = json!(legacy);
        for rule in old["automation"]["config"]["accounts"]
            .as_array_mut()
            .unwrap()
        {
            rule.as_object_mut().unwrap().remove("order");
        }
        f.platform
            .save_db(&f.root.join("accounts.sqlite3"), &old)
            .unwrap();
        let migrated = f.manager();
        assert_eq!(migrated.db.version, 8);
        assert!(migrated.db.automation.config.enabled);
        let rules = &migrated.db.automation.config.accounts;
        assert_eq!((rules[0].order, rules[0].below), (migrated_order, 2));
        // No automatic reset before the upgrade means none after it.
        assert_eq!((rules[1].order, rules[1].below), (Order::SwitchOnly, 2));
        assert!(!rules[1].participate);
        migrated.save().unwrap();
        let saved = read_json(&f.root.join("accounts.sqlite3"))
            .unwrap()
            .unwrap();
        assert_eq!(saved["accounts"], old["accounts"]);
        let latch = &saved["automation"]["latches"]["a"];
        assert_eq!(latch["request_id"], "unchanged-reset-request");
        assert_eq!(latch["kind"], "reset");
        assert!(saved["automation"].get("reset_latches").is_none());
        let config = &saved["automation"]["config"];
        assert!(config.get("order").is_none() && config.get("switch_below").is_none());
        let reopened = f.manager();
        assert_eq!(
            reopened.db.automation.config.accounts[0].order,
            migrated_order
        );
        assert!(!f.root.join("desktop-operations").exists());
        assert!(
            reopened.job.is_none()
                && reopened.reset_job.is_none()
                && reopened.automation.job.is_none()
        );
    }
}

#[test]
fn v7_migration_keeps_each_accounts_earliest_trigger_as_its_single_line() {
    let f = Fixture::new();
    let mut m = setup(&f);
    let mut rules = vec![];
    for (id, order) in [
        ("a", Order::ResetFirst),
        ("b", Order::SwitchFirst),
        ("c", Order::ResetFirst),
        ("d", Order::SwitchFirst),
    ] {
        rules.push(Rule {
            id: id.into(),
            order,
            participate: true,
            below: 0,
        });
        if !m.db.accounts.iter().any(|a| a.id == id) {
            let mut extra = m.db.accounts[1].clone();
            extra.id = id.into();
            extra.native = false;
            m.db.accounts.push(extra);
        }
    }
    m.db.automation.config.accounts = rules;
    // The reported case: reset ≤2% first under a 3% switch line used to switch at 3%.
    let old = legacy_db(&m, 7, 3, &[Some(2), Some(1), None, Some(5)]);
    f.platform
        .save_db(&f.root.join("accounts.sqlite3"), &old)
        .unwrap();
    let migrated = f.manager();
    let got: Vec<_> = migrated
        .db
        .automation
        .config
        .accounts
        .iter()
        .map(|r| (r.id.as_str(), r.order, r.below))
        .collect();
    assert_eq!(
        got,
        vec![
            ("a", Order::ResetFirst, 3),
            ("b", Order::SwitchFirst, 3),
            ("c", Order::SwitchOnly, 3),
            ("d", Order::SwitchFirst, 5),
        ]
    );
    // A v7 rule must carry its own order; a leftover global order is not guessed around.
    let mut mixed = old.clone();
    mixed["automation"]["config"]["order"] = json!("reset_first");
    f.platform
        .save_db(&f.root.join("accounts.sqlite3"), &mixed)
        .unwrap();
    let home = f.root.join("home");
    let sources = Sources {
        codex_home: home.join(".codex"),
        pi_home: home.join(".pi"),
        claude_bin: None,
    };
    assert!(Accounts::with_platform(&f.root, &home, sources, None, f.platform.clone()).is_err());
}

fn group_reset_success(_: &Access, r: &Request) -> Result<Response> {
    if let Request::Consume { .. } = r {
        return Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({"code":"reset"}),
        });
    }
    Ok(response(r, 99))
}

#[test]
fn group_reset_never_switches_on_old_http_or_desktop_cache() {
    let f = Fixture::new();
    let mut m = setup(&f);
    m.db.automation.config.accounts[0].order = Order::ResetFirst;
    m.reset_transport = Some(group_reset_success);
    let old = crate::automation::Observation {
        remaining: 1.,
        checked_at: now(),
        expires_at: now() + 3600,
    };
    m.automation.observation = Some(("codex:a:workspace-a".into(), old.clone()));
    m.automation_tick();
    assert!(m.reset_job.is_some());
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(m.automation.job.is_none());
    assert!(m.db.automation.run.is_none());
    m.finish_reset();
    assert!(m.quota.automation_observation("a", now()).is_none());
    for _ in 0..3 {
        // Repeating the same old event, even with the same second timestamp, must not authorize switch.
        m.automation.observation = Some(("codex:a:workspace-a".into(), old.clone()));
        m.automation.next_check = 0;
        m.automation_tick();
        assert!(m.automation.job.is_none());
        assert!(m.db.automation.run.is_none());
    }
    assert!(!f.root.join("desktop-operations").exists());
    quota(&mut m, "a", high, now());
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(m.automation.job.is_none());
    assert!(m.db.automation.run.is_none());
}

#[test]
fn group_reset_new_low_get_falls_back_to_switch() {
    let f = Fixture::new();
    let mut m = setup(&f);
    m.db.automation.config.accounts[0].order = Order::ResetFirst;
    m.reset_transport = Some(group_reset_success);
    m.automation_tick();
    m.finish_reset();
    quota(&mut m, "a", low, now());
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(m.reset_job.is_none());
    assert_eq!(m.db.automation.run.as_ref().unwrap().target, "b");
    m.stop_strategy(true).unwrap();
    settle(&mut m);
}

#[test]
fn disconnected_desktop_does_not_block_resets_for_default_or_other_accounts() {
    for id in ["a", "b"] {
        let f = Fixture::new();
        let mut m = setup(&f);
        let native_path = f.root.join("home/.codex/auth.json");
        let native_before = fs::read(&native_path).unwrap();
        m.db.desktop.enabled = false;
        m.desktop.connected = false;
        m.desktop.owner = None;
        m.desktop.other_clients = || true; // Running clients do not block access-only reset.
        let mut config = m.db.automation.config.clone();
        for rule in &mut config.accounts {
            rule.order = if rule.id == id {
                Order::ResetFirst
            } else {
                Order::SwitchOnly
            };
        }
        m.configure_strategy(config, true).unwrap();
        quota(&mut m, id, low, now());
        m.reset_transport = Some(group_reset_success);
        m.automation_tick();
        assert!(m.reset_job.is_some());
        assert_eq!(m.db.reset_attempts.last().unwrap().account_id, id);
        assert!(m.db.automation.run.is_none());
        m.finish_reset();
        assert_eq!(
            m.db.reset_attempts.last().unwrap().outcome,
            crate::resets::Outcome::Reset
        );
        assert!(!f.root.join("desktop-operations").exists());
        assert_eq!(fs::read(native_path).unwrap(), native_before);
    }
}

#[test]
fn disconnected_switch_waits_and_unknown_reset_does_not_block_other_accounts() {
    let f = Fixture::new();
    let mut m = setup(&f);
    m.desktop.connected = false;
    m.automation_tick();
    assert!(m.db.automation.run.is_none());
    assert!(m.automation.account_messages["a"].contains("等待桌面"));
    m.db.reset_attempts.push(crate::resets::Attempt {
        executions: vec![],
        account_id: "a".into(),
        owner: "codex:a:workspace-a".into(),
        request_id: "unknown".into(),
        credit_id: "credit".into(),
        expires_at: None,
        created_at: now(),
        outcome: crate::resets::Outcome::Unknown,
    });
    m.db.automation.config.accounts[1].order = Order::SwitchFirst;
    quota(&mut m, "b", low, now());
    m.reset_transport = Some(group_reset_success);
    m.automation.next_check = 0;
    m.automation_tick();
    assert_eq!(m.db.reset_attempts.last().unwrap().account_id, "b");
    assert!(m.automation.account_messages["a"].contains("待确认"));
    m.finish_reset();
    assert_eq!(
        m.db.reset_attempts[0].outcome,
        crate::resets::Outcome::Unknown
    );
    assert!(!f.root.join("desktop-operations").exists());
}

#[test]
fn preparation_failure_releases_unsent_latch_and_retries_after_backoff() {
    let f = Fixture::new();
    let mut m = setup(&f);
    m.desktop.connected = false;
    m.db.automation.config.accounts[0].order = Order::SwitchFirst;
    let native = f.root.join("home/.codex/auth.json");
    let before = fs::read(&native).unwrap();
    fs::write(&native, b"{}").unwrap();
    m.reset_transport = Some(group_reset_success);
    m.automation_tick();
    assert!(m.reset_job.is_none());
    assert_eq!(m.db.reset_attempts.len(), 1);
    assert_eq!(
        m.db.reset_attempts[0].outcome,
        crate::resets::Outcome::NotSent
    );
    assert!(!m.db.automation.latches.contains_key("a"));
    assert!(m.automation.retry_after["a"] > now());
    fs::write(native, before).unwrap();
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(m.reset_job.is_none());
    m.automation.retry_after.insert("a".into(), 0);
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(m.reset_job.is_some());
    m.finish_reset();
    assert_eq!(m.db.reset_attempts.len(), 2);
    assert_eq!(
        m.db.reset_attempts[1].outcome,
        crate::resets::Outcome::Reset
    );
    let history = m.reset_history(Some("a"), None, None).unwrap();
    assert_eq!(history["total"], 2);
    assert_eq!(history["entries"][0]["outcome"], "reset");
    assert_eq!(history["entries"][1]["outcome"], "not_sent");
    assert_eq!(history["entries"][1]["source"], "automatic");
}

#[test]
fn not_sent_reset_releases_latch_but_requires_fresh_usage_and_backoff() {
    fn no_connection(_: &Access, r: &Request) -> Result<Response> {
        assert!(matches!(r, Request::ResetCredits));
        Err("synthetic offline before POST")
    }
    let f = Fixture::new();
    let mut m = setup(&f);
    m.desktop.connected = false;
    m.db.automation.config.accounts[0].order = Order::SwitchFirst;
    m.reset_transport = Some(no_connection);
    m.automation_tick();
    m.finish_reset();
    assert_eq!(
        m.db.reset_attempts[0].outcome,
        crate::resets::Outcome::NotSent
    );
    assert!(!m.db.automation.latches.contains_key("a"));
    assert!(m.quota.automation_observation("a", now()).is_none());
    m.reset_transport = Some(group_reset_success);
    quota(&mut m, "a", low, now());
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(m.reset_job.is_none());
    m.automation.retry_after.insert("a".into(), 0);
    m.automation.next_check = 0;
    m.automation_tick();
    m.finish_reset();
    assert_eq!(m.db.reset_attempts.len(), 2);
    assert_eq!(
        m.db.reset_attempts[1].outcome,
        crate::resets::Outcome::Reset
    );
}

#[test]
fn restart_clears_only_proven_unsent_legacy_latches() {
    let f = Fixture::new();
    let mut m = setup(&f);
    for (id, outcome) in [
        ("orphan", None),
        ("not-sent", Some(crate::resets::Outcome::NotSent)),
        ("unknown", Some(crate::resets::Outcome::Unknown)),
        ("pending", Some(crate::resets::Outcome::Pending)),
        ("reset", Some(crate::resets::Outcome::Reset)),
        ("redeemed", Some(crate::resets::Outcome::AlreadyRedeemed)),
    ] {
        m.db.automation.latches.insert(
            id.into(),
            crate::automation::Latch {
                request_id: id.into(),
                created_at: now(),
                threshold: 2,
                kind: LatchKind::Reset,
            },
        );
        if let Some(outcome) = outcome {
            m.db.reset_attempts.push(crate::resets::Attempt {
                executions: vec![],
                account_id: id.into(),
                owner: "synthetic".into(),
                request_id: id.into(),
                credit_id: "credit".into(),
                expires_at: None,
                created_at: now(),
                outcome,
            });
        }
    }
    // A switch latch has no Attempt and still survives the restart.
    m.db.automation.latches.insert(
        "switched".into(),
        crate::automation::Latch {
            request_id: "batch".into(),
            created_at: now(),
            threshold: 2,
            kind: LatchKind::Switch,
        },
    );
    m.save().unwrap();
    let reopened = f.manager();
    assert_eq!(reopened.db.automation.latches.len(), 5);
    for id in ["unknown", "pending", "reset", "redeemed", "switched"] {
        assert!(reopened.db.automation.latches.contains_key(id));
    }
    assert!(reopened.reset_job.is_none());
}

#[test]
fn automatic_reset_history_is_durable_before_post_and_keeps_trigger_and_result_times() {
    let f = Fixture::new();
    let mut m = setup(&f);
    m.desktop.connected = false;
    m.db.automation.config.accounts[0].order = Order::SwitchFirst;
    m.db.automation.config.accounts[0].below = 3;
    m.reset_transport = Some(group_reset_success);
    let start = now();
    m.automation_tick();
    let saved = read_json(&f.root.join("accounts.sqlite3"))
        .unwrap()
        .unwrap();
    let entry = &saved["reset_attempts"][0]["executions"][0];
    assert_eq!(entry["source"], "automatic");
    assert_eq!(entry["outcome"], "pending");
    assert!(entry["finished_at"].is_null());
    assert_eq!(entry["trigger"]["remaining_percent"], 1.0);
    assert_eq!(entry["trigger"]["threshold_percent"], 3);
    assert!(entry["started_at"].as_u64().unwrap() >= start);
    let pending = m.reset_history(None, None, Some(30)).unwrap();
    assert_eq!(pending["entries"][0]["outcome"], "pending");
    m.finish_reset();
    let history = f.manager().reset_history(None, None, None).unwrap();
    let entry = &history["entries"][0];
    assert_eq!(entry["outcome"], "reset");
    assert!(entry["finished_at"].as_u64().unwrap() >= entry["started_at"].as_u64().unwrap());
    assert!(!history.to_string().contains("codex:a:workspace-a"));
    assert!(!history.to_string().contains("credit"));
}

fn nearly_low(_: &Access, r: &Request) -> Result<Response> {
    Ok(response(r, 97))
}
fn low_without_credit(_: &Access, r: &Request) -> Result<Response> {
    if let Request::ResetCredits = r {
        return Ok(Response {
            status: 200,
            retry_after: None,
            body: json!({"available_count":0,"credits":[]}),
        });
    }
    Ok(response(r, 99))
}

#[test]
fn one_line_per_account_makes_the_chosen_order_decide_at_the_line() {
    use crate::automation::Action;
    let f = Fixture::new();
    let mut m = setup(&f);
    // The reported case: reset first at a 3% line. Remaining 3% must reset, not switch.
    quota(&mut m, "a", nearly_low, now());
    m.db.automation.config.accounts[0].below = 3;
    let usage = m.quota.automation_observation("a", now()).unwrap();
    assert_eq!(usage.remaining, 3.);
    let action = |m: &mut Accounts, order| {
        m.db.automation.config.accounts[0].order = order;
        m.strategy_action(&m.db.accounts[0], &usage)
    };
    assert_eq!(
        action(&mut m, Order::ResetFirst),
        Action::Reset("credit".into())
    );
    assert_eq!(
        action(&mut m, Order::ResetOnly),
        Action::Reset("credit".into())
    );
    assert_eq!(
        action(&mut m, Order::SwitchFirst),
        Action::Switch("b".into())
    );
    assert_eq!(
        action(&mut m, Order::SwitchOnly),
        Action::Switch("b".into())
    );
    // Above the account's own line nothing happens, whatever the order.
    m.db.automation.config.accounts[0].below = 2;
    for order in [Order::ResetFirst, Order::SwitchFirst] {
        assert_eq!(action(&mut m, order), Action::None);
    }
    // A candidate counts only while above its own line.
    m.db.automation.config.accounts[0].below = 3;
    m.db.automation.config.accounts[1].below = 95;
    assert_eq!(action(&mut m, Order::SwitchOnly), Action::None);
    assert_eq!(
        action(&mut m, Order::SwitchFirst),
        Action::Reset("credit".into())
    );
    m.db.automation.config.accounts[1].below = 2;
    // Without credits a reset-first account falls back to switching; reset-only waits.
    quota(&mut m, "a", low_without_credit, now());
    let usage = m.quota.automation_observation("a", now()).unwrap();
    let action = |m: &mut Accounts, order| {
        m.db.automation.config.accounts[0].order = order;
        m.strategy_action(&m.db.accounts[0], &usage)
    };
    assert_eq!(
        action(&mut m, Order::ResetFirst),
        Action::Switch("b".into())
    );
    assert_eq!(action(&mut m, Order::ResetOnly), Action::None);
    m.automation_tick();
    assert!(m.automation.job.is_none() && m.reset_job.is_none());
    let message = &m.automation.account_messages["a"];
    assert!(message.contains("已见底") && message.contains("没有可用的重置卡"));
    assert!(!f.root.join("desktop-operations").exists());
}

#[test]
fn switching_away_ends_the_episode_so_the_source_is_not_also_reset() {
    let f = Fixture::new();
    let mut m = setup(&f);
    m.db.automation.config.accounts[0].order = Order::SwitchFirst;
    m.reset_transport = Some(group_reset_success);
    settle(&mut m);
    assert!(m.db.automation.run.is_none());
    assert!(!m.db.accounts[0].native && m.db.accounts[1].native);
    let latch = m.db.automation.latches["a"].clone();
    assert_eq!((latch.kind, latch.threshold), (LatchKind::Switch, 2));
    // The former current account is still depleted and has a credit, yet stays untouched.
    for _ in 0..3 {
        quota(&mut m, "a", low, now());
        m.automation.next_check = 0;
        m.automation_tick();
        assert!(m.reset_job.is_none() && m.automation.job.is_none());
        assert!(m.db.reset_attempts.is_empty());
    }
    assert!(m.automation.account_messages["a"].contains("已换走"));
    let reopened = f.manager();
    assert_eq!(reopened.db.automation.latches["a"].kind, LatchKind::Switch);
    // A newer observation above the line starts the next episode.
    quota(&mut m, "a", high, now());
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(!m.db.automation.latches.contains_key("a"));
    assert!(m.automation.account_messages["a"].contains("额度充足"));
}

const GRANT: &str = "grant_launch";
// The real shape: an idle five-hour window has no reset time, and utilization is not capped.
fn claude_usage(week_used: u64) -> Value {
    json!({"five_hour":{"utilization":0,"resets_at":null},
        "seven_day":{"utilization":week_used,"resets_at":"2099-01-02T00:00:00Z"},
        "cedar_ember":{"eligible":true,"at_limit":false,"next_grant_id":GRANT,"grants":[{"id":GRANT,"label":"Synthetic launch",
            "resets_total":1,"resets_left":1,"usable_now":true,"paused":false,"use_requires_limit":false,
            "ends_at":"2099-01-01T00:00:00Z","clears":["five_hour","seven_day"]}]}})
}
fn claude_quota(m: &mut Accounts, bundle: &Bundle, transport: crate::maintenance::Transport) {
    m.quota.claude_transport = Some(transport);
    m.quota.start_claude(
        "c",
        crate::quota::ClaudeAccess::from_bundle(bundle),
        None,
        now(),
    );
    m.quota.stop(now());
}
fn claude_low(_: &str, _: Option<&str>, body: Option<&Value>, _: bool) -> Result<(u16, Value)> {
    assert!(body.is_none(), "quota polling must never claim");
    Ok((200, claude_usage(101)))
}
fn claude_high(_: &str, _: Option<&str>, _: Option<&Value>, _: bool) -> Result<(u16, Value)> {
    Ok((200, claude_usage(10)))
}

#[test]
fn claude_reset_only_spends_one_grant_per_episode_and_codex_never_switches_to_claude() {
    use crate::claude_resets::{Access as ClaudeAccess, Request as ClaudeRequest};
    static CLAIMS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
    fn claim(_: &ClaudeAccess, request: &ClaudeRequest) -> Result<(u16, Value)> {
        match request {
            ClaudeRequest::Status => Ok((200, claude_usage(101))),
            ClaudeRequest::Claim {
                grant_id,
                request_id,
            } => {
                CLAIMS
                    .lock()
                    .unwrap()
                    .push((grant_id.clone(), request_id.clone()));
                Ok((200, json!({"result":"reset"})))
            }
        }
    }
    CLAIMS.lock().unwrap().clear();
    let f = Fixture::new();
    let mut m = setup(&f);
    let mut c = claude("c", "SYNTHETIC_REFRESH_C");
    c.credential["expiresAt"] = json!((now() + 3600) * 1000);
    f.platform.vault_write("c", &c.encoded()).unwrap();
    m.db.accounts.push(account("c", &c, false));
    m.save().unwrap();
    m.claude_reset_transport = Some(claim);
    claude_quota(&mut m, &c, claude_high);
    let mut config = m.db.automation.config.clone();
    config.accounts[1].participate = false;
    config.accounts.push(Rule {
        id: "c".into(),
        order: Order::ResetOnly,
        participate: true,
        below: 2,
    });
    m.configure_strategy(config.clone(), true).unwrap();
    // A depleted Codex account never switches to a Claude account, however much it has left.
    let usage = m.quota.automation_observation("a", now()).unwrap();
    assert_eq!(
        m.strategy_action(&m.db.accounts[0], &usage),
        crate::automation::Action::None
    );
    // Only the Claude account participates now; its reset needs no desktop.
    config.accounts[0].participate = false;
    m.configure_strategy(config, true).unwrap();
    m.desktop.connected = false;
    claude_quota(&mut m, &c, claude_low);
    assert_eq!(
        m.quota
            .automation_observation("c", now())
            .unwrap()
            .remaining,
        0.
    );
    m.automation_tick();
    assert!(m.reset_job.is_some() && m.automation.job.is_none());
    m.finish_reset();
    let attempt = m.db.reset_attempts.last().unwrap().clone();
    assert_eq!(
        (attempt.account_id.as_str(), attempt.credit_id.as_str()),
        ("c", GRANT)
    );
    assert_eq!(attempt.outcome, crate::resets::Outcome::Reset);
    // Resetting an account no session is signed in with moves no running session.
    assert!(m.claude_mod.change.is_none());
    assert_eq!(
        attempt.executions[0]
            .trigger
            .as_ref()
            .unwrap()
            .threshold_percent,
        2
    );
    assert_eq!(
        *CLAIMS.lock().unwrap(),
        vec![(GRANT.to_string(), attempt.request_id.clone())]
    );
    // Still depleted after the reset: the episode spent its one grant and reset-only never switches.
    for _ in 0..3 {
        claude_quota(&mut m, &c, claude_low);
        m.automation.next_check = 0;
        m.automation_tick();
        assert!(m.reset_job.is_none() && m.automation.job.is_none());
    }
    assert_eq!(CLAIMS.lock().unwrap().len(), 1);
    assert!(m.automation.account_messages["c"].contains("这一轮已重置过"));
    assert!(!f.root.join("desktop-operations").exists());
    // Grant IDs stay out of the UI; a retry is addressed by the operation alone.
    let snapshot = m.snapshot();
    assert!(!snapshot.to_string().contains(GRANT));
    assert!(snapshot["accounts"][2]["reset"]["credit_id"].is_null());
    assert_eq!(m.retry_credit(&attempt.request_id).unwrap(), GRANT);
    claude_quota(&mut m, &c, claude_high);
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(!m.db.automation.latches.contains_key("c"));
    assert!(m.automation.account_messages["c"].contains("额度充足"));
}

#[test]
fn a_manual_claude_reset_spends_the_next_grant_without_the_view_naming_it() {
    use crate::claude_resets::{Access as ClaudeAccess, Request as ClaudeRequest};
    static CLAIMS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
    fn claim(_: &ClaudeAccess, request: &ClaudeRequest) -> Result<(u16, Value)> {
        match request {
            ClaudeRequest::Status => Ok((200, claude_usage(40))),
            ClaudeRequest::Claim {
                grant_id,
                request_id,
            } => {
                CLAIMS
                    .lock()
                    .unwrap()
                    .push((grant_id.clone(), request_id.clone()));
                Ok((200, json!({"result":"reset"})))
            }
        }
    }
    CLAIMS.lock().unwrap().clear();
    let f = Fixture::new();
    let mut m = setup(&f);
    let mut c = claude("c", "SYNTHETIC_REFRESH_C");
    c.credential["expiresAt"] = json!((now() + 3600) * 1000);
    f.platform.vault_write("c", &c.encoded()).unwrap();
    m.db.accounts.push(account("c", &c, false));
    m.save().unwrap();
    m.claude_reset_transport = Some(claim);
    // Nothing to name before a fresh reading; Codex credits are always named by the view.
    assert!(m.next_grant("c").is_err());
    assert!(m.next_grant("a").is_err());
    claude_quota(&mut m, &c, claude_high);
    // The view learns that a card is usable now and when it ends, never which one.
    let snapshot = m.snapshot();
    let entry = snapshot["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "c")
        .unwrap();
    assert_eq!(
        entry["usage"]["claude_resets"]["next"],
        json!({"ends_at": "2099-01-01T00:00:00Z"})
    );
    assert!(!snapshot.to_string().contains(GRANT));
    let grant = m.next_grant("c").unwrap();
    m.reset_quota("c", &grant, "manual-claude-1", true).unwrap();
    m.finish_reset();
    let attempt = m.db.reset_attempts.last().unwrap();
    assert_eq!(
        (attempt.credit_id.as_str(), attempt.outcome),
        (GRANT, crate::resets::Outcome::Reset)
    );
    assert!(attempt.executions[0].source == crate::resets::Source::Manual);
    assert_eq!(
        *CLAIMS.lock().unwrap(),
        vec![(GRANT.to_string(), "manual-claude-1".to_string())]
    );
}

#[test]
fn depleted_claude_switches_in_place_beside_running_clients() {
    use crate::claude_locks::RefreshLocks;
    fn no_claim(
        _: &crate::claude_resets::Access,
        _: &crate::claude_resets::Request,
    ) -> Result<(u16, Value)> {
        panic!("switch-only must never touch Claude resets")
    }
    let f = Fixture::new();
    let mut m = f.manager();
    let native = f.native();
    let mut c = claude("c", "SYNTHETIC_REFRESH_C");
    let mut d = claude("d", "SYNTHETIC_REFRESH_D");
    for bundle in [&mut c, &mut d] {
        bundle.credential["expiresAt"] = json!((now() + 3600) * 1000);
    }
    native.initialize(&f.platform, &c).unwrap();
    f.platform.vault_write("c", &c.encoded()).unwrap();
    f.platform.vault_write("d", &d.encoded()).unwrap();
    m.native_override = Some(native.clone());
    // Claude Code keeps running and no desktop is connected: neither blocks a Claude switch.
    m.client_running = |_| true;
    m.desktop.connected = false;
    m.claude_reset_transport = Some(no_claim);
    m.db.accounts.push(account("c", &c, true));
    m.db.accounts.push(account("d", &d, false));
    m.save().unwrap();
    for (id, bundle, transport) in [("c", &c, claude_low as _), ("d", &d, claude_high as _)] {
        m.quota.claude_transport = Some(transport);
        m.quota.start_claude(
            id,
            crate::quota::ClaudeAccess::from_bundle(bundle),
            None,
            now(),
        );
        m.quota.stop(now());
    }
    let rule = |id: &str| Rule {
        id: id.into(),
        order: Order::SwitchOnly,
        participate: true,
        below: 2,
    };
    m.configure_strategy(
        Config {
            enabled: true,
            accounts: vec![rule("c"), rule("d")],
        },
        true,
    )
    .unwrap();
    // Companion mod readings only bring the GET forward, at the account's own line, at most
    // once per 30 s; they never decide a switch themselves.
    let report = |limited, used| crate::claude_mod::Report {
        limited,
        windows: vec![crate::claude_mod::Window {
            kind: "five_hour".into(),
            used,
        }],
    };
    let reply = m.mod_report(report(false, 97.0));
    assert_eq!(m.claude_mod.last_query, 0);
    assert!(reply["change"].is_null() && reply["account"] == m.db.accounts[0].label);
    m.mod_report(report(false, 98.0));
    let asked = m.claude_mod.last_query;
    assert!(asked > 0 && m.db.switch.is_none() && m.db.automation.latches.is_empty());
    m.claude_mod.last_query = asked - 10;
    m.mod_report(report(true, 0.0));
    assert_eq!(m.claude_mod.last_query, asked - 10);
    // An official refresh in progress defers the switch; nothing is written or latched.
    let locks = RefreshLocks::acquire(&native.directory).unwrap();
    m.automation_tick();
    assert!(native.read(&f.platform).unwrap() == c);
    assert!(m.db.automation.latches.is_empty() && m.db.switch.is_none());
    assert!(m.automation.retry_after.contains_key("c"));
    assert_eq!(
        m.automation.account_messages["c"],
        crate::claude_locks::BUSY
    );
    drop(locks);
    m.automation.retry_after.clear();
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(native.read(&f.platform).unwrap() == d);
    assert!(!m.db.accounts[0].native && m.db.accounts[1].native);
    assert!(m.db.switch.is_none() && m.db.automation.run.is_none());
    assert!(m.reset_job.is_none() && m.automation.job.is_none());
    assert_eq!(m.db.automation.latches["c"].kind, LatchKind::Switch);
    assert!(m.automation.account_messages["c"].contains("30 秒内"));
    let reply = m.mod_report(report(true, 100.0));
    assert_eq!(reply["change"]["kind"], "switch");
    assert!(
        reply["account"] == m.db.accounts[1].label && reply["change"]["label"] == reply["account"]
    );
    assert!(!f.root.join("desktop-operations").exists());
    // The switched-away source keeps its latch and is not switched back to while depleted.
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(native.read(&f.platform).unwrap() == d);
    assert!(m.automation.account_messages["c"].contains("已换走"));
}

#[test]
fn reset_of_the_signed_in_claude_account_is_announced_to_the_companion_mod() {
    let f = Fixture::new();
    let mut m = f.manager();
    let c = claude("c", "SYNTHETIC_REFRESH_C");
    m.db.accounts.push(account("c", &c, true));
    for (request_id, outcome) in [
        ("r1", crate::resets::Outcome::NothingToReset),
        ("r2", crate::resets::Outcome::Reset),
    ] {
        m.db.reset_attempts.push(crate::resets::Attempt {
            account_id: "c".into(),
            owner: c.metadata().unwrap().owner,
            request_id: request_id.into(),
            credit_id: GRANT.into(),
            expires_at: None,
            created_at: now(),
            outcome: crate::resets::Outcome::Pending,
            executions: vec![],
        });
        m.reset_job = Some((request_id.into(), std::thread::spawn(move || outcome)));
        m.finish_reset();
        if outcome != crate::resets::Outcome::Reset {
            assert!(m.claude_mod.change.is_none());
        }
    }
    let change = m.claude_mod.change.clone().unwrap();
    assert_eq!(
        (change.kind, change.label),
        ("reset", m.db.accounts[0].label.clone())
    );
}

#[test]
fn a_credit_gone_before_the_post_leaves_the_episode_open_for_another() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static POSTS: AtomicUsize = AtomicUsize::new(0);
    fn credit2() -> Value {
        json!({"available_count":1,"credits":[{"id":"credit2","reset_type":"codex_rate_limits","status":"available","expires_at":"2099-01-01T00:00:00Z"}]})
    }
    // The chosen credit was used elsewhere; another one is still available.
    fn reset(_: &Access, r: &Request) -> Result<Response> {
        match r {
            Request::ResetCredits => Ok(Response {
                status: 200,
                retry_after: None,
                body: credit2(),
            }),
            Request::Consume { .. } => {
                POSTS.fetch_add(1, Ordering::SeqCst);
                Ok(Response {
                    status: 200,
                    retry_after: None,
                    body: json!({"code":"reset"}),
                })
            }
            Request::Usage => unreachable!(),
        }
    }
    fn low_with_credit2(_: &Access, r: &Request) -> Result<Response> {
        if let Request::ResetCredits = r {
            return Ok(Response {
                status: 200,
                retry_after: None,
                body: credit2(),
            });
        }
        Ok(response(r, 99))
    }
    POSTS.store(0, Ordering::SeqCst);
    let f = Fixture::new();
    let mut m = setup(&f);
    m.db.automation.config.accounts[0].order = Order::ResetOnly;
    m.desktop.connected = false;
    m.reset_transport = Some(reset);
    m.automation_tick();
    m.finish_reset();
    let first = m.db.reset_attempts.last().unwrap();
    assert_eq!(first.outcome, crate::resets::Outcome::NoCredit);
    assert_eq!(POSTS.load(Ordering::SeqCst), 0);
    // Nothing was spent, so the episode stays open, and a restart does not close it either.
    assert!(!m.db.automation.latches.contains_key("a"));
    assert!(!f.manager().db.automation.latches.contains_key("a"));
    quota(&mut m, "a", low_with_credit2, now());
    m.automation.retry_after.clear(); // the wait has passed
    m.automation.next_check = 0;
    m.automation_tick();
    assert!(m.reset_job.is_some());
    m.finish_reset();
    assert_eq!(POSTS.load(Ordering::SeqCst), 1);
    assert_eq!(m.db.reset_attempts.last().unwrap().credit_id, "credit2");
}

#[test]
fn a_claim_the_server_keeps_refusing_waits_longer_each_time() {
    use crate::claude_resets::{Access as ClaudeAccess, Request as ClaudeRequest};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static POSTS: AtomicUsize = AtomicUsize::new(0);
    fn claim(_: &ClaudeAccess, request: &ClaudeRequest) -> Result<(u16, Value)> {
        match request {
            ClaudeRequest::Status => Ok((200, claude_usage(101))),
            ClaudeRequest::Claim { .. } => {
                POSTS.fetch_add(1, Ordering::SeqCst);
                Ok((403, json!({"error":"forbidden"})))
            }
        }
    }
    POSTS.store(0, Ordering::SeqCst);
    let f = Fixture::new();
    let mut m = setup(&f);
    let mut c = claude("c", "SYNTHETIC_REFRESH_C");
    c.credential["expiresAt"] = json!((now() + 3600) * 1000);
    f.platform.vault_write("c", &c.encoded()).unwrap();
    m.db.accounts.push(account("c", &c, false));
    m.save().unwrap();
    m.claude_reset_transport = Some(claim);
    let mut config = m.db.automation.config.clone();
    config.accounts[0].participate = false;
    config.accounts[1].participate = false;
    config.accounts.push(Rule {
        id: "c".into(),
        order: Order::ResetOnly,
        participate: true,
        below: 2,
    });
    m.configure_strategy(config, true).unwrap();
    m.desktop.connected = false;
    let mut waits = vec![];
    for _ in 0..8 {
        claude_quota(&mut m, &c, claude_low);
        m.automation.retry_after.clear(); // the wait has passed
        m.automation.next_check = 0;
        m.automation_tick();
        if m.reset_job.is_some() {
            m.finish_reset();
        }
        waits.push(m.automation.retry_after["c"] - now());
    }
    assert_eq!(POSTS.load(Ordering::SeqCst), 8);
    assert!(waits.windows(2).all(|w| w[1] >= w[0]), "{waits:?}");
    assert!(waits[0] <= 60 && waits[7] >= 3600, "{waits:?}");
}
