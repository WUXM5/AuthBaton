use super::*;

impl Accounts {
    pub fn refresh_clients(&mut self) -> Result<()> {
        self.ensure_idle()?;
        if self.clients_checked_at.is_some_and(|t| now() < t + 30) {
            return Ok(());
        }
        let mut sources = Vec::new();
        for provider in ["codex", "claude"] {
            let metadata = self
                .native(provider)
                .and_then(|n| n.read(&self.platform))
                .and_then(|b| b.metadata());
            // Observe the same native account only; never silently adopt an external switch.
            if let Ok(ref observed) = metadata
                && let Some(account) = self.db.accounts.iter_mut().find(|a| {
                    a.native
                        && a.provider == provider
                        && a.metadata
                            .as_ref()
                            .is_some_and(|m| m.owner == observed.owner)
                })
            {
                account.metadata = Some(observed.clone());
                account.last_local = Some(now());
            }
            sources.push(crate::clients::Source::native(provider, metadata));
        }
        self.client_sources = sources;
        self.client_sources.push(crate::clients::Source::pi(
            &self.sources.pi_home.join("auth.json"),
        ));
        self.clients_checked_at = Some(now());
        self.save()
    }
    pub fn clients_snapshot(&self) -> Value {
        let account_for = |owner: Option<&str>| {
            self.db.accounts.iter().find(|a| {
                a.metadata
                    .as_ref()
                    .is_some_and(|m| Some(m.owner.as_str()) == owner)
            })
        };
        let fresh = self.desktop.connected
            && self
                .desktop
                .observed_at
                .is_some_and(|t| now().saturating_sub(t) <= 30);
        let desktop_account = account_for(self.desktop.owner.as_deref());
        let mut result = vec![
            json!({"id":"codex_desktop","name":"Codex 桌面","evidence":if fresh {"confirmed"} else {"unknown"},"account_label":desktop_account.map(|a|a.label.as_str()).or(if fresh {Some("未登记的桌面账号")} else {None}),"identity":desktop_account.and_then(|a|a.metadata.as_ref()).map(|m|&m.display),"checked_at":self.desktop.observed_at,"source":"已连接桌面后端的身份观测","message":if fresh {"身份由当前桌面后端报告；未用默认文件替代桌面观测"} else {"尚无有效的桌面身份观测；如有旧账号，仅表示上次结果"}}),
        ];
        for (id, name) in [
            ("codex", "Codex CLI · 默认来源"),
            ("claude", "Claude Code · 默认来源"),
            ("pi", "pi · 原生默认来源"),
        ] {
            let source = self.client_sources.iter().find(|s| s.id == id);
            let account = account_for(source.and_then(|s| s.owner.as_deref()));
            let observed = source.is_some_and(|s| s.owner.is_some());
            let stale = self
                .clients_checked_at
                .is_none_or(|t| now().saturating_sub(t) > 300);
            result.push(json!({"id":id,"name":name,"evidence":if observed && !stale {"configured"}else{"unknown"},"account_label":account.map(|a|&a.label),"identity":source.and_then(|s|s.display.as_ref()).or_else(|| account.and_then(|a|a.metadata.as_ref()).map(|m|&m.display)),"checked_at":self.clients_checked_at,"source":if id=="pi"{"pi 原生 auth.json"}else{"官方默认认证存储"},"message":source.map_or("尚未检查默认认证来源",|s|s.message)}));
        }
        for (id, name, message) in [
            (
                "vscode",
                "VS Code · Codex 扩展",
                "运行中扩展未接入身份回报；不能仅凭默认文件确定账号",
            ),
            (
                "claude_sdk",
                "Claude Agent SDK",
                "各进程可覆盖环境凭据；初始化信息不是持续身份观测",
            ),
        ] {
            result.push(json!({"id":id,"name":name,"evidence":"unknown","source":"运行中客户端","message":message}));
        }
        let issued = self.pi_last_access.as_ref();
        let account = issued.and_then(|(id, _)| self.db.accounts.iter().find(|a| a.id == *id));
        result.push(json!({"id":"pi_runtime","name":"pi · 最近取用账号","evidence":if issued.is_some(){"issued"}else{"unknown"},"account_label":account.map(|a|&a.label),"identity":account.and_then(|a|a.metadata.as_ref()).map(|m|&m.display),"checked_at":issued.map(|(_,time)|time),"source":"AuthBaton pi 凭据服务","message":"显示最近一次成功提供给 pi 的账号；不是所有 pi 会话的实时身份，也不代表模型请求已成功"}));
        Value::Array(result)
    }
}
