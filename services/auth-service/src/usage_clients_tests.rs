use super::*;

#[test]
fn source_check_updates_native_expiry_without_switching_identity_or_health() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let native = fixture.native();
    let old = claude("a", "SYNTHETIC_OLD");
    let mut fresh = old.clone();
    fresh.credential["refreshTokenExpiresAt"] = json!((now() + 10 * 86400) * 1000);
    write_json(
        &native.directory.join(".credentials.json"),
        &json!({"claudeAiOauth":fresh.credential}),
    )
    .unwrap();
    write_json(&native.config, &json!({"oauthAccount":fresh.identity})).unwrap();
    manager.native_override = Some(native.clone());
    manager.db.accounts.push(account("a", &old, true));
    manager.refresh_clients().unwrap();
    assert_eq!(
        manager.db.accounts[0]
            .metadata
            .as_ref()
            .unwrap()
            .refresh_expires_at,
        fresh.metadata().unwrap().refresh_expires_at
    );
    assert_eq!(manager.db.accounts[0].state, "saved");
    assert!(manager.db.accounts[0].native);
    assert!(native.read(&fixture.platform).unwrap() == fresh);
}

fn usage_only(
    url: &str,
    token: Option<&str>,
    body: Option<&Value>,
    _: bool,
) -> Result<(u16, Value)> {
    assert_eq!(url, crate::claude_resets::USAGE_URL);
    assert_eq!(token, Some("SYNTHETIC_ACCESS_a"));
    assert!(body.is_none());
    Ok((
        200,
        json!({"five_hour":{"utilization":20},"seven_day_opus":{"utilization":80}}),
    ))
}
#[test]
fn claude_quota_runs_with_maintenance_disabled_and_does_not_change_health_or_credentials() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let mut bundle = claude("a", "SYNTHETIC_REFRESH");
    bundle.credential["expiresAt"] = json!((now() + 3600) * 1000);
    fixture
        .platform
        .vault_write("a", &bundle.encoded())
        .unwrap();
    let mut record = account("a", &bundle, false);
    record.automatic = false;
    record.state = "reauth".into();
    manager.db.accounts.push(record);
    manager.quota.claude_transport = Some(usage_only);
    manager.quota_tick();
    manager.quota.stop(now());
    let projected = manager.snapshot();
    assert_eq!(projected["accounts"][0]["usage"]["state"], "ready");
    assert_eq!(projected["accounts"][0]["state"], "reauth");
    assert!(manager.db.accounts[0].last_online.is_none());
    assert_eq!(fixture.platform.vault_read("a").unwrap(), bundle.encoded());
    assert!(!projected.to_string().contains("SYNTHETIC_REFRESH"));
    assert!(!projected.to_string().contains("SYNTHETIC_ACCESS"));
}

static RESET_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn reset_metadata_only(
    url: &str,
    token: Option<&str>,
    body: Option<&Value>,
    usage: bool,
) -> Result<(u16, Value)> {
    assert_eq!(url, crate::claude_resets::USAGE_URL);
    assert_eq!(token, Some("SYNTHETIC_ACCESS_a"));
    assert!(body.is_none() && usage);
    RESET_READS.fetch_add(1, Ordering::Relaxed);
    Ok((
        200,
        json!({"five_hour":{"utilization":10},"cedar_ember":{"eligible":true,"grants":[{"resets_left":1,"ends_at":"2030-10-01T00:00:00Z","usable_now":true}]}}),
    ))
}
#[test]
fn native_claude_pending_recovery_can_read_reset_metadata_without_touching_recovery() {
    for state in ["unsaved", "uncertain"] {
        let fixture = Fixture::new();
        let mut manager = fixture.manager();
        let native = fixture.native();
        let old = claude("a", "SYNTHETIC_OLD_REFRESH");
        let mut fresh = claude("a", "SYNTHETIC_NATIVE_REFRESH");
        fresh.credential["expiresAt"] = json!((now() + 3600) * 1000);
        native.initialize(&fixture.platform, &fresh).unwrap();
        manager.native_override = Some(native.clone());
        fixture.platform.vault_write("a", &old.encoded()).unwrap();
        let journal = json!({"expected":old.encoded(),"next":null});
        fixture
            .platform
            .vault_write(&crate::claude::journal_id("a"), &journal)
            .unwrap();
        manager.db.accounts.push(account("a", &old, true));
        manager.db.accounts[0].state = state.into();
        manager.db.accounts[0].automatic = false;
        let metadata = serde_json::to_value(&manager.db.accounts[0]).unwrap();
        manager.quota.claude_transport = Some(reset_metadata_only);
        RESET_READS.store(0, Ordering::Relaxed);
        manager.quota_tick();
        manager.quota.stop(now());
        assert_eq!(RESET_READS.load(Ordering::Relaxed), 1);
        assert_eq!(
            manager.snapshot()["accounts"][0]["usage"]["claude_resets"]["grants"]["remaining"],
            1
        );
        assert_eq!(
            serde_json::to_value(&manager.db.accounts[0]).unwrap(),
            metadata
        );
        assert_eq!(
            fixture
                .platform
                .vault_read(&crate::claude::journal_id("a"))
                .unwrap(),
            journal
        );
        assert_eq!(fixture.platform.vault_read("a").unwrap(), old.encoded());
        assert!(native.read(&fixture.platform).unwrap() == fresh);
    }
}
#[test]
fn reset_metadata_read_rejects_owner_mismatch_and_waits_for_authentication() {
    for scenario in ["mismatch", "inflight", "non-native"] {
        let fixture = Fixture::new();
        let mut manager = fixture.manager();
        let native = fixture.native();
        let old = claude("a", "SYNTHETIC_OLD");
        let mut fresh = claude(
            if scenario == "mismatch" { "other" } else { "a" },
            "SYNTHETIC_FRESH",
        );
        fresh.credential["expiresAt"] = json!((now() + 3600) * 1000);
        native.initialize(&fixture.platform, &fresh).unwrap();
        manager.native_override = Some(native);
        manager
            .db
            .accounts
            .push(account("a", &old, scenario != "non-native"));
        manager.db.accounts[0].state = "uncertain".into();
        manager.quota.claude_transport = Some(reset_metadata_only);
        if scenario == "inflight" {
            manager.job = Some(Job {
                id: "a".into(),
                cancel: Arc::new(AtomicBool::new(false)),
                handle: std::thread::spawn(|| JobResult::ReadError("synthetic")),
            });
        }
        RESET_READS.store(0, Ordering::Relaxed);
        manager.quota_tick();
        manager.quota.stop(now());
        assert_eq!(RESET_READS.load(Ordering::Relaxed), 0);
        assert_eq!(manager.db.accounts[0].state, "uncertain");
        if let Some(job) = manager.job.take() {
            job.handle.join().unwrap();
        }
    }
}
#[test]
fn runtime_identity_and_default_source_are_independent_and_expire() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let a = claude("a", "SYNTHETIC_A");
    let b = claude("b", "SYNTHETIC_B");
    manager.db.accounts.push(account("desktop-a", &a, false));
    manager.db.accounts.push(account("default-b", &b, true));
    manager.desktop.owner = Some(a.metadata().unwrap().owner);
    manager.desktop.connected = true;
    manager.desktop.observed_at = Some(now());
    manager
        .client_sources
        .push(crate::clients::Source::native("claude", b.metadata()));
    manager.clients_checked_at = Some(now());
    let rows = manager.clients_snapshot();
    assert_eq!(rows[0]["evidence"], "confirmed");
    assert_eq!(rows[0]["account_label"], "desktop-a");
    let claude = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "claude")
        .unwrap();
    assert_eq!(claude["evidence"], "configured");
    assert_eq!(claude["account_label"], "default-b");
    let sdk = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "claude_sdk")
        .unwrap();
    assert_eq!(sdk["evidence"], "unknown");
    assert!(sdk["account_label"].is_null());
    assert!(!rows.to_string().contains("claude:a:synthetic-org"));
    manager.desktop.observed_at = Some(now() - 31);
    manager.clients_checked_at = Some(now() - 301);
    let rows = manager.clients_snapshot();
    assert_eq!(rows[0]["evidence"], "unknown");
    assert_eq!(
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "claude")
            .unwrap()["evidence"],
        "unknown"
    );
}

fn warmup_manager(fixture: &Fixture, launch: crate::warmup::Launch) -> Accounts {
    let mut manager = fixture.manager();
    let mut bundle = claude("a", "SYNTHETIC_REFRESH");
    bundle.credential["expiresAt"] = json!((now() + 3600) * 1000);
    fixture
        .platform
        .vault_write("a", &bundle.encoded())
        .unwrap();
    if manager.db.accounts.is_empty() {
        manager.db.accounts.push(account("a", &bundle, false));
    }
    manager.sources.claude_bin = Some(PathBuf::from("/synthetic/claude"));
    manager.quota.claude_transport = Some(usage_only);
    manager.warmup.launch = Some(launch);
    manager
}
/// An hour whose day started less than an hour ago, so its span is under way.
fn current_hour() -> u8 {
    let time = now();
    (0..24u8)
        .find(|h| crate::warmup::slot(*h, time).is_some_and(|s| time - s < 3600))
        .unwrap()
}
fn settle(manager: &mut Accounts) {
    while manager.warmup.running("a") || manager.warmup.queued("a") {
        std::thread::sleep(std::time::Duration::from_millis(5));
        manager.quota_tick();
    }
    manager.quota.stop(now());
}
/// Account a's usage as read now: an idle five-hour window and 10% of the week used.
fn idle(manager: &mut Accounts) {
    manager.quota.record_claude("a", None, 10.0, now());
}
fn tick(manager: &mut Accounts) {
    manager.warmup.recheck();
    manager.quota_tick();
}
fn last(manager: &Accounts) -> crate::warmup::Record {
    let a = &manager.db.accounts[0];
    a.warmup
        .as_ref()
        .or(a.warmup_off.as_ref())
        .unwrap()
        .last
        .clone()
        .unwrap()
}

static WARMUPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[test]
fn claude_warmup_starts_each_window_as_the_last_one_ends_with_the_account_token() {
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |binary, _, token| {
        assert_eq!(binary, Path::new("/synthetic/claude"));
        assert_eq!(token, "SYNTHETIC_ACCESS_a");
        WARMUPS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let codex = codex_bundle();
    manager.db.accounts.push(account("codex", &codex, false));
    assert!(manager.configure_warmup("codex", true, Some(6)).is_err());
    assert!(manager.configure_warmup("a", true, Some(24)).is_err());
    // Outside the day's span nothing is sent, even with the window idle.
    let time = now();
    let outside = (0..24u8)
        .find(|h| {
            crate::warmup::slot(*h, time)
                .is_some_and(|s| time >= s + crate::warmup::SPAN && s + 86400 > time + 600)
        })
        .unwrap();
    manager.configure_warmup("a", true, Some(outside)).unwrap();
    idle(&mut manager);
    tick(&mut manager);
    assert!(!manager.warmup.running("a"));
    let next = manager.snapshot()["accounts"][0]["warmup"]["next"]
        .as_u64()
        .unwrap();
    assert_eq!(Some(next), crate::warmup::slot(outside, time + 86400));
    // Within it, a reading of the idle window sends.
    let current = current_hour();
    manager.configure_warmup("a", true, Some(current)).unwrap();
    manager.quota.stop(now());
    idle(&mut manager);
    tick(&mut manager);
    assert!(manager.warmup.running("a"));
    settle(&mut manager);
    assert_eq!(WARMUPS.load(Ordering::SeqCst), 1);
    let plan = manager.db.accounts[0].warmup.clone().unwrap();
    let day = crate::warmup::slot(current, now()).unwrap();
    assert!(plan.slot == Some(day) && plan.sent.is_some() && plan.last.unwrap().ok);
    // The send asked for a reading; one taken before it, still idle, sends nothing more.
    manager
        .quota
        .record_claude("a", None, 10.0, plan.sent.unwrap() - 1);
    tick(&mut manager);
    assert!(!manager.warmup.running("a"));
    // The window it started runs: nothing is sent, and the card says when the next one is.
    let sent = now() - 5 * 3600;
    manager.db.accounts[0].warmup.as_mut().unwrap().sent = Some(sent);
    manager
        .quota
        .record_claude("a", Some(now() + 1800), 10.0, now());
    tick(&mut manager);
    assert!(!manager.warmup.running("a"));
    let projected = manager.snapshot();
    let warmup = &projected["accounts"][0]["warmup"];
    assert_eq!(
        (
            warmup["enabled"].clone(),
            warmup["hour"].clone(),
            warmup["next"].clone()
        ),
        (json!(true), json!(current), json!(now() + 1800))
    );
    assert!(!projected.to_string().contains("SYNTHETIC_ACCESS"));
    // Once it has reset, the next one is sent.
    idle(&mut manager);
    tick(&mut manager);
    settle(&mut manager);
    assert_eq!(WARMUPS.load(Ordering::SeqCst), 2);
    assert!(manager.db.accounts[0].warmup.as_ref().unwrap().sent > Some(sent));
    // What was sent survives a restart, so the window is not warmed twice.
    let mut reopened = warmup_manager(&fixture, manager.warmup.launch.unwrap());
    idle(&mut reopened);
    tick(&mut reopened);
    assert!(!reopened.warmup.running("a"));
    // Switching off keeps the hour and the last send; switching on again does not resend.
    reopened.configure_warmup("a", false, None).unwrap();
    let off = &reopened.snapshot()["accounts"][0]["warmup"];
    assert_eq!(
        (
            off["enabled"].clone(),
            off["hour"].clone(),
            off["next"].clone()
        ),
        (json!(false), json!(current), Value::Null)
    );
    reopened.configure_warmup("a", true, Some(current)).unwrap();
    idle(&mut reopened);
    tick(&mut reopened);
    assert!(!reopened.warmup.running("a"));
    assert_eq!(WARMUPS.load(Ordering::SeqCst), 2);
    // A build without `warmup_off` reads a switched-off plan as no plan; one before 0.20 reads
    // the day as already sent.
    reopened.configure_warmup("a", false, None).unwrap();
    let stored = serde_json::to_value(&reopened.db.accounts[0]).unwrap();
    assert!(stored.get("warmup").is_none() && stored["warmup_off"]["hour"] == current);
    assert_eq!(stored["warmup_off"]["slot"], day);
}

static RESET_SENDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn usage_reset_window(
    _: &str,
    _: Option<&str>,
    _: Option<&Value>,
    _: bool,
) -> Result<(u16, Value)> {
    Ok((
        200,
        json!({"five_hour":{"utilization":0,"resets_at":null},"seven_day":{"utilization":10}}),
    ))
}
#[test]
fn a_reading_after_the_window_resets_is_asked_for_and_decides() {
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |_, _, _| {
        RESET_SENDS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    manager.quota.claude_transport = Some(usage_reset_window);
    manager
        .configure_warmup("a", true, Some(current_hour()))
        .unwrap();
    // Read while the window ran; it reset a little over a minute ago.
    let time = now();
    manager
        .quota
        .record_claude("a", Some(time - 61), 10.0, time - 120);
    tick(&mut manager);
    assert!(!manager.warmup.running("a") && manager.quota.is_running("a"));
    while manager.quota.is_running("a") {
        std::thread::sleep(std::time::Duration::from_millis(5));
        manager.quota.poll(now());
    }
    tick(&mut manager);
    assert!(manager.warmup.running("a"));
    settle(&mut manager);
    assert_eq!(RESET_SENDS.load(Ordering::SeqCst), 1);
}

#[test]
fn the_seven_day_line_holds_automatic_warmups_back() {
    use crate::automation::{Config, Order, Rule};
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |_, _, _| panic!("held back"));
    manager
        .configure_warmup("a", true, Some(current_hour()))
        .unwrap();
    // The line counts as set even with the strategy off.
    manager.db.automation.config = Config {
        enabled: false,
        accounts: vec![Rule {
            id: "a".into(),
            order: Order::SwitchFirst,
            participate: false,
            below: 3,
        }],
    };
    manager.quota.record_claude("a", None, 97.0, now());
    tick(&mut manager);
    assert!(!manager.warmup.running("a"));
    let held = last(&manager);
    assert!(held.ok && held.message == "7 天额度已到见底线 3%，暂不预热");
    // Said once, not rewritten on every look.
    tick(&mut manager);
    assert_eq!(last(&manager).at, held.at);
    // Without a rule only an exhausted week holds it back.
    manager.db.automation.config = Config::default();
    manager.quota.record_claude("a", None, 100.0, now());
    tick(&mut manager);
    assert_eq!(last(&manager).message, "7 天额度已用完，暂不预热");
    manager.warmup.launch = Some(|_, _, _| Ok(()));
    manager.quota.record_claude("a", None, 97.0, now());
    tick(&mut manager);
    assert!(manager.warmup.running("a"));
    settle(&mut manager);
}

static MANUAL_SENDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[test]
fn a_warmup_by_hand_is_sent_once_whatever_the_window_and_the_plan() {
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |_, _, token| {
        assert_eq!(token, "SYNTHETIC_ACCESS_a");
        MANUAL_SENDS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let codex = codex_bundle();
    manager.db.accounts.push(account("codex", &codex, false));
    assert!(manager.warmup_now("codex").is_err());
    // No plan, and the window running: still sent, and shown on a switched-off plan.
    manager
        .quota
        .record_claude("a", Some(now() + 3600), 10.0, now());
    manager.warmup_now("a").unwrap();
    assert_eq!(manager.snapshot()["accounts"][0]["warmup"]["running"], true);
    manager.quota_tick();
    assert!(manager.warmup.running("a"));
    assert_eq!(manager.warmup_now("a"), Err("这个账号正在预热"));
    settle(&mut manager);
    assert_eq!(MANUAL_SENDS.load(Ordering::SeqCst), 1);
    let plan = manager.db.accounts[0].warmup_off.clone().unwrap();
    assert!(plan.sent.is_none() && plan.slot.is_none());
    assert!(
        plan.last
            .is_some_and(|l| l.ok && l.message == "手动预热已发送")
    );
    // It does not count as the plan's send: switched on, the plan still warms the idle window.
    manager
        .configure_warmup("a", true, Some(current_hour()))
        .unwrap();
    idle(&mut manager);
    tick(&mut manager);
    assert!(manager.warmup.running("a"));
    settle(&mut manager);
    assert_eq!(MANUAL_SENDS.load(Ordering::SeqCst), 2);
    // A failure is shown and not retried.
    manager.warmup.launch =
        Some(|_, _, _| Err(crate::warmup::Failure::Failed("Claude 服务暂时限流")));
    manager.warmup_now("a").unwrap();
    settle(&mut manager);
    assert_eq!(
        last(&manager).message,
        "手动预热未完成：Claude 服务暂时限流"
    );
    tick(&mut manager);
    assert!(!manager.warmup.running("a"));
    // The same account conditions as automatic warm-ups apply.
    manager.db.accounts[0].state = "unsaved".into();
    assert_eq!(
        manager.warmup_now("a"),
        Err("账号认证状态待处理，暂不能预热")
    );
    manager.db.accounts[0].state = "saved".into();
    manager.storage_fault.set(true);
    assert!(manager.warmup_now("a").is_err());
    assert!(!manager.warmup.queued("a"));
}

static BLOCKED_SENDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[test]
fn a_due_warmup_the_account_cannot_send_is_noted_once_and_sent_after_recovery() {
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |_, _, _| {
        BLOCKED_SENDS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    manager.db.accounts[0].state = "unsaved".into();
    manager
        .configure_warmup("a", true, Some(current_hour()))
        .unwrap();
    manager.quota_tick();
    assert!(!manager.warmup.running("a"));
    let noted = last(&manager);
    assert!(!noted.ok && noted.message.contains("待处理"));
    tick(&mut manager);
    assert_eq!(last(&manager).at, noted.at);
    assert_eq!(BLOCKED_SENDS.load(Ordering::SeqCst), 0);
    manager.db.accounts[0].state = "saved".into();
    manager.quota.stop(now());
    idle(&mut manager);
    tick(&mut manager);
    assert!(manager.warmup.running("a"));
    settle(&mut manager);
    assert_eq!(BLOCKED_SENDS.load(Ordering::SeqCst), 1);
    assert!(last(&manager).ok);
}

#[test]
fn shutdown_waits_for_a_running_warmup_and_records_it() {
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |_, _, _| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        Ok(())
    });
    manager
        .configure_warmup("a", true, Some(current_hour()))
        .unwrap();
    idle(&mut manager);
    tick(&mut manager);
    assert!(manager.warmup.running("a"));
    manager.stop();
    let plan = manager.db.accounts[0].warmup.clone().unwrap();
    assert!(plan.sent.is_some() && plan.last.unwrap().ok);
    // Recorded durably, so a restart does not warm the window again.
    let mut reopened = warmup_manager(&fixture, |_, _, _| panic!("window already warmed"));
    idle(&mut reopened);
    tick(&mut reopened);
    assert!(!reopened.warmup.running("a"));
}

#[test]
fn warmup_settings_wait_while_storage_is_faulted() {
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |_, _, _| Ok(()));
    manager.storage_fault.set(true);
    assert!(manager.configure_warmup("a", true, Some(6)).is_err());
    assert!(manager.db.accounts[0].warmup.is_none());
}

#[test]
fn global_pauses_hold_quota_and_warmup_back_and_say_so() {
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |_, _, _| panic!("paused"));
    manager
        .configure_warmup("a", true, Some(current_hour()))
        .unwrap();
    idle(&mut manager);
    manager.storage_fault.set(true);
    // A request from elsewhere (the Claude mod) is kept but does not read as running, and a
    // manual one is refused instead of staying on 查询中.
    manager.quota.request("a", now());
    assert_eq!(
        manager.snapshot()["accounts"][0]["usage"]["refreshing"],
        false
    );
    assert!(manager.request_quota(Some("a")).is_err());
    tick(&mut manager);
    assert!(!manager.warmup.running("a"));
    let last = manager.db.accounts[0].warmup.as_ref().unwrap().last.clone();
    assert!(last.is_some_and(|l| !l.ok && l.message.starts_with("账号数据库保存失败")));
}

static OFF_SENDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[test]
fn switching_off_while_sending_still_records_the_send() {
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |_, _, _| {
        std::thread::sleep(std::time::Duration::from_millis(200));
        OFF_SENDS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let hour = current_hour();
    manager.configure_warmup("a", true, Some(hour)).unwrap();
    idle(&mut manager);
    tick(&mut manager);
    assert!(manager.warmup.running("a"));
    manager.configure_warmup("a", false, None).unwrap();
    settle(&mut manager);
    let off = manager.db.accounts[0].warmup_off.clone().unwrap();
    assert!(off.sent.is_some() && off.last.unwrap().ok);
    manager.configure_warmup("a", true, Some(hour)).unwrap();
    idle(&mut manager);
    tick(&mut manager);
    assert!(!manager.warmup.running("a"));
    assert_eq!(OFF_SENDS.load(Ordering::SeqCst), 1);
}

#[test]
fn removal_waits_for_a_running_warmup_and_leaves_nothing_behind() {
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |_, _, _| {
        std::thread::sleep(std::time::Duration::from_millis(200));
        Ok(())
    });
    manager
        .configure_warmup("a", true, Some(current_hour()))
        .unwrap();
    idle(&mut manager);
    tick(&mut manager);
    assert!(manager.warmup.running("a"));
    assert_eq!(
        manager.remove("a", true),
        Err("此账号正在发送预热，请在发送结束后移除")
    );
    settle(&mut manager);
    manager.remove("a", true).unwrap();
    manager.quota_tick();
    let usage = serde_json::to_value(manager.quota.snapshot("a", now(), true)).unwrap();
    assert_eq!(
        (usage["state"].clone(), usage["refreshing"].clone()),
        (json!("pending"), json!(false))
    );
}

#[test]
fn claude_accounts_the_strategy_resets_are_paced_too() {
    use crate::automation::{Config, Order, Rule};
    let fixture = Fixture::new();
    let mut manager = warmup_manager(&fixture, |_, _, _| panic!("no warm-up planned"));
    manager.db.automation.config = Config {
        enabled: true,
        accounts: vec![Rule {
            id: "a".into(),
            order: Order::SwitchFirst,
            participate: true,
            below: 3,
        }],
    };
    manager.quota_tick();
    assert_eq!(manager.quota.line("a"), Some(3.0));
}
