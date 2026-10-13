//! Claude Code usage-reset metadata and the official cedar_ember grant claim (Claude Code 2.1.284).
//! The UI projection never contains grant IDs; workers receive access only, never a refresh writer.
use crate::credentials::Bundle;
use crate::platform::Result;
use crate::resets::{Attempt, Outcome};
use serde::Serialize;
use serde_json::{Value, json};

pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage?at_wall=1&skip_spend=1";
// The read the official `/limit-reset` makes right before claiming outside a usage wall.
const STATUS_URL: &str = "https://api.anthropic.com/api/oauth/usage?cedar_ember=1&skip_spend=1";
// The status endpoint classifies reset eligibility by client surface and CLI version.
// Identify this adapter as well; only the reset metadata reads and the grant claim use this header.
pub const USAGE_AGENT: &str = "claude-cli/2.1.280 (external, cli, client-app/authmanager)";

#[derive(Clone, Serialize)]
pub struct Status {
    grants: Option<Grants>,
    weekly: Option<Weekly>,
    #[serde(skip)]
    next: Option<Claim>,
}
/// The grant the official client would claim next.
#[derive(Clone)]
pub struct Claim {
    pub grant_id: String,
    pub ends_at: Option<String>,
}
impl Status {
    /// Mirrors the official anytime offer: only the server's next grant, usable now, not paused,
    /// with no known expiry within 30 seconds. A grant that needs the limit waits until at it.
    pub fn claimable(&self, time: u64) -> Option<&Claim> {
        self.next.as_ref().filter(|c| {
            c.ends_at.as_deref().is_none_or(|end| {
                crate::resets::parse_expiry(end).is_some_and(|end| end > time + 30)
            })
        })
    }
}
#[derive(Clone, Serialize)]
struct Grants {
    eligible: bool,
    message: String,
    remaining: Option<u64>,
    items: Vec<Grant>,
    cooldown_until: Option<String>,
    weekly_resets_at: Option<String>,
}
#[derive(Clone, Serialize)]
struct Grant {
    label: String,
    remaining: u32,
    total: Option<u32>,
    starts_at: Option<String>,
    ends_at: Option<String>,
    limits: Vec<&'static str>,
    paused: bool,
    usable_now: Option<bool>,
    requires_limit: Option<bool>,
}
#[derive(Clone, Serialize)]
struct Weekly {
    eligible: bool,
    message: String,
    available: Option<bool>,
    per_week: Option<u32>,
    next_available_at: Option<String>,
    weekly_resets_at: Option<String>,
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.chars().count() <= 160 && !s.chars().any(char::is_control))
        .map(str::to_owned)
}
fn count(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|n| n.try_into().ok())
}
fn reason(value: &Value) -> String {
    match value.as_str() {
        Some("tier" | "seat") => "当前套餐或席位不适用",
        Some("surface" | "mobile") => "当前使用端不适用",
        Some("cli_version") => "当前客户端版本不适用",
        Some("tenure") => "账号尚未满足开放条件",
        Some("config_off" | "no_grant" | "other_experiment") => "当前账号暂未提供此类重置",
        Some("not_at_wall") => "达到会话额度上限后才可用",
        Some("weekly_limit") => "每周额度已达上限",
        Some("no_weekly_limit") => "当前账号没有对应的每周额度",
        Some("extra_usage") => "当前额外用量设置不适用",
        _ => "此类重置当前不可用",
    }
    .into()
}
fn grant(value: &Value) -> Option<Grant> {
    let remaining = count(&value["resets_left"])?;
    let mut limits = vec![];
    for key in value["clears"].as_array().into_iter().flatten().take(16) {
        let label = match key.as_str() {
            Some("five_hour") => "5 小时额度",
            Some("seven_day" | "seven_day_overage_included") => "每周额度",
            Some("seven_day_opus") => "Opus 每周额度",
            Some("seven_day_sonnet") => "Sonnet 每周额度",
            Some("seven_day_cowork") => "Cowork 每周额度",
            Some("seven_day_oauth_apps") => "OAuth 应用每周额度",
            Some("seven_day_omelette") => "其他专项每周额度",
            _ => continue,
        };
        if !limits.contains(&label) {
            limits.push(label);
        }
    }
    Some(Grant {
        label: text(&value["label"]).unwrap_or_else(|| "额度重置".into()),
        remaining,
        total: count(&value["resets_total"]),
        starts_at: text(&value["starts_at"]),
        ends_at: text(&value["ends_at"]),
        limits,
        paused: value["paused"].as_bool().unwrap_or(false),
        usable_now: value["usable_now"].as_bool(),
        requires_limit: value["use_requires_limit"].as_bool(),
    })
}
fn grants(value: &Value) -> Option<Grants> {
    let eligible = value["eligible"].as_bool()?;
    // A missing/malformed list is unknown, never a fabricated zero or a partial sum.
    let parsed = value["grants"]
        .as_array()
        .filter(|items| items.len() <= 64)
        .and_then(|items| items.iter().map(grant).collect::<Option<Vec<_>>>());
    let remaining = parsed
        .as_ref()
        .map(|items| items.iter().map(|g| u64::from(g.remaining)).sum());
    Some(Grants {
        eligible,
        message: if eligible {
            "官方提供的额度重置机会".into()
        } else {
            reason(&value["ineligible_reason"])
        },
        remaining,
        items: parsed.unwrap_or_default(),
        cooldown_until: text(&value["cooldown_until"]),
        weekly_resets_at: text(&value["weekly_resets_at"]),
    })
}
fn weekly(value: &Value) -> Option<Weekly> {
    let eligible = value["eligible"].as_bool()?;
    let reset_arm = value["arm"].as_str() == Some("reset");
    Some(Weekly {
        eligible,
        message: if eligible {
            "重置会话额度后，仍受每周额度限制".into()
        } else {
            reason(&value["ineligible_reason"])
        },
        available: value["available"]
            .as_bool()
            .map(|available| eligible && reset_arm && available),
        per_week: count(&value["resets_per_week"]),
        next_available_at: text(&value["next_available_at"]),
        weekly_resets_at: text(&value["weekly_resets_at"]),
    })
}
fn next(value: &Value) -> Option<Claim> {
    // Official grant ID format; defaults follow its schema (usable_now false, use_requires_limit true).
    let valid = |id: &str| {
        (1..=40).contains(&id.len())
            && id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
    };
    let id = value["next_grant_id"].as_str().filter(|id| valid(id))?;
    let grant = value["grants"].as_array()?.iter().find(|g| g["id"] == id)?;
    let usable = value["eligible"] == true
        && grant["usable_now"] == true
        && grant["paused"] != true
        && grant["resets_left"].as_u64().is_some_and(|n| n > 0)
        && (grant["use_requires_limit"] == false || value["at_limit"] == true);
    let ends_at = match &grant["ends_at"] {
        Value::Null => None,
        end => Some(text(end).filter(|e| crate::resets::parse_expiry(e).is_some())?),
    };
    usable.then(|| Claim {
        grant_id: id.into(),
        ends_at,
    })
}
pub fn parse(body: &Value) -> Status {
    Status {
        grants: grants(&body["cedar_ember"]),
        weekly: weekly(&body["juniper_tide"]),
        next: next(&body["cedar_ember"]),
    }
}

pub struct Access {
    token: String,
    organization: String,
    expires_at: Option<u64>,
}
impl Access {
    pub fn from_bundle(bundle: &Bundle) -> Result<Self> {
        if bundle.provider != "claude" {
            return Err("此入口仅支持 Claude");
        }
        let metadata = bundle.metadata()?;
        let token = bundle.credential["accessToken"].as_str().unwrap();
        let organization = bundle.identity["organizationUuid"].as_str().unwrap();
        if token.chars().any(char::is_control)
            || organization.len() > 64
            || !organization
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err("重置凭据或组织标识无效");
        }
        Ok(Self {
            token: token.into(),
            organization: organization.into(),
            expires_at: metadata.expires_at,
        })
    }
    #[cfg(test)]
    pub fn synthetic(expires_at: u64) -> Self {
        Self {
            token: "synthetic".into(),
            organization: "synthetic-org".into(),
            expires_at: Some(expires_at),
        }
    }
}
pub enum Request {
    Status,
    Claim {
        grant_id: String,
        request_id: String,
    },
}
pub type Transport = fn(&Access, &Request) -> Result<(u16, Value)>;

fn request_config(access: &Access, request: &Request) -> String {
    let quote = |value: &str| format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""));
    let url = match request {
        Request::Status => STATUS_URL.into(),
        Request::Claim { .. } => format!(
            "https://api.anthropic.com/api/organizations/{}/reset_rate_limits",
            access.organization
        ),
    };
    let mut config = format!(
        "url = {}\nheader = {}\nheader = \"anthropic-beta: oauth-2025-04-20\"\nheader = \"x-app: cli\"\nheader = \"Content-Type: application/json\"\n",
        quote(&url),
        quote(&format!("Authorization: Bearer {}", access.token)),
    );
    if let Request::Claim {
        grant_id,
        request_id,
    } = request
    {
        let body = json!({"program":"cedar_ember","grant_id":grant_id,"request_id":request_id});
        config.push_str(&format!(
            "request = \"POST\"\ndata = {}\n",
            quote(&body.to_string())
        ));
    }
    config
}
#[cfg(not(test))]
pub fn http(access: &Access, request: &Request) -> Result<(u16, Value)> {
    let bytes = crate::network::curl(
        &[
            "-q",
            "--silent",
            "--max-time",
            "25",
            "--max-filesize",
            "1048576",
            "--user-agent",
            USAGE_AGENT,
            "--write-out",
            "\n%{http_code}",
            "--config",
            "-",
        ],
        request_config(access, request).as_bytes(),
        std::time::Duration::from_secs(28),
    )?;
    let text = String::from_utf8(bytes).map_err(|_| "重置响应无效")?;
    let (body, status) = text.rsplit_once('\n').ok_or("重置响应无效")?;
    Ok((
        status.parse().map_err(|_| "重置响应无效")?,
        serde_json::from_str(body).unwrap_or(Value::Null),
    ))
}

/// Claims the attempt's grant with its own request_id, which the server uses to settle retries.
pub fn run(
    access: Access,
    attempt: &Attempt,
    retry: bool,
    transport: Transport,
    time: u64,
) -> Outcome {
    if access.expires_at.is_some_and(|expiry| expiry <= time) {
        return if retry {
            Outcome::Unknown
        } else {
            Outcome::NotSent
        };
    }
    if !retry {
        // Recheck the chosen grant first. A retry must claim the same pair even when the grant
        // is gone: the earlier claim may already have spent it.
        let Ok((200, body)) = transport(&access, &Request::Status) else {
            return Outcome::NotSent;
        };
        if !body["cedar_ember"].is_object() {
            return Outcome::NotSent;
        }
        if parse(&body)
            .claimable(time)
            .is_none_or(|c| c.grant_id != attempt.credit_id)
        {
            return Outcome::NoCredit;
        }
    }
    let request = Request::Claim {
        grant_id: attempt.credit_id.clone(),
        request_id: attempt.request_id.clone(),
    };
    // Result meanings follow the official client; anything it cannot settle stays unknown.
    match transport(&access, &request) {
        Ok((200, body)) => match body["result"].as_str() {
            Some("reset") => Outcome::Reset,
            // Spent by this same request only when retrying; otherwise by someone else.
            Some("already_used") if retry => Outcome::AlreadyRedeemed,
            Some("already_used" | "ineligible") => Outcome::NoCredit,
            Some("not_limited") => Outcome::NothingToReset,
            // Another reset is in flight; on a retry that may be this very claim.
            Some("cooldown") if !retry => Outcome::NotSent,
            _ => Outcome::Unknown,
        },
        // Rejected before processing; the official client keeps no unsettled claim for these.
        Ok((401 | 403 | 429, _)) if !retry => Outcome::NotSent,
        _ => Outcome::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grant_counts_dates_and_scope_are_projected_without_ids_or_event_data() {
        let status = parse(
            &json!({"cedar_ember":{"eligible":true,"next_grant_id":"PRIVATE_ID","event_props":{"identity":"PRIVATE_ID"},"grants":[
                {"id":"PRIVATE_ID","label":"Synthetic offer","resets_total":3,"resets_left":2,"ends_at":"2030-10-01T00:00:00Z","clears":["five_hour","seven_day","seven_day_overage_included"],"paused":false,"usable_now":true,"use_requires_limit":false},
                {"resets_left":1,"paused":true,"usable_now":false}
            ]}}),
        );
        let output = serde_json::to_value(status).unwrap();
        assert_eq!(output["grants"]["remaining"], 3);
        assert_eq!(
            output["grants"]["items"][0]["ends_at"],
            "2030-10-01T00:00:00Z"
        );
        assert_eq!(
            output["grants"]["items"][0]["limits"],
            json!(["5 小时额度", "每周额度"])
        );
        assert_eq!(output["grants"]["items"][1]["paused"], true);
        assert!(!output.to_string().contains("PRIVATE_ID"));
    }
    #[test]
    fn unknown_or_malformed_grants_do_not_become_zero() {
        for block in [
            json!({"eligible":true}),
            json!({"eligible":true,"grants":[{"resets_left":2},{"resets_left":-1}]}),
            json!({"eligible":true,"grants":"unexpected"}),
        ] {
            assert!(serde_json::to_value(parse(&json!({"cedar_ember":block}))).unwrap()["grants"]["remaining"].is_null());
        }
        assert_eq!(
            serde_json::to_value(parse(&json!({"cedar_ember":{"eligible":true,"grants":[]}})))
                .unwrap()["grants"]["remaining"],
            0
        );
        let absent = serde_json::to_value(parse(&json!({"five_hour":{"utilization":0}}))).unwrap();
        assert!(absent["grants"].is_null() && absent["weekly"].is_null());
    }
    #[test]
    fn weekly_allowance_is_not_a_remaining_count_and_unknown_is_preserved() {
        let output = serde_json::to_value(parse(&json!({"juniper_tide":{"eligible":true,"arm":"reset","available":false,"resets_per_week":2,"next_available_at":"2030-10-02T00:00:00Z"}}))).unwrap();
        assert_eq!(output["weekly"]["per_week"], 2);
        assert_eq!(output["weekly"]["available"], false);
        assert!(output["weekly"].get("remaining").is_none());
        assert!(serde_json::to_value(parse(&json!({"juniper_tide":{"eligible":true}}))).unwrap()["weekly"]["available"].is_null());
    }

    fn block(grant: Value) -> Value {
        json!({"cedar_ember":{"eligible":true,"at_limit":false,"next_grant_id":"grant_1","grants":[grant]}})
    }
    fn usable() -> Value {
        json!({"id":"grant_1","resets_left":1,"usable_now":true,"paused":false,"use_requires_limit":false,"ends_at":"2030-10-22T16:00:00+00:00"})
    }
    #[test]
    fn only_the_servers_next_usable_grant_is_claimable_and_its_id_stays_out_of_the_ui() {
        let time = 1_900_000_000;
        let status = parse(&block(usable()));
        assert_eq!(status.claimable(time).unwrap().grant_id, "grant_1");
        assert!(!serde_json::to_string(&status).unwrap().contains("grant_1"));
        assert!(status.claimable(1_918_915_200 - 31).is_some());
        assert!(status.claimable(1_918_915_200 - 30).is_none());
        let with = |key: &str, value: Value| {
            let mut grant = usable();
            grant[key] = value;
            parse(&block(grant)).claimable(time).is_some()
        };
        assert!(!with("usable_now", json!(false)));
        assert!(!with("paused", json!(true)));
        assert!(!with("resets_left", json!(0)));
        assert!(!with("ends_at", json!("not a date")));
        assert!(with("ends_at", Value::Null));
        // A grant that needs the limit (also the official default) waits until at it.
        assert!(!with("use_requires_limit", json!(true)));
        assert!(!with("use_requires_limit", Value::Null));
        let mut at_limit = block({
            let mut grant = usable();
            grant["use_requires_limit"] = json!(true);
            grant
        });
        at_limit["cedar_ember"]["at_limit"] = json!(true);
        assert!(parse(&at_limit).claimable(time).is_some());
        for (key, value) in [
            ("next_grant_id", json!("other")),
            ("next_grant_id", json!("Grant_1")),
            ("next_grant_id", Value::Null),
            ("eligible", json!(false)),
        ] {
            let mut body = block(usable());
            body["cedar_ember"][key] = value;
            assert!(parse(&body).claimable(time).is_none());
        }
    }
    #[test]
    fn claim_targets_the_accounts_organization_with_the_exact_grant_and_request() {
        let access = Access::synthetic(100);
        let read = request_config(&access, &Request::Status);
        assert!(read.contains(STATUS_URL));
        assert!(!read.contains("POST") && !read.contains("data ="));
        let claim = request_config(
            &access,
            &Request::Claim {
                grant_id: "grant_1".into(),
                request_id: "request-1".into(),
            },
        );
        assert!(claim.contains(
            "https://api.anthropic.com/api/organizations/synthetic-org/reset_rate_limits"
        ));
        assert!(claim.contains("request = \"POST\""));
        assert!(claim.contains(r#"\"program\":\"cedar_ember\""#));
        assert!(claim.contains(r#"\"grant_id\":\"grant_1\""#));
        assert!(claim.contains(r#"\"request_id\":\"request-1\""#));
        assert!(claim.contains("x-app: cli") && claim.contains("oauth-2025-04-20"));
    }

    fn attempt() -> Attempt {
        Attempt {
            account_id: "a".into(),
            owner: "owner".into(),
            request_id: "request-1".into(),
            credit_id: "grant_1".into(),
            expires_at: None,
            created_at: 1,
            outcome: Outcome::Pending,
            executions: vec![],
        }
    }
    fn claim_result(result: &'static str) -> Transport {
        thread_local!(static RESULT: std::cell::Cell<&'static str> = const { std::cell::Cell::new("") });
        RESULT.with(|r| r.set(result));
        |_, request| match request {
            Request::Status => Ok((200, block(usable()))),
            Request::Claim { .. } => Ok((200, json!({"result":RESULT.with(|r| r.get())}))),
        }
    }
    #[test]
    fn claim_results_follow_the_official_meaning_for_first_tries_and_retries() {
        let run = |retry, transport| {
            run(
                Access::synthetic(2_000_000_000),
                &attempt(),
                retry,
                transport,
                10,
            )
        };
        for (result, first, retried) in [
            ("reset", Outcome::Reset, Outcome::Reset),
            ("already_used", Outcome::NoCredit, Outcome::AlreadyRedeemed),
            ("ineligible", Outcome::NoCredit, Outcome::NoCredit),
            (
                "not_limited",
                Outcome::NothingToReset,
                Outcome::NothingToReset,
            ),
            ("cooldown", Outcome::NotSent, Outcome::Unknown),
            ("unavailable", Outcome::Unknown, Outcome::Unknown),
            ("unexpected", Outcome::Unknown, Outcome::Unknown),
        ] {
            assert_eq!(run(false, claim_result(result)), first, "{result}");
            assert_eq!(run(true, claim_result(result)), retried, "{result}");
        }
        for status in [401, 403, 429] {
            thread_local!(static STATUS: std::cell::Cell<u16> = const { std::cell::Cell::new(0) });
            STATUS.with(|s| s.set(status));
            let rejected: Transport = |_, request| match request {
                Request::Status => Ok((200, block(usable()))),
                Request::Claim { .. } => Ok((STATUS.with(|s| s.get()), Value::Null)),
            };
            assert_eq!(run(false, rejected), Outcome::NotSent);
            assert_eq!(run(true, rejected), Outcome::Unknown);
        }
        assert_eq!(run(false, |_, _| Err("offline")), Outcome::NotSent);
        let lost: Transport = |_, request| match request {
            Request::Status => Ok((200, block(usable()))),
            Request::Claim { .. } => Err("timeout after send"),
        };
        assert_eq!(run(false, lost), Outcome::Unknown);
        assert_eq!(
            super::run(Access::synthetic(5), &attempt(), true, lost, 10),
            Outcome::Unknown
        );
    }
    #[test]
    fn changed_grant_never_claims_but_a_retry_claims_the_same_pair_without_rechecking() {
        let changed: Transport = |_, request| match request {
            Request::Status => Ok((200, json!({"cedar_ember":{"eligible":true,"grants":[]}}))),
            Request::Claim { .. } => panic!("a grant that is no longer next must not be claimed"),
        };
        assert_eq!(
            run(Access::synthetic(100), &attempt(), false, changed, 10),
            Outcome::NoCredit
        );
        let missing: Transport = |_, request| match request {
            Request::Status => Ok((200, json!({"five_hour":{"utilization":100}}))),
            Request::Claim { .. } => panic!("an unknown grant list must not be claimed"),
        };
        assert_eq!(
            run(Access::synthetic(100), &attempt(), false, missing, 10),
            Outcome::NotSent
        );
        let replay: Transport = |_, request| {
            let Request::Claim {
                grant_id,
                request_id,
            } = request
            else {
                panic!("a retry must not gate on the grant still being offered")
            };
            assert_eq!(
                (grant_id.as_str(), request_id.as_str()),
                ("grant_1", "request-1")
            );
            Ok((200, json!({"result":"already_used"})))
        };
        assert_eq!(
            run(Access::synthetic(100), &attempt(), true, replay, 10),
            Outcome::AlreadyRedeemed
        );
        assert_eq!(
            run(Access::synthetic(10), &attempt(), false, replay, 10),
            Outcome::NotSent
        );
    }
}
