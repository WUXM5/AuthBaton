//! Access-only consumer projection. This is not exposed through the SwiftUI socket.
use super::*;

impl Accounts {
    pub fn pi_accounts(&self) -> Value {
        // A cloud row is listed like any other under its own id, so pi keeps its name.
        // External rows lend their access to this Mac's own clients only, never to pi.
        let accounts: Vec<Value> = self.db.accounts.iter().filter(|a| a.provider == "codex" && a.metadata.is_some() && a.pending_profile.is_none() && !self.is_external(&a.id) && self.cloud.config.bindings.get(&a.id).is_none_or(|b| b.cloud())).map(|a| {
            json!({"id":a.id,"label":a.label,"current":a.native,"state":self.pi_state(a),"automatic":a.automatic})
        }).collect();
        json!({"accounts":accounts,"recovery_needed":self.db.switch.is_some()})
    }
    /// A cloud row the cloud cannot renew right now reads to pi as one that needs a new login.
    fn pi_state<'a>(&self, account: &'a Account) -> &'a str {
        match self.cloud_lease_id(&account.id) {
            Some(cloud_id)
                if self
                    .cloud
                    .account(cloud_id)
                    .and_then(|d| d["state"].as_str())
                    != Some("ready") =>
            {
                "reauth"
            }
            _ => &account.state,
        }
    }

    pub fn pi_access(&mut self, requested: Option<&str>) -> Result<Value> {
        if self.storage_fault.get() {
            return Err("账号存储故障；请在 AuthBaton 修复后重试");
        }
        if self.db.switch.is_some() {
            return Err("账号切换还没确认，请先在 AuthBaton 恢复当前状态");
        }
        let id = match requested {
            // Builds with a global cloud mode named cloud accounts `cloud-<id>`.
            Some(name) if name.starts_with("cloud-") => {
                let cloud_id = &name["cloud-".len()..];
                self.cloud
                    .config
                    .bindings
                    .iter()
                    .find(|(_, b)| b.cloud() && b.cloud_id.as_deref() == Some(cloud_id))
                    .map(|(id, _)| id.clone())
                    .ok_or("云端 Codex 账号不存在")?
            }
            Some(id) => id.to_owned(),
            None => {
                let mut current = self
                    .db
                    .accounts
                    .iter()
                    .filter(|a| a.provider == "codex" && a.native);
                let account = current
                    .next()
                    .ok_or("还没选全局在用的 Codex 账号，请在 AuthBaton 换上")?;
                if current.next().is_some() {
                    return Err("全局有多个在用账号，请在 AuthBaton 核对");
                }
                account.id.clone()
            }
        };
        // pi checks that the reply names the account it asked for.
        let reply_id = requested.unwrap_or(&id).to_string();
        let account = self.db.accounts[self.index(&id)?].clone();
        if account.provider != "codex" {
            return Err("pi 接入仅支持已登记的 Codex 订阅账号");
        }
        if self.is_external(&id) {
            return Err("外部账号不提供给 pi");
        }
        // A stopped transfer leaves the login where it was, as it does for 换上.
        self.transfer_guard(&id, true)?;
        if let Some(cloud_id) = self.cloud_lease_id(&id).map(str::to_string) {
            return self.pi_cloud_access(&reply_id, &account, &cloud_id);
        }
        if account.pending_profile.is_some()
            || matches!(
                account.state.as_str(),
                "reauth" | "attention" | "unsaved" | "uncertain" | "pending"
            )
        {
            return Err("此账号需要处理，请先在 AuthBaton 检查登录或恢复续期");
        }
        if self.pending_login.is_some()
            || self.desktop.job.is_some()
            || self.desktop.refresh.is_some()
            || self.job.as_ref().is_some_and(|job| job.id == id)
            || self.check_queue.iter().any(|request| request.id == id)
        {
            return Ok(json!({"state":"waiting","account_id":reply_id,"retry_after_ms":750}));
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
        let expires = access["exp"].as_u64().ok_or("Access token 缺少有效期")?;
        if token.chars().any(char::is_control) {
            return Err("Access token 格式无效");
        }
        if expires > now() + 60 {
            self.pi_last_access = Some((id.clone(), now()));
            return Ok(
                json!({"state":"ready","account_id":reply_id,"label":account.label,"access_token":token,"expires_at":expires}),
            );
        }
        if !account.automatic {
            return Err("此账号即将到期且自动维护已关闭，请在 AuthBaton 检查认证");
        }
        if account.native && !self.desktop_owns(&id) && (self.client_running)("codex") {
            return Err("当前账号由官方 Codex 客户端续期；请等它更新，或在 AuthBaton 接管桌面维护");
        }
        if account.state == "retry" && account.next_check > now() {
            return Err("认证续期正在等待重试，请在 AuthBaton 查看维护状态");
        }
        if self.job.is_none() && self.check_queue.is_empty() && self.reset_job.is_none() {
            // Use the existing owner-aware, persisted maintenance path, not a pi refresh grant.
            self.check(&id, account.native && !self.desktop_owns(&id))?;
        }
        Ok(json!({"state":"waiting","account_id":reply_id,"retry_after_ms":750}))
    }
}
