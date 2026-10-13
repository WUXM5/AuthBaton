use super::*;
use crate::automation::{self, Action, Job as AutoJob, Latch, LatchKind, Phase, Run, Task};

impl Accounts {
    pub fn configure_strategy(
        &mut self,
        config: automation::Config,
        confirmed: bool,
    ) -> Result<()> {
        config.validate()?;
        if config.enabled && !confirmed {
            return Err("请确认允许按规则暂停桌面任务、切换账号及消耗已开启账号的重置次数");
        }
        self.ensure_idle()?;
        for rule in &config.accounts {
            let account = &self.db.accounts[self.index(&rule.id)?];
            if !matches!(account.provider.as_str(), "codex" | "claude") {
                return Err("自动额度策略只支持 Codex 和 Claude 账号");
            }
        }
        if config.enabled && self.cloud.config.mode != "local" {
            return Err("自动额度策略目前仅适用于本地模式");
        }
        self.db.automation.config = config;
        self.db.automation.last_message =
            "策略已保存；逐账号独立检查主额度，重置不依赖桌面连接".into();
        self.automation.message.clear();
        self.automation.account_messages.clear();
        self.automation.retry_after.clear();
        self.automation.next_check = 0;
        self.save()
    }
    pub fn stop_strategy(&mut self, confirmed: bool) -> Result<()> {
        if !confirmed {
            return Err("请确认停止自动策略；已经暂停的会话可能需要在桌面手动继续");
        }
        self.db.automation.config.enabled = false;
        if let Some(run) = &mut self.db.automation.run {
            run.cancel_requested = true;
        }
        self.db.automation.last_message = "正在停止自动流程；不会发送新的暂停或接续指令".into();
        self.automation.message.clear();
        self.save()
    }
    pub fn forget_strategy(&mut self, confirmed: bool) -> Result<()> {
        if !confirmed {
            return Err("请先在桌面核对会话，再确认清除此自动流程记录");
        }
        if self.automation.job.is_some()
            || self.desktop.job.is_some()
            || self.job.is_some()
            || self.reset_job.is_some()
        {
            return Err("请等待在途操作结束后再核对清理");
        }
        let run = self
            .db
            .automation
            .run
            .as_ref()
            .ok_or("没有待核对的自动流程")?;
        if run.phase != Phase::Attention {
            return Err("请先停止自动流程");
        }
        self.db.automation.retired_batch = Some(run.id.clone());
        self.release_unused_switch_latch();
        self.db.automation.run = None;
        self.db.automation.config.enabled = false;
        self.db.automation.last_message =
            "已按确认清除自动流程记录；会话由用户核对，认证切换恢复记录仍保留".into();
        self.automation.message.clear();
        self.save()
    }
    pub(super) fn strategy_snapshot(&self) -> Value {
        let store = &self.db.automation;
        json!({"config":store.config,"phase":store.run.as_ref().map(|r|r.phase),
            "busy":self.automation.job.is_some(),
            "message":if self.automation.message.is_empty(){&store.last_message}else{&self.automation.message},
            "last_at":store.last_at,
            "account_states":store.config.accounts.iter().filter_map(|r|self.automation.account_messages.get(&r.id).map(|message|json!({"id":r.id,"message":message}))).collect::<Vec<_>>(),
            "run":store.run.as_ref().map(|r|json!({"id":r.id,"source":r.source,"target":r.target,"created_at":r.created_at,
                "tasks":r.tasks.iter().map(|t|json!({"thread_id":t.thread_id,"state":t.state})).collect::<Vec<_>>() }))})
    }
    pub(super) fn observe_strategy_usage(&mut self, value: &Value) {
        let v = &value["usageObservation"];
        if let (Some(owner), Ok(observation)) = (
            v["owner"].as_str(),
            serde_json::from_value::<automation::Observation>(v.clone()),
        ) && value["owner"] == owner
            && observation.fresh(now())
        {
            self.automation.observation = Some((owner.into(), observation));
            return;
        }
        if self
            .automation
            .observation
            .as_ref()
            .is_some_and(|(owner, _)| value["owner"] != *owner)
        {
            self.automation.observation = None;
        }
    }
    fn strategy_observation(&self, account: &Account) -> Option<automation::Observation> {
        let http = self.quota.automation_observation(&account.id, now());
        // While an episode is latched only a newly completed access-only GET can release it.
        if self.db.automation.latches.contains_key(&account.id) {
            return http;
        }
        let event = self
            .automation
            .observation
            .as_ref()
            .filter(|(owner, o)| {
                account.native
                    && account.metadata.as_ref().is_some_and(|m| m.owner == *owner)
                    && o.fresh(now())
            })
            .map(|(_, o)| o.clone());
        match (http, event) {
            (Some(a), Some(b)) => Some(if a.checked_at > b.checked_at { a } else { b }),
            (a, b) => a.or(b),
        }
    }
    fn strategy_account_ready(&self, a: &Account) -> bool {
        matches!(a.provider.as_str(), "codex" | "claude")
            && !self.cloud.config.bindings.contains_key(&a.id)
            && a.pending_profile.is_none()
            && matches!(a.state.as_str(), "saved" | "healthy" | "delegated")
            && a.metadata
                .as_ref()
                .and_then(|m| m.expires_at)
                .is_some_and(|t| t > now() + 60)
    }
    fn strategy_switch_blocker(&self, account: &Account) -> Option<&'static str> {
        if !account.native {
            return Some("不是当前账号，换号只对当前账号生效");
        }
        // Claude switches the default login in place; running sessions are never paused.
        if account.provider == "claude" {
            return None;
        }
        if !self.db.desktop.enabled || !self.desktop.connected {
            return Some("换号等待桌面连接恢复，重置不受连接影响");
        }
        if account.metadata.as_ref().map(|m| &m.owner) != self.desktop.owner.as_ref() {
            return Some("桌面与默认账号尚未确认一致，暂不换号");
        }
        if (self.desktop.other_clients)() {
            return Some("检测到 VS Code 的 Codex 插件或独立 Codex CLI，暂不进行桌面换号");
        }
        None
    }
    pub(super) fn strategy_action(
        &self,
        current: &Account,
        observation: &automation::Observation,
    ) -> Action {
        let config = &self.db.automation.config;
        let Some(rule) = config
            .accounts
            .iter()
            .find(|r| r.id == current.id && r.participate)
        else {
            return Action::None;
        };
        let latch = self.db.automation.latches.get(&current.id);
        if observation.remaining > f64::from(rule.below)
            || latch.is_some_and(|l| l.kind == LatchKind::Switch)
        {
            return Action::None;
        }
        // A reset is spent once per episode; switching stays open as the fallback when a
        // newer observation after that reset is still depleted.
        let reset = (rule.order.resets() && latch.is_none())
            .then(|| self.quota.automatic_credit(&current.id, now()))
            .flatten();
        let target = (rule.order.switches() && self.strategy_switch_blocker(current).is_none())
            .then(|| {
                config
                    .accounts
                    .iter()
                    .filter(|r| r.participate && r.id != current.id)
                    .find_map(|r| {
                        let account = self.db.accounts.iter().find(|a| a.id == r.id)?;
                        if account.provider != current.provider
                            || !self.strategy_account_ready(account)
                        {
                            return None;
                        }
                        let usage = self.quota.automation_observation(&account.id, now())?;
                        (usage.remaining > f64::from(r.below)).then(|| account.id.clone())
                    })
            })
            .flatten();
        automation::decide(rule.order, reset, target)
    }
    /// Plain-language state of an account the strategy leaves alone on this check.
    fn strategy_waiting(&self, account: &Account, observation: &automation::Observation) -> String {
        let rule = self
            .db
            .automation
            .config
            .accounts
            .iter()
            .find(|r| r.id == account.id)
            .unwrap();
        if observation.remaining > f64::from(rule.below) {
            return format!("额度充足，高于见底线 {}%", rule.below);
        }
        let mut reasons = vec![];
        if rule.order.resets() {
            reasons.push(if self.db.automation.latches.contains_key(&account.id) {
                "这一轮已重置过"
            } else {
                "没有可用的重置次数"
            });
        }
        if rule.order.switches() && account.native {
            reasons.push(
                self.strategy_switch_blocker(account)
                    .unwrap_or("没有额度高于各自见底线的候选账号"),
            );
        }
        if reasons.is_empty() {
            return "已见底；不是当前账号，只在成为当前账号时换号".into();
        }
        format!("已见底，暂不处理：{}", reasons.join("；"))
    }
    pub(super) fn automation_tick(&mut self) {
        if self
            .automation
            .job
            .as_ref()
            .is_some_and(|j| j.handle.is_finished())
        {
            let job = self.automation.job.take().unwrap();
            let result = job
                .handle
                .join()
                .unwrap_or(Err("自动任务操作中断，请核对桌面会话"));
            if let Err(message) = self.finish_automation(job.stage, result) {
                self.automation_attention(message);
            }
        }
        if self.storage_fault.get() || self.automation.job.is_some() || self.desktop.job.is_some() {
            return;
        }
        if self.db.automation.run.is_some() {
            if let Err(message) = self.advance_automation() {
                self.automation_attention(message);
            }
            return;
        }
        if !self.db.automation.config.enabled || now() < self.automation.next_check {
            return;
        }
        if self.ensure_idle().is_err() || self.desktop.refresh.is_some() {
            return;
        }
        self.automation.next_check = now() + 5;
        if self.cloud.config.mode != "local" {
            self.automation.message = "本地额度策略在云端模式暂停".into();
            return;
        }
        let ids: Vec<_> = self
            .db
            .automation
            .config
            .accounts
            .iter()
            .filter(|r| r.participate)
            .map(|r| r.id.clone())
            .collect();
        self.automation
            .account_messages
            .retain(|id, _| ids.contains(id));
        self.automation.message = format!("正在独立监测 {} 个账号；重置无需桌面连接", ids.len());
        for id in ids {
            let Some(account) = self.db.accounts.iter().find(|a| a.id == id).cloned() else {
                continue;
            };
            if self
                .automation
                .retry_after
                .get(&id)
                .is_some_and(|t| *t > now())
            {
                continue;
            }
            let waiting = if !self.strategy_account_ready(&account) {
                Some("等待有效认证，尚未发起额度操作")
            } else if self
                .db
                .reset_attempts
                .iter()
                .any(|a| a.account_id == id && a.outcome.unresolved())
            {
                Some("上次重置结果待确认；其他账号仍可处理")
            } else {
                None
            };
            if let Some(message) = waiting {
                self.automation.account_messages.insert(id, message.into());
                continue;
            }
            let Some(observation) = self.strategy_observation(&account) else {
                self.automation
                    .account_messages
                    .insert(id, "等待新鲜额度或重置后的新查询".into());
                continue;
            };
            if let Some(latch) = self.db.automation.latches.get(&id) {
                let newer = observation.checked_at >= latch.created_at;
                if newer && observation.remaining > f64::from(latch.threshold) {
                    self.db.automation.latches.remove(&id);
                    if self.save().is_err() {
                        return;
                    }
                } else if latch.kind == LatchKind::Switch {
                    let message = format!(
                        "这一轮见底已换走；额度回到 {}% 以上后重新开始",
                        latch.threshold
                    );
                    self.automation.account_messages.insert(id, message);
                    continue;
                } else if !newer {
                    self.automation
                        .account_messages
                        .insert(id, "已重置，等待重置后的新额度".into());
                    continue;
                }
            }
            let action = self.strategy_action(&account, &observation);
            let result = match action {
                Action::None => {
                    let message = self.strategy_waiting(&account, &observation);
                    self.automation.account_messages.insert(id, message);
                    continue;
                }
                Action::Reset(credit) => {
                    self.start_strategy_reset(&account, &credit, observation.remaining)
                }
                Action::Switch(target) if account.provider == "claude" => {
                    self.switch_claude(&account, &target)
                }
                Action::Switch(target) => self.prepare_automation(&account, &target),
            };
            match result {
                Ok(()) => {
                    self.automation
                        .account_messages
                        .insert(id, self.automation.message.clone());
                    return; // One irreversible action at a time across all accounts.
                }
                Err(message) => {
                    self.automation
                        .account_messages
                        .insert(id.clone(), message.into());
                    self.automation.retry_after.insert(id, now() + 60);
                    if self.storage_fault.get() || self.db.automation.run.is_some() {
                        return;
                    }
                }
            }
        }
    }
    fn start_strategy_reset(
        &mut self,
        account: &Account,
        credit: &str,
        remaining: f64,
    ) -> Result<()> {
        let request_id = random_id()?;
        let started_at = now();
        let threshold = self
            .db
            .automation
            .config
            .accounts
            .iter()
            .find(|r| r.id == account.id)
            .unwrap()
            .below;
        self.db.automation.latches.insert(
            account.id.clone(),
            Latch {
                request_id: request_id.clone(),
                created_at: now(),
                threshold,
                kind: LatchKind::Reset,
            },
        );
        self.save()?; // Episode admission precedes the durable POST intent.
        let trigger = crate::resets::Trigger {
            remaining_percent: remaining,
            threshold_percent: threshold,
        };
        if let Err(message) = self.reset_quota_with_trigger(
            &account.id,
            credit,
            &request_id,
            true,
            Some(trigger.clone()),
        ) {
            // A failure before POST admission is logged as NotSent, never as an unknown grant.
            if !self
                .db
                .reset_attempts
                .iter()
                .any(|a| a.request_id == request_id)
            {
                self.db.reset_attempts.push(crate::resets::Attempt {
                    account_id: account.id.clone(),
                    owner: account.metadata.as_ref().unwrap().owner.clone(),
                    request_id: request_id.clone(),
                    credit_id: credit.into(),
                    expires_at: None,
                    created_at: started_at,
                    outcome: crate::resets::Outcome::NotSent,
                    executions: vec![crate::resets::Execution {
                        source: crate::resets::Source::Automatic,
                        account_label: account.label.clone(),
                        started_at,
                        finished_at: Some(now()),
                        outcome: crate::resets::Outcome::NotSent,
                        trigger: Some(trigger),
                        detail: Some(message.into()),
                    }],
                });
                self.db.automation.latches.remove(&account.id);
                self.save()?;
            }
            return Err(message);
        }
        self.automation.message = format!("{}：已按确认规则提交自动重置", account.label);
        Ok(())
    }
    /// Claude has no batch to pause or resume: the switch is one locked write of the default
    /// login, after which running sessions move to the target within 30 s.
    fn switch_claude(&mut self, current: &Account, target_id: &str) -> Result<()> {
        let threshold = self
            .db
            .automation
            .config
            .accounts
            .iter()
            .find(|r| r.id == current.id)
            .unwrap()
            .below;
        // Same episode rule as a desktop switch: the source must not spend a reset right after.
        self.db.automation.latches.insert(
            current.id.clone(),
            Latch {
                request_id: random_id()?,
                created_at: now(),
                threshold,
                kind: LatchKind::Switch,
            },
        );
        self.save()?;
        if let Err(message) = self.activate_native(target_id) {
            if self.db.switch.is_none() {
                self.db.automation.latches.remove(&current.id);
                self.save()?;
            }
            return Err(message);
        }
        let target = &self.db.accounts[self.index(target_id)?].label;
        self.automation.message = format!(
            "{} 已见底，已换到 {target}；运行中的 Claude Code 约 30 秒内改用新账号",
            current.label
        );
        self.db.automation.last_message = self.automation.message.clone();
        self.db.automation.last_at = Some(now());
        self.save()
    }
    fn prepare_automation(&mut self, current: &Account, target_id: &str) -> Result<()> {
        // Read and validate both credentials before even preparing a task interruption batch.
        let source = self.bundle(current)?;
        let target = self.bundle(&self.db.accounts[self.index(target_id)?])?;
        let source_owner = source.metadata()?.owner;
        let target_meta = target.metadata()?;
        if target_meta.expires_at.is_none_or(|t| t <= now() + 60) {
            return Err("候选凭据即将到期，未暂停任务");
        }
        crate::desktop::external_auth(&target)?;
        let id = random_id()?;
        self.db.automation.run = Some(Run {
            id: id.clone(),
            source: current.id.clone(),
            target: target_id.into(),
            source_owner: source_owner.clone(),
            target_owner: target_meta.owner.clone(),
            instance: self.desktop.instance.clone().ok_or("桌面实例尚未确认")?,
            renderer: self.desktop.target.clone().ok_or("桌面窗口尚未确认")?,
            tasks: vec![],
            phase: Phase::Preparing,
            created_at: now(),
            cancel_requested: false,
        });
        self.save()?; // Keep the batch ID even when prepare creates a receipt but its reply is lost.
        let client = self.desktop.client(&self.db.desktop);
        let retired = self.db.automation.retired_batch.clone();
        self.automation.message = "正在核对目标账号与桌面适配；尚未暂停任务".into();
        self.automation.job = Some(AutoJob {
            stage: "prepare",
            handle: std::thread::spawn(move || {
                client.raw("auto_prepare",json!({"batchId":id,"owner":source_owner,"targetOwner":target_meta.owner,"retireBatch":retired}))
            }),
        });
        Ok(())
    }
    fn automation_attention(&mut self, message: &str) {
        if let Some(run) = &mut self.db.automation.run {
            run.phase = Phase::Attention;
            run.cancel_requested = false;
            self.db.automation.config.enabled = false;
        }
        self.automation.message = message.into();
        self.db.automation.last_message = message.into();
        self.db.automation.last_at = Some(now());
        self.automation.next_check = now() + 30;
        let _ = self.save();
    }
    fn finish_automation(&mut self, stage: &str, result: Result<Value>) -> Result<()> {
        if stage == "prepare" {
            let value = result?;
            if value["ok"] != true {
                // The bridge refuses before it makes a batch, so nothing was paused. Busy tasks
                // only delay the switch; any other refusal still asks the user to check.
                if !crate::desktop::declined_for_tasks(&value) {
                    return Err(crate::desktop::error_message(value["code"].as_str()));
                }
                self.db.automation.run = None;
                self.automation.message = crate::desktop::refusal(&value);
                self.db.automation.last_message = self.automation.message.clone();
                self.db.automation.last_at = Some(now());
                self.automation.next_check = now() + 30;
                return self.save();
            }
            let run = self
                .db
                .automation
                .run
                .as_ref()
                .ok_or("自动批次缺少准备记录")?;
            if value["owner"] != run.source_owner || value["instance"] != run.instance {
                return Err("准备时桌面身份或实例已变化，尚未暂停任务");
            }
            let tasks = decode_tasks(&value)?;
            self.desktop.observed(&value)?;
            let run = self.db.automation.run.as_mut().unwrap();
            run.tasks = tasks;
            run.phase = Phase::Pausing;
            return self.save();
        }
        if stage == "cancel" || stage == "complete" {
            result?;
            let resumed = self.db.automation.run.as_ref().map_or(0, |r| {
                r.tasks.iter().filter(|t| t.state == "resumed").count()
            });
            self.release_unused_switch_latch();
            self.db.automation.run = None;
            self.db.automation.last_message = if stage == "complete" {
                format!("账号已切换，已在 {resumed} 个原会话接续任务")
            } else {
                "自动流程已结束；请在 Codex 核对并手动继续尚未接续的会话".into()
            };
            self.db.automation.last_at = Some(now());
            self.automation.message.clear();
            return self.save();
        }
        let value = result?;
        let run = self.db.automation.run.as_mut().ok_or("自动批次已不存在")?;
        if value["instance"] != run.instance {
            return Err("桌面实例已变化，未继续自动操作");
        }
        let tasks = decode_tasks(&value)?;
        if tasks.len() != run.tasks.len()
            || !tasks.iter().zip(&run.tasks).all(|(a, b)| {
                a.thread_id == b.thread_id && a.turn_id == b.turn_id && a.resume_key == b.resume_key
            })
        {
            return Err("桌面返回的任务不属于原批次");
        }
        run.tasks = tasks;
        if stage == "verify" && !run.cancel_requested {
            run.phase = Phase::Switching;
            let (id, source, target) = (run.id.clone(), run.source.clone(), run.target.clone());
            // Switching away ends the source's episode: without this latch the now non-current
            // source would still be depleted and could spend a reset right after the switch.
            let threshold = self
                .db
                .automation
                .config
                .accounts
                .iter()
                .find(|r| r.id == source)
                .unwrap()
                .below;
            self.db.automation.latches.insert(
                source,
                Latch {
                    request_id: id,
                    created_at: now(),
                    threshold,
                    kind: LatchKind::Switch,
                },
            );
            self.save()?;
            self.commit_desktop(&target, false, value)?;
        } else {
            self.save()?;
        }
        Ok(())
    }
    /// A batch that ends with its source still current never switched away, so its latch
    /// must not hold that account's episode.
    fn release_unused_switch_latch(&mut self) {
        let Some(run) = &self.db.automation.run else {
            return;
        };
        if self
            .db
            .accounts
            .iter()
            .any(|a| a.id == run.source && a.native)
            && self
                .db
                .automation
                .latches
                .get(&run.source)
                .is_some_and(|l| l.kind == LatchKind::Switch && l.request_id == run.id)
        {
            self.db.automation.latches.remove(&run.source);
        }
    }
    fn advance_automation(&mut self) -> Result<()> {
        let run = self.db.automation.run.as_ref().unwrap().clone();
        if run.cancel_requested {
            return self.automation_call("cancel", "auto_cancel", None);
        }
        match run.phase {
            Phase::Preparing => {
                return Err("桌面准备结果未确认，请停止并核对此流程");
            }
            Phase::Attention => {}
            Phase::Cancelling => {}
            Phase::Pausing => {
                if let Some(task) = run.tasks.iter().find(|t| t.state == "planned") {
                    self.automation.message = format!(
                        "正在暂停桌面会话（{} / {}）",
                        run.tasks.iter().filter(|t| t.state != "planned").count() + 1,
                        run.tasks.len()
                    );
                    self.automation_call("pause", "auto_pause", Some(&task.thread_id))?;
                } else {
                    if run
                        .tasks
                        .iter()
                        .any(|t| !matches!(t.state.as_str(), "paused" | "skipped"))
                    {
                        return Err("暂停结果待确认，不会重复发送中断");
                    }
                    self.db.automation.run.as_mut().unwrap().phase = Phase::Verifying;
                    self.save()?;
                }
            }
            Phase::Verifying => {
                self.automation.message = "正在核对所有受影响会话已停止".into();
                self.automation_call("verify", "auto_verify", None)?;
            }
            Phase::Switching => {
                if self.db.switch.is_some() {
                    return Err("认证切换尚未完成；请停止自动流程并恢复当前登录状态");
                }
                if self.desktop.owner.as_deref() != Some(&run.target_owner)
                    || !self
                        .db
                        .accounts
                        .iter()
                        .any(|a| a.id == run.target && a.native)
                    || !self.desktop.connected
                    || !self.desktop.bound
                {
                    return Err("尚未确认目标身份，未接续任何会话");
                }
                self.db.automation.run.as_mut().unwrap().phase = Phase::Resuming;
                self.save()?;
                self.automation.observation = None;
            }
            Phase::Resuming => {
                if let Some(task) = run.tasks.iter().find(|t| t.state == "paused") {
                    self.automation.message = "正在原会话逐个继续任务".into();
                    self.automation_call("resume", "auto_resume", Some(&task.thread_id))?;
                } else {
                    if run
                        .tasks
                        .iter()
                        .any(|t| !matches!(t.state.as_str(), "resumed" | "skipped"))
                    {
                        return Err("接续结果待确认，不会重复发送继续指令");
                    }
                    // Clear the renderer receipt before admitting another batch. This is not a resume.
                    self.automation_call("complete", "auto_cancel", None)?;
                }
            }
        }
        Ok(())
    }
    fn automation_call(
        &mut self,
        stage: &'static str,
        operation: &'static str,
        thread: Option<&str>,
    ) -> Result<()> {
        let run = self.db.automation.run.as_mut().unwrap();
        if stage == "pause" || stage == "resume" {
            let task = run
                .tasks
                .iter_mut()
                .find(|t| Some(t.thread_id.as_str()) == thread)
                .ok_or("缺少任务记录")?;
            task.state = if stage == "pause" {
                "interrupt_sent"
            } else {
                "resume_sent"
            }
            .into();
        }
        let params = json!({"batchId":run.id,"instance":run.instance,"threadId":thread});
        let renderer = run.renderer.clone();
        self.save()?; // Intent before any turn/interrupt or turn/start.
        let mut client = self.desktop.client(&self.db.desktop);
        client.target = Some(renderer);
        self.automation.job = Some(AutoJob {
            stage,
            handle: std::thread::spawn(move || client.call(operation, params)),
        });
        Ok(())
    }
    pub(super) fn automation_shutdown(&mut self) {
        if let Some(job) = self.automation.job.take() {
            let _ = job.handle.join();
        }
        if self.db.automation.run.is_some() {
            self.automation_attention("自动流程因退出而停止；重开后请核对桌面会话，不会重放操作");
        }
    }
}
fn decode_tasks(value: &Value) -> Result<Vec<Task>> {
    let tasks: Vec<Task> =
        serde_json::from_value(value["tasks"].clone()).map_err(|_| "桌面任务列表格式无效")?;
    let mut ids = std::collections::HashSet::new();
    if tasks.len() > 256
        || tasks.iter().any(|t| {
            !automation::valid_id(&t.thread_id)
                || !automation::valid_id(&t.turn_id)
                || !automation::valid_id(&t.resume_key)
                || !ids.insert(&t.thread_id)
                || t.resumed_turn_id
                    .as_ref()
                    .is_some_and(|id| !automation::valid_id(id))
                || !matches!(
                    t.state.as_str(),
                    "planned" | "interrupt_sent" | "paused" | "skipped" | "resume_sent" | "resumed"
                )
        })
    {
        return Err("桌面任务列表包含无效或重复记录");
    }
    Ok(tasks)
}
