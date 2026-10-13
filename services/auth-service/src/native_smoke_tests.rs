//! Opt-in integration test. Only temporary files and synthetic Keychain entries.
use super::*;
use crate::platform::{read_json, write_json};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::panic::{AssertUnwindSafe, catch_unwind};

fn codex_fixture(identity: &str, refresh: &str) -> Bundle {
    let payload = json!({"email":format!("{identity}@example.invalid"),"exp":now()+3600,"https://api.openai.com/auth":{"chatgpt_user_id":identity,"chatgpt_account_id":format!("workspace-{identity}"),"chatgpt_plan_type":"pro"}});
    let jwt = format!(
        "e30.{}.c3ludGhldGlj",
        URL_SAFE_NO_PAD.encode(payload.to_string())
    );
    Bundle {
        provider: "codex".into(),
        credential: json!({"auth_mode":"chatgpt","tokens":{"id_token":jwt,"access_token":jwt,"refresh_token":refresh,"account_id":format!("workspace-{identity}")}}),
        identity: Value::Null,
    }
}
fn claude_fixture(identity: &str) -> Bundle {
    Bundle {
        provider: "claude".into(),
        credential: json!({"accessToken":format!("synthetic-access-{identity}"),"refreshToken":format!("synthetic-refresh-{identity}"),"expiresAt":(now()+3600)*1000,"scopes":["user:inference","user:profile"],"subscriptionType":"max"}),
        identity: json!({"accountUuid":identity,"organizationUuid":format!("org-{identity}"),"emailAddress":format!("{identity}@example.invalid")}),
    }
}

#[test]
#[ignore = "requires built native credential-store helper; writes isolated synthetic Keychain entries"]
fn native_sqlite_keychain_and_account_switch_smoke() {
    let root = std::env::temp_dir().join(format!("authmanager-native-{}", random_id().unwrap()));
    private_dir(&root).unwrap();
    let codex_home = root.join("codex");
    private_dir(&codex_home).unwrap();
    let claude_home = root.join("claude");
    private_dir(&claude_home).unwrap();
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../dist/AuthManager.app/Contents/MacOS/credential-store");
    assert!(
        helper.is_file(),
        "build the app bundle before running this ignored test"
    );
    let platform = Platform {
        helper,
        recovery_wait: None,
        fixture_keychain: false,
    };
    let sources = Sources {
        codex_home: codex_home.clone(),
        pi_home: root.join("pi"),
        claude_bin: Some(PathBuf::from("/opt/homebrew/bin/claude")),
    };
    let fake_claude = NativeStore::claude(&root, Some(&claude_home), None).unwrap();
    let mut manager =
        Accounts::with_platform(&root, &root, sources.clone(), None, platform.clone()).unwrap();
    manager.client_running = |_| false;
    manager.native_override = Some(fake_claude.clone());
    let result = catch_unwind(AssertUnwindSafe(|| {
        let a = codex_fixture("a", "synthetic-r0");
        let b = codex_fixture("b", "synthetic-rb");
        write_json(&codex_home.join("auth.json"), &a.credential).unwrap();
        manager.import("codex", "A").unwrap();
        let id_a = manager.db.accounts[0].id.clone();
        write_json(&codex_home.join("auth.json"), &b.credential).unwrap();
        manager.import("codex", "B").unwrap();
        let id_b = manager.db.accounts[1].id.clone();
        manager.activate(&id_a, true).unwrap();
        assert!(
            NativeStore::codex(&codex_home, false)
                .unwrap()
                .read(&platform)
                .unwrap()
                == a
        );
        let rotated = codex_fixture("a", "synthetic-r1");
        write_json(&codex_home.join("auth.json"), &rotated.credential).unwrap();
        manager.activate(&id_b, true).unwrap();
        assert!(Bundle::decode(platform.vault_read(&id_a).unwrap()).unwrap() == rotated);
        manager.activate(&id_a, true).unwrap();
        assert!(
            NativeStore::codex(&codex_home, false)
                .unwrap()
                .read(&platform)
                .unwrap()
                == rotated
        );
        assert!(!manager.snapshot().to_string().contains("synthetic-r1"));
        let relogged = codex_fixture("a", "synthetic-reauthorized");
        let staging = manager.staging("codex", "synthetic-login").unwrap();
        write_json(&staging.directory.join("auth.json"), &relogged.credential).unwrap();
        manager.db.accounts[0].pending_profile = Some("synthetic-login".into());
        manager.capture_login(&id_a).unwrap();
        assert!(manager.db.switch.is_none());
        assert!(manager.db.accounts[0].pending_profile.is_none());
        assert!(
            NativeStore::codex(&codex_home, false)
                .unwrap()
                .read(&platform)
                .unwrap()
                == relogged
        );
        assert!(Bundle::decode(platform.vault_read(&id_a).unwrap()).unwrap() == relogged);
        manager
            .configure(&id_a, Some("A renamed"), Some(false))
            .unwrap();
        let reloaded =
            Accounts::with_platform(&root, &root, sources.clone(), None, platform.clone()).unwrap();
        assert_eq!(reloaded.db.accounts[0].label, "A renamed");
        assert!(!reloaded.db.accounts[0].automatic);

        let ca = claude_fixture("ca");
        let cb = claude_fixture("cb");
        write_json(
            &claude_home.join(".credentials.json"),
            &json!({"claudeAiOauth":ca.credential,"coworkRemoteDevice":{"keep":true}}),
        )
        .unwrap();
        write_json(&fake_claude.config,&json!({"oauthAccount":ca.identity,"projects":{"kept":true},"modelAccessCache":{"old":true}})).unwrap();
        manager.import("claude", "Claude A").unwrap();
        let ca_id = manager.db.accounts.last().unwrap().id.clone();
        write_json(
            &claude_home.join(".credentials.json"),
            &json!({"claudeAiOauth":cb.credential,"coworkRemoteDevice":{"keep":true}}),
        )
        .unwrap();
        write_json(&fake_claude.config,&json!({"oauthAccount":cb.identity,"projects":{"kept":true},"modelAccessCache":{"old":true}})).unwrap();
        manager.import("claude", "Claude B").unwrap();
        let cb_id = manager.db.accounts.last().unwrap().id.clone();
        manager.activate(&ca_id, true).unwrap();
        assert!(fake_claude.read(&platform).unwrap() == ca);
        let config = read_json(&fake_claude.config).unwrap().unwrap();
        assert_eq!(config["projects"]["kept"], true);
        assert!(config.get("modelAccessCache").is_none());
        assert_eq!(
            read_json(&claude_home.join(".credentials.json"))
                .unwrap()
                .unwrap()["coworkRemoteDevice"]["keep"],
            true
        );
        manager.activate(&cb_id, true).unwrap();
        assert!(fake_claude.read(&platform).unwrap() == cb);
        // Exercise the same security(1) ACL path used by the official Claude CLI.
        let fallback = claude_home.join(".credentials.json");
        let payload = read_json(&fallback).unwrap().unwrap();
        platform
            .native_write_key(&fake_claude.service, &fake_claude.account, &payload)
            .unwrap();
        std::fs::remove_file(&fallback).unwrap();
        assert!(fake_claude.read(&platform).unwrap() == cb);
        manager.activate(&ca_id, true).unwrap();
        assert!(fake_claude.read(&platform).unwrap() == ca);
        assert_eq!(
            platform
                .native_read_key(&fake_claude.service, &fake_claude.account)
                .unwrap()
                .unwrap()["coworkRemoteDevice"]["keep"],
            true
        );
        manager.activate(&cb_id, true).unwrap();
        // Running Claude clients no longer block a Claude switch; they re-read per request.
        manager.client_running = |_| true;
        manager.activate(&ca_id, true).unwrap();
        assert!(fake_claude.read(&platform).unwrap() == ca);
    }));
    for account in &manager.db.accounts {
        platform
            .delete_key("io.github.wuxm5.authbaton.vault", &account.id)
            .unwrap();
    }
    // The native Claude service above is hashed from this unique temporary path.
    platform
        .native_delete_key(&fake_claude.service, &fake_claude.account)
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[test]
#[ignore = "requires built helper; writes a unique synthetic Keychain item"]
fn native_keychain_large_json_roundtrip() {
    let root = std::env::temp_dir().join(format!(
        "authmanager-pinned-helper-{}",
        random_id().unwrap()
    ));
    private_dir(&root).unwrap();
    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../dist/AuthManager.app/Contents/MacOS/credential-store");
    let platform = Platform {
        helper: crate::platform::install_helper(&root, &bundled).unwrap(),
        recovery_wait: None,
        fixture_keychain: false,
    };
    let service = format!("io.github.wuxm5.authbaton.test.{}", random_id().unwrap());
    let account = format!("synthetic-large-{}", random_id().unwrap());
    let result = catch_unwind(AssertUnwindSafe(|| {
        for size in [1024, 2048, 8192, 16384] {
            let value =
                json!({"synthetic": "x".repeat(size), "unicode": "合成账号", "quoted": "a\"b\nc"});
            platform.vault_write(&account, &value).unwrap();
            assert!(
                platform.vault_read(&account).unwrap() == value,
                "complete JSON must survive a write and update"
            );
            let replacement = root.join("changed-build");
            std::fs::write(&replacement, b"synthetic replacement must never run").unwrap();
            assert_eq!(
                crate::platform::install_helper(&root, &replacement).unwrap(),
                platform.helper
            );
            assert!(platform.vault_read(&account).unwrap() == value);
        }
        let native = json!({"synthetic": "合成账号😀"});
        platform
            .native_write_key(&service, &account, &native)
            .unwrap();
        assert!(platform.native_read_key(&service, &account).unwrap() == Some(native));
        // Unlike the Vault loop above, this is the real official security(1) writer.
        for size in [5120, 16384] {
            let mut native = json!({"claudeAiOauth":{"accessToken":"synthetic-old","refreshToken":"synthetic-refresh"},"mcpOAuth":{"synthetic":"x".repeat(size)},"unicode":"合成😀"});
            platform
                .native_write_key(&service, &account, &native)
                .unwrap();
            native["claudeAiOauth"]["accessToken"] = json!("synthetic-new");
            platform
                .native_write_key(&service, &account, &native)
                .unwrap();
            assert_eq!(
                platform.native_read_key(&service, &account).unwrap(),
                Some(native)
            );
        }
    }));
    platform
        .delete_key("io.github.wuxm5.authbaton.vault", &account)
        .unwrap();
    platform.native_delete_key(&service, &account).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
