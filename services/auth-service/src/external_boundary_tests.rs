//! External account sources (contract 2026-11-27 §2.8). A synthetic center answers every request;
//! no test reaches a real network, Keychain or center.
use super::*;
use crate::external::Fail;
use std::cell::RefCell;
use std::rc::Rc;

const SITE: &str = "https://lab.example";
const ENTRY_SECRET: &str = "SYNTHETIC-entry-secret";
const DEVICE_KEY: &str = "SYNTHETIC-device-key";

fn enrollment() -> String {
    URL_SAFE_NO_PAD.encode(
        json!({"site_url":SITE,"code":"pair-1","access_client_id":"synthetic.access","access_client_secret":ENTRY_SECRET})
            .to_string(),
    )
}
fn token(user: &str, generation: u64) -> String {
    let claims = json!({"exp":now()+3600,"generation":generation,"email":format!("{user}@lab.example"),
        "https://api.openai.com/auth":{"chatgpt_user_id":user,"chatgpt_account_id":format!("workspace-{user}"),"chatgpt_plan_type":"team"}});
    format!("x.{}.x", URL_SAFE_NO_PAD.encode(claims.to_string()))
}
fn codex_entry(user: &str) -> Value {
    json!({"account_ref":format!("ref-{user}"),"provider":"openai","label":format!("{user} label"),
        "identity":{"email":format!("{user}@lab.example"),"user_id":user,"workspace_id":format!("workspace-{user}")},
        "plan":"team","status":"ready","login_expires_at":now()+20*86400,"quota_readable":true,"credential_version":1,
        "unknown_field":"SYNTHETIC-dropped"})
}
fn claude_entry(user: &str) -> Value {
    json!({"account_ref":format!("ref-{user}"),"provider":"claude","label":format!("{user} label"),
        "identity":{"email":format!("{user}@example.invalid"),"account_uuid":user,"organization_uuid":"synthetic-org"},
        "plan":"max","status":"ready","login_expires_at":now()+20*86400,"quota_readable":true,"credential_version":1})
}
fn codex_lease(user: &str, version: u64) -> Value {
    let t = token(user, version);
    json!({"account_ref":format!("ref-{user}"),"provider":"openai","credential_version":version,"expires_at":now()+3600,
        "credential":{"id_token":t,"access_token":t,"account_id":format!("workspace-{user}")},
        "identity":{"email":format!("{user}@lab.example"),"user_id":user,"workspace_id":format!("workspace-{user}"),"plan":"team"}})
}
fn claude_lease(user: &str, version: u64) -> Value {
    json!({"account_ref":format!("ref-{user}"),"provider":"claude","credential_version":version,"expires_at":now()+3600,
        "credential":{"access_token":format!("SYNTHETIC-access-{user}-{version}"),"expires_at_ms":(now()+3600)*1000,
            "scopes":["user:inference","user:profile"],"subscription_type":"max"},
        "identity":{"email":format!("{user}@example.invalid"),"account_uuid":user,"organization_uuid":"synthetic-org"}})
}
/// A synthetic center: its directory, the lease each account gets, and every request it was sent.
struct Center {
    accounts: Rc<RefCell<Vec<Value>>>,
    leases: Rc<RefCell<HashMap<String, Value>>>,
    calls: Rc<RefCell<Vec<(String, Value)>>>,
    down: Rc<RefCell<Option<Fail>>>,
}
impl Center {
    fn new(accounts: Vec<Value>) -> Self {
        let center = Center {
            accounts: Rc::new(RefCell::new(accounts)),
            leases: Rc::new(RefCell::new(HashMap::new())),
            calls: Rc::new(RefCell::new(vec![])),
            down: Rc::new(RefCell::new(None)),
        };
        for a in center.accounts.borrow().iter() {
            let user = a["account_ref"]
                .as_str()
                .unwrap()
                .trim_start_matches("ref-");
            let lease = if a["provider"] == "openai" {
                codex_lease(user, 1)
            } else {
                claude_lease(user, 1)
            };
            center
                .leases
                .borrow_mut()
                .insert(a["account_ref"].as_str().unwrap().into(), lease);
        }
        let (accounts, leases, calls, down) = (
            center.accounts.clone(),
            center.leases.clone(),
            center.calls.clone(),
            center.down.clone(),
        );
        crate::external::test_transport(Box::new(move |op, body, key| {
            calls.borrow_mut().push((op.into(), body.clone()));
            if let Some(fail) = down.borrow().clone() {
                return Err(fail);
            }
            if op != "enroll" {
                assert_eq!(
                    key,
                    Some(DEVICE_KEY),
                    "every account request carries the device key"
                );
            }
            match op {
                "enroll" => {
                    assert_eq!(body, &json!({"code":"pair-1"}));
                    Ok(
                        json!({"host_id":"authbaton-abc123","label":"Synthetic Mac","device_key":DEVICE_KEY,"role":"account_viewer"}),
                    )
                }
                "accounts" => Ok(
                    json!({"source":{"label":"lab-center","site_url":SITE,"manage_url":format!("{SITE}/accounts")},
                    "accounts":*accounts.borrow(),"read_at":now()}),
                ),
                "lease" => {
                    // Optional, 0–7200: the center renews first when less is left (default 600).
                    if let Some(seconds) = body.get("min_validity_s") {
                        assert!(seconds.as_u64().is_some_and(|s| s <= 7200));
                    }
                    leases
                        .borrow()
                        .get(body["account_ref"].as_str().unwrap())
                        .cloned()
                        .ok_or(Fail::Code("account_not_found".into()))
                }
                "observations" => Ok(json!({"accepted":true})),
                _ => panic!("unexpected synthetic center request {op}"),
            }
        }));
        center
    }
    fn ops(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .map(|(op, _)| op.clone())
            .collect()
    }
    fn leased(&self, purpose: &str) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|(op, body)| op == "lease" && body["purpose"] == purpose)
            .count()
    }
}
fn row<'a>(m: &'a Accounts, id: &str) -> &'a Account {
    m.db.accounts.iter().find(|a| a.id == id).unwrap()
}
fn finish_desktop(m: &mut Accounts) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while let Some(job) = m.desktop.job.as_ref() {
        assert!(std::time::Instant::now() < deadline);
        if job.is_finished() {
            m.desktop_tick();
        } else {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
/// A full Codex login of this Mac.
fn local_codex(user: &str) -> Bundle {
    let t = token(user, 1);
    Bundle {
        provider: "codex".into(),
        identity: Value::Null,
        credential: json!({"auth_mode":"chatgpt","tokens":{"id_token":t,"access_token":t,
            "refresh_token":format!("SYNTHETIC-{user}-refresh"),"account_id":format!("workspace-{user}")}}),
    }
}
/// Pairs, then lets the first access lease land as the next tick would.
fn connect(m: &mut Accounts) {
    m.external_command("connect", Some(&enrollment()), None, None, false, false)
        .unwrap();
    m.external_tick();
    m.external_tick();
}
fn leased_token(center: &Center, account_ref: &str) -> String {
    center.leases.borrow()[account_ref]["credential"]["access_token"]
        .as_str()
        .unwrap()
        .into()
}
/// The row a source made for an account.
fn ext(m: &Accounts, account_ref: &str) -> String {
    m.external
        .config
        .rows
        .iter()
        .find(|(_, r)| r.account_ref == account_ref)
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| panic!("no row for {account_ref}"))
}
fn card(m: &Accounts, id: &str) -> Value {
    m.snapshot()["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == id)
        .unwrap()
        .clone()
}
fn mode(m: &mut Accounts, id: &str, mode: &str) {
    m.external_command("set_mode", None, Some(id), Some(mode), true, false)
        .unwrap();
}
fn sync(m: &mut Accounts) {
    m.external_command("sync", None, None, None, false, false)
        .unwrap();
    m.external_tick();
}
fn relay_rule(id: &str) -> crate::automation::Rule {
    crate::automation::Rule {
        id: id.into(),
        order: crate::automation::Order::SwitchFirst,
        participate: true,
        below: 5,
    }
}

#[test]
fn pairing_makes_one_row_per_account_merges_this_macs_own_and_keeps_secrets_in_the_vault() {
    let f = Fixture::new();
    let mut m = f.manager();
    let lab = claude("lab", "SYNTHETIC-lab-refresh");
    f.platform.vault_write("lab", &lab.encoded()).unwrap();
    m.db.accounts.push(account("lab", &lab, false));
    m.save().unwrap();
    let center = Center::new(vec![
        codex_entry("nova"),
        claude_entry("orbit"),
        claude_entry("lab"),
        json!({"account_ref":"ref-grok","provider":"xai","status":"ready"}),
    ]);
    for bad in ["", "not a pairing string", "e30"] {
        assert!(
            m.external_command("connect", Some(bad), None, None, false, false)
                .is_err()
        );
    }
    assert!(
        center.ops().is_empty(),
        "a bad string never reaches the center"
    );
    m.external_command("connect", Some(&enrollment()), None, None, false, false)
        .unwrap();
    let pairing = m.snapshot()["external"]["sources"][0].clone();
    assert_eq!(
        (pairing["state"].as_str(), pairing["job"].as_str()),
        (Some("connecting"), Some("connecting")),
        "a source being paired reads as connecting, not as one that cannot be reached"
    );
    assert_eq!(
        m.external.config.sources[0].state, "error",
        "nothing from it is usable until the center accepts the pairing"
    );
    m.external_tick();
    m.external_tick();
    assert_eq!(&center.ops()[..2], ["enroll", "accounts"]);
    let source = m.snapshot()["external"]["sources"][0].clone();
    assert_eq!(source["label"], "lab-center");
    assert_eq!(source["site_url"], SITE);
    assert_eq!(source["manage_url"], "https://lab.example/accounts");
    assert_eq!(source["device_label"], "Synthetic Mac");
    assert_eq!(source["state"], "connected");
    assert!(source["error"].is_null());
    assert!(source["read_at"].is_u64());
    assert_eq!(source["counts"], json!({"codex":1,"claude":2}));
    assert_eq!(source["default_mode"], "view");
    assert_eq!(
        source["merged"],
        json!([{"provider":"claude","email":"lab@example.invalid"}])
    );
    assert!(source["job"].is_null());
    let source_id = source["id"].as_str().unwrap().to_string();
    // Two external rows; the account this Mac has its own login of shows once, as its own.
    assert_eq!(m.db.accounts.len(), 3);
    let nova = ext(&m, "ref-nova");
    let orbit = ext(&m, "ref-orbit");
    let row = m.db.accounts.iter().find(|a| a.id == nova).unwrap();
    assert_eq!(
        (
            row.state.as_str(),
            row.native,
            row.automatic,
            row.provider.as_str()
        ),
        ("external", false, false, "codex")
    );
    assert_eq!(row.label, "nova@lab.example");
    assert_eq!(row.message, "由外部来源续期；这台 Mac 只用领来的 Access");
    let view = card(&m, &orbit);
    assert_eq!(
        view["external"],
        json!({"source":source_id,"source_label":"lab-center","account_ref":"ref-orbit","mode":"view",
            "quota_readable":true,"status":"ready","source_state":"connected",
            "manage_url":"https://lab.example/accounts","reason":null})
    );
    assert!(view["cloud"].is_null());
    assert!(
        view["refresh_expires_at"]
            .as_u64()
            .is_some_and(|t| t > now() + 19 * 86400)
    );
    assert!(card(&m, "lab")["external"].is_null());
    // The device key and entry token are only in the Vault.
    let vault = f
        .platform
        .vault_read(&crate::external::vault_key(&source_id))
        .unwrap()
        .to_string();
    assert!(vault.contains(DEVICE_KEY) && vault.contains(ENTRY_SECRET));
    let file = fs::read_to_string(f.root.join("external-config.json")).unwrap();
    let snapshot = m.snapshot().to_string();
    for secret in [
        DEVICE_KEY,
        ENTRY_SECRET,
        "SYNTHETIC-access",
        "SYNTHETIC-dropped",
    ] {
        assert!(!file.contains(secret), "{secret}");
        assert!(!snapshot.contains(secret), "{secret}");
    }
    assert!(
        !fs::read_to_string(f.root.join("accounts.sqlite3"))
            .unwrap_or_default()
            .contains(DEVICE_KEY)
    );
    // A second source is refused while one is connected.
    assert_eq!(
        m.external_command("connect", Some(&enrollment()), None, None, false, false),
        Err("已经连着一个外部来源，先断开")
    );
    // Reopened, the rows and their source are still there.
    let reopened = f.manager();
    assert_eq!(reopened.external.config.rows.len(), 2);
    assert_eq!(reopened.external.config.sources[0].state, "connected");
}

#[test]
fn a_pairing_the_center_refuses_leaves_an_error_source_that_a_new_string_repairs() {
    let f = Fixture::new();
    let mut m = f.manager();
    let center = Center::new(vec![codex_entry("nova")]);
    *center.down.borrow_mut() = Some(Fail::Code("pairing_expired".into()));
    connect(&mut m);
    let source = m.snapshot()["external"]["sources"][0].clone();
    assert_eq!(source["state"], "error");
    assert_eq!(source["error"], "来源出错（pairing_expired）");
    assert!(m.db.accounts.is_empty());
    *center.down.borrow_mut() = None;
    connect(&mut m);
    assert_eq!(m.snapshot()["external"]["sources"][0]["state"], "connected");
    assert_eq!(m.external.config.sources.len(), 1);
    assert_eq!(m.db.accounts.len(), 1);
    // The pairing stops working: the source says so with the fixed text, the rows stay.
    *center.down.borrow_mut() = Some(Fail::Unpaired);
    sync(&mut m);
    let source = m.snapshot()["external"]["sources"][0].clone();
    assert_eq!(source["state"], "error");
    assert_eq!(
        source["error"],
        "lab-center 的配对已失效，暂时拉不到最新额度，也不能给外部账号用重置卡。"
    );
    assert_eq!(m.db.accounts.len(), 1);
    *center.down.borrow_mut() = Some(Fail::Unreachable("网络不通或超时"));
    sync(&mut m);
    assert_eq!(
        m.snapshot()["external"]["sources"][0]["error"],
        "连不上 lab-center：网络不通或超时"
    );
}

#[test]
fn accounts_that_leave_park_their_mode_and_get_it_back_and_a_local_twin_takes_their_place() {
    let f = Fixture::new();
    let mut m = f.manager();
    let center = Center::new(vec![codex_entry("nova"), claude_entry("orbit")]);
    connect(&mut m);
    let nova = ext(&m, "ref-nova");
    mode(&mut m, &nova, "reset");
    // Gone from the center: not in use, so the row goes and its mode waits.
    center
        .accounts
        .borrow_mut()
        .retain(|a| a["account_ref"] != "ref-nova");
    sync(&mut m);
    assert!(!m.db.accounts.iter().any(|a| a.id == nova));
    let key = format!("{}:ref-nova", m.external.config.sources[0].id);
    assert_eq!(m.external.config.parked[&key].mode, "reset");
    assert!(m.external.config.parked[&key].until > now() + 29 * 86400);
    // Back again: a new row with the mode it had; a new account gets the default mode.
    m.external_command("set_default", None, None, Some("switch"), false, false)
        .unwrap();
    center.accounts.borrow_mut().push(codex_entry("nova"));
    center.accounts.borrow_mut().push(codex_entry("late"));
    m.external_command("sync", None, None, None, false, false)
        .unwrap();
    assert_eq!(m.snapshot()["external"]["sources"][0]["job"], "syncing");
    m.external_tick();
    let late = ext(&m, "ref-late");
    assert_eq!(m.external.config.rows[&late].mode, "switch");
    assert_eq!(
        m.snapshot()["external"]["sources"][0]["default_mode"],
        "switch"
    );
    let back = ext(&m, "ref-nova");
    assert_eq!(m.external.config.rows[&back].mode, "reset");
    assert!(m.external.config.parked.is_empty());
    // This Mac logs in to orbit itself: its own row is shown, the external one is merged.
    let orbit = ext(&m, "ref-orbit");
    let own = claude("orbit", "SYNTHETIC-orbit-local");
    f.platform.vault_write("own", &own.encoded()).unwrap();
    m.db.accounts.push(account("own", &own, false));
    m.external_tick();
    assert!(!m.db.accounts.iter().any(|a| a.id == orbit));
    assert_eq!(
        m.snapshot()["external"]["sources"][0]["merged"],
        json!([{"provider":"claude","email":"orbit@example.invalid"}])
    );
    // Its own row removed, the external row comes back on the next read.
    m.remove("own", true).unwrap();
    sync(&mut m);
    assert!(
        m.external
            .config
            .rows
            .values()
            .any(|r| r.account_ref == "ref-orbit")
    );
    assert_eq!(m.snapshot()["external"]["sources"][0]["merged"], json!([]));
    // A mode parked longer than 30 days is forgotten.
    m.external.config.parked.insert(
        "old:ref".into(),
        crate::external::Parked {
            mode: "switch".into(),
            until: now() - 1,
        },
    );
    m.external_tick();
    assert!(m.external.config.parked.is_empty());
}

fn codex_reading(request: &crate::codex_api::Request) -> crate::codex_api::Response {
    use crate::codex_api::{Request, Response};
    Response {
        status: 200,
        retry_after: None,
        body: match request {
            Request::Usage => {
                json!({"rate_limit":{"primary_window":{"used_percent":40,"reset_at":now()+3600},"secondary_window":{"used_percent":70}},"rate_limit_reset_credits":{"available_count":1}})
            }
            Request::ResetCredits => {
                json!({"available_count":1,"credits":[{"id":"credit-1","reset_type":"codex_rate_limits","status":"available","expires_at":"2026-12-31T00:00:00Z"}]})
            }
            Request::Consume { .. } => json!({"code":"reset"}),
        },
    }
}
fn read_quota(m: &mut Accounts) {
    m.external_tick();
    m.quota.transport = Some(|_, request| Ok(codex_reading(request)));
    m.quota_tick();
    m.quota.stop(now());
}

#[test]
fn modes_allow_quota_to_all_cards_from_reset_and_switching_and_the_relay_only_at_switch() {
    use crate::automation::Config;
    let f = Fixture::new();
    let mut m = f.manager();
    let center = Center::new(vec![codex_entry("nova")]);
    connect(&mut m);
    let nova = ext(&m, "ref-nova");
    assert_eq!(
        center.leased("quota"),
        1,
        "a due reading asks for quota access"
    );
    m.reset_transport = Some(|_, request| Ok(codex_reading(request)));
    read_quota(&mut m);
    assert_eq!(card(&m, &nova)["usage"]["state"], "ready");
    m.request_quota(Some(&nova)).unwrap();
    let rules = |id: &str| Config {
        enabled: false,
        accounts: vec![relay_rule(id)],
    };
    // view: quota only.
    assert_eq!(
        m.reset_quota(&nova, "credit-1", "op-view", true),
        Err("这个外部账号只能看额度：先把它设为可用重置卡")
    );
    assert_eq!(
        m.activate(&nova, true),
        Err("这个外部账号还不能换上本机：先把它设为可换上本机")
    );
    assert_eq!(
        m.configure_strategy(rules(&nova), false),
        Err("这个外部账号还不能加入接力：先把它设为可换上本机")
    );
    assert!(m.db.reset_attempts.is_empty());
    // reset: a card by hand, with access leased for that use; never by a rule, no switching.
    mode(&mut m, &nova, "reset");
    assert!(m.activate(&nova, true).is_err());
    assert!(m.configure_strategy(rules(&nova), false).is_err());
    let trigger = crate::resets::Trigger {
        remaining_percent: 1.0,
        threshold_percent: 5,
        expiring: false,
    };
    assert_eq!(
        m.reset_quota_with_trigger(&nova, "credit-1", "op-auto", true, Some(trigger)),
        Err("只有可换上本机、在接力队列里的外部账号才会自动用卡")
    );
    let uses = center.leased("use");
    m.reset_quota(&nova, "credit-1", "op-hand", true).unwrap();
    m.finish_reset();
    assert_eq!(
        center.leased("use"),
        uses + 1,
        "the card goes with access leased for it"
    );
    assert_eq!(
        m.db.reset_attempts[0].outcome,
        crate::resets::Outcome::Reset
    );
    // Without access for the use nothing is sent.
    read_quota(&mut m);
    *center.down.borrow_mut() = Some(Fail::Unreachable("网络不通或超时"));
    m.reset_quota(&nova, "credit-1", "op-down", true).unwrap();
    m.finish_reset();
    assert_eq!(
        m.db.reset_attempts[1].outcome,
        crate::resets::Outcome::NotSent
    );
    *center.down.borrow_mut() = None;
    // switch: rules too.
    mode(&mut m, &nova, "switch");
    m.configure_strategy(rules(&nova), false).unwrap();
    assert_eq!(m.db.automation.config.accounts.len(), 1);
    // A source that cannot lend refuses cards by hand as well.
    m.external.config.sources[0].state = "error".into();
    read_quota(&mut m);
    assert_eq!(
        m.reset_quota(&nova, "credit-1", "op-error", true),
        Err("外部来源连不上或配对已失效，暂不能用这个账号")
    );
}

#[test]
fn downgrading_is_refused_in_use_and_takes_the_row_out_of_the_relay_only_when_confirmed() {
    use crate::automation::Config;
    let f = Fixture::new();
    let mut m = f.manager();
    let native = f.native();
    m.native_override = Some(native.clone());
    let lab = claude("lab", "SYNTHETIC-lab-refresh");
    native.initialize(&f.platform, &lab).unwrap();
    f.platform.vault_write("lab", &lab.encoded()).unwrap();
    m.db.accounts.push(account("lab", &lab, true));
    m.save().unwrap();
    Center::new(vec![claude_entry("orbit")]);
    connect(&mut m);
    let orbit = ext(&m, "ref-orbit");
    mode(&mut m, &orbit, "switch");
    m.configure_strategy(
        Config {
            enabled: false,
            accounts: vec![relay_rule("lab"), relay_rule(&orbit)],
        },
        false,
    )
    .unwrap();
    assert_eq!(
        m.external_command("set_mode", None, Some(&orbit), Some("reset"), false, false),
        Err("这个号在自动接力队列里：确认降档后会移出队列，见底线与规则不保留")
    );
    assert_eq!(m.external.config.rows[&orbit].mode, "switch");
    m.external_command("set_mode", None, Some(&orbit), Some("reset"), true, false)
        .unwrap();
    assert_eq!(m.external.config.rows[&orbit].mode, "reset");
    let ids: Vec<_> =
        m.db.automation
            .config
            .accounts
            .iter()
            .map(|r| r.id.clone())
            .collect();
    assert_eq!(ids, ["lab"], "no rule is kept for it");
    // In use for Code: it cannot be lowered until Code moves on.
    mode(&mut m, &orbit, "switch");
    m.activate(&orbit, true).unwrap();
    m.external_tick();
    assert!(row(&m, &orbit).native);
    for lower in ["reset", "view"] {
        assert_eq!(
            m.external_command("set_mode", None, Some(&orbit), Some(lower), true, false),
            Err("Code 正在用这个号，先换到其他账号再降档")
        );
    }
    assert!(
        m.external_command("set_mode", None, Some(&orbit), Some("bogus"), true, false)
            .is_err()
    );
}

#[test]
fn the_relay_skips_an_external_row_whose_source_is_down_and_keeps_its_rule() {
    use crate::automation::Config;
    let f = Fixture::new();
    let mut m = f.manager();
    let lab = claude("lab", "SYNTHETIC-lab-refresh");
    f.platform.vault_write("lab", &lab.encoded()).unwrap();
    let mut current = account("lab", &lab, true);
    current.metadata.as_mut().unwrap().expires_at = Some(now() + 3600);
    m.db.accounts.push(current);
    m.save().unwrap();
    Center::new(vec![claude_entry("orbit")]);
    connect(&mut m);
    let orbit = ext(&m, "ref-orbit");
    mode(&mut m, &orbit, "switch");
    m.configure_strategy(
        Config {
            enabled: true,
            accounts: vec![relay_rule("lab"), relay_rule(&orbit)],
        },
        true,
    )
    .unwrap();
    m.quota.record_claude(&orbit, None, 10.0, now());
    assert!(m.strategy_account_ready(row(&m, &orbit)));
    assert_eq!(m.snapshot()["strategy"]["next_claude"], orbit.as_str());
    m.external.config.sources[0].state = "error".into();
    assert!(!m.strategy_account_ready(row(&m, &orbit)));
    assert!(m.snapshot()["strategy"]["next_claude"].is_null());
    assert_eq!(m.db.automation.config.accounts.len(), 2, "the rule stays");
    // The same when the center says the account needs a new login there.
    m.external.config.sources[0].state = "connected".into();
    m.external.config.sources[0].directory[0]["status"] = json!("needs_reauthorization");
    assert!(m.snapshot()["strategy"]["next_claude"].is_null());
    assert_eq!(
        card(&m, &orbit)["external"]["reason"],
        "lab-center 那边需要重新登录这个号"
    );
    // Only a setup-token there: no quota to read or wait for, and the card says why.
    m.external.config.sources[0].directory[0]["status"] = json!("ready");
    m.external.config.sources[0].directory[0]["quota_readable"] = json!(false);
    let view = card(&m, &orbit);
    assert_eq!(
        view["external"]["reason"],
        "只有 setup-token，读不了额度和重置卡"
    );
    assert_eq!(view["external"]["quota_readable"], false);
    assert_eq!(view["usage"]["state"], "unavailable");
    assert_eq!(
        view["usage"]["message"],
        "只有 setup-token，读不了额度和重置卡"
    );
    assert_eq!(
        m.request_quota(Some(&orbit)),
        Err("只有 setup-token，读不了额度和重置卡")
    );
    assert!(!m.quota_access_ready(row(&m, &orbit)));
    assert!(!m.strategy_account_ready(row(&m, &orbit)));
}

#[test]
fn leases_become_access_only_bundles_and_anything_carrying_a_refresh_token_is_refused() {
    use crate::external::lease_of;
    let codex = lease_of(&codex_lease("nova", 3), &codex_entry("nova")).unwrap();
    assert_eq!(codex.version, 3);
    assert_eq!(codex.bundle.credential["auth_mode"], "chatgptAuthTokens");
    assert!(
        codex
            .bundle
            .credential
            .pointer("/tokens/refresh_token")
            .is_none()
    );
    assert!(
        codex.bundle.metadata().is_err(),
        "never a login this Mac could renew"
    );
    assert_eq!(
        codex.bundle.cloud_metadata().unwrap().owner,
        "codex:nova:workspace-nova"
    );
    let claude = lease_of(&claude_lease("orbit", 1), &claude_entry("orbit")).unwrap();
    assert_eq!(
        claude.bundle.identity,
        json!({"accountUuid":"orbit","organizationUuid":"synthetic-org","emailAddress":"orbit@example.invalid"})
    );
    assert_eq!(claude.bundle.credential["subscriptionType"], "max");
    assert!(claude.bundle.credential.get("rateLimitTier").is_none());
    assert_eq!(
        claude.bundle.cloud_metadata().unwrap().owner,
        "claude:orbit:synthetic-org"
    );
    for (mut lease, entry) in [
        (codex_lease("nova", 1), codex_entry("nova")),
        (claude_lease("orbit", 1), claude_entry("orbit")),
    ] {
        lease["credential"]["refresh_token"] = json!("SYNTHETIC-refresh");
        assert_eq!(
            lease_of(&lease, &entry).err(),
            Some(crate::external::CARRIES_REFRESH)
        );
        lease["credential"]
            .as_object_mut()
            .unwrap()
            .remove("refresh_token");
        lease["credential"]["refreshToken"] = json!("SYNTHETIC-refresh");
        assert_eq!(
            lease_of(&lease, &entry).err(),
            Some(crate::external::CARRIES_REFRESH)
        );
    }
    // Someone else's access under this account's ref.
    let mut other = claude_lease("orbit", 1);
    other["identity"]["account_uuid"] = json!("mallory");
    assert_eq!(
        lease_of(&other, &claude_entry("orbit")).err(),
        Some(crate::external::IDENTITY_CHANGED)
    );
    assert!(lease_of(&codex_lease("nova", 1), &claude_entry("nova")).is_err());
    // Through the service: a refused lease is never kept, so the account is not read with it.
    let f = Fixture::new();
    let mut m = f.manager();
    let center = Center::new(vec![claude_entry("orbit")]);
    center.leases.borrow_mut().get_mut("ref-orbit").unwrap()["credential"]["refreshToken"] =
        json!("SYNTHETIC-refresh");
    connect(&mut m);
    let orbit = ext(&m, "ref-orbit");
    assert_eq!(center.leased("quota"), 1);
    assert!(!m.quota_access_ready(row(&m, &orbit)));
    assert!(m.bundle(row(&m, &orbit)).is_err());
    assert!(
        !fs::read_to_string(f.root.join("external-config.json"))
            .unwrap()
            .contains("SYNTHETIC-refresh")
    );
}

#[test]
fn external_rows_never_enter_renewal_warmup_pi_the_cloud_or_claude_desktop() {
    let f = Fixture::new();
    let mut m = f.manager();
    Center::new(vec![codex_entry("nova"), claude_entry("orbit")]);
    connect(&mut m);
    let nova = ext(&m, "ref-nova");
    let orbit = ext(&m, "ref-orbit");
    for id in [&nova, &orbit] {
        assert!(m.local_check(id).is_err());
        assert!(m.request_check(id, false).is_err());
        assert!(m.request_check(id, true).is_err());
        assert!(m.check(id, true).is_err());
        assert!(m.retry_save(id).is_err());
        assert_eq!(
            m.configure(id, Some("renamed"), None),
            Err("外部账号只能在来源网页上登录、改名或移除")
        );
        assert!(m.configure(id, None, Some(true)).is_err());
        assert!(m.remove(id, true).is_err());
        let provider = row(&m, id).provider.clone();
        assert!(
            m.begin_login(
                &provider,
                "x",
                Some(id),
                &mut crate::login::LoginManager::new()
            )
            .is_err()
        );
        assert!(
            m.cloud_command("import", None, None, Some(id), true)
                .is_err()
        );
    }
    assert_eq!(
        m.configure_warmup(&orbit, true, Some(6)),
        Err("外部账号不预热")
    );
    assert_eq!(m.warmup_now(&orbit), Err("外部账号不预热"));
    assert!(row(&m, &orbit).warmup.is_none() && row(&m, &orbit).warmup_off.is_none());
    assert!(m.pi_accounts()["accounts"].as_array().unwrap().is_empty());
    assert_eq!(m.pi_access(Some(&nova)), Err("外部账号不提供给 pi"));
    // The routine scan never picks them, however due they look.
    for a in &mut m.db.accounts {
        a.next_check = 0;
        a.automatic = true;
    }
    m.last_scan = 0;
    m.tick();
    assert!(m.job.is_none());
    assert!(
        !row(&m, &nova).automatic,
        "the next pass turns it off again"
    );
    // Claude 桌面's quota and Code relay do not count it as an AuthBaton login.
    let snapshot = m.snapshot().to_string();
    assert!(!snapshot.contains("SYNTHETIC-access"));
    assert!(f.platform.vault_read_optional(&orbit).unwrap().is_none());
}

#[test]
fn readings_go_back_to_the_center_once_and_only_after_they_were_read() {
    let f = Fixture::new();
    let mut m = f.manager();
    let center = Center::new(vec![claude_entry("orbit")]);
    connect(&mut m);
    let orbit = ext(&m, "ref-orbit");
    for _ in 0..3 {
        m.external_tick();
    }
    assert!(
        !center.ops().contains(&"observations".to_string()),
        "nothing read, nothing sent"
    );
    m.quota
        .record_claude(&orbit, Some(now() + 3600), 30.0, now());
    m.external_tick();
    m.external_tick();
    let sent: Vec<Value> = center
        .calls
        .borrow()
        .iter()
        .filter(|(op, _)| op == "observations")
        .map(|(_, body)| body.clone())
        .collect();
    assert_eq!(sent.len(), 1, "one reading, one report");
    let body = &sent[0];
    assert_eq!(body["account_ref"], "ref-orbit");
    assert_eq!(body["credential_version"], 1);
    assert!(body["observed_at"].is_u64());
    let windows = body["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0]["kind"], "five_hour");
    assert_eq!(windows[0]["remaining_percent"], 100.0);
    assert_eq!(windows[1]["kind"], "seven_day");
    assert_eq!(windows[1]["remaining_percent"], 70.0);
    assert!(body["reset_credits"].is_null());
    // A failed report is not sent again: the next reading is.
    *center.down.borrow_mut() = Some(Fail::Unreachable("网络不通或超时"));
    m.quota
        .record_claude(&orbit, Some(now() + 3600), 31.0, now() + 1);
    m.external_tick();
    m.external_tick();
    *center.down.borrow_mut() = None;
    m.external_tick();
    let count = center
        .ops()
        .iter()
        .filter(|op| *op == "observations")
        .count();
    assert_eq!(count, 2);
}

#[test]
fn disconnecting_asks_to_switch_away_first_then_drops_rows_rules_and_the_pairing() {
    use crate::automation::Config;
    let f = Fixture::new();
    let mut m = f.manager();
    let native = f.native();
    m.native_override = Some(native.clone());
    let lab = claude("lab", "SYNTHETIC-lab-refresh");
    native.initialize(&f.platform, &lab).unwrap();
    f.platform.vault_write("lab", &lab.encoded()).unwrap();
    m.db.accounts.push(account("lab", &lab, true));
    m.save().unwrap();
    let center = Center::new(vec![claude_entry("orbit"), codex_entry("nova")]);
    connect(&mut m);
    let orbit = ext(&m, "ref-orbit");
    let source = m.external.config.sources[0].id.clone();
    mode(&mut m, &orbit, "switch");
    m.configure_strategy(
        Config {
            enabled: false,
            accounts: vec![relay_rule("lab"), relay_rule(&orbit)],
        },
        false,
    )
    .unwrap();
    m.activate(&orbit, true).unwrap();
    m.external_tick();
    assert!(row(&m, &orbit).native);
    assert!(
        m.external_command("disconnect", None, None, None, false, false)
            .is_err()
    );
    assert_eq!(
        m.external_command("disconnect", None, None, None, true, false),
        Ok(Some(json!({"needs_switch": orbit})))
    );
    assert_eq!(m.db.accounts.len(), 3, "nothing changed yet");
    let before = center.calls.borrow().len();
    m.external_command("disconnect", None, None, None, true, true)
        .unwrap();
    assert_eq!(
        center.calls.borrow().len(),
        before,
        "the center is not called"
    );
    // Back on the row of this Mac it replaced, with its full login.
    assert!(row(&m, "lab").native);
    assert!(native.read(&f.platform).unwrap() == lab);
    assert_eq!(m.db.accounts.len(), 1);
    assert!(m.external.config.rows.is_empty());
    assert!(m.external.config.selected.is_empty());
    let ids: Vec<_> =
        m.db.automation
            .config
            .accounts
            .iter()
            .map(|r| r.id.clone())
            .collect();
    assert_eq!(ids, ["lab"]);
    assert_eq!(
        m.external.config.parked[&format!("{source}:ref-orbit")].mode,
        "switch"
    );
    assert_eq!(
        m.external.config.parked[&format!("{source}:ref-nova")].mode,
        "view"
    );
    assert!(
        f.platform
            .vault_read_optional(&crate::external::vault_key(&source))
            .unwrap()
            .is_none()
    );
    assert!(
        f.platform
            .vault_read_optional("cloud-native-claude")
            .unwrap()
            .is_none()
    );
    assert_eq!(m.snapshot()["external"], json!({"sources":[]}));
    // Nothing reads the source any more.
    m.external.next_directory = 0;
    m.external_tick();
    assert_eq!(center.calls.borrow().len(), before);
    // Paired with the same center again within 30 days: the modes come back.
    connect(&mut m);
    assert_eq!(m.external.config.rows[&ext(&m, "ref-orbit")].mode, "switch");
    assert!(m.external.config.parked.is_empty());
}

#[test]
fn switching_claude_to_an_external_row_leases_for_the_use_and_writes_access_only() {
    let f = Fixture::new();
    let mut m = f.manager();
    let native = f.native();
    m.native_override = Some(native.clone());
    let lab = claude("lab", "SYNTHETIC-lab-refresh");
    let renewed = claude("lab", "SYNTHETIC-lab-renewed");
    native.initialize(&f.platform, &renewed).unwrap();
    f.platform.vault_write("lab", &lab.encoded()).unwrap();
    m.db.accounts.push(account("lab", &lab, true));
    m.save().unwrap();
    let center = Center::new(vec![claude_entry("orbit")]);
    connect(&mut m);
    let orbit = ext(&m, "ref-orbit");
    mode(&mut m, &orbit, "switch");
    // Asked for by a person: access is leased for this use first, in the background.
    m.activate(&orbit, true).unwrap();
    assert_eq!(center.leased("use"), 1);
    assert!(row(&m, "lab").native, "the switch waits for that access");
    m.external_tick();
    let held = native.cloud_read(&f.platform).unwrap();
    assert!(held.credential.get("refreshToken").is_none());
    assert_eq!(held.credential["accessToken"], "SYNTHETIC-access-orbit-1");
    assert_eq!(held.identity["accountUuid"], "orbit");
    assert!(row(&m, &orbit).native && !row(&m, "lab").native);
    assert_eq!(m.external.config.selected["claude"], orbit);
    assert_eq!(m.external.config.previous["claude"], "lab");
    // The login switched away from is kept with the generation its client renewed.
    assert_eq!(f.platform.vault_read("lab").unwrap(), renewed.encoded());
    assert!(
        f.platform
            .vault_read_optional("cloud-native-claude")
            .unwrap()
            .is_some()
    );
    // Before the access in use runs out, it is leased again and rewritten.
    let mut next = claude_lease("orbit", 2);
    next["credential"]["expires_at_ms"] = json!((now() + 7200) * 1000);
    center.leases.borrow_mut().insert("ref-orbit".into(), next);
    m.external.written.insert("claude".into(), (now() + 100, 1));
    m.external_tick();
    m.external_tick();
    assert_eq!(
        native.cloud_read(&f.platform).unwrap().credential["accessToken"],
        "SYNTHETIC-access-orbit-2"
    );
    assert_eq!(m.external.written["claude"].1, 2);
    // Only that renewal asks the center to leave 40 minutes on the access; other leases do not.
    let asked: Vec<Value> = center
        .calls
        .borrow()
        .iter()
        .filter(|(op, _)| op == "lease")
        .map(|(_, body)| body["min_validity_s"].clone())
        .collect();
    assert_eq!(asked.iter().filter(|v| **v == 2400).count(), 1);
    assert!(asked.iter().filter(|v| **v != 2400).all(Value::is_null));
    assert!(asked.len() >= 3);
    // Gone from the center while in use: it stays until switched away, then goes.
    center.accounts.borrow_mut().clear();
    sync(&mut m);
    assert_eq!(row(&m, &orbit).state, "reauth");
    assert_eq!(card(&m, &orbit)["external"]["status"], "gone");
    m.activate("lab", true).unwrap();
    assert!(native.read(&f.platform).unwrap() == renewed);
    assert!(
        f.platform
            .vault_read_optional("cloud-native-claude")
            .unwrap()
            .is_none()
    );
    assert!(m.external.config.selected.is_empty());
    m.external_tick();
    assert!(!m.db.accounts.iter().any(|a| a.id == orbit));
}

#[test]
fn a_switch_whose_lease_fails_says_so_and_changes_nothing() {
    let f = Fixture::new();
    let mut m = f.manager();
    let native = f.native();
    m.native_override = Some(native.clone());
    let lab = claude("lab", "SYNTHETIC-lab-refresh");
    native.initialize(&f.platform, &lab).unwrap();
    f.platform.vault_write("lab", &lab.encoded()).unwrap();
    m.db.accounts.push(account("lab", &lab, true));
    m.save().unwrap();
    let center = Center::new(vec![claude_entry("orbit")]);
    connect(&mut m);
    let orbit = ext(&m, "ref-orbit");
    mode(&mut m, &orbit, "switch");
    *center.down.borrow_mut() = Some(Fail::Code("needs_reauthorization".into()));
    m.activate(&orbit, true).unwrap();
    m.external_tick();
    assert!(row(&m, "lab").native);
    assert!(native.read(&f.platform).unwrap() == lab);
    assert!(
        m.notice
            .as_deref()
            .is_some_and(|n| n.starts_with("没有换上外部账号"))
    );
    assert!(m.external.follow.is_none());
    // The center's answer holds until the next read: the row is not a switch target meanwhile.
    assert_eq!(
        card(&m, &orbit)["external"]["status"],
        "needs_reauthorization"
    );
    // A rule's switch never waits: it asks for the access and tries again later.
    *center.down.borrow_mut() = None;
    m.external.config.sources[0].directory[0]["status"] = json!("ready");
    assert_eq!(
        m.activate_native(&orbit),
        Err("正在向外部来源领取这次要用的 Access，稍后自动重试")
    );
    m.external_tick();
    m.activate_native(&orbit).unwrap();
    assert!(row(&m, &orbit).native);
}

#[test]
fn switching_codex_to_an_external_row_writes_a_login_codex_loads() {
    let f = Fixture::new();
    let mut m = f.manager();
    private_dir(&m.sources.codex_home).unwrap();
    let file = m.sources.codex_home.join("auth.json");
    let local = local_codex("a");
    write_json(&file, &local.credential).unwrap();
    f.platform.vault_write("a", &local.encoded()).unwrap();
    m.db.accounts.push(account("a", &local, true));
    m.save().unwrap();
    let center = Center::new(vec![codex_entry("nova")]);
    connect(&mut m);
    let nova = ext(&m, "ref-nova");
    mode(&mut m, &nova, "switch");
    m.activate(&nova, true).unwrap();
    m.external_tick();
    let written = read_json(&file).unwrap().unwrap();
    assert_eq!(written["auth_mode"], "chatgpt");
    assert_eq!(written["tokens"]["refresh_token"], "");
    assert_eq!(
        written["tokens"]["access_token"],
        leased_token(&center, "ref-nova")
    );
    assert!(row(&m, &nova).native);
    m.activate("a", true).unwrap();
    assert_eq!(read_json(&file).unwrap().unwrap(), local.credential);
    assert!(row(&m, "a").native);
}

#[test]
fn a_refused_desktop_gets_access_leased_again_naming_the_version_it_was_refused_with() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new();
    let (mut m, _, _) = super::desktop_boundary_tests::setup(&f);
    let center = Center::new(vec![codex_entry("nova")]);
    connect(&mut m);
    let nova = ext(&m, "ref-nova");
    mode(&mut m, &nova, "switch");
    m.activate(&nova, true).unwrap();
    m.external_tick();
    finish_desktop(&mut m);
    assert_eq!(m.db.desktop.selected.as_deref(), Some(nova.as_str()));
    assert!(row(&m, &nova).native);
    assert_eq!(
        read_json(&m.sources.codex_home.join("auth.json"))
            .unwrap()
            .unwrap()["tokens"]["refresh_token"],
        ""
    );
    let digest = |token: &str| format!("{:x}", Sha256::digest(token.as_bytes()));
    let owner = "codex:nova:workspace-nova";
    fs::write(
        f.root.join("poll.json"),
        json!({"ok":true,"owner":owner,"boundOwner":owner,"mode":"chatgptAuthTokens","target":"synthetic-target",
            "instance":"synthetic-instance","hash":digest(&leased_token(&center, "ref-nova")),"requests":[{"id":7,"epoch":1,"ageMs":100}]})
        .to_string(),
    )
    .unwrap();
    center
        .leases
        .borrow_mut()
        .insert("ref-nova".into(), codex_lease("nova", 2));
    m.desktop.last_poll = 0;
    m.desktop_tick();
    finish_desktop(&mut m);
    let asked: Vec<Value> = center
        .calls
        .borrow()
        .iter()
        .filter(|(op, body)| op == "lease" && body["rejected_version"] == 1)
        .map(|(_, b)| b.clone())
        .collect();
    assert_eq!(
        asked.len(),
        1,
        "names the version the desktop was refused with"
    );
    assert_eq!(asked[0]["purpose"], "use");
    m.external_tick();
    m.desktop_tick();
    finish_desktop(&mut m);
    assert!(m.desktop.refresh.is_none());
    assert_eq!(
        fs::read_to_string(f.root.join("reply-hash")).unwrap(),
        digest(&leased_token(&center, "ref-nova"))
    );
}

#[test]
fn a_rule_may_spend_cards_of_a_switch_row_only_with_access_leased_for_it() {
    let f = Fixture::new();
    let mut m = f.manager();
    let center = Center::new(vec![codex_entry("nova")]);
    connect(&mut m);
    let nova = ext(&m, "ref-nova");
    mode(&mut m, &nova, "switch");
    m.reset_transport = Some(|_, request| Ok(codex_reading(request)));
    read_quota(&mut m);
    let trigger = crate::resets::Trigger {
        remaining_percent: 1.0,
        threshold_percent: 5,
        expiring: false,
    };
    let uses = center.leased("use");
    m.reset_quota_with_trigger(&nova, "credit-1", "op-auto", true, Some(trigger))
        .unwrap();
    m.finish_reset();
    assert_eq!(center.leased("use"), uses + 1);
    assert_eq!(
        m.db.reset_attempts[0].outcome,
        crate::resets::Outcome::Reset
    );
    // The lease set aside for the card is never kept as the row's access.
    let key = format!("{}:ref-nova", m.external.config.sources[0].id);
    assert!(!m.external.use_ready(&key));
}
