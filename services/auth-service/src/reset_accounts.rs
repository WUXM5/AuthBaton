use super::*;
use crate::codex_api::Access;
use crate::resets::{Attempt, Execution, Outcome as ResetOutcome, Source, Trigger};

enum ResetAccess {
    Codex(Access),
    Claude(crate::claude_resets::Access),
}

impl Accounts {
    pub fn reset_quota(
        &mut self,
        id: &str,
        credit_id: &str,
        request_id: &str,
        confirmed: bool,
    ) -> Result<()> {
        self.reset_quota_with_trigger(id, credit_id, request_id, confirmed, None)
    }
    /// The credit of an earlier operation, for retries from views that never see Claude grant IDs.
    pub fn retry_credit(&self, request_id: &str) -> Result<String> {
        self.db
            .reset_attempts
            .iter()
            .find(|attempt| attempt.request_id == request_id)
            .map(|attempt| attempt.credit_id.clone())
            .ok_or("缺少重置卡标识")
    }
    pub(super) fn reset_quota_with_trigger(
        &mut self,
        id: &str,
        credit_id: &str,
        request_id: &str,
        confirmed: bool,
        trigger: Option<Trigger>,
    ) -> Result<()> {
        let started_at = now();
        if !confirmed {
            return Err("请再次确认：这个操作会用掉 1 张重置卡");
        }
        self.ensure_idle()?;
        let account = self.db.accounts[self.index(id)?].clone();
        let claude = account.provider == "claude";
        // The official claim accepts request IDs of at most 64 characters.
        if request_id.is_empty()
            || request_id.len() > if claude { 64 } else { 80 }
            || !request_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("重置操作标识无效");
        }
        if !matches!(account.provider.as_str(), "codex" | "claude")
            || account.pending_profile.is_some()
            || matches!(account.state.as_str(), "unsaved" | "uncertain")
        {
            return Err("请先完成此账号的认证保存");
        }
        self.quota.poll(now());
        if self.quota.is_running(id) {
            return Err("额度正在更新，请稍后确认重置");
        }
        let existing = self
            .db
            .reset_attempts
            .iter()
            .position(|attempt| attempt.request_id == request_id);
        if let Some(index) = existing {
            let attempt = &self.db.reset_attempts[index];
            if attempt.account_id != id || attempt.credit_id != credit_id {
                return Err("重置操作与原账号或重置卡不匹配");
            }
            if !attempt.outcome.unresolved() {
                return Ok(());
            }
        }
        if self.db.reset_attempts.iter().any(|attempt| {
            attempt.account_id == id
                && attempt.outcome.unresolved()
                && attempt.request_id != request_id
        }) {
            return Err("上次重置结果待确认，请重试上次操作，不能改用新的重置卡");
        }
        let bundle = self.bundle(&account)?;
        let owner = bundle.metadata()?.owner;
        let access = if claude {
            ResetAccess::Claude(crate::claude_resets::Access::from_bundle(&bundle)?)
        } else {
            ResetAccess::Codex(Access::from_bundle(&bundle)?)
        };
        let index = if let Some(index) = existing {
            if self.db.reset_attempts[index].owner != owner {
                return Err("账号身份已变化，不能重试原重置操作");
            }
            index
        } else {
            let expires_at = self
                .quota
                .credit(id, credit_id, now())
                .ok_or("请先检查用量，取得有效的重置卡明细")?;
            self.db.reset_attempts.push(Attempt {
                account_id: id.into(),
                owner,
                request_id: request_id.into(),
                credit_id: credit_id.into(),
                expires_at,
                created_at: now(),
                outcome: ResetOutcome::Pending,
                executions: vec![],
            });
            self.db.reset_attempts.len() - 1
        };
        let previous = self.db.reset_attempts[index].clone();
        if existing.is_some() && previous.executions.is_empty() {
            self.db.reset_attempts[index]
                .executions
                .push(previous.legacy_execution(&account.label));
        }
        self.db.reset_attempts[index].executions.push(Execution {
            source: if trigger.is_some() {
                Source::Automatic
            } else {
                Source::Manual
            },
            account_label: account.label.clone(),
            started_at,
            finished_at: None,
            outcome: ResetOutcome::Pending,
            trigger,
            detail: None,
        });
        self.db.reset_attempts[index].outcome = ResetOutcome::Pending;
        if let Err(message) = self.save() {
            if existing.is_some() {
                self.db.reset_attempts[index] = previous;
            } else {
                self.db.reset_attempts.pop();
            }
            return Err(message);
        }
        let attempt = self.db.reset_attempts[index].clone();
        let retry = existing.is_some();
        #[cfg(test)]
        let (transport, claude_transport) = (
            self.reset_transport
                .unwrap_or(|_, _| Err("test transport not configured")),
            self.claude_reset_transport
                .unwrap_or(|_, _| Err("test transport not configured")),
        );
        #[cfg(not(test))]
        let (transport, claude_transport) = (crate::codex_api::http, crate::claude_resets::http);
        self.reset_job = Some((
            request_id.into(),
            std::thread::spawn(move || match access {
                ResetAccess::Codex(access) => {
                    crate::resets::run(access, &attempt, retry, transport, now())
                }
                ResetAccess::Claude(access) => {
                    crate::claude_resets::run(access, &attempt, retry, claude_transport, now())
                }
            }),
        ));
        Ok(())
    }

    pub(super) fn finish_reset(&mut self) {
        let Some((request_id, job)) = self.reset_job.take() else {
            return;
        };
        let outcome = job.join().unwrap_or(ResetOutcome::Unknown);
        let attempt = self
            .db
            .reset_attempts
            .iter_mut()
            .find(|attempt| attempt.request_id == request_id)
            .expect("an active reset always has a durable journal entry");
        attempt.outcome = outcome;
        if let Some(execution) = attempt.executions.last_mut() {
            execution.finished_at = Some(now());
            execution.outcome = outcome;
        }
        let id = attempt.account_id.clone();
        if outcome == ResetOutcome::Reset
            && let Some(account) = self
                .db
                .accounts
                .iter()
                .find(|a| a.id == id && a.provider == "claude" && a.native)
        {
            self.claude_mod.change = Some(crate::claude_mod::Change {
                at: now(),
                kind: "reset",
                label: account.label.clone(),
            });
        }
        if let Some(latch) = self.db.automation.latches.get_mut(&id)
            && latch.request_id == request_id
        {
            latch.created_at = now();
            // Nothing was spent, so another credit may still serve this episode.
            if matches!(outcome, ResetOutcome::NotSent | ResetOutcome::NoCredit) {
                self.db.automation.latches.remove(&id);
                let retry = self.reset_retry_after(&id);
                self.automation.retry_after.insert(id.clone(), retry);
            }
            self.automation
                .account_messages
                .insert(id.clone(), outcome.message().into());
        }
        self.automation.observation = None;
        if let Err(message) = self.save() {
            self.notice = Some(message.into());
        }
        // The POST outcome is durable before querying again. Failed reads never redeem again.
        self.quota.invalidate(&id);
    }

    pub fn reset_history(
        &self,
        account_id: Option<&str>,
        before: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Value> {
        if let Some(id) = account_id {
            self.index(id)?;
        }
        let limit = limit.unwrap_or(30);
        if !(1..=100).contains(&limit) {
            return Err("每次最多读取 100 条记录");
        }
        let mut entries = vec![];
        for (ordinal, attempt) in self
            .db
            .reset_attempts
            .iter()
            .enumerate()
            .filter(|(_, a)| account_id.is_none_or(|id| id == a.account_id))
        {
            let label = self
                .db
                .accounts
                .iter()
                .find(|a| a.id == attempt.account_id)
                .map(|a| a.label.as_str())
                .unwrap_or("已移除账号");
            let executions = if attempt.executions.is_empty() {
                vec![attempt.legacy_execution(label)]
            } else {
                attempt.executions.clone()
            };
            for (index, execution) in executions.into_iter().enumerate() {
                let id = format!("{}:{index}", attempt.request_id);
                let value = json!({"id":id,"account_id":attempt.account_id,"account_label":execution.account_label,
                    "source":execution.source,"started_at":execution.started_at,"finished_at":execution.finished_at,
                    "outcome":execution.outcome,"message":execution.detail.as_deref().unwrap_or(execution.outcome.message()),
                    "trigger":execution.trigger,"retry":index>0});
                entries.push(((execution.started_at, ordinal, index), value));
            }
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        let total = entries.len();
        let start = match before {
            Some(cursor) => {
                entries
                    .iter()
                    .position(|(_, v)| v["id"] == cursor)
                    .ok_or("记录已变化，请刷新后重试")?
                    + 1
            }
            None => 0,
        };
        let end = (start + limit).min(total);
        let next_cursor = (end < total).then(|| entries[end - 1].1["id"].clone());
        Ok(
            json!({"entries":entries[start..end].iter().map(|(_,v)|v).collect::<Vec<_>>(),"next_cursor":next_cursor,"total":total}),
        )
    }
}
