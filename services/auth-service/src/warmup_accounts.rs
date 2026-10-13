use super::*;

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
            // A plan switched off keeps its settled slot, so switching it back on the same day
            // does not send that slot again.
            let mut plan = account
                .warmup
                .take()
                .or_else(|| account.warmup_off.take())
                .unwrap_or(crate::warmup::Plan {
                    hour,
                    slot: None,
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
    pub(super) fn warmup_finish(&mut self) {
        let Some((id, settled, record)) = self.warmup.poll(now()) else {
            return;
        };
        self.warmup.recheck();
        let ok = record.ok;
        // The plan may have been switched off while the attempt ran; it still records the
        // result, so switching on again does not resend the slot.
        let Some(plan) = self
            .db
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .and_then(|a| a.warmup.as_mut().or(a.warmup_off.as_mut()))
        else {
            return;
        };
        plan.slot = settled.or(plan.slot);
        plan.last = Some(record);
        let _ = self.save();
        if ok {
            // Show the window that just started.
            self.quota.request(&id, now());
        }
    }
    pub(super) fn warmup_stop(&mut self) {
        self.warmup.wait();
        self.warmup_finish();
    }
    /// A due slot that cannot be sent yet says why, once per slot, instead of leaving an older
    /// result on screen; it is still sent if the cause clears within the catch-up time. Runs
    /// outside the quota admission, which is what holds the slot back.
    pub(super) fn warmup_note_blocked(&mut self) {
        let time = now();
        let paused = self.quota_paused();
        let mut noted = false;
        for account in &mut self.db.accounts {
            let reason =
                paused.or_else(|| (!quota_eligible(account)).then_some("账号认证状态待处理"));
            if let Some(reason) = reason
                && let Some(plan) = account.warmup.as_mut()
                && let Some(slot) = plan.due(time)
                && plan.last.as_ref().is_none_or(|last| last.at < slot)
            {
                plan.last = Some(crate::warmup::Record {
                    at: time,
                    ok: false,
                    message: format!("{reason}，暂不能预热；处理好后仍在补发时间内会自动补发"),
                });
                noted = true;
            }
        }
        if noted {
            let _ = self.save();
        }
    }
    /// Called under the quota admission, so no credential write is in flight.
    pub(super) fn warmup_start(&mut self) {
        let time = now();
        let Some((account, slot)) = self.db.accounts.iter().find_map(|a| {
            let slot = a.warmup.as_ref()?.due(time)?;
            (quota_eligible(a) && self.warmup.ready(&a.id, slot, time)).then_some((a, slot))
        }) else {
            return;
        };
        let prepared = self
            .bundle(account)
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
            .start(&id, slot, prepared, self.root.join("claude-warmup"));
    }
}
