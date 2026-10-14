//! Operations a crash, a cancelled login, an official client or the network cut short.
use super::*;

/// Answers a profile request for the account the synthetic access token was issued to;
/// a renewed token keeps its owner before the first `_`.
fn profile_by_token(
    url: &str,
    token: Option<&str>,
    _: Option<&Value>,
    _: bool,
) -> Result<(u16, Value)> {
    assert!(url.ends_with("/api/oauth/profile"));
    let owner = token
        .and_then(|t| t.strip_prefix("SYNTHETIC_ACCESS_"))
        .and_then(|t| t.split('_').next())
        .ok_or("unexpected token")?;
    Ok((
        200,
        json!({"account":{"uuid":owner},"organization":{"uuid":"synthetic-org"}}),
    ))
}

fn renewed(bundle: &Bundle) -> Bundle {
    let mut next = bundle.clone();
    let owner = &bundle.identity["accountUuid"];
    next.credential["accessToken"] =
        json!(format!("SYNTHETIC_ACCESS_{}_2", owner.as_str().unwrap()));
    next.credential["refreshToken"] = json!("SYNTHETIC_RENEWED_BY_OFFICIAL_CLIENT");
    next
}

/// A Claude switch from `a` to `b` that stopped with `stored` in the native store and
/// `identity` in its identity file.
fn interrupted_claude_switch(
    fixture: &Fixture,
    stored: Option<Value>,
    identity: &Value,
) -> (Accounts, NativeStore, Bundle, Bundle) {
    let mut manager = fixture.manager();
    let native = fixture.native();
    manager.native_override = Some(native.clone());
    manager.profile_transport = Some(profile_by_token);
    let a = claude("a", "SYNTHETIC_A_REFRESH");
    let b = claude("b", "SYNTHETIC_B_REFRESH");
    if let Some(stored) = stored {
        write_json(&native.directory.join(".credentials.json"), &stored).unwrap();
    }
    write_json(
        &native.config,
        &json!({"oauthAccount":identity,"projects":{"keep":true}}),
    )
    .unwrap();
    fixture.platform.vault_write("a", &a.encoded()).unwrap();
    fixture.platform.vault_write("b", &b.encoded()).unwrap();
    manager.db.accounts = vec![account("a", &a, true), account("b", &b, false)];
    manager.db.switch = Some(Switch {
        desktop: false,
        provider: "claude".into(),
        target: "b".into(),
        previous: Some("a".into()),
    });
    manager.save().unwrap();
    (manager, native, a, b)
}

#[test]
fn interrupted_switch_recovers_a_login_the_official_client_renewed() {
    let fixture = Fixture::new();
    let b = claude("b", "SYNTHETIC_B_REFRESH");
    let (mut manager, native, a, _) = interrupted_claude_switch(
        &fixture,
        Some(json!({"claudeAiOauth":renewed(&b).credential})),
        &b.identity,
    );
    manager.recover().unwrap();
    finish_account_job(&mut manager);
    assert!(manager.db.switch.is_none(), "{:?}", manager.notice);
    assert_eq!(
        native.read(&fixture.platform).unwrap().encoded(),
        renewed(&b).encoded()
    );
    assert_eq!(
        fixture.platform.vault_read("b").unwrap(),
        renewed(&b).encoded()
    );
    assert_eq!(fixture.platform.vault_read("a").unwrap(), a.encoded());
    assert!(!manager.db.accounts[0].native && manager.db.accounts[1].native);
    // Every operation the journal blocked works again.
    assert!(manager.request_quota(Some("b")).is_ok());
    assert!(fixture.manager().db.switch.is_none());
}

#[test]
fn interrupted_switch_places_a_renewed_login_by_its_owner_not_the_identity_file() {
    let fixture = Fixture::new();
    let a = claude("a", "SYNTHETIC_A_REFRESH");
    let b = claude("b", "SYNTHETIC_B_REFRESH");
    // The switch wrote b's credential, stopped before the identity file, and a client renewed it.
    let (mut manager, native, _, _) = interrupted_claude_switch(
        &fixture,
        Some(json!({"claudeAiOauth":renewed(&b).credential})),
        &a.identity,
    );
    manager.recover().unwrap();
    finish_account_job(&mut manager);
    assert!(manager.db.switch.is_none(), "{:?}", manager.notice);
    assert_eq!(
        native.read(&fixture.platform).unwrap().encoded(),
        renewed(&b).encoded()
    );
    let config = read_json(&native.config).unwrap().unwrap();
    assert_eq!(config["oauthAccount"], b.identity);
    assert_eq!(config["projects"]["keep"], true);
    assert_eq!(fixture.platform.vault_read("a").unwrap(), a.encoded());
    assert!(manager.db.accounts[1].native);
}

#[test]
fn interrupted_switch_keeps_its_record_when_the_login_cannot_be_placed() {
    fn expired(_: &str, _: Option<&str>, _: Option<&Value>, _: bool) -> Result<(u16, Value)> {
        Ok((401, json!({})))
    }
    for (stored, transport, expected) in [
        (
            claude("c", "SYNTHETIC_C_REFRESH"),
            profile_by_token as crate::maintenance::Transport,
            "未登记",
        ),
        (renewed(&claude("b", "x")), expired, "重新登录"),
    ] {
        let fixture = Fixture::new();
        let a = claude("a", "SYNTHETIC_A_REFRESH");
        let (mut manager, native, a_saved, b) = interrupted_claude_switch(
            &fixture,
            Some(json!({"claudeAiOauth":stored.credential})),
            &a.identity,
        );
        manager.profile_transport = Some(transport);
        let config = read_json(&native.config).unwrap();
        manager.recover().unwrap();
        finish_account_job(&mut manager);
        assert!(manager.db.switch.is_some());
        assert!(manager.notice.as_deref().unwrap().contains(expected));
        assert_eq!(read_json(&native.config).unwrap(), config);
        assert_eq!(fixture.platform.vault_read("a").unwrap(), a_saved.encoded());
        assert_eq!(fixture.platform.vault_read("b").unwrap(), b.encoded());
    }
}

#[test]
fn interrupted_switch_settles_when_the_store_holds_no_login() {
    let a = claude("a", "SYNTHETIC_A_REFRESH");
    let mut cleared = a.credential.clone();
    cleared["accessToken"] = json!("");
    cleared["refreshToken"] = json!("");
    cleared["expiresAt"] = json!(0);
    for stored in [None, Some(json!({"claudeAiOauth":cleared}))] {
        let fixture = Fixture::new();
        let (mut manager, _, a_saved, b) = interrupted_claude_switch(&fixture, stored, &a.identity);
        manager.recover().unwrap();
        finish_account_job(&mut manager);
        assert!(manager.db.switch.is_none(), "{:?}", manager.notice);
        assert!(manager.db.accounts.iter().all(|a| !a.native));
        assert_eq!(fixture.platform.vault_read("a").unwrap(), a_saved.encoded());
        assert_eq!(fixture.platform.vault_read("b").unwrap(), b.encoded());
        // Nothing is the default login now; switching to either account writes it in.
        manager.activate("b", true).unwrap();
        assert!(manager.db.accounts[1].native);
    }
}

fn codex_for(user: &str, refresh: &str) -> Bundle {
    let token = |claims: Value| format!("x.{}.x", URL_SAFE_NO_PAD.encode(claims.to_string()));
    Bundle {
        provider: "codex".into(),
        identity: Value::Null,
        credential: json!({"tokens":{
            "id_token":token(json!({"https://api.openai.com/auth":{"chatgpt_user_id":user,"chatgpt_account_id":format!("workspace-{user}")}})),
            "access_token":token(json!({"exp":9999999999u64})),
            "refresh_token":refresh,"account_id":format!("workspace-{user}")}}),
    }
}

#[test]
fn interrupted_codex_switch_recovers_the_login_by_its_own_owner() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let home = fixture.root.join("codex-home");
    private_dir(&home).unwrap();
    let native = NativeStore::codex(&home, true).unwrap();
    manager.native_override = Some(native.clone());
    let a = codex_for("a", "SYNTHETIC_A_REFRESH");
    let b = codex_for("b", "SYNTHETIC_B_REFRESH");
    let mut renewed = b.clone();
    renewed.credential["tokens"]["refresh_token"] = json!("SYNTHETIC_B_RENEWED");
    write_json(&home.join("auth.json"), &renewed.credential).unwrap();
    fixture.platform.vault_write("a", &a.encoded()).unwrap();
    fixture.platform.vault_write("b", &b.encoded()).unwrap();
    manager.db.accounts = vec![account("a", &a, true), account("b", &b, false)];
    manager.db.switch = Some(Switch {
        desktop: false,
        provider: "codex".into(),
        target: "b".into(),
        previous: Some("a".into()),
    });
    manager.recover().unwrap();
    finish_account_job(&mut manager);
    assert!(manager.db.switch.is_none(), "{:?}", manager.notice);
    assert_eq!(fixture.platform.vault_read("b").unwrap(), renewed.encoded());
    assert_eq!(fixture.platform.vault_read("a").unwrap(), a.encoded());
    assert!(manager.db.accounts[1].native && !manager.db.accounts[0].native);
}

#[test]
fn switching_never_writes_a_target_whose_saved_login_is_stale_or_rejected() {
    for state in ["reauth", "unsaved", "uncertain"] {
        let fixture = Fixture::new();
        let mut manager = fixture.manager();
        let native = fixture.native();
        manager.native_override = Some(native.clone());
        let a = claude("a", "SYNTHETIC_A_REFRESH");
        let b = claude("b", "SYNTHETIC_B_REFRESH");
        write_json(
            &native.directory.join(".credentials.json"),
            &json!({"claudeAiOauth":a.credential}),
        )
        .unwrap();
        write_json(&native.config, &json!({"oauthAccount":a.identity})).unwrap();
        fixture.platform.vault_write("a", &a.encoded()).unwrap();
        fixture.platform.vault_write("b", &b.encoded()).unwrap();
        let mut target = account("b", &b, false);
        target.state = state.into();
        manager.db.accounts = vec![account("a", &a, true), target];
        assert!(manager.activate("b", true).is_err(), "{state}");
        assert_eq!(
            native.read(&fixture.platform).unwrap().encoded(),
            a.encoded()
        );
        assert_eq!(manager.db.accounts[1].state, state);
        assert!(manager.db.accounts[0].native);
    }
}

#[test]
fn a_login_that_never_starts_leaves_no_attempt_behind() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let a = claude("a", "SYNTHETIC_A_REFRESH");
    fixture.platform.vault_write("a", &a.encoded()).unwrap();
    let mut entry = account("a", &a, false);
    entry.state = "healthy".into();
    manager.db.accounts.push(entry);
    // A directory cannot be started as the official CLI.
    manager.sources.claude_bin = Some(fixture.root.clone());
    let mut login = LoginManager::new();
    assert!(
        manager
            .begin_login("claude", "x", Some("a"), &mut login)
            .is_err()
    );
    assert!(
        manager
            .begin_login("claude", "new", None, &mut login)
            .is_err()
    );
    for accounts in [&manager.db.accounts, &fixture.manager().db.accounts] {
        assert_eq!(accounts.len(), 1);
        let a = &accounts[0];
        assert!(a.pending_profile.is_none());
        assert_eq!(a.state, "healthy");
        assert!(due(a, u64::MAX) && quota_eligible(a));
    }
    assert!(
        !fixture.root.join("profiles").exists()
            || fs::read_dir(fixture.root.join("profiles"))
                .unwrap()
                .next()
                .is_none()
    );
}

fn ended_login(manager: &mut Accounts, id: &str, phase: &str) -> LoginManager {
    manager.pending_login = Some(id.into());
    let mut login = LoginManager::new();
    login.state = Some(LoginState {
        attempt_id: "synthetic".into(),
        provider: "claude".into(),
        phase: phase.into(),
        message: String::new(),
        verification_url: None,
        user_code: None,
    });
    manager.finish_login(&mut login);
    login
}

#[test]
fn a_login_that_ends_without_a_result_leaves_the_account_maintained() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let a = claude("a", "SYNTHETIC_A_REFRESH");
    fixture.platform.vault_write("a", &a.encoded()).unwrap();
    let mut entry = account("a", &a, false);
    entry.pending_profile = Some("relogin-a".into());
    manager.db.accounts.push(entry);
    manager.staging("claude", "relogin-a").unwrap();
    let mut placeholder = account("new", &a, false);
    placeholder.metadata = None;
    placeholder.state = "pending".into();
    placeholder.pending_profile = Some("login-new".into());
    manager.db.accounts.push(placeholder);
    manager.staging("claude", "login-new").unwrap();
    for id in ["a", "new"] {
        let login = ended_login(&mut manager, id, "cancelled");
        assert_eq!(login.state.unwrap().phase, "failed");
    }
    assert_eq!(
        manager.db.accounts.len(),
        1,
        "the placeholder goes with its attempt"
    );
    let a = &manager.db.accounts[0];
    assert!(a.pending_profile.is_none());
    assert_eq!(a.state, "saved");
    assert!(due(a, u64::MAX) && quota_eligible(a));
    assert!(!fixture.root.join("profiles/relogin-a").exists());
    assert!(!fixture.root.join("profiles/login-new").exists());
}

#[test]
fn after_a_restart_only_a_login_that_can_still_be_saved_is_kept() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let a = claude("a", "SYNTHETIC_A_REFRESH");
    let b = claude("b", "SYNTHETIC_B_REFRESH");
    fixture.platform.vault_write("a", &a.encoded()).unwrap();
    fixture.platform.vault_write("b", &b.encoded()).unwrap();
    let mut first = account("a", &a, false);
    first.pending_profile = Some("relogin-a".into());
    let mut second = account("b", &b, false);
    second.pending_profile = Some("relogin-b".into());
    manager.db.accounts = vec![first, second];
    manager.staging("claude", "relogin-a").unwrap();
    let staged = manager.staging("claude", "relogin-b").unwrap();
    let fresh = claude("b", "SYNTHETIC_B_FRESH_LOGIN");
    write_json(
        &staged.directory.join(".credentials.json"),
        &json!({"claudeAiOauth":fresh.credential}),
    )
    .unwrap();
    write_json(&staged.config, &json!({"oauthAccount":fresh.identity})).unwrap();
    manager.save().unwrap();
    let mut reopened = fixture.manager();
    assert!(reopened.db.accounts[0].pending_profile.is_none());
    assert_eq!(reopened.db.accounts[0].state, "saved");
    assert!(reopened.db.accounts[1].pending_profile.is_some());
    assert_eq!(reopened.db.accounts[1].state, "attention");
    reopened.local_check("b").unwrap();
    assert_eq!(fixture.platform.vault_read("b").unwrap(), fresh.encoded());
    assert!(reopened.db.accounts[1].pending_profile.is_none());
}

#[test]
fn a_renewal_blocked_by_proxy_settings_counts_as_unsent() {
    fn proxy_unusable(
        url: &str,
        _: Option<&str>,
        _: Option<&Value>,
        _: bool,
    ) -> Result<(u16, Value)> {
        assert!(url.ends_with("/v1/oauth/token"));
        Err(crate::network::PROXY_NOT_SENT)
    }
    let fixture = Fixture::new();
    let native = fixture.native();
    let a = claude("a", "SYNTHETIC_A_REFRESH");
    let b = claude("b", "SYNTHETIC_B_REFRESH");
    write_json(
        &native.directory.join(".credentials.json"),
        &json!({"claudeAiOauth":a.credential}),
    )
    .unwrap();
    write_json(&native.config, &json!({"oauthAccount":a.identity})).unwrap();
    fixture.platform.vault_write("a", &a.encoded()).unwrap();
    fixture.platform.vault_write("b", &b.encoded()).unwrap();
    let out = Work {
        id: "a".into(),
        bundle: a.clone(),
        platform: fixture.platform.clone(),
        native: Some(native.clone()),
        profile: fixture.root.join("unused"),
        codex: None,
        cancel: Arc::new(AtomicBool::new(false)),
        transport: proxy_unusable,
    }
    .run();
    assert_eq!(out.state, "retry");
    assert_eq!(out.message, crate::network::PROXY_NOT_SENT);
    assert!(
        fixture
            .platform
            .vault_read_optional(&crate::claude::journal_id("a"))
            .unwrap()
            .is_none()
    );
    let mut manager = fixture.manager();
    manager.native_override = Some(native);
    manager.db.accounts = vec![account("a", &a, true), account("b", &b, false)];
    manager.activate("b", true).unwrap();
}

#[test]
fn a_signed_out_or_cleared_default_can_be_switched_and_logged_in_again() {
    let a = claude("a", "SYNTHETIC_A_REFRESH");
    let b = claude("b", "SYNTHETIC_B_REFRESH");
    let mut cleared = a.credential.clone();
    cleared["accessToken"] = json!("");
    cleared["refreshToken"] = json!("");
    cleared["expiresAt"] = json!(0);
    for stored in [
        json!({"claudeAiOauth":cleared,"mcpOAuth":{"keep":true}}),
        json!({"mcpOAuth":{"keep":true}}),
    ] {
        for relogin in [false, true] {
            let fixture = Fixture::new();
            let mut manager = fixture.manager();
            let native = fixture.native();
            manager.native_override = Some(native.clone());
            write_json(&native.directory.join(".credentials.json"), &stored).unwrap();
            write_json(&native.config, &json!({"oauthAccount":a.identity})).unwrap();
            fixture.platform.vault_write("a", &a.encoded()).unwrap();
            fixture.platform.vault_write("b", &b.encoded()).unwrap();
            manager.db.accounts = vec![account("a", &a, true), account("b", &b, false)];
            // The scheduler says what happened instead of a storage error.
            manager.check("a", false).unwrap();
            finish_account_job(&mut manager);
            assert_eq!(manager.db.accounts[0].state, "attention");
            assert_eq!(manager.db.accounts[0].message, SIGNED_OUT);
            let expected = if relogin {
                let fresh = claude("a", "SYNTHETIC_A_FRESH_LOGIN");
                manager.db.accounts[0].pending_profile = Some("relogin-a".into());
                let staging = manager.staging("claude", "relogin-a").unwrap();
                write_json(
                    &staging.directory.join(".credentials.json"),
                    &json!({"claudeAiOauth":fresh.credential}),
                )
                .unwrap();
                write_json(&staging.config, &json!({"oauthAccount":fresh.identity})).unwrap();
                manager.local_check("a").unwrap();
                fresh
            } else {
                manager.activate("b", true).unwrap();
                b.clone()
            };
            assert_eq!(
                native.read(&fixture.platform).unwrap().encoded(),
                expected.encoded()
            );
            let payload = read_json(&native.directory.join(".credentials.json"))
                .unwrap()
                .unwrap();
            assert_eq!(payload["mcpOAuth"]["keep"], true);
        }
    }
}

/// A check stopped by a crash after the official CLI rotated the credential in its profile.
fn crashed_codex_check(fixture: &Fixture, vault_changed_since: bool) -> (Accounts, Bundle, Bundle) {
    let old = codex_bundle();
    let mut rotated = old.clone();
    rotated.credential["tokens"]["refresh_token"] = json!("SYNTHETIC_ROTATED_BEFORE_CRASH");
    let profile = fixture.root.join("profiles").join("check-a");
    private_dir(&profile).unwrap();
    fs::write(profile.join("base"), crate::maintenance::generation(&old)).unwrap();
    write_json(&profile.join("auth.json"), &rotated.credential).unwrap();
    let mut current = old.clone();
    if vault_changed_since {
        current.credential["tokens"]["refresh_token"] = json!("SYNTHETIC_SAVED_LATER");
    }
    fixture
        .platform
        .vault_write("a", &current.encoded())
        .unwrap();
    let mut manager = fixture.manager();
    manager.codex = Some(fixture.codex(false));
    manager.db.accounts = vec![account("a", &current, false)];
    (manager, current, rotated)
}

#[test]
fn a_codex_check_saves_what_an_interrupted_check_left_before_reusing_its_profile() {
    for vault_changed_since in [false, true] {
        let fixture = Fixture::new();
        let (mut manager, current, rotated) = crashed_codex_check(&fixture, vault_changed_since);
        // The CLI fails before rotating again, so the vault shows what the check started from.
        fs::write(fixture.root.join("quota-outage"), b"synthetic").unwrap();
        manager.check("a", true).unwrap();
        finish_account_job(&mut manager);
        let expected = if vault_changed_since {
            &current
        } else {
            &rotated
        };
        assert_eq!(
            fixture.platform.vault_read("a").unwrap(),
            expected.encoded(),
            "vault changed since: {vault_changed_since}"
        );
    }
}

#[test]
fn retry_save_after_a_restart_recovers_a_codex_rotation_from_the_profile() {
    let fixture = Fixture::new();
    let (mut manager, _, rotated) = crashed_codex_check(&fixture, false);
    manager.db.accounts[0].state = "unsaved".into();
    manager.retry_save("a").unwrap();
    assert_eq!(fixture.platform.vault_read("a").unwrap(), rotated.encoded());
    assert_eq!(manager.db.accounts[0].state, "saved");
}

fn codex_login_for(user: &str, padding: usize) -> Bundle {
    let mut bundle = codex_bundle();
    bundle.credential["tokens"]["id_token"] = json!(format!(
        "x.{}.x",
        URL_SAFE_NO_PAD.encode(
            json!({"https://api.openai.com/auth":{
                "chatgpt_user_id":user,"chatgpt_account_id":"synthetic-account"},
                "padding":"x".repeat(padding)})
            .to_string()
        )
    ));
    bundle
}

#[test]
fn a_codex_keychain_login_beyond_the_writer_line_limit_switches() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let home = manager.sources.codex_home.clone();
    private_dir(&home).unwrap();
    fs::write(
        home.join("config.toml"),
        "cli_auth_credentials_store = \"keyring\"\n",
    )
    .unwrap();
    let native = NativeStore::codex(&home, false).unwrap();
    let current = codex_login_for("synthetic-user-a", 0);
    let large = codex_login_for("synthetic-user-b", 3000);
    fixture
        .platform
        .write_key(&native.service, &native.account, &current.credential)
        .unwrap();
    for (id, bundle) in [("a", &current), ("b", &large)] {
        fixture.platform.vault_write(id, &bundle.encoded()).unwrap();
        manager.db.accounts.push(account(id, bundle, id == "a"));
    }
    manager.save().unwrap();

    manager.activate("b", true).unwrap();
    assert!(manager.db.switch.is_none());
    assert_eq!(
        fixture
            .platform
            .read_key(&native.service, &native.account)
            .unwrap(),
        Some(large.credential)
    );
}

#[test]
fn switching_back_to_a_codex_default_writes_the_renewal_its_official_client_made() {
    let fixture = Fixture::new();
    let mut manager = fixture.manager();
    let home = manager.sources.codex_home.clone();
    private_dir(&home).unwrap();
    let saved = codex_login_for("synthetic-user-a", 0);
    let mut renewed = saved.clone();
    renewed.credential["tokens"]["refresh_token"] = json!("SYNTHETIC_OFFICIAL_RENEWAL");
    write_json(&home.join("auth.json"), &renewed.credential).unwrap();
    fixture.platform.vault_write("a", &saved.encoded()).unwrap();
    manager.db.accounts.push(account("a", &saved, true));
    manager.save().unwrap();

    // A scheduled check of the default account only observes what the official client keeps.
    manager.check("a", false).unwrap();
    finish_account_job(&mut manager);
    assert_eq!(manager.db.accounts[0].state, "delegated");
    assert_eq!(fixture.platform.vault_read("a").unwrap(), renewed.encoded());

    // The user then logs in to another account outside AuthManager and later switches back.
    let other = codex_login_for("synthetic-user-x", 0);
    write_json(&home.join("auth.json"), &other.credential).unwrap();
    manager.check("a", false).unwrap();
    finish_account_job(&mut manager);
    assert!(!manager.db.accounts[0].native);
    manager.activate("a", true).unwrap();
    assert_eq!(
        read_json(&home.join("auth.json")).unwrap().unwrap(),
        renewed.credential
    );
}
