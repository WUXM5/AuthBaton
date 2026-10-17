use super::*;
use crate::warmup::{Plan, Step};

/// The hour the app offers first; a plan made to show a warm-up by hand starts off with it.
const DEFAULT_HOUR: u8 = 6;

impl Accounts {
    pub fn configure_warmup(&mut self, id: &str, enabled: bool, hour: Option<u8>) -> Result<()> {
        if self.storage_fault.get() {
            return Err("账号数据库保存失败，已暂停变更；修复存储后重新连接服务");
        }
        let index = self.index(id)?;
        let account = &mut self.db.accounts[index];
        if account.provider != "claude" {
            return Err("只有 Claude 账号有 5 小时窗口");
        }
        if enabled {
            let hour = hour.filter(|h| *h < 24).ok_or("预热时间须为 0–23 点")?;
            // A plan switched off keeps when it last sent, so switching it back on does not warm
            // the window that send started again.
            let mut plan = account
                .warmup
                .take()
                .or_else(|| account.warmup_off.take())
                .unwrap_or(Plan {
                    hour,
                    slot: None,
                    sent: None,
                    last: None,
                });
            plan.hour = hour;
            account.warmup = Some(plan);
        } else if let Some(plan) = account.warmup.take() {
            account.warmup_off = Some(plan);
        }
        self.warmup.recheck();
        self.save()
    }
    /// One warm-up by hand: sent whatever the window and with the plan on or off, never retried.
    pub fn warmup_now(&mut self, id: &str) -> Result<()> {
        if self.quota_paused().is_some() {
            return Err(
                "AuthBaton 有待处理的问题（数据库故障、换号恢复或自动换号流程），暂不能预热",
            );
        }
        let index = self.index(id)?;
        let account = &mut self.db.accounts[index];
        if account.provider != "claude" {
            return Err("只有 Claude 账号有 5 小时窗口");
        }
        if !quota_eligible(account) {
            return Err("账号认证状态待处理，暂不能预热");
        }
        // The result is shown with the plan; an account without one gets a switched-off plan.
        if account.warmup.is_none() && account.warmup_off.is_none() {
            account.warmup_off = Some(Plan {
                hour: DEFAULT_HOUR,
                slot: None,
                sent: None,
                last: None,
            });
            self.save()?;
        }
        self.warmup.request(id)
    }
    pub(super) fn warmup_view(&self, account: &Account) -> Option<Value> {
        let (plan, enabled) = match (&account.warmup, &account.warmup_off) {
            (Some(plan), _) => (plan, true),
            (None, Some(plan)) => (plan, false),
            (None, None) => return None,
        };
        let resets_at = self
            .quota
            .warmup_reading(&account.id)
            .and_then(|r| r.resets_at);
        Some(json!({
            "enabled": enabled,
            "hour": plan.hour,
            "running": self.warmup.running(&account.id) || self.warmup.queued(&account.id),
            "last": plan.last,
            "next": if enabled { plan.next(now(), resets_at) } else { None },
        }))
    }
    pub(super) fn warmup_finish(&mut self) {
        let Some((id, day, record)) = self.warmup.poll(now()) else {
            return;
        };
        self.warmup.recheck();
        // The plan may have been switched off while the attempt ran; it still records the
        // result, so switching on again does not warm the same window twice.
        if let Some(plan) = self
            .db
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .and_then(|a| a.warmup.as_mut().or(a.warmup_off.as_mut()))
        {
            if let Some(day) = day.filter(|_| record.ok) {
                plan.slot = Some(day);
                plan.sent = Some(record.at);
            }
            plan.last = Some(record);
            let _ = self.save();
        }
        // Shows the window the attempt may have started; the next decision waits for it.
        self.quota.request(&id, now());
    }
    pub(super) fn warmup_stop(&mut self) {
        self.warmup.wait();
        self.warmup_finish();
    }
    /// The account's 见底线 as set, whether or not the strategy runs; without a rule only an
    /// exhausted seven-day quota holds a warm-up back.
    fn warmup_line(&self, id: &str) -> f64 {
        self.db
            .automation
            .config
            .accounts
            .iter()
            .find(|r| r.id == id)
            .map_or(0.0, |r| f64::from(r.below))
    }
    fn warmup_step(&self, account: &Account, plan: &Plan, time: u64) -> Step {
        plan.step(
            time,
            self.warmup.tried(&account.id),
            self.quota.warmup_reading(&account.id).as_ref(),
            self.warmup_line(&account.id),
        )
    }
    /// Looks at each plan: asks for the reading that decides once a window has reset, and says
    /// why a due warm-up is not sent instead of leaving an older result on screen. Runs outside
    /// the quota admission, which is what holds sending back.
    pub(super) fn warmup_plan(&mut self) {
        let time = now();
        let paused = self.quota_paused();
        let mut steps = Vec::new();
        for account in &self.db.accounts {
            let Some(plan) = &account.warmup else {
                continue;
            };
            let blocked =
                paused.or_else(|| (!quota_eligible(account)).then_some("账号认证状态待处理"));
            let step = match blocked {
                Some(reason) if plan.day(time).is_some() => Err(reason),
                _ => Ok(self.warmup_step(account, plan, time)),
            };
            steps.push((account.id.clone(), step));
        }
        let mut noted = false;
        for (id, step) in steps {
            let (ok, message) = match step {
                Ok(Step::Rest) => {
                    self.warmup.clear_failures(&id);
                    continue;
                }
                Ok(Step::Query(after)) => {
                    if self.quota.attempted(&id).is_none_or(|at| at < after) {
                        self.quota.request(&id, time);
                    }
                    continue;
                }
                Ok(Step::Hold) => (
                    true,
                    match self.warmup_line(&id) {
                        0.0 => "7 天额度已用完，暂不预热".to_string(),
                        line => format!("7 天额度已到见底线 {line}%，暂不预热"),
                    },
                ),
                Ok(Step::Wait | Step::Send(_)) => continue,
                Err(reason) => (false, format!("{reason}，暂不能预热；处理好后会接着预热")),
            };
            if let Some(plan) = self
                .db
                .accounts
                .iter_mut()
                .find(|a| a.id == id)
                .and_then(|a| a.warmup.as_mut())
                && plan
                    .last
                    .as_ref()
                    .is_none_or(|last| last.message != message)
            {
                plan.last = Some(crate::warmup::Record {
                    at: time,
                    ok,
                    message,
                });
                noted = true;
            }
        }
        if noted {
            let _ = self.save();
        }
    }
    /// Called under the quota admission, so no credential write is in flight. A warm-up by hand
    /// goes first.
    pub(super) fn warmup_start(&mut self) {
        let time = now();
        let manual = self
            .warmup
            .next_manual()
            .and_then(|id| self.db.accounts.iter().find(|a| a.id == id))
            .map(|a| (a, None));
        let Some((account, day)) = manual.or_else(|| {
            self.db.accounts.iter().find_map(|a| {
                let Step::Send(day) = self.warmup_step(a, a.warmup.as_ref()?, time) else {
                    return None;
                };
                (quota_eligible(a) && self.warmup.ready(&a.id, time)).then_some((a, Some(day)))
            })
        }) else {
            return;
        };
        let prepared = if quota_eligible(account) {
            self.bundle(account)
        } else {
            Err("账号认证状态待处理")
        }
        .and_then(|b| crate::quota::ClaudeAccess::from_bundle(&b))
        .and_then(|access| {
            if access.expires_at.is_some_and(|e| e <= time + 60) {
                return Err("Access 已到期，等待认证维护续期");
            }
            let binary = self.sources.claude_bin.clone().ok_or("未找到 Claude CLI")?;
            Ok((binary, access.token))
        });
        let id = account.id.clone();
        self.warmup
            .start(&id, day, prepared, self.root.join("claude-warmup"));
    }
}
