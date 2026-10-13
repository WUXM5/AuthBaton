use crate::codex_api::{Access, Request, Transport};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// RFC3339 at the API boundary. Automatic redemption requires a known future expiry.
pub fn parse_expiry(value: &str) -> Option<u64> {
    if !value.is_ascii() || value.len() < 20 {
        return None;
    }
    let number = |a: usize, b: usize| {
        let s = value.get(a..b)?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse::<i64>().ok())
            .flatten()
    };
    if value.get(4..5)? != "-"
        || value.get(7..8)? != "-"
        || value.get(10..11)? != "T"
        || value.get(13..14)? != ":"
        || value.get(16..17)? != ":"
    {
        return None;
    }
    let (year, month, day, hour, minute, second) = (
        number(0, 4)?,
        number(5, 7)?,
        number(8, 10)?,
        number(11, 13)?,
        number(14, 16)?,
        number(17, 19)?,
    );
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if year < 1970
        || !(1..=12).contains(&month)
        || day < 1
        || day > months[(month - 1) as usize]
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let mut suffix = &value[19..];
    if suffix.starts_with('.') {
        let end = suffix[1..].find(|c: char| !c.is_ascii_digit())? + 1;
        if end == 1 {
            return None;
        }
        suffix = &suffix[end..];
    }
    let offset = if suffix == "Z" {
        0
    } else {
        if suffix.len() != 6 || !matches!(&suffix[..1], "+" | "-") || &suffix[3..4] != ":" {
            return None;
        }
        if !suffix[1..3]
            .bytes()
            .chain(suffix[4..6].bytes())
            .all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let h = suffix[1..3].parse::<i64>().ok()?;
        let m = suffix[4..6].parse::<i64>().ok()?;
        if h > 23 || m > 59 {
            return None;
        }
        (h * 3600 + m * 60) * if suffix.starts_with('+') { 1 } else { -1 }
    };
    let days = (1970..year)
        .map(|y| {
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                366
            } else {
                365
            }
        })
        .sum::<i64>()
        + months[..(month - 1) as usize].iter().sum::<i64>()
        + day
        - 1;
    (days * 86400 + hour * 3600 + minute * 60 + second - offset)
        .try_into()
        .ok()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Credit {
    pub id: String,
    pub reset_type: String,
    pub status: String,
    pub expires_at: Option<String>,
}
impl Credit {
    pub fn available(&self) -> bool {
        self.reset_type == "codex_rate_limits" && self.status == "available"
    }
}
#[derive(Clone, Serialize)]
pub struct Credits {
    pub available_count: u64,
    pub details: Option<Vec<Credit>>,
}
impl Credits {
    pub fn parse(details: &Value, usage: &Value) -> Option<Self> {
        let available_count = details["available_count"]
            .as_u64()
            .or_else(|| usage["rate_limit_reset_credits"]["available_count"].as_u64())?;
        let credits = serde_json::from_value::<Vec<Credit>>(details["credits"].clone())
            .ok()
            .filter(|credits| {
                credits.iter().all(|credit| {
                    !credit.id.is_empty()
                        && credit.id.len() <= 512
                        && !credit.id.chars().any(char::is_control)
                        && credit.expires_at.as_ref().is_none_or(|expiry| {
                            expiry.len() <= 64 && !expiry.chars().any(char::is_control)
                        })
                })
            });
        Some(Self {
            available_count,
            details: credits,
        })
    }
    pub fn credit(&self, id: &str) -> Option<&Credit> {
        if self.available_count == 0 {
            return None;
        }
        self.details
            .as_ref()?
            .iter()
            .find(|credit| credit.id == id && credit.available())
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pending,
    Unknown,
    Reset,
    AlreadyRedeemed,
    NothingToReset,
    NoCredit,
    NotSent,
}
impl Outcome {
    pub fn unresolved(self) -> bool {
        matches!(self, Self::Pending | Self::Unknown)
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::Pending => "正在提交额度重置，请勿重复操作",
            Self::Unknown => "上次重置结果待确认；请重试同一次操作，不会改用另一张重置券",
            Self::Reset => "已使用 1 次额度重置；用量和剩余次数将重新查询",
            Self::AlreadyRedeemed => "已确认上次重置成功，没有重复使用次数",
            Self::NothingToReset => "官方返回当前没有可重置的额度窗口，本次未使用次数",
            Self::NoCredit => "所选重置次数已不可用，本次未使用次数",
            Self::NotSent => "未提交重置：请更新认证和额度明细后重新操作",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Trigger {
    pub remaining_percent: f64,
    pub threshold_percent: u8,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Automatic,
    Manual,
    Legacy,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Execution {
    pub source: Source,
    pub account_label: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub outcome: Outcome,
    pub trigger: Option<Trigger>,
    pub detail: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub account_id: String,
    pub owner: String,
    pub request_id: String,
    pub credit_id: String,
    pub expires_at: Option<String>,
    pub created_at: u64,
    pub outcome: Outcome,
    #[serde(default)]
    pub executions: Vec<Execution>,
}
impl Attempt {
    pub fn legacy_execution(&self, label: &str) -> Execution {
        Execution {
            source: Source::Legacy,
            account_label: label.into(),
            started_at: self.created_at,
            finished_at: None,
            outcome: self.outcome,
            trigger: None,
            detail: None,
        }
    }
    /// Claude grant IDs stay out of the UI; a retry names only the original operation.
    pub fn snapshot(&self, credit_visible: bool) -> Value {
        serde_json::json!({"request_id":self.request_id,"credit_id":credit_visible.then_some(&self.credit_id),
            "expires_at":self.expires_at,"created_at":self.created_at,"outcome":self.outcome,
            "source":self.executions.last().map(|e|e.source),"started_at":self.executions.last().map(|e|e.started_at).unwrap_or(self.created_at),
            "finished_at":self.executions.last().and_then(|e|e.finished_at),
            "message":self.outcome.message(),"unresolved":self.outcome.unresolved()})
    }
}

pub fn run(
    access: Access,
    attempt: &Attempt,
    retry: bool,
    transport: Transport,
    time: u64,
) -> Outcome {
    if access.expired(time) {
        return if retry {
            Outcome::Unknown
        } else {
            Outcome::NotSent
        };
    }
    if !retry {
        // Recheck the chosen credit before the first POST. A retry must still POST the same
        // pair when the credit is absent: the earlier request may already have redeemed it.
        let Ok(response) = transport(&access, &Request::ResetCredits) else {
            return Outcome::NotSent;
        };
        if response.status != 200 {
            return Outcome::NotSent;
        }
        let Some(credits) = Credits::parse(&response.body, &Value::Null) else {
            return Outcome::NotSent;
        };
        if credits.details.is_none() {
            return Outcome::NotSent;
        }
        if credits.credit(&attempt.credit_id).is_none() {
            return Outcome::NoCredit;
        }
    }
    let request = Request::Consume {
        request_id: attempt.request_id.clone(),
        credit_id: attempt.credit_id.clone(),
    };
    let Ok(response) = transport(&access, &request) else {
        return Outcome::Unknown;
    };
    if response.status != 200 {
        return Outcome::Unknown;
    }
    match response.body["code"].as_str() {
        Some("reset") => Outcome::Reset,
        Some("already_redeemed") => Outcome::AlreadyRedeemed,
        Some("nothing_to_reset") => Outcome::NothingToReset,
        Some("no_credit") => Outcome::NoCredit,
        _ => Outcome::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_api::Response;
    use crate::platform::Result;
    use serde_json::json;

    pub fn details() -> Value {
        json!({"available_count":3,"credits":[{"id":"credit-1","reset_type":"codex_rate_limits","status":"available","expires_at":"2026-10-01T00:00:00Z"}]})
    }
    fn attempt() -> Attempt {
        Attempt {
            account_id: "a".into(),
            owner: "owner".into(),
            request_id: "request-1".into(),
            credit_id: "credit-1".into(),
            expires_at: None,
            created_at: 1,
            outcome: Outcome::Pending,
            executions: vec![],
        }
    }
    #[test]
    fn authoritative_count_is_not_detail_length_and_unknown_is_not_zero() {
        let parsed = Credits::parse(&details(), &Value::Null).unwrap();
        assert_eq!(parsed.available_count, 3);
        assert_eq!(parsed.details.as_ref().unwrap().len(), 1);
        assert!(Credits::parse(&Value::Null, &Value::Null).is_none());
        let summary = Credits::parse(
            &Value::Null,
            &json!({"rate_limit_reset_credits":{"available_count":2}}),
        )
        .unwrap();
        assert!(summary.details.is_none());
        assert!(summary.credit("credit-1").is_none());
    }
    #[test]
    fn retry_posts_the_same_pair_without_selecting_another_credit() {
        fn replay(_: &Access, request: &Request) -> Result<Response> {
            let Request::Consume {
                request_id,
                credit_id,
            } = request
            else {
                panic!("retry must not gate on a now-missing credit")
            };
            assert_eq!(request_id, "request-1");
            assert_eq!(credit_id, "credit-1");
            Ok(Response {
                status: 200,
                body: json!({"code":"already_redeemed"}),
                retry_after: None,
            })
        }
        assert_eq!(
            run(Access::synthetic(100), &attempt(), true, replay, 10),
            Outcome::AlreadyRedeemed
        );
    }
    #[test]
    fn no_post_after_failed_preflight_but_post_failure_remains_unknown() {
        fn fail(_: &Access, _: &Request) -> Result<Response> {
            Err("offline")
        }
        assert_eq!(
            run(Access::synthetic(100), &attempt(), false, fail, 10),
            Outcome::NotSent
        );
        assert_eq!(
            run(Access::synthetic(100), &attempt(), true, fail, 10),
            Outcome::Unknown
        );
        assert_eq!(
            run(Access::synthetic(1), &attempt(), true, fail, 10),
            Outcome::Unknown
        );
    }
}
