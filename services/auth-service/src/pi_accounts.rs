//! Access-only consumer projection. This is not exposed through the SwiftUI socket.
use super::*;

impl Accounts {
    pub fn pi_accounts(&self) -> Value {
        if self.cloud.config.mode == "cloud" {
            return self.cloud_pi_accounts();
        }
        let accounts: Vec<Value> = self.db.accounts.iter().filter(|a| a.provider == "codex" && a.metadata.is_some() && a.pending_profile.is_none() && !self.cloud.config.bindings.contains_key(&a.id)).map(|a| {
            json!({"id":a.id,"label":a.label,"current":a.native,"state":a.state,"automatic":a.automatic})
        }).collect();
        json!({"accounts":accounts,"recovery_needed":self.db.switch.is_some()})
    }

    pub fn pi_access(&mut self, requested: Option<&str>) -> Result<Value> {
        if let Some(id) = requested.and_then(|id| id.strip_prefix("cloud-")) {
            return self.cloud_pi_access(Some(id));
        }
        if requested.is_none() && self.cloud.config.mode == "cloud" {
            return self.cloud_pi_access(None);
        }
        if self.storage_fault.get() {
            return Err("账号存储故障；请在 BatonPass 修复后重试");
        }
        if self.db.switch.is_some() {
            return Err("账号切换尚未确认；请先在 BatonPass 恢复当前状态");
        }
        let id = match requested {
            Some(id) => id.to_owned(),
            None => {
                let mut current = self
                    .db
                    .accounts
                    .iter()
                    .filter(|a| a.provider == "codex" && a.native);
                let account = current
                    .next()
                    .ok_or("尚未选择全局 Codex 当前账号，请在 BatonPass 设为当前")?;
                if current.next().is_some() {
                    return Err("全局当前账号不唯一，请在 BatonPass 核对");
                }
                account.id.clone()
            }
        };
        let account = self.db.accounts[self.index(&id)?].clone();
        if account.provider != "codex" {
            return Err("pi 接入仅支持已登记的 Codex 订阅账号");
        }
        if account.pending_profile.is_some()
            || matches!(
                account.state.as_str(),
                "reauth" | "attention" | "unsaved" | "uncertain" | "pending"
            )
        {
            return Err("此账号需要处理，请先在 BatonPass 检查登录或恢复续期");
        }
        if self.pending_login.is_some()
            || self.desktop.job.is_some()
            || self.desktop.refresh.is_some()
            || self.job.as_ref().is_some_and(|job| job.id == id)
            || self.check_queue.iter().any(|request| request.id == id)
        {
            return Ok(json!({"state":"waiting","account_id":id,"retry_after_ms":750}));
        }
        // Native material remains authoritative for the global current account.
        let bundle = self.bundle(&account)?;
        let token = bundle.credential["tokens"]["access_token"]
            .as_str()
            .unwrap();
        let access = crate::credentials::claims(token)?;
        let auth = &access["https://api.openai.com/auth"];
        let owner = &bundle.metadata()?.owner;
        let user = auth["chatgpt_user_id"]
            .as_str()
            .or(auth["user_id"].as_str())
            .ok_or("Access token 缺少用户身份")?;
        let workspace = auth["chatgpt_account_id"]
            .as_str()
            .ok_or("Access token 缺少工作区身份")?;
        if *owner != format!("codex:{user}:{workspace}") {
            return Err("Access token 与登记账号身份不一致，未向 pi 提供凭据");
        }
        let expires = access["exp"].as_u64().ok_or("Access token 缺少有效期限")?;
        if token.chars().any(char::is_control) {
            return Err("Access token 格式无效");
        }
        if expires > now() + 60 {
            self.pi_last_access = Some((id.clone(), now()));
            return Ok(
                json!({"state":"ready","account_id":id,"label":account.label,"access_token":token,"expires_at":expires}),
            );
        }
        if !account.automatic {
            return Err("此账号即将到期且自动维护已关闭，请在 BatonPass 检查认证");
        }
        if account.native && !self.desktop_owns(&id) && (self.client_running)("codex") {
            return Err(
                "当前账号由官方 Codex 客户端续期；请等待其更新，或在 BatonPass 接管桌面维护",
            );
        }
        if account.state == "retry" && account.next_check > now() {
            return Err("认证续期正在退避等待，请在 BatonPass 查看维护状态");
        }
        if self.job.is_none() && self.check_queue.is_empty() && self.reset_job.is_none() {
            // Use the existing owner-aware, persisted maintenance path, not a pi refresh grant.
            self.check(&id, account.native && !self.desktop_owns(&id))?;
        }
        Ok(json!({"state":"waiting","account_id":id,"retry_after_ms":750}))
    }
}
