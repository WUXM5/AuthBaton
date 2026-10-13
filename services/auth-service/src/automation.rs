//! Codex desktop policy and durable operation records. Never contains credentials.
use crate::platform::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::thread::JoinHandle;

/// What an account does once its remaining primary quota reaches its own line.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    ResetFirst,
    SwitchFirst,
    ResetOnly,
    SwitchOnly,
}
impl Order {
    pub fn resets(self) -> bool {
        self != Order::SwitchOnly
    }
    pub fn switches(self) -> bool {
        self != Order::ResetOnly
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub order: Order,
    pub participate: bool,
    /// The account is depleted once its remaining primary quota is at or below this percent.
    pub below: u8,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub accounts: Vec<Rule>,
}
impl Config {
    /// One-time storage migration from the two-threshold format (DB v7 and older; v4 and older
    /// also kept one global order). Each account keeps its earliest old trigger as its single
    /// line, and an account without automatic reset becomes switch-only, so no credit is newly
    /// authorised by the upgrade. Live IPC only accepts the current format.
    pub fn from_legacy(value: Value, version: u64) -> Result<Self> {
        #[derive(Deserialize, Clone, Copy)]
        #[serde(rename_all = "snake_case")]
        enum LegacyOrder {
            ResetFirst,
            SwitchFirst,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyRule {
            id: String,
            order: Option<LegacyOrder>,
            participate: bool,
            reset_below: Option<u8>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyConfig {
            enabled: bool,
            order: Option<LegacyOrder>,
            switch_below: u8,
            accounts: Vec<LegacyRule>,
        }
        const INVALID: &str = "旧版额度策略格式无效，未覆盖其内容";
        let old: LegacyConfig = serde_json::from_value(value).map_err(|_| INVALID)?;
        let (global, switch_below) = (old.order, old.switch_below);
        let accounts = old
            .accounts
            .into_iter()
            .map(|r| {
                let order = if version <= 4 {
                    global.filter(|_| r.order.is_none())
                } else {
                    r.order.filter(|_| global.is_none())
                }
                .ok_or(INVALID)?;
                Ok(Rule {
                    id: r.id,
                    order: match (r.reset_below, order) {
                        (None, _) => Order::SwitchOnly,
                        (Some(_), LegacyOrder::ResetFirst) => Order::ResetFirst,
                        (Some(_), LegacyOrder::SwitchFirst) => Order::SwitchFirst,
                    },
                    participate: r.participate,
                    below: r.reset_below.map_or(switch_below, |v| v.max(switch_below)),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let config = Self {
            enabled: old.enabled,
            accounts,
        };
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        if self.accounts.len() > 32 {
            return Err("最多设置 32 个账号");
        }
        for rule in &self.accounts {
            if !valid_id(&rule.id) || !seen.insert(&rule.id) || rule.below > 99 {
                return Err("账号策略包含重复账号或无效见底线（须为 0–99%）");
            }
        }
        Ok(())
    }
}
pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Observation {
    pub remaining: f64,
    pub checked_at: u64,
    pub expires_at: u64,
}
impl Observation {
    pub fn fresh(&self, time: u64) -> bool {
        self.remaining.is_finite()
            && (0.0..=100.0).contains(&self.remaining)
            && self.checked_at <= time
            && time - self.checked_at <= 300
            && self.expires_at > time
    }
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preparing,
    Pausing,
    Verifying,
    Switching,
    Resuming,
    Cancelling,
    Attention,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub thread_id: String,
    pub turn_id: String,
    pub resume_key: String,
    pub state: String,
    pub resumed_turn_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub source: String,
    pub target: String,
    pub source_owner: String,
    pub target_owner: String,
    pub instance: String,
    pub renderer: String,
    pub phase: Phase,
    pub tasks: Vec<Task>,
    pub created_at: u64,
    pub cancel_requested: bool,
}
#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum LatchKind {
    #[default]
    Reset,
    Switch,
}
/// One low-quota episode per account: a reset is spent at most once, and a switch away ends the
/// episode. Only a newer successful observation above `threshold` releases it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Latch {
    /// The reset request, or the switch batch that handled the episode.
    pub request_id: String,
    pub created_at: u64,
    pub threshold: u8,
    #[serde(default)]
    pub kind: LatchKind,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub config: Config,
    pub run: Option<Run>,
    #[serde(alias = "reset_latches")]
    pub latches: HashMap<String, Latch>,
    pub last_message: String,
    pub last_at: Option<u64>,
    pub retired_batch: Option<String>,
}
pub struct Job {
    pub stage: &'static str,
    pub handle: JoinHandle<Result<Value>>,
}
#[derive(Default)]
pub struct Runtime {
    pub job: Option<Job>,
    pub next_check: u64,
    pub message: String,
    pub observation: Option<(String, Observation)>,
    pub account_messages: HashMap<String, String>,
    pub retry_after: HashMap<String, u64>,
}

#[derive(PartialEq, Debug)]
pub enum Action {
    Reset(String),
    Switch(String),
    None,
}
pub fn decide(order: Order, reset: Option<String>, target: Option<String>) -> Action {
    let (reset, target) = (reset.map(Action::Reset), target.map(Action::Switch));
    match order {
        Order::ResetFirst => reset.or(target),
        Order::SwitchFirst => target.or(reset),
        Order::ResetOnly => reset,
        Order::SwitchOnly => target,
    }
    .unwrap_or(Action::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn precedence_and_unknown_observations() {
        assert_eq!(
            decide(Order::ResetFirst, Some("credit".into()), Some("b".into())),
            Action::Reset("credit".into())
        );
        assert_eq!(
            decide(Order::SwitchFirst, Some("credit".into()), Some("b".into())),
            Action::Switch("b".into())
        );
        assert_eq!(
            decide(Order::SwitchFirst, Some("credit".into()), None),
            Action::Reset("credit".into())
        );
        assert_eq!(
            decide(Order::ResetOnly, None, Some("b".into())),
            Action::None
        );
        assert_eq!(
            decide(Order::SwitchOnly, Some("credit".into()), None),
            Action::None
        );
        assert!(
            !Observation {
                remaining: f64::NAN,
                checked_at: 1,
                expires_at: 999
            }
            .fresh(2)
        );
        assert!(
            !Observation {
                remaining: 0.,
                checked_at: 1,
                expires_at: 999
            }
            .fresh(302)
        );
        assert!(
            !Observation {
                remaining: 0.,
                checked_at: 3,
                expires_at: 999
            }
            .fresh(2)
        );
        assert!(
            !Observation {
                remaining: 0.,
                checked_at: 1,
                expires_at: 2
            }
            .fresh(2)
        );
    }
}
