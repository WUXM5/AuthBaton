use super::*;
use crate::cloud;

impl Accounts {
    pub fn begin_cloud_login(&mut self, cloud_id: &str, login: &mut LoginManager) -> Result<()> {
        self.ensure_idle()?;
        if !self.cloud.config.paired {
            return Err("请先完成云端配对");
        }
        let cloud = self
            .cloud
            .config
            .accounts
            .iter()
            .find(|a| a["id"] == cloud_id)
            .ok_or("云端账号不存在")?;
        let provider = cloud["provider"]
            .as_str()
            .filter(|p| matches!(*p, "codex" | "claude"))
            .ok_or("云端账号类型无效")?
            .to_string();
        let label = format!("{} · 新授权", cloud["label"].as_str().unwrap_or("云端账号"));
        let id = random_id()?;
        self.db.accounts.push(Account {
            id: id.clone(),
            provider: provider.clone(),
            label: label.clone(),
            metadata: None,
            native: false,
            automatic: false,
            state: "pending".into(),
            message: "等待官方登录后更新同一云端账号".into(),
            last_local: None,
            last_online: None,
            last_refresh: None,
            next_check: 0,
            failures: 0,
            quota: vec![],
            pending_profile: None,
            warmup: None,
            warmup_off: None,
        });
        self.save()?;
        self.cloud
            .config
            .reauthorizations
            .insert(id.clone(), cloud_id.into());
        self.cloud.save()?;
        self.begin_login(&provider, &label, Some(&id), login)
    }
    pub fn cloud_snapshot(&self) -> Value {
        self.cloud.snapshot()
    }
    #[allow(clippy::too_many_arguments)]
    pub fn cloud_command(
        &mut self,
        action: &str,
        mode: Option<&str>,
        endpoint: Option<&str>,
        label: Option<&str>,
        id: Option<&str>,
        confirm: bool,
        automatic: Option<bool>,
    ) -> Result<()> {
        self.ensure_idle()?;
        match action {
            "configure" => self.cloud.configure(mode, endpoint, label),
            "pair" => self.cloud.start(self.platform.clone(), cloud::pair),
            "pair_poll" => self.cloud.start(self.platform.clone(), cloud::pair_poll),
            "refresh" => self.cloud.start(self.platform.clone(), cloud::directory),
            "import" | "resume_import" => {
                if !confirm {
                    return Err("请确认将所选账号的维护交给个人云端");
                }
                if !self.cloud.config.paired {
                    return Err("请先完成设备配对");
                }
                let id = id.ok_or("缺少本机账号")?.to_string();
                let i = self
                    .db
                    .accounts
                    .iter()
                    .position(|a| a.id == id)
                    .ok_or("本机账号不存在")?;
                let a = self.db.accounts[i].clone();
                if a.native && (self.client_running)(&a.provider) {
                    return Err("请结束任务并退出此提供方的原生客户端后再交接；不需要重新登录");
                }
                if matches!(
                    a.state.as_str(),
                    "unsaved" | "uncertain" | "pending" | "checking"
                ) || (self.db.desktop.enabled
                    && self.db.desktop.selected.as_deref() == Some(&id))
                {
                    return Err("此账号有待处理操作，请先完成恢复或解除桌面实验接管");
                }
                if !self.cloud.config.bindings.contains_key(&id) {
                    self.cloud.config.bindings.insert(
                        id.clone(),
                        cloud::Binding {
                            operation: random_id()?,
                            cloud_id: None,
                            owner: a.metadata.as_ref().ok_or("账号身份尚未取得")?.owner.clone(),
                            provider: a.provider.clone(),
                            native: a.native,
                            phase: "prepared".into(),
                            replace_id: self.cloud.config.reauthorizations.get(&id).cloned(),
                        },
                    );
                    self.cloud.save()?;
                }
                self.db.accounts[i].automatic = false;
                self.db.accounts[i].state = "cloud_transfer".into();
                self.db.accounts[i].message = "云端交接中；本机刷新已暂停，可在云端模式恢复".into();
                self.save()?;
                let native = a.native.then(|| self.native(&a.provider)).transpose()?;
                self.cloud
                    .start(self.platform.clone(), move |cfg, p, path| {
                        cloud::transfer(cfg, p, path, id, a.label, native)
                    })
            }
            "select" => {
                let id = id.ok_or("缺少云端账号")?.to_string();
                let a = self
                    .cloud
                    .config
                    .accounts
                    .iter()
                    .find(|a| a["id"] == id)
                    .ok_or("云端账号不存在")?;
                if a["state"] != "ready" {
                    return Err("云端账号尚未就绪");
                }
                let provider = a["provider"].as_str().ok_or("账号类型无效")?.to_string();
                // Selection for broker consumers does not rewrite native credentials.
                self.cloud
                    .config
                    .selected
                    .insert(format!("{provider}_broker"), id);
                self.cloud.save()
            }
            "unselect" => {
                let id = id.ok_or("缺少云端账号")?;
                if !confirm || self.cloud.config.pending_activation.as_deref() == Some(id) {
                    return Err("请先处理待恢复切换，并确认停止本机使用");
                }
                let previous = self.cloud.config.selected.clone();
                self.cloud
                    .config
                    .selected
                    .retain(|_, selected| selected != id);
                if let Err(e) = self.cloud.save() {
                    self.cloud.config.selected = previous;
                    return Err(e);
                }
                Ok(())
            }
            "cancel_import" => {
                if !confirm {
                    return Err("请确认取消尚未完成的云端交接并恢复本地维护");
                }
                let id = id.ok_or("缺少本机账号")?.to_string();
                let b = self
                    .cloud
                    .config
                    .bindings
                    .get(&id)
                    .ok_or("交接记录不存在")?
                    .clone();
                if b.phase == "cloud_managed" {
                    return Err("云端已接管，不能恢复旧刷新凭据");
                }
                if b.native && (self.client_running)(&b.provider) {
                    return Err("请先退出此提供方的客户端再取消交接");
                }
                let native = b.native.then(|| self.native(&b.provider)).transpose()?;
                self.cloud
                    .start(self.platform.clone(), move |cfg, p, path| {
                        cloud::cancel_transfer(cfg, p, path, id, native)
                    })
            }
            "activate" => {
                if !confirm {
                    return Err("请确认将云端账号设为原生客户端当前登录");
                }
                let id = id.ok_or("缺少云端账号")?.to_string();
                let a = self
                    .cloud
                    .config
                    .accounts
                    .iter()
                    .find(|a| a["id"] == id)
                    .ok_or("云端账号不存在")?;
                let provider = a["provider"].as_str().ok_or("账号类型无效")?.to_string();
                if (self.client_running)(&provider) {
                    return Err("请结束任务并退出此提供方客户端后再切换；不会自动退出客户端");
                }
                if self.db.desktop.enabled && provider == "codex" {
                    return Err("请先关闭本地桌面实验接管，再启用云端原生同步");
                }
                let previous = self
                    .db
                    .accounts
                    .iter()
                    .find(|a| {
                        a.provider == provider
                            && a.native
                            && !self.cloud.config.bindings.contains_key(&a.id)
                    })
                    .and_then(|a| a.metadata.as_ref().map(|m| (a.id.clone(), m.owner.clone())));
                if provider == "codex" {
                    private_dir(&self.sources.codex_home)?;
                }
                let native = self.native(&provider)?;
                self.cloud
                    .start(self.platform.clone(), move |cfg, p, path| {
                        cloud::native_sync(cfg, p, path, id, native, true, previous)
                    })
            }
            "usage" | "check" | "update" | "remove" => {
                let id = id.ok_or("缺少云端账号")?.to_string();
                if action == "remove"
                    && (!confirm || self.cloud.config.selected.values().any(|v| v == &id))
                {
                    return Err("请先取消此账号的本机使用并确认移除");
                }
                let body = json!({"id":id,"label":label,"automatic":automatic});
                let body = Value::Object(
                    body.as_object()
                        .unwrap()
                        .iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                );
                let action = action.to_string();
                self.cloud
                    .start(self.platform.clone(), move |cfg, p, path| {
                        cloud::action(cfg, p, path, action, body)
                    })
            }
            _ => Err("云端操作不受支持"),
        }
    }
    pub fn cloud_pi_accounts(&self) -> Value {
        let selected = self.cloud.config.selected.get("codex_broker").or(self
            .cloud
            .config
            .selected
            .get("codex"));
        json!({"accounts":self.cloud.config.accounts.iter().filter(|a|a["provider"]=="codex").map(|a|json!({"id":format!("cloud-{}",a["id"].as_str().unwrap_or("")),"label":a["label"],"current":selected.is_some_and(|s|a["id"]==s.as_str()),"state":a["state"],"automatic":a["automatic"]})).collect::<Vec<_>>(),"recovery_needed":self.cloud.config.pending_activation.is_some()})
    }
    pub fn cloud_pi_access(&mut self, requested: Option<&str>) -> Result<Value> {
        if self.cloud.config.pending_activation.is_some() {
            return Err("本机云端切换待恢复");
        }
        let id = requested
            .map(str::to_string)
            .or_else(|| {
                self.cloud
                    .config
                    .selected
                    .get("codex_broker")
                    .or(self.cloud.config.selected.get("codex"))
                    .cloned()
            })
            .ok_or("请在云端账号页选择 pi 当前账号")?;
        let a = self
            .cloud
            .config
            .accounts
            .iter()
            .find(|a| a["id"] == id && a["provider"] == "codex")
            .ok_or("云端 Codex 账号不存在")?;
        if a["state"] != "ready" {
            return Err("云端账号尚未就绪，请在 BatonPass 检查");
        }
        if let Some(lease) = self.cloud.lease(&id) {
            return Ok(
                json!({"state":"ready","account_id":format!("cloud-{id}"),"label":lease["label"],"access_token":lease["access_token"],"expires_at":lease["expires_at"]}),
            );
        }
        if self.cloud.job.is_none() {
            if self.cloud.error.is_some() && self.cloud.next_poll > now() {
                return Err("云端领取失败，正在等待重试；请查看云端状态");
            }
            let value = id.clone();
            self.cloud
                .start(self.platform.clone(), move |cfg, p, path| {
                    cloud::lease(cfg, p, path, value)
                })?;
        }
        Ok(json!({"state":"waiting","account_id":format!("cloud-{id}"),"retry_after_ms":750}))
    }
    pub(super) fn cloud_tick(&mut self) {
        self.cloud.poll();
        for a in &mut self.db.accounts {
            if let Some(binding) = self.cloud.config.bindings.get(&a.id) {
                a.automatic = false;
                a.state = if binding.phase == "cloud_managed" {
                    "cloud_managed"
                } else {
                    "cloud_transfer"
                }
                .into();
                a.message = if binding.phase == "cloud_managed" {
                    "凭据由个人云端维护，请在云端模式查看"
                } else {
                    "云端交接尚未完成，请在云端模式恢复"
                }
                .into();
            } else if a.state == "cloud_transfer" {
                a.state = "saved".into();
                a.message = "云端交接已取消；本地自动维护保持关闭，可手动开启".into();
            }
            if self.cloud.config.selected.contains_key(&a.provider)
                && !self
                    .cloud
                    .config
                    .bindings
                    .get(&a.id)
                    .is_some_and(|b| b.native)
            {
                a.native = false;
            }
        }
        if self.cloud.job.is_some()
            || self.automation.job.is_some()
            || self.db.automation.run.is_some()
            || self.job.is_some()
            || !self.check_queue.is_empty()
            || self.pending_login.is_some()
            || self.reset_job.is_some()
            || self.desktop.job.is_some()
            || self.desktop.refresh.is_some()
            || self.db.switch.is_some()
            || self.storage_fault.get()
        {
            return;
        }
        if self.cloud.config.pair.is_some() && self.cloud.next_poll <= now() {
            let _ = self.cloud.start(self.platform.clone(), cloud::pair_poll);
            return;
        }
        if !self.cloud.config.paired {
            return;
        }
        if let Some(id) = self
            .cloud
            .config
            .reauthorizations
            .keys()
            .find(|id| {
                !self.cloud.config.bindings.contains_key(*id)
                    && self
                        .db
                        .accounts
                        .iter()
                        .any(|a| &a.id == *id && a.state == "saved" && a.metadata.is_some())
            })
            .cloned()
        {
            if let Err(e) = self.cloud_command("import", None, None, None, Some(&id), true, None) {
                self.cloud.error = Some(e.into());
            }
            return;
        }
        if self.cloud.config.mode == "cloud" && self.cloud.next_directory <= now() {
            self.cloud.next_directory = now() + 30;
            let _ = self.cloud.start(self.platform.clone(), cloud::directory);
            return;
        }
        if self.cloud.config.mode == "cloud"
            && let Some(id) = self
                .cloud
                .config
                .accounts
                .iter()
                .find(|a| {
                    a["state"] == "ready"
                        && a["id"].as_str().is_some_and(|id| {
                            self.cloud
                                .config
                                .usage_attempts
                                .get(id)
                                .is_none_or(|t| *t + 300 <= now())
                        })
                })
                .and_then(|a| a["id"].as_str())
                .map(str::to_owned)
        {
            let _ = self
                .cloud
                .start(self.platform.clone(), move |cfg, p, path| {
                    cloud::action(cfg, p, path, "usage".into(), json!({"id":id}))
                });
            return;
        }
        if self.cloud.config.pending_activation.is_none() && self.cloud_native_next <= now() {
            let selected: Vec<_> = self
                .cloud
                .config
                .selected
                .iter()
                .filter(|(p, _)| matches!(p.as_str(), "codex" | "claude"))
                .map(|(p, id)| (p.clone(), id.clone()))
                .collect();
            if !selected.is_empty() {
                let (provider, id) = selected[(now() / 30) as usize % selected.len()].clone();
                if let Ok(native) = self.native(&provider) {
                    self.cloud_native_next = now() + 30;
                    let _ = self
                        .cloud
                        .start(self.platform.clone(), move |cfg, p, path| {
                            cloud::native_sync(cfg, p, path, id, native, false, None)
                        });
                }
            }
        }
    }
}
