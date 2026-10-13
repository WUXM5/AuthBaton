use super::*;
use crate::codex_api::{Access, Request, Response, Transport};
use crate::resets::Outcome as ResetOutcome;
use std::sync::Mutex;

fn read_response(request: &Request) -> Response {
    Response {
        status: 200,
        retry_after: None,
        body: match request {
            Request::Usage => {
                json!({"rate_limit":{"primary_window":{"used_percent":50}},"rate_limit_reset_credits":{"available_count":2}})
            }
            Request::ResetCredits => {
                json!({"available_count":2,"credits":[{"id":"credit-1","reset_type":"codex_rate_limits","status":"available","expires_at":"2026-10-01T00:00:00Z"}]})
            }
            Request::Consume { .. } => panic!("read-only polling must never consume"),
        },
    }
}
fn prepare(fixture: &Fixture, transport: Transport) -> Accounts {
    let mut manager = fixture.manager();
    let bundle = codex_bundle();
    fixture
        .platform
        .vault_write("a", &bundle.encoded())
        .unwrap();
    let mut a = account("a", &bundle, false);
    a.automatic = false;
    manager.db.accounts.push(a);
    manager.save().unwrap();
    manager.quota.transport = Some(|_, request| Ok(read_response(request)));
    manager.reset_transport = Some(transport);
    manager.quota_tick();
    manager.quota.stop(now());
    manager
}

#[test]
fn confirmation_durable_intent_and_unknown_result_reuse_one_credit_after_restart() {
    static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
    static POSTS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    fn api(_: &Access, request: &Request) -> Result<Response> {
        if let Request::Consume {
            request_id,
            credit_id,
        } = request
        {
            let journal = read_json(PATH.lock().unwrap().as_ref().unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(journal["version"], 8);
            assert_eq!(
                journal["reset_attempts"][0]["request_id"],
                request_id.as_str()
            );
            assert_eq!(journal["reset_attempts"][0]["outcome"], "pending");
            let events = journal["reset_attempts"][0]["executions"]
                .as_array()
                .unwrap();
            assert_eq!(events.last().unwrap()["source"], "manual");
            assert!(events.last().unwrap()["finished_at"].is_null());
            let mut posts = POSTS.lock().unwrap();
            posts.push((request_id.clone(), credit_id.clone()));
            if posts.len() == 1 {
                return Err("lost response after remote reset");
            }
            assert_eq!(posts[0], posts[1]);
            return Ok(Response {
                status: 200,
                body: json!({"code":"already_redeemed"}),
                retry_after: None,
            });
        }
        Ok(read_response(request))
    }
    let fixture = Fixture::new();
    *PATH.lock().unwrap() = Some(fixture.root.join("accounts.sqlite3"));
    POSTS.lock().unwrap().clear();
    let mut manager = prepare(&fixture, api);
    let vault = fixture.platform.vault_read("a").unwrap();
    assert!(
        manager
            .reset_quota("a", "credit-1", "operation-1", false)
            .is_err()
    );
    assert!(
        manager
            .reset_quota("a", "unknown-credit", "operation-1", true)
            .is_err()
    );
    assert!(POSTS.lock().unwrap().is_empty());
    manager
        .reset_quota("a", "credit-1", "operation-1", true)
        .unwrap();
    manager.finish_reset();
    assert_eq!(manager.db.reset_attempts[0].outcome, ResetOutcome::Unknown);
    assert!(
        manager
            .reset_quota("a", "credit-1", "operation-2", true)
            .is_err()
    );
    assert!(manager.remove("a", true).is_err());
    let mut reopened = fixture.manager();
    assert!(reopened.reset_job.is_none());
    assert_eq!(POSTS.lock().unwrap().len(), 1);
    reopened.reset_transport = Some(api);
    assert!(
        reopened
            .reset_quota("a", "other-credit", "operation-1", true)
            .is_err()
    );
    reopened
        .reset_quota("a", "credit-1", "operation-1", true)
        .unwrap();
    reopened.finish_reset();
    assert_eq!(
        reopened.db.reset_attempts[0].outcome,
        ResetOutcome::AlreadyRedeemed
    );
    reopened
        .reset_quota("a", "credit-1", "operation-1", true)
        .unwrap();
    assert_eq!(POSTS.lock().unwrap().len(), 2);
    let history = reopened.reset_history(Some("a"), None, Some(30)).unwrap();
    assert_eq!(history["total"], 2);
    assert_eq!(history["entries"][0]["outcome"], "already_redeemed");
    assert_eq!(history["entries"][0]["retry"], true);
    assert_eq!(history["entries"][1]["outcome"], "unknown");
    assert_eq!(reopened.db.reset_attempts.len(), 1);
    assert_eq!(fixture.platform.vault_read("a").unwrap(), vault);
    assert_eq!(reopened.db.accounts[0].last_online, None);
    assert_eq!(reopened.db.accounts[0].last_refresh, None);
    assert!(
        !reopened
            .snapshot()
            .to_string()
            .contains("synthetic-account")
    );
}

#[test]
fn reset_history_pages_are_read_only_and_legacy_times_are_not_invented() {
    let fixture = Fixture::new();
    let mut m = prepare(&fixture, |_, _| {
        panic!("history must not contact transport")
    });
    for i in 0..65 {
        m.db.reset_attempts.push(crate::resets::Attempt {
            account_id: "a".into(),
            owner: "SYNTHETIC_PRIVATE_OWNER".into(),
            request_id: format!("old-{i}"),
            credit_id: "SYNTHETIC_PRIVATE_CREDIT".into(),
            expires_at: None,
            created_at: 1000 + i,
            outcome: ResetOutcome::Reset,
            executions: vec![],
        });
    }
    let mut old = serde_json::to_value(&m.db).unwrap();
    old["version"] = json!(5);
    old["automation"]["config"] = json!({"enabled":false,"switch_below":2,"accounts":[]});
    for a in old["reset_attempts"].as_array_mut().unwrap() {
        a.as_object_mut().unwrap().remove("executions");
    }
    fixture
        .platform
        .save_db(&fixture.root.join("accounts.sqlite3"), &old)
        .unwrap();
    let m = fixture.manager();
    assert_eq!(m.db.version, 8);
    let before = fs::read(fixture.root.join("accounts.sqlite3")).unwrap();
    let page = m.reset_history(None, None, Some(30)).unwrap();
    assert_eq!(page["entries"].as_array().unwrap().len(), 30);
    assert_eq!(page["total"], 65);
    assert_eq!(page["entries"][0]["source"], "legacy");
    assert_eq!(page["entries"][0]["started_at"], 1064);
    assert!(page["entries"][0]["finished_at"].is_null());
    let second = m
        .reset_history(Some("a"), page["next_cursor"].as_str(), Some(30))
        .unwrap();
    let third = m
        .reset_history(Some("a"), second["next_cursor"].as_str(), Some(30))
        .unwrap();
    assert_eq!(third["entries"].as_array().unwrap().len(), 5);
    assert!(third["next_cursor"].is_null());
    let mut ids = std::collections::HashSet::new();
    for page in [&page, &second, &third] {
        for row in page["entries"].as_array().unwrap() {
            assert!(ids.insert(row["id"].as_str().unwrap()));
        }
        assert!(!page.to_string().contains("SYNTHETIC_PRIVATE"));
    }
    assert_eq!(ids.len(), 65);
    assert!(m.reset_history(None, Some("missing"), None).is_err());
    assert!(m.reset_history(None, None, Some(0)).is_err());
    assert!(m.reset_history(None, None, Some(101)).is_err());
    assert_eq!(
        fs::read(fixture.root.join("accounts.sqlite3")).unwrap(),
        before
    );
    assert!(m.reset_job.is_none());
}

#[test]
fn pending_history_survives_restart_as_unknown_without_fabricating_completion() {
    let fixture = Fixture::new();
    let mut m = prepare(&fixture, |_, _| Err("synthetic lost response"));
    m.reset_quota("a", "credit-1", "pending-history", true)
        .unwrap();
    // Persisted intent is read by the restarted service, before old worker completion is committed.
    let reopened = fixture.manager();
    let page = reopened.reset_history(None, None, None).unwrap();
    assert_eq!(page["entries"][0]["outcome"], "unknown");
    assert!(page["entries"][0]["finished_at"].is_null());
    assert_eq!(page["entries"][0]["source"], "manual");
    assert!(reopened.reset_job.is_none());
    m.finish_reset();
}

#[test]
fn failed_intent_save_prevents_post_and_failed_result_save_keeps_recoverable_key() {
    fn api(_: &Access, request: &Request) -> Result<Response> {
        match request {
            Request::Consume { .. } => Ok(Response {
                status: 200,
                body: json!({"code":"reset"}),
                retry_after: None,
            }),
            _ => Ok(read_response(request)),
        }
    }
    let fixture = Fixture::new();
    let mut manager = prepare(&fixture, api);
    fs::write(fixture.root.join("fail-db"), b"synthetic").unwrap();
    assert!(
        manager
            .reset_quota("a", "credit-1", "operation-1", true)
            .is_err()
    );
    assert!(manager.reset_job.is_none());
    assert!(fixture.manager().db.reset_attempts.is_empty());
    fs::remove_file(fixture.root.join("fail-db")).unwrap();
    let mut reopened = fixture.manager();
    reopened.quota.transport = Some(|_, request| Ok(read_response(request)));
    reopened.reset_transport = Some(api);
    reopened.quota_tick();
    reopened.quota.stop(now());
    reopened
        .reset_quota("a", "credit-1", "operation-2", true)
        .unwrap();
    fs::write(fixture.root.join("fail-db"), b"synthetic").unwrap();
    reopened.finish_reset();
    assert!(reopened.storage_fault.get());
    let recovered = fixture.manager();
    assert_eq!(recovered.db.reset_attempts[0].request_id, "operation-2");
    assert_eq!(
        recovered.db.reset_attempts[0].outcome,
        ResetOutcome::Unknown
    );
    assert!(recovered.reset_job.is_none());
}

#[test]
fn pending_reset_refuses_rebound_account_identity() {
    let fixture = Fixture::new();
    let mut manager = prepare(&fixture, |_, _| Err("uncertain"));
    manager
        .reset_quota("a", "credit-1", "operation-1", true)
        .unwrap();
    // The first GET failure is known not sent; plant the persisted pending boundary as if
    // the process exited after recording intent, before receiving any result.
    manager.reset_job.take().unwrap().1.join().unwrap();
    let mut reopened = fixture.manager();
    reopened.db.reset_attempts[0].owner = "different-owner".into();
    assert!(
        reopened
            .reset_quota("a", "credit-1", "operation-1", true)
            .is_err()
    );
    assert!(reopened.reset_job.is_none());
}
