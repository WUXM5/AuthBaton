//! Read-only subscription usage. Workers receive access credentials only, never a refresh writer.
#[cfg(test)]
use crate::codex_api::Response;
use crate::codex_api::{Access, Request, Transport};
use crate::platform::Result;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::thread::JoinHandle;

pub const INTERVAL: u64 = 300;
const CACHE_TTL: u64 = 600;
/// Shortest wait for an account nearing its strategy line. Going lower risks the usage
/// endpoint's rate limit, which stops queries for 15 minutes.
const MIN_INTERVAL: u64 = 30;
/// Readings this old no longer shape the burn rate; long enough that whole-percent steps average out.
const LOOKBACK: u64 = 600;

#[derive(Clone, Default, Serialize)]
pub struct Window {
    label: String,
    used_percent: f64,
    resets_at: Option<u64>,
    resets_at_iso: Option<String>,
    scope: Option<String>,
    kind: Option<String>,
    expires: bool,
}

fn bucket_windows(value: &Value, scope: Option<String>, kind: Option<String>) -> Vec<Window> {
    let mut result = Vec::new();
    for (key, fallback) in [
        ("primary_window", "主要窗口"),
        ("secondary_window", "次要窗口"),
    ] {
        let window = &value[key];
        if window.is_null() {
            continue;
        }
        let Some(used) = window["used_percent"]
            .as_f64()
            .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
        else {
            continue;
        };
        let label = match window["limit_window_seconds"].as_u64() {
            Some(seconds) if seconds > 0 && seconds % 86400 == 0 => {
                format!("{} 天窗口", seconds / 86400)
            }
            Some(seconds) if seconds > 0 && seconds % 3600 == 0 => {
                format!("{} 小时窗口", seconds / 3600)
            }
            Some(seconds) if seconds > 0 && seconds % 60 == 0 => {
                format!("{} 分钟窗口", seconds / 60)
            }
            _ => fallback.into(),
        };
        result.push(Window {
            label,
            used_percent: used,
            resets_at: window["reset_at"].as_u64(),
            scope: scope.clone(),
            kind: kind.clone(),
            ..Window::default()
        });
    }
    result
}
fn windows(value: &Value) -> Result<Vec<Window>> {
    let mut result = bucket_windows(&value["rate_limit"], None, None);
    result.extend(bucket_windows(
        &value["code_review_rate_limit"],
        Some("代码审查".into()),
        Some("feature".into()),
    ));
    if let Some(extra) = value["additional_rate_limits"].as_array() {
        for limit in extra.iter().take(64) {
            let name =
                safe_label(&limit["limit_name"]).or_else(|| safe_label(&limit["metered_feature"]));
            let model = safe_label(&limit["normal_model_slug"]);
            if let Some(name) = name.or_else(|| model.clone()) {
                let scope = match &model {
                    Some(model) if *model != name => format!("{name} · {model}"),
                    _ => name,
                };
                result.extend(bucket_windows(
                    &limit["rate_limit"],
                    Some(scope),
                    Some(if model.is_some() { "model" } else { "feature" }.into()),
                ));
            }
        }
    }
    if result.is_empty() {
        return Err("未返回可用额度窗口");
    }
    Ok(result)
}

struct Outcome {
    state: &'static str,
    message: &'static str,
    windows: Vec<Window>,
    delay: u64,
    reset_credits: Option<crate::resets::Credits>,
    claude_resets: Option<crate::claude_resets::Status>,
}
impl Outcome {
    fn failure(state: &'static str, message: &'static str) -> Self {
        Self {
            state,
            message,
            windows: vec![],
            delay: INTERVAL,
            reset_credits: None,
            claude_resets: None,
        }
    }
}
fn safe_label(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().count() <= 80 && !s.chars().any(char::is_control))
        .map(str::to_owned)
}
fn claude_windows(value: &Value) -> Result<Vec<Window>> {
    let mut result = Vec::new();
    let mut keys = Vec::new();
    if let Some(object) = value.as_object() {
        for (key, window) in object {
            if matches!(key.as_str(), "limits" | "extra_usage") {
                continue;
            }
            let Some(used) = window["utilization"]
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
            else {
                continue;
            };
            let (label, scope, kind) = match key.as_str() {
                "five_hour" => ("5 小时窗口".into(), None, None),
                "seven_day" => ("7 天窗口".into(), None, None),
                "seven_day_opus" => ("7 天窗口".into(), Some("Opus".into()), Some("model".into())),
                "seven_day_sonnet" => (
                    "7 天窗口".into(),
                    Some("Sonnet".into()),
                    Some("model".into()),
                ),
                "seven_day_oauth_apps" => (
                    "7 天窗口".into(),
                    Some("OAuth 应用".into()),
                    Some("feature".into()),
                ),
                "cinder_cove" => (
                    "一次性额度".into(),
                    Some("Claude Code / Cowork".into()),
                    Some("feature".into()),
                ),
                _ => {
                    let Some(name) = safe_label(&Value::String(key.clone())) else {
                        continue;
                    };
                    ("独立窗口".into(), Some(name), Some("feature".into()))
                }
            };
            keys.push(key.clone());
            result.push(Window {
                label,
                used_percent: used,
                resets_at: window["resets_at"].as_str().and_then(iso_time),
                resets_at_iso: safe_label(&window["resets_at"]),
                scope,
                kind,
                expires: key == "cinder_cove",
            });
        }
    }
    let mut seen = std::collections::HashSet::new();
    if let Some(limits) = value["limits"].as_array() {
        for limit in limits.iter().take(64) {
            let Some(used) = limit["percent"]
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.0)
            else {
                continue;
            };
            let model = safe_label(&limit["scope"]["model"]["display_name"]);
            let surface = safe_label(&limit["scope"]["surface"]["display_name"]);
            let scope = [model.clone(), surface.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            if scope.is_empty() {
                continue;
            }
            let Some(group) = safe_label(&limit["group"]) else {
                continue;
            };
            if !seen.insert((group.clone(), scope.clone())) {
                continue;
            }
            let label = match group.as_str() {
                "weekly" => "7 天窗口".into(),
                "daily" => "1 天窗口".into(),
                _ => group.clone(),
            };
            let window = Window {
                label,
                used_percent: used,
                resets_at_iso: safe_label(&limit["resets_at"]),
                scope: Some(scope),
                kind: Some(
                    if surface.is_some() {
                        "feature"
                    } else {
                        "model"
                    }
                    .into(),
                ),
                ..Window::default()
            };
            // A scoped model week supersedes its legacy alias; model+surface is a separate limit.
            let legacy = model
                .as_ref()
                .map(|m| format!("seven_day_{}", m.to_ascii_lowercase()));
            if group == "weekly"
                && surface.is_none()
                && let Some(index) = legacy.and_then(|k| keys.iter().position(|v| *v == k))
            {
                result[index] = window;
            } else {
                result.push(window);
            }
        }
    }
    if value["extra_usage"]["is_enabled"] == true
        && let Some(used) = value["extra_usage"]["utilization"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
    {
        result.push(Window {
            label: "月度额外用量".into(),
            scope: Some("额外消费".into()),
            kind: Some("feature".into()),
            used_percent: used,
            ..Window::default()
        });
    }
    if result.is_empty() {
        Err("未返回可用额度窗口")
    } else {
        Ok(result)
    }
}

pub struct ClaudeAccess {
    pub(crate) token: String,
    pub(crate) expires_at: Option<u64>,
}
impl ClaudeAccess {
    pub fn from_bundle(bundle: &crate::credentials::Bundle) -> Result<Self> {
        if bundle.provider != "claude" {
            return Err("此入口仅支持 Claude");
        }
        let metadata = bundle.metadata()?;
        let token = bundle.credential["accessToken"].as_str().unwrap();
        if token.chars().any(char::is_control) {
            return Err("额度查询凭据包含无效字符");
        }
        Ok(Self {
            token: token.into(),
            expires_at: metadata.expires_at,
        })
    }
}
fn query_claude(
    access: ClaudeAccess,
    transport: crate::maintenance::Transport,
    time: u64,
) -> Outcome {
    if access.expires_at.is_some_and(|expiry| expiry <= time) {
        return Outcome::failure(
            "expired",
            "Access 已到期，等待认证维护更新后重查；额度查询不会续期",
        );
    }
    match transport(
        crate::claude_resets::USAGE_URL,
        Some(&access.token),
        None,
        true,
    ) {
        Ok((200, body)) => match claude_windows(&body) {
            Ok(windows) => Outcome {
                state: "ready",
                message: "额度已检查",
                windows,
                delay: INTERVAL,
                reset_credits: None,
                claude_resets: Some(crate::claude_resets::parse(&body)),
            },
            Err(_) => Outcome::failure("unavailable", "未返回有效额度；未将缺失窗口解释为零用量"),
        },
        Ok((401, _)) => Outcome::failure(
            "unauthorized",
            "额度查询未接受现有凭据；请检查认证，本次查询未续期",
        ),
        Ok((403, _)) => Outcome::failure("forbidden", "额度接口无访问权限；未将登录判为失效"),
        Ok((429, _)) => Outcome {
            delay: 900,
            ..Outcome::failure("rate_limited", "额度查询受到限流，稍后自动重试")
        },
        _ => Outcome::failure("retry", "额度服务暂不可用，稍后重试"),
    }
}
fn query(access: Access, transport: Transport, time: u64) -> Outcome {
    if access.expired(time) {
        return Outcome::failure(
            "expired",
            "Access 已到期，等待认证维护更新后重查；额度查询不会续期",
        );
    }
    match transport(&access, &Request::Usage) {
        Ok(response) => match response.status {
            200 => {
                let details = transport(&access, &Request::ResetCredits)
                    .ok()
                    .filter(|result| result.status == 200)
                    .map(|result| result.body)
                    .unwrap_or(Value::Null);
                let reset_credits = crate::resets::Credits::parse(&details, &response.body);
                match windows(&response.body) {
                    Ok(windows) => Outcome {
                        state: "ready",
                        message: "额度已更新",
                        windows,
                        delay: INTERVAL,
                        reset_credits,
                        claude_resets: None,
                    },
                    Err(_) => Outcome::failure(
                        "unavailable",
                        "额度响应缺少有效窗口，未将未知额度当作可用",
                    ),
                }
            }
            401 => Outcome::failure(
                "unauthorized",
                "额度接口未接受现有凭据；等待认证更新，未将账号判为失效",
            ),
            403 => Outcome::failure("forbidden", "额度接口暂不允许访问，未将账号判为失效"),
            429 => Outcome {
                delay: response.retry_after.unwrap_or(900).max(900),
                ..Outcome::failure("rate_limited", "额度查询受到限流，稍后自动重试")
            },
            _ => Outcome::failure("retry", "额度服务暂不可用，稍后自动重试"),
        },
        Err(_) => Outcome::failure("retry", "额度查询网络失败或超时，稍后自动重试"),
    }
}

#[derive(Clone, Serialize)]
pub struct Snapshot {
    state: &'static str,
    message: &'static str,
    windows: Vec<Window>,
    checked_at: Option<u64>,
    attempted_at: Option<u64>,
    next_update: u64,
    refreshing: bool,
    /// Asked for by the user and waiting for the scheduler; reported as refreshing so the UI
    /// does not fall back to idle between the request and the job start.
    #[serde(skip)]
    queued: bool,
    /// The line the strategy resets this account at, taken when its query starts.
    #[serde(skip)]
    line: Option<f64>,
    /// Successful readings (time, remaining %) of the last `LOOKBACK` seconds while quota fell.
    #[serde(skip)]
    readings: Vec<(u64, f64)>,
    reset_credits: Option<crate::resets::Credits>,
    claude_resets: Option<crate::claude_resets::Status>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: "pending",
            message: "等待首次额度查询",
            windows: vec![],
            checked_at: None,
            attempted_at: None,
            next_update: 0,
            refreshing: false,
            queued: false,
            line: None,
            readings: vec![],
            reset_credits: None,
            claude_resets: None,
        }
    }
}
impl Snapshot {
    /// Wait after a successful reading. An account the strategy resets is looked at sooner as
    /// it nears its line, so the line is not crossed unseen between queries: the wait is half
    /// the time the recent burn rate predicts to the line, kept within 30–300 seconds.
    fn pace(&mut self, time: u64) -> u64 {
        let Some(remaining) = remaining(&self.windows) else {
            self.readings.clear();
            return INTERVAL;
        };
        // Quota rose (a reset or a new window): older readings describe another burn.
        if self
            .readings
            .last()
            .is_some_and(|&(_, last)| remaining > last)
        {
            self.readings.clear();
        }
        self.readings
            .retain(|&(at, _)| time.saturating_sub(at) <= LOOKBACK);
        self.readings.push((time, remaining));
        let (Some(line), &(since, first)) = (self.line, &self.readings[0]) else {
            return INTERVAL;
        };
        if since >= time || remaining <= line || first <= remaining {
            return INTERVAL;
        }
        let rate = (first - remaining) / (time - since) as f64;
        ((remaining - line) / rate / 2.0).clamp(MIN_INTERVAL as f64, INTERVAL as f64) as u64
    }
}
/// Remaining percent of the tightest core window, as the strategy reads it.
fn remaining(windows: &[Window]) -> Option<f64> {
    windows
        .iter()
        .filter(|w| w.scope.is_none() && w.kind.is_none())
        .map(|w| (100.0 - w.used_percent).clamp(0.0, 100.0))
        .reduce(f64::min)
}
#[derive(Default)]
pub struct Runtime {
    entries: HashMap<String, Snapshot>,
    job: Option<(String, JoinHandle<Outcome>)>,
    #[cfg(test)]
    pub transport: Option<Transport>,
    #[cfg(test)]
    pub claude_transport: Option<crate::maintenance::Transport>,
}
impl Runtime {
    pub fn automation_observation(
        &self,
        id: &str,
        time: u64,
    ) -> Option<crate::automation::Observation> {
        let entry = self.entries.get(id)?;
        if entry.state != "ready" {
            return None;
        }
        let core: Vec<_> = entry
            .windows
            .iter()
            .filter(|w| w.scope.is_none() && w.kind.is_none())
            .collect();
        if core.is_empty() {
            return None;
        }
        let checked_at = entry.checked_at?;
        // Claude reports no reset time for an idle window and does not cap utilization at 100.
        let observation = crate::automation::Observation {
            remaining: remaining(&entry.windows)?,
            checked_at,
            expires_at: core
                .iter()
                .filter_map(|w| w.resets_at)
                .min()
                .unwrap_or(checked_at + INTERVAL),
        };
        observation.fresh(time).then_some(observation)
    }
    pub fn automatic_credit(&self, id: &str, time: u64) -> Option<String> {
        let entry = self.entries.get(id)?;
        if entry.state != "ready"
            || entry.refreshing
            || entry
                .checked_at
                .is_none_or(|t| t > time || time - t > INTERVAL)
        {
            return None;
        }
        if let Some(status) = &entry.claude_resets {
            return status.claimable(time).map(|c| c.grant_id.clone());
        }
        let credits = entry.reset_credits.as_ref()?;
        if credits.available_count == 0 {
            return None;
        }
        credits
            .details
            .as_ref()?
            .iter()
            .filter(|c| c.available())
            .filter_map(|c| {
                let expiry = crate::resets::parse_expiry(c.expires_at.as_deref()?)?;
                (expiry > time + 30).then_some((expiry, c.id.clone()))
            })
            .min_by_key(|c| c.0)
            .map(|c| c.1)
    }
    /// `schedulable`: whether the scheduler would pick this account now. A request for one it
    /// skips stays queued but is not shown as running.
    pub fn snapshot(&self, id: &str, time: u64, schedulable: bool) -> Snapshot {
        let mut snapshot = self.entries.get(id).cloned().unwrap_or_default();
        let stale = snapshot
            .checked_at
            .is_some_and(|checked| time.saturating_sub(checked) > CACHE_TTL);
        if !matches!(snapshot.state, "ready" | "retry" | "rate_limited") || stale {
            snapshot.windows.clear();
            snapshot.reset_credits = None;
            snapshot.claude_resets = None;
        }
        if stale && snapshot.state == "ready" {
            snapshot.state = "stale";
            snapshot.message = "额度缓存已过期，等待重新查询";
        }
        snapshot.refreshing |= snapshot.queued && schedulable;
        snapshot
    }
    pub fn request(&mut self, id: &str, time: u64) {
        let entry = self.entries.entry(id.into()).or_default();
        if entry.refreshing {
            // The query in flight may predate what the caller just changed; run once more after.
            entry.queued = true;
        } else if entry.state != "rate_limited" {
            entry.next_update = entry
                .attempted_at
                .map_or(time, |last| time.max(last.saturating_add(10)));
            entry.queued = true;
        }
    }
    /// Expiry of a Codex credit or Claude grant that a fresh successful query still offers.
    pub fn credit(&self, id: &str, credit_id: &str, time: u64) -> Option<Option<String>> {
        let entry = self.entries.get(id)?;
        if entry.state != "ready"
            || entry.refreshing
            || entry
                .checked_at
                .is_none_or(|checked| time.saturating_sub(checked) > INTERVAL)
        {
            return None;
        }
        if let Some(status) = &entry.claude_resets {
            let claim = status.claimable(time)?;
            return (claim.grant_id == credit_id).then(|| claim.ends_at.clone());
        }
        let credit = entry.reset_credits.as_ref()?.credit(credit_id)?;
        Some(credit.expires_at.clone())
    }
    pub fn invalidate(&mut self, id: &str) {
        self.entries.remove(id);
    }
    pub fn due(&self, id: &str, time: u64) -> bool {
        self.job.is_none()
            && self
                .entries
                .get(id)
                .is_none_or(|entry| entry.next_update <= time)
    }
    pub fn is_running(&self, id: &str) -> bool {
        self.job.as_ref().is_some_and(|(account, _)| account == id)
    }
    pub fn forget(&mut self, id: &str) {
        self.entries.remove(id);
    }
    pub fn start(&mut self, id: &str, access: Result<Access>, line: Option<f64>, time: u64) {
        let entry = self.entries.entry(id.into()).or_default();
        entry.attempted_at = Some(time);
        entry.queued = false;
        entry.line = line;
        let access = match access {
            Ok(access) => access,
            Err(_) => {
                self.complete(
                    id,
                    Outcome::failure("unavailable", "无法读取匹配账号的凭据，请核对本地认证状态"),
                    time,
                );
                return;
            }
        };
        entry.refreshing = true;
        #[cfg(test)]
        let transport = self
            .transport
            .unwrap_or(|_, _| Err("test transport not configured"));
        #[cfg(not(test))]
        let transport = crate::codex_api::http;
        self.job = Some((
            id.into(),
            std::thread::spawn(move || query(access, transport, time)),
        ));
    }
    pub fn start_claude(
        &mut self,
        id: &str,
        access: Result<ClaudeAccess>,
        line: Option<f64>,
        time: u64,
    ) {
        let entry = self.entries.entry(id.into()).or_default();
        entry.attempted_at = Some(time);
        entry.queued = false;
        entry.line = line;
        let access = match access {
            Ok(access) => access,
            Err(_) => {
                self.complete(
                    id,
                    Outcome::failure("unavailable", "无法读取匹配账号的凭据，请核对本地认证状态"),
                    time,
                );
                return;
            }
        };
        entry.refreshing = true;
        #[cfg(test)]
        let transport = self
            .claude_transport
            .unwrap_or(|_, _, _, _| Err("test transport not configured"));
        #[cfg(not(test))]
        let transport = crate::maintenance::http;
        self.job = Some((
            id.into(),
            std::thread::spawn(move || query_claude(access, transport, time)),
        ));
    }
    fn complete(&mut self, id: &str, outcome: Outcome, time: u64) {
        let entry = self.entries.entry(id.into()).or_default();
        if outcome.state == "ready" {
            entry.checked_at = Some(time);
            entry.windows = outcome.windows;
            entry.reset_credits = outcome.reset_credits;
            entry.claude_resets = outcome.claude_resets;
        }
        entry.state = outcome.state;
        entry.message = outcome.message;
        let delay = if entry.state == "ready" {
            entry.pace(time)
        } else {
            outcome.delay
        };
        entry.next_update = time.saturating_add(delay);
        if entry.queued && entry.state != "rate_limited" {
            entry.next_update = entry
                .attempted_at
                .map_or(time, |last| time.max(last.saturating_add(10)));
        } else {
            entry.queued = false;
        }
        entry.refreshing = false;
    }
    pub fn poll(&mut self, time: u64) {
        if self.job.as_ref().is_some_and(|(_, job)| job.is_finished()) {
            self.finish(time);
        }
    }
    fn finish(&mut self, time: u64) {
        if let Some((id, job)) = self.job.take() {
            let outcome = job
                .join()
                .unwrap_or_else(|_| Outcome::failure("retry", "额度查询中断，稍后重试"));
            self.complete(&id, outcome, time);
        }
    }
    pub fn stop(&mut self, time: u64) {
        self.finish(time);
    }
    /// The last successful reading of a Claude account, as warm-up reads it.
    pub fn warmup_reading(&self, id: &str) -> Option<crate::warmup::Reading> {
        let entry = self.entries.get(id)?;
        let core = |label: &str| {
            entry
                .windows
                .iter()
                .find(|w| w.label == label && w.scope.is_none() && w.kind.is_none())
        };
        Some(crate::warmup::Reading {
            checked_at: entry.checked_at?,
            resets_at: core("5 小时窗口").and_then(|w| w.resets_at),
            weekly: core("7 天窗口").map(|w| (100.0 - w.used_percent).clamp(0.0, 100.0)),
        })
    }
    /// When the last query of this account started, successful or not.
    pub fn attempted(&self, id: &str) -> Option<u64> {
        self.entries.get(id)?.attempted_at
    }
    /// A successful Claude reading taken at `time`, without a query: the five-hour window resets
    /// at `resets_at` and `weekly_used` percent of the seven-day window is used.
    #[cfg(test)]
    pub fn record_claude(&mut self, id: &str, resets_at: Option<u64>, weekly_used: f64, time: u64) {
        let window = |label: &str, used_percent, resets_at| Window {
            label: label.into(),
            used_percent,
            resets_at,
            ..Default::default()
        };
        let outcome = Outcome {
            state: "ready",
            message: "额度已检查",
            windows: vec![
                window("5 小时窗口", 0.0, resets_at),
                window("7 天窗口", weekly_used, None),
            ],
            delay: INTERVAL,
            reset_credits: None,
            claude_resets: None,
        };
        self.complete(id, outcome, time);
    }
    #[cfg(test)]
    pub fn line(&self, id: &str) -> Option<f64> {
        self.entries.get(id)?.line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn independent_codex_model_and_feature_windows_keep_their_own_reset_times() {
        let parsed = windows(&json!({"rate_limit":{"primary_window":{"used_percent":20,"reset_at":11}},"code_review_rate_limit":{"primary_window":{"used_percent":40,"reset_at":22}},"additional_rate_limits":[{"limit_name":"Fast model","metered_feature":"fast","normal_model_slug":"model-example","rate_limit":{"primary_window":{"used_percent":70,"reset_at":33}}}]})).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].scope, None);
        assert_eq!(parsed[1].scope.as_deref(), Some("代码审查"));
        assert_eq!(parsed[1].resets_at, Some(22));
        assert_eq!(parsed[2].kind.as_deref(), Some("model"));
        assert_eq!(parsed[2].used_percent, 70.0);
        assert_eq!(parsed[2].resets_at, Some(33));
    }
    #[test]
    fn claude_scoped_models_override_legacy_but_surface_limits_stay_separate() {
        let parsed=claude_windows(&json!({"five_hour":{"utilization":10,"resets_at":"2026-09-23T12:00:00Z"},"seven_day_opus":{"utilization":2},"seven_day_sonnet":null,"cinder_cove":{"utilization":60,"resets_at":"2026-10-01T00:00:00Z"},"limits":[{"kind":"weekly_scoped","group":"weekly","scope":{"model":{"display_name":"Opus"}},"percent":80,"resets_at":"2026-09-24T12:00:00Z"},{"kind":"weekly_scoped","group":"weekly","scope":{"model":{"display_name":"Opus"},"surface":{"display_name":"Code"}},"percent":90},{"kind":"weekly_scoped","group":"weekly","scope":{"model":{"display_name":"NewModel"}},"percent":0,"is_active":false}]})).unwrap();
        assert_eq!(parsed.len(), 5);
        assert_eq!(
            parsed
                .iter()
                .find(|w| w.scope.as_deref() == Some("Opus"))
                .unwrap()
                .used_percent,
            80.0
        );
        assert_eq!(
            parsed
                .iter()
                .filter(|w| w.scope.as_deref() == Some("Opus"))
                .count(),
            1
        );
        assert!(
            parsed
                .iter()
                .any(|w| w.scope.as_deref() == Some("Opus · Code") && w.used_percent == 90.0)
        );
        assert!(
            parsed
                .iter()
                .any(|w| w.expires && w.resets_at_iso.as_deref() == Some("2026-10-01T00:00:00Z"))
        );
        assert!(
            parsed
                .iter()
                .any(|w| w.scope.as_deref() == Some("NewModel") && w.used_percent == 0.0)
        );
        assert!(claude_windows(&json!({"seven_day":null})).is_err());
    }
    #[test]
    fn warmup_reads_the_five_hour_reset_time_and_the_seven_day_remainder() {
        let mut runtime = Runtime::default();
        let reading = |runtime: &mut Runtime, body: Value| {
            let outcome = Outcome {
                state: "ready",
                message: "额度已检查",
                windows: claude_windows(&body).unwrap(),
                delay: INTERVAL,
                reset_credits: None,
                claude_resets: None,
            };
            runtime.complete("a", outcome, 100);
            runtime.warmup_reading("a").unwrap()
        };
        let idle = reading(
            &mut runtime,
            json!({"five_hour":{"utilization":0,"resets_at":null},"seven_day":{"utilization":97,"resets_at":"2026-10-20T00:00:00Z"},"seven_day_opus":{"utilization":10}}),
        );
        assert_eq!(
            (idle.checked_at, idle.resets_at, idle.weekly),
            (100, None, Some(3.0))
        );
        let running = reading(
            &mut runtime,
            json!({"five_hour":{"utilization":4,"resets_at":"2026-10-17T05:00:00.123+00:00"},"seven_day_sonnet":{"utilization":120}}),
        );
        assert_eq!(running.resets_at, iso_time("2026-10-17T05:00:00Z"));
        assert!(running.resets_at.is_some() && running.weekly.is_none());
    }
    #[test]
    fn independent_claude_query_only_uses_get_access_and_hides_auth_errors() {
        fn read_only(
            url: &str,
            token: Option<&str>,
            body: Option<&Value>,
            usage: bool,
        ) -> Result<(u16, Value)> {
            assert_eq!(url, crate::claude_resets::USAGE_URL);
            assert_eq!(token, Some("SYNTHETIC_ACCESS"));
            assert!(body.is_none());
            assert!(usage);
            Ok((401, json!({"secret":"SYNTHETIC_DO_NOT_EXPOSE"})))
        }
        let outcome = query_claude(
            ClaudeAccess {
                token: "SYNTHETIC_ACCESS".into(),
                expires_at: Some(100),
            },
            read_only,
            99,
        );
        assert_eq!(outcome.state, "unauthorized");
        assert!(!outcome.message.contains("SYNTHETIC"));
        let outcome = query_claude(
            ClaudeAccess {
                token: "SYNTHETIC_ACCESS".into(),
                expires_at: Some(100),
            },
            |_, _, _, _| panic!("expired token must not call network"),
            100,
        );
        assert_eq!(outcome.state, "expired");
    }

    #[test]
    fn claude_reset_cache_expires_and_does_not_survive_auth_rejection_or_absent_fields() {
        let success = || Outcome {
            state: "ready",
            message: "synthetic",
            windows: vec![Window::default()],
            delay: INTERVAL,
            reset_credits: None,
            claude_resets: Some(crate::claude_resets::parse(
                &json!({"cedar_ember":{"eligible":true,"grants":[{"resets_left":2}]}}),
            )),
        };
        let mut runtime = Runtime::default();
        runtime.complete("a", success(), 100);
        assert!(runtime.snapshot("a", 100, true).claude_resets.is_some());
        assert!(
            runtime
                .snapshot("a", 100 + CACHE_TTL + 1, true)
                .claude_resets
                .is_none()
        );
        runtime.complete("a", Outcome::failure("unauthorized", "synthetic"), 200);
        assert!(runtime.snapshot("a", 200, true).claude_resets.is_none());
        runtime.complete(
            "a",
            Outcome {
                claude_resets: Some(crate::claude_resets::parse(&json!({}))),
                ..success()
            },
            300,
        );
        assert!(
            serde_json::to_value(runtime.snapshot("a", 300, true)).unwrap()["claude_resets"]["grants"]
                .is_null()
        );
    }

    fn access() -> Access {
        Access::synthetic(100)
    }
    fn rejected(_: &Access, _: &Request) -> Result<Response> {
        Ok(Response {
            status: 401,
            body: json!({"secret":"do-not-expose"}),
            retry_after: None,
        })
    }
    fn limited(_: &Access, _: &Request) -> Result<Response> {
        Ok(Response {
            status: 429,
            body: Value::Null,
            retry_after: Some(1800),
        })
    }

    #[test]
    fn expired_access_never_calls_transport_and_401_never_refreshes() {
        fn never(_: &Access, _: &Request) -> Result<Response> {
            panic!("expired access must not make an HTTP request")
        }
        assert_eq!(query(access(), never, 100).state, "expired");
        let result = query(access(), rejected, 99);
        assert_eq!(result.state, "unauthorized");
        assert!(!result.message.contains("do-not-expose"));
    }
    #[test]
    fn windows_keep_real_durations_and_unknown_is_not_full_quota() {
        let value = json!({"rate_limit":{"primary_window":{"used_percent":25,"limit_window_seconds":18000,"reset_at":123},"secondary_window":{"used_percent":100,"limit_window_seconds":2592000}}});
        let parsed = windows(&value).unwrap();
        assert_eq!(parsed[0].label, "5 小时窗口");
        assert_eq!(parsed[0].resets_at, Some(123));
        assert_eq!(parsed[1].label, "30 天窗口");
        assert!(windows(&Value::Null).is_err());
        assert!(windows(&json!({"rate_limit":{"primary_window":{"used_percent":-1}}})).is_err());
    }
    #[test]
    fn failure_cache_is_bounded_scoped_and_manual_refresh_respects_rate_limit() {
        let mut runtime = Runtime::default();
        runtime.complete(
            "a",
            Outcome {
                state: "ready",
                message: "ok",
                reset_credits: None,
                claude_resets: None,
                windows: vec![Window {
                    label: "test".into(),
                    used_percent: 25.0,
                    resets_at: None,
                    ..Window::default()
                }],
                delay: INTERVAL,
            },
            100,
        );
        runtime.complete("a", Outcome::failure("retry", "unavailable"), 110);
        assert_eq!(runtime.snapshot("a", 700, true).windows.len(), 1);
        assert!(runtime.snapshot("a", 701, true).windows.is_empty());
        assert!(runtime.snapshot("b", 100, true).windows.is_empty());
        runtime.complete("a", query(access(), limited, 1), 120);
        runtime.request("a", 121);
        assert!(!runtime.due("a", 1919));
        assert!(runtime.due("a", 1920));
        runtime.complete("a", query(access(), rejected, 1), 122);
        assert!(runtime.snapshot("a", 122, true).windows.is_empty());
    }

    #[test]
    fn a_request_during_a_query_reruns_it_and_a_skipped_account_does_not_read_as_running() {
        let mut runtime = Runtime::default();
        runtime.request("a", 100);
        assert!(runtime.snapshot("a", 100, true).refreshing);
        // Queued for an account the scheduler skips for now: kept, but not shown as running.
        assert!(!runtime.snapshot("a", 100, false).refreshing);
        // The scheduler starts the query; another request arrives before its result.
        let entry = runtime.entries.get_mut("a").unwrap();
        (entry.attempted_at, entry.queued, entry.refreshing) = (Some(100), false, true);
        runtime.request("a", 103);
        runtime.complete("a", Outcome::failure("retry", "offline"), 105);
        assert!(runtime.snapshot("a", 105, true).refreshing);
        assert!(!runtime.due("a", 109));
        assert!(runtime.due("a", 110));
        // Without a new request the result settles; a rate limit drops a pending rerun.
        let entry = runtime.entries.get_mut("a").unwrap();
        (entry.queued, entry.refreshing) = (false, true);
        runtime.complete("a", Outcome::failure("retry", "offline"), 120);
        assert!(!runtime.snapshot("a", 120, true).refreshing);
        let entry = runtime.entries.get_mut("a").unwrap();
        (entry.queued, entry.refreshing) = (true, true);
        let limited = Outcome {
            delay: 900,
            ..Outcome::failure("rate_limited", "slow down")
        };
        runtime.complete("a", limited, 130);
        assert!(!runtime.snapshot("a", 130, true).refreshing);
        assert!(!runtime.due("a", 1029));
    }

    fn reading(used: f64) -> Outcome {
        Outcome {
            state: "ready",
            message: "synthetic",
            windows: vec![Window {
                used_percent: used,
                ..Window::default()
            }],
            delay: INTERVAL,
            reset_credits: None,
            claude_resets: None,
        }
    }

    /// Queries one steady burn of whole-percent readings until one is at or below the line (or
    /// two hours pass); returns the waits and the remaining percent of the last reading.
    fn burn(line: Option<f64>, per_minute: f64) -> (Vec<u64>, f64) {
        let mut runtime = Runtime::default();
        let (mut time, mut waits) = (1000, vec![]);
        loop {
            runtime.entries.entry("a".into()).or_default().line = line;
            let used = ((time - 1000) as f64 * per_minute / 60.0)
                .floor()
                .min(100.0);
            runtime.complete("a", reading(used), time);
            if line.is_some_and(|l| 100.0 - used <= l) || time > 1000 + 7200 {
                return (waits, 100.0 - used);
            }
            let next = runtime.entries["a"].next_update;
            waits.push(next - time);
            time = next;
        }
    }

    #[test]
    fn an_account_nearing_its_strategy_line_is_queried_sooner() {
        // A Codex Team account on 2026-10-13: about 3.45 % a minute against a 5 % line. Every 5 minutes
        // burns 17 %, so the plain interval mostly crosses the line unseen.
        let (waits, last) = burn(Some(5.0), 3.45);
        assert!(last > 0.0 && last <= 5.0, "{waits:?} ended at {last}");
        assert_eq!(waits[..4], [INTERVAL; 4]);
        assert!(
            waits.iter().all(|w| (MIN_INTERVAL..=INTERVAL).contains(w)),
            "{waits:?}"
        );
        assert!(waits.windows(2).all(|p| p[1] <= p[0]), "{waits:?}");
        assert!(waits.len() <= 10, "{waits:?}");
        // Without a line, or burning slowly, nothing changes.
        assert!(burn(None, 3.45).0.iter().all(|w| *w == INTERVAL));
        assert!(burn(Some(5.0), 0.05).0.iter().all(|w| *w == INTERVAL));
    }

    #[test]
    fn pacing_keeps_failure_waits_and_restarts_when_quota_rises() {
        let mut runtime = Runtime::default();
        runtime.entries.entry("a".into()).or_default().line = Some(5.0);
        let next = |runtime: &Runtime| runtime.entries["a"].next_update;
        runtime.complete("a", reading(70.0), 1000);
        assert_eq!(next(&runtime), 1000 + INTERVAL);
        // A failure keeps its own wait and is not a reading.
        runtime.complete("a", Outcome::failure("retry", "offline"), 1100);
        assert_eq!(next(&runtime), 1100 + INTERVAL);
        let limited = Outcome {
            delay: 900,
            ..Outcome::failure("rate_limited", "synthetic")
        };
        runtime.complete("a", limited, 1200);
        assert_eq!(next(&runtime), 1200 + 900);
        // 20 % in 450 s, 5 % above the line: half of 112.5 s.
        runtime.complete("a", reading(90.0), 1450);
        assert_eq!(next(&runtime), 1450 + 56);
        // Quota rose (a reset or a new window): the earlier burn does not carry over.
        runtime.complete("a", reading(50.0), 1500);
        assert_eq!(next(&runtime), 1500 + INTERVAL);
        // 5 % in 30 s, 40 % above the line: half of 240 s.
        runtime.complete("a", reading(55.0), 1530);
        assert_eq!(next(&runtime), 1530 + 120);
        // At or below the line the strategy acts; there is nothing left to predict.
        runtime.complete("a", reading(96.0), 1560);
        assert_eq!(next(&runtime), 1560 + INTERVAL);
        // A reading older than the lookback does not shape the rate.
        let mut runtime = Runtime::default();
        runtime.entries.entry("a".into()).or_default().line = Some(5.0);
        runtime.complete("a", reading(10.0), 1000);
        runtime.complete("a", reading(70.0), 1000 + LOOKBACK + 1);
        assert_eq!(next(&runtime), 1000 + LOOKBACK + 1 + INTERVAL);
    }

    #[test]
    fn each_account_is_due_after_five_minutes_and_manual_requests_are_throttled() {
        let mut runtime = Runtime::default();
        runtime.complete("a", Outcome::failure("retry", "offline"), 100);
        runtime.complete("b", Outcome::failure("retry", "offline"), 110);
        assert!(!runtime.due("a", 399));
        assert!(runtime.due("a", 400));
        assert!(!runtime.due("b", 400));
        assert!(runtime.due("b", 410));
        let entry = runtime.entries.get_mut("a").unwrap();
        entry.attempted_at = Some(500);
        runtime.request("a", 501);
        assert!(!runtime.due("a", 509));
        assert!(runtime.due("a", 510));
    }
}

// Normalize the provider's RFC3339 boundary into the same expiry used for Codex
// policy decisions. Keep the original ISO field for existing UI consumers.
pub(crate) fn iso_time(input: &str) -> Option<u64> {
    if !input.is_ascii() || input.len() < 20 || input.len() > 40 {
        return None;
    }
    let mut suffix = &input[19..];
    if let Some(fraction) = suffix.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        suffix = &fraction[digits..];
    }
    let offset = if suffix == "Z" {
        0i64
    } else {
        if suffix.len() != 6 || !matches!(&suffix[..1], "+" | "-") || &suffix[3..4] != ":" {
            return None;
        }
        let hours: i64 = suffix[1..3].parse().ok()?;
        let minutes: i64 = suffix[4..6].parse().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        (hours * 3600 + minutes * 60) * if suffix.starts_with('-') { -1 } else { 1 }
    };
    let text = std::ffi::CString::new(&input[..19]).ok()?;
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let end = libc::strptime(text.as_ptr(), c"%Y-%m-%dT%H:%M:%S".as_ptr(), &mut tm);
        if end.is_null() || *end != 0 || tm.tm_sec > 59 {
            return None;
        }
        let before = (
            tm.tm_year, tm.tm_mon, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec,
        );
        let epoch = libc::timegm(&mut tm);
        if before
            != (
                tm.tm_year, tm.tm_mon, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec,
            )
        {
            return None;
        }
        u64::try_from(epoch.checked_sub(offset)?).ok()
    }
}
#[cfg(test)]
mod iso_tests {
    use super::*;
    #[test]
    fn normalizes_claude_reset_time_with_fraction_and_timezone() {
        assert_eq!(iso_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            iso_time("2026-09-24T08:30:00+08:00"),
            iso_time("2026-09-24T00:30:00.123456Z")
        );
        for bad in [
            "2026-02-30T00:00:00Z",
            "2026-09-24T00:00:00",
            "2026-09-24T00:00:00.Z",
            "2026-09-24T00:00:00+25:00",
        ] {
            assert_eq!(iso_time(bad), None);
        }
    }
}
