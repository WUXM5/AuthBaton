//! Only synthetic credentials and the existing temporary fake Keychain helper.
use super::*;

fn credential(user: &str, expiry: u64) -> Bundle {
    let claims = json!({"https://api.openai.com/auth":{"chatgpt_user_id":user,"chatgpt_account_id":"synthetic-account"},"exp":expiry});
    let token = format!("x.{}.x", URL_SAFE_NO_PAD.encode(claims.to_string()));
    let mut bundle = codex_bundle();
    bundle.credential["tokens"]["id_token"] = json!(token);
    bundle.credential["tokens"]["access_token"] = json!(token);
    bundle
}
fn put(fixture: &Fixture, manager: &mut Accounts, id: &str, bundle: &Bundle, native: bool) {
    fixture.platform.vault_write(id, &bundle.encoded()).unwrap();
    manager.db.accounts.push(account(id, bundle, native));
    if native {
        private_dir(&manager.sources.codex_home).unwrap();
        write_json(
            &manager.sources.codex_home.join("auth.json"),
            &bundle.credential,
        )
        .unwrap();
    }
}
#[test]
fn pi_uses_native_authority_and_exports_only_access() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let old = credential("a", now() + 3600);
    put(&fixture, &mut manager, "a", &old, true);
    let fresh = credential("a", now() + 7200);
    write_json(
        &manager.sources.codex_home.join("auth.json"),
        &fresh.credential,
    )
    .unwrap();
    let result = manager.pi_access(None).unwrap();
    assert_eq!(
        result["access_token"],
        fresh.credential["tokens"]["access_token"]
    );
    assert_eq!(result["account_id"], "a");
    assert!(!result.to_string().contains("refresh_token"));
    assert!(!result.to_string().contains("SYNTHETIC_OLD_REFRESH"));
    assert!(!manager.pi_accounts().to_string().contains("token"));
    assert_eq!(fixture.platform.vault_read("a").unwrap(), old.encoded());
    assert!(manager.job.is_none());
    let clients = manager.clients_snapshot();
    let pi = clients
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "pi_runtime")
        .unwrap();
    assert_eq!(pi["account_label"], "a");
    assert_eq!(pi["evidence"], "issued");
    assert!(
        !clients
            .to_string()
            .contains(fresh.credential["tokens"]["access_token"].as_str().unwrap())
    );
}
#[test]
fn pi_pinned_account_survives_global_selection_change_and_never_falls_back() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let a = credential("a", now() + 3600);
    let b = credential("b", now() + 3600);
    put(&fixture, &mut manager, "a", &a, true);
    put(&fixture, &mut manager, "b", &b, false);
    assert_eq!(manager.pi_access(None).unwrap()["account_id"], "a");
    manager.db.accounts[0].native = false;
    manager.db.accounts[1].native = true;
    write_json(&manager.sources.codex_home.join("auth.json"), &b.credential).unwrap();
    assert_eq!(manager.pi_access(Some("a")).unwrap()["account_id"], "a");
    assert_eq!(manager.pi_access(None).unwrap()["account_id"], "b");
    assert!(manager.pi_access(Some("missing")).is_err());
    manager.db.accounts[0].state = "reauth".into();
    assert!(manager.pi_access(Some("a")).is_err());
}
#[test]
fn pi_rejects_identity_mismatch_unresolved_switch_and_wrong_provider() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let a = credential("a", now() + 3600);
    put(&fixture, &mut manager, "a", &a, false);
    let mut mismatch = a.clone();
    mismatch.credential["tokens"]["access_token"] =
        credential("b", now() + 3600).credential["tokens"]["access_token"].clone();
    fixture
        .platform
        .vault_write("a", &mismatch.encoded())
        .unwrap();
    assert!(manager.pi_access(Some("a")).is_err());
    fixture.platform.vault_write("a", &a.encoded()).unwrap();
    manager.db.switch = Some(Switch {
        provider: "codex".into(),
        target: "a".into(),
        previous: None,
        desktop: false,
    });
    assert!(manager.pi_access(Some("a")).is_err());
    manager.db.switch = None;
    let c = claude("c", "SYNTHETIC_REFRESH");
    put(&fixture, &mut manager, "c", &c, false);
    assert!(manager.pi_access(Some("c")).is_err());
    assert_eq!(
        manager.pi_accounts()["accounts"].as_array().unwrap().len(),
        1
    );
}
#[test]
fn pi_respects_official_refresh_owner_pause_and_retry_backoff() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let a = credential("a", now() + 20);
    put(&fixture, &mut manager, "a", &a, true);
    manager.client_running = |_| true;
    assert!(manager.pi_access(Some("a")).unwrap_err().contains("官方"));
    assert!(manager.job.is_none());
    manager.db.accounts[0].native = false;
    manager.db.accounts[0].automatic = false;
    assert!(manager.pi_access(Some("a")).unwrap_err().contains("关闭"));
    manager.db.accounts[0].automatic = true;
    manager.db.accounts[0].state = "retry".into();
    manager.db.accounts[0].next_check = now() + 300;
    assert!(manager.pi_access(Some("a")).unwrap_err().contains("退避"));
    assert!(manager.job.is_none());
}
#[test]
fn pi_routes_due_access_through_single_existing_maintenance_job() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let a = credential("a", now() + 20);
    put(&fixture, &mut manager, "a", &a, false);
    manager.codex = Some(fixture.codex(false));
    assert_eq!(manager.pi_access(Some("a")).unwrap()["state"], "waiting");
    let id = manager.job.as_ref().unwrap().id.clone();
    assert_eq!(manager.pi_access(Some("a")).unwrap()["state"], "waiting");
    assert_eq!(manager.job.as_ref().unwrap().id, id);
    manager.finish_job();
}
#[test]
fn pi_wire_rejects_extra_fields_and_mutation_methods() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    for input in [
        json!({"version":2,"id":"a","method":"accounts"}),
        json!({"version":1,"id":"a","method":"access","refresh":true}),
        json!({"version":1,"id":"a","method":"accounts.activate","account_id":"a"}),
    ] {
        assert!(
            crate::pi_broker::response(&serde_json::to_vec(&input).unwrap(), &mut manager)
                .get("error")
                .is_some()
        );
    }
}
#[test]
fn pi_socket_answers_a_request_written_after_accept() {
    use std::io::{BufRead, BufReader, Write};
    // A busy pi process can write its request only after the broker has accepted the connection.
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let root = std::env::temp_dir().join(format!("am-pi-{}", std::process::id()));
    private_dir(&root).unwrap();
    let listener = crate::pi_broker::Listener::bind(&root).unwrap();
    let mut client = std::os::unix::net::UnixStream::connect(root.join("pi.sock")).unwrap();
    let late = std::thread::spawn(move || -> std::io::Result<String> {
        std::thread::sleep(std::time::Duration::from_millis(100));
        client.write_all(b"{\"version\":1,\"id\":\"late\",\"method\":\"accounts\"}\n")?;
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line)?;
        Ok(line)
    });
    listener.poll(&mut manager);
    let line = late.join().unwrap();
    drop(listener);
    fs::remove_dir_all(&root).unwrap();
    let reply: Value = serde_json::from_str(&line.expect("broker dropped the connection")).unwrap();
    assert_eq!(reply["id"], "late");
    assert!(reply.get("result").is_some());
}
