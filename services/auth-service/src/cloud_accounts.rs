use super::*;
use crate::cloud::{self, Binding};
use std::collections::HashSet;

const VAULT: &str = "io.github.wuxm5.authbaton.vault";
const CLOUD_RENEWS: &str = "这个账号由云端续期，本机不检查也不续期";
const TRANSFERRING: &str = "正在转换续期方，请等转换完成";
const NO_LOCAL_CHECK: &str = "云端续期的账号没有本地检查，可以做云端状态检查";
const STOPPED: &str = "转换没有完成，可以重试或放弃";
const MOVED_AWAY: &str = "已在另一台 Mac 转回本机续期，这台需要重新登录";
const LEFT: &str = "已断开云端配对，这台需要重新登录";
const CLOUD_ROW: &str = "由个人云端续期；这台 Mac 只用领来的 Access";
const TO_CLOUD_ROW: &str = "正在转到云端续期；本机续期已暂停";
const BACK_HERE: &str = "已转回本机续期，由这台 Mac 续期";

impl Accounts {
    /// The binding of a row whose login the cloud renews; none for a row this Mac renews.
    pub(super) fn cloud_binding(&self, id: &str) -> Option<&Binding> {
        self.cloud.config.bindings.get(id).filter(|b| b.cloud())
    }
    /// The cloud account whose leased access a row uses.
    pub(super) fn cloud_lease_id(&self, id: &str) -> Option<&str> {
        self.cloud_binding(id).and_then(|b| b.cloud_id.as_deref())
    }
    /// The row for work that may renew, reconcile or write a refresh token. A row the cloud
    /// renews, or one moving between the cloud and this Mac, stops here.
    pub(super) fn local_index(&self, id: &str) -> Result<usize> {
        let index = self.index(id)?;
        self.external_local_guard(id)?;
        match self.cloud.config.bindings.get(id) {
            None => Ok(index),
            Some(b) if b.cloud() => Err(CLOUD_RENEWS),
            Some(_) => Err(TRANSFERRING),
        }
    }
    /// A row moving between the cloud and this Mac takes no other operation while that runs.
    /// One that stopped can still be switched to (`switch`): its login is still where it was.
    pub(super) fn transfer_guard(&self, id: &str, switch: bool) -> Result<()> {
        let Some(b) = self.cloud.config.bindings.get(id) else {
            return Ok(());
        };
        if b.phase == cloud::CLOUD_MANAGED {
            return Ok(());
        }
        // Once the cloud may have committed an import, only the cloud renews the login.
        let stopped = self.cloud.job_row.as_deref() != Some(id)
            && matches!(b.phase.as_str(), "prepared" | "imported" | cloud::EXPORTING);
        if switch && stopped {
            Ok(())
        } else {
            Err(TRANSFERRING)
        }
    }
    /// Whether a switch to this row can go ahead: a cloud row needs its leased access, any other
    /// a saved login in good standing.
    pub(super) fn switch_ready(&self, account: &Account) -> Result<()> {
        if let Some(leased) = self.external_bundle(&account.id) {
            return self.external_switchable(&account.id).and(leased.map(drop));
        }
        match self.cloud_lease_id(&account.id) {
            Some(cloud_id) => self.cloud.lease_bundle(cloud_id).map(drop),
            None => switch_target_ready(account),
        }
    }
    /// Quota for a cloud row waits for its leased access instead of failing without it.
    pub(super) fn quota_access_ready(&self, account: &Account) -> bool {
        if let Some(ready) = self.external_quota_ready(&account.id) {
            return ready;
        }
        self.cloud_lease_id(&account.id)
            .is_none_or(|cloud_id| self.cloud.lease(cloud_id).is_some())
    }
    pub fn begin_cloud_login(&mut self, cloud_id: &str, login: &mut LoginManager) -> Result<()> {
        self.ensure_idle()?;
        if !self.cloud.config.paired {
            return Err("请先完成云端配对");
        }
        if self
            .cloud
            .config
            .bindings
            .values()
            .any(|b| b.cloud_id.as_deref() == Some(cloud_id) && b.phase != cloud::CLOUD_MANAGED)
        {
            return Err(TRANSFERRING);
        }
        let cloud = self.cloud.account(cloud_id).ok_or("云端账号不存在")?;
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
    /// What the account card shows about the cloud; null for a row this Mac renews.
    pub(super) fn cloud_view(&self, account: &Account) -> Value {
        let Some(b) = self.cloud.config.bindings.get(&account.id) else {
            return Value::Null;
        };
        let cloud_id = b.cloud_id.as_deref();
        let directory = cloud_id.and_then(|id| self.cloud.account(id));
        let lease = cloud_id.and_then(|id| self.cloud.lease(id));
        let running = self.cloud.job_row.as_deref() == Some(account.id.as_str());
        let direction = if cloud::TO_CLOUD.contains(&b.phase.as_str()) {
            Some("to_cloud")
        } else if b.phase == cloud::EXPORTING {
            Some("to_local")
        } else {
            None
        };
        let transfer = direction.map(|direction| {
            let error = (!running).then(|| b.error.as_deref().unwrap_or(STOPPED));
            json!({"direction":direction,"running":running,"error":error})
        });
        let state = if b.phase == cloud::EXPORTING {
            Some("exporting")
        } else {
            match directory.and_then(|d| d["state"].as_str()) {
                // Renewing right now; the access it hands out is still good.
                Some("ready" | "refreshing") => Some("ready"),
                Some(
                    s @ ("reauthorization_required"
                    | "uncertain"
                    | "verification_pending"
                    | "exporting"),
                ) => Some(s),
                _ => None,
            }
        };
        let error = directory
            .and_then(|d| d["error"].as_str())
            .filter(|_| state.is_some_and(|s| s != "ready" && s != "exporting"))
            .map(cloud::account_error);
        let this = self.cloud.config.device_id.as_deref();
        let used_by: Vec<Value> = directory
            .and_then(|d| d["used_by"].as_array())
            .map(|u| {
                u.iter()
                    .map(|d| json!({"label":d["label"],"at":d["at"],"this":this.is_some_and(|t| d["device_id"] == t)}))
                    .collect()
            })
            .unwrap_or_default();
        let expires = lease
            .as_ref()
            .and_then(|l| l["expires_at"].as_u64())
            .or_else(|| directory.and_then(|d| d["expires_at"].as_u64()));
        let ruled = self
            .db
            .automation
            .config
            .accounts
            .iter()
            .any(|r| r.id == account.id);
        json!({
            "id": cloud_id,
            "maintainer": if b.cloud() { "cloud" } else { "local" },
            "origin": if b.other() { "other" } else { "this" },
            "fresh": b.other() && !b.used && !ruled,
            "transfer": transfer,
            "state": state,
            "error": error,
            "used_by": used_by,
            "last_refresh": directory.and_then(|d| d["last_refresh"].as_u64()),
            // The cloud renews within the last 15 minutes of an access.
            "next_refresh": expires.map(|t| t.saturating_sub(900)),
            "access_expires_at": expires,
        })
    }
    pub fn cloud_command(
        &mut self,
        action: &str,
        endpoint: Option<&str>,
        label: Option<&str>,
        id: Option<&str>,
        confirm: bool,
    ) -> Result<()> {
        self.ensure_idle()?;
        match action {
            "configure" => self.cloud.configure(endpoint, label),
            "pair" => self.cloud.start(self.platform.clone(), cloud::pair),
            "pair_poll" => self
                .cloud
                .start_light("pairing", self.platform.clone(), cloud::pair_poll),
            "refresh" => {
                self.cloud
                    .start_light("directory", self.platform.clone(), cloud::sync_directory)
            }
            "import" | "resume_import" => {
                if !confirm {
                    return Err("请确认将所选账号交给个人云端维护");
                }
                if !self.cloud.config.paired {
                    return Err("请先完成设备配对");
                }
                let id = id.ok_or("缺少本机账号")?.to_string();
                let i = self.index(&id)?;
                if self.cloud_binding(&id).is_some() {
                    return Err("这个账号已由云端续期");
                }
                if self.is_external(&id) {
                    return Err("外部账号由来源续期，不能转到云端");
                }
                let a = self.db.accounts[i].clone();
                if a.native && (self.client_running)(&a.provider) {
                    return Err("请先结束任务并退出对应原生客户端，再交接；无需重新登录");
                }
                if matches!(
                    a.state.as_str(),
                    "unsaved" | "uncertain" | "pending" | "checking"
                ) || (self.db.desktop.enabled
                    && self.db.desktop.selected.as_deref() == Some(&id))
                {
                    return Err("此账号有待处理操作，请先完成恢复或解除桌面实验接管");
                }
                match self.cloud.config.bindings.get_mut(&id) {
                    // A stopped transfer resumes with the row as it is now: it may have been put
                    // to use meanwhile, and then the native login is handed over too.
                    Some(b) => {
                        b.native = a.native;
                        b.error = None;
                    }
                    None => {
                        let binding = Binding {
                            operation: random_id()?,
                            cloud_id: None,
                            owner: a.metadata.as_ref().ok_or("尚未获取账号身份")?.owner.clone(),
                            provider: a.provider.clone(),
                            native: a.native,
                            phase: "prepared".into(),
                            replace_id: self.cloud.config.reauthorizations.get(&id).cloned(),
                            ..Default::default()
                        };
                        self.cloud.config.bindings.insert(id.clone(), binding);
                    }
                }
                self.cloud.save()?;
                self.db.accounts[i].automatic = false;
                self.db.accounts[i].state = "cloud_transfer".into();
                self.db.accounts[i].message = TO_CLOUD_ROW.into();
                self.save()?;
                let native = a.native.then(|| self.native(&a.provider)).transpose()?;
                let row = id.clone();
                self.cloud
                    .start(self.platform.clone(), move |cfg, p, path| {
                        cloud::transfer(cfg, p, path, row, a.label, native)
                    })?;
                self.cloud.job_row = Some(id);
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
                if !cloud::TO_CLOUD.contains(&b.phase.as_str()) {
                    return Err("云端已接管，不能恢复旧刷新凭据");
                }
                if b.native && (self.client_running)(&b.provider) {
                    return Err("请先退出对应客户端再取消交接");
                }
                let native = b.native.then(|| self.native(&b.provider)).transpose()?;
                let row = id.clone();
                self.cloud
                    .start(self.platform.clone(), move |cfg, p, path| {
                        cloud::cancel_transfer(cfg, p, path, row, native)
                    })?;
                self.cloud.job_row = Some(id);
                Ok(())
            }
            "export" | "cancel_export" => {
                if !confirm {
                    return Err(if action == "export" {
                        "请确认把这个账号转回本机续期"
                    } else {
                        "请确认放弃转回本机"
                    });
                }
                let id = id.ok_or("缺少本机账号")?.to_string();
                let a = self.db.accounts[self.index(&id)?].clone();
                let b = self
                    .cloud_binding(&id)
                    .ok_or("这个账号不由云端续期")?
                    .clone();
                if action == "cancel_export" && b.phase != cloud::EXPORTING {
                    return Err("没有进行中的转回本机");
                }
                let in_use = a.native
                    && b.cloud_id.is_some()
                    && self.cloud.config.selected.get(&a.provider) == b.cloud_id.as_ref();
                // Codex rereads its login only when it starts; the desktop on its own login is fine.
                if action == "export"
                    && in_use
                    && a.provider == "codex"
                    && (self.client_running)("codex")
                    && !self.desktop_owns(&id)
                {
                    return Err("请先退出 Codex 客户端再转回本机；无需重新登录");
                }
                let native = in_use.then(|| self.native(&a.provider)).transpose()?;
                let row = id.clone();
                if action == "export" {
                    self.cloud
                        .start(self.platform.clone(), move |cfg, p, path| {
                            cloud::export(cfg, p, path, row, native)
                        })?;
                } else {
                    self.cloud
                        .start(self.platform.clone(), move |cfg, p, path| {
                            cloud::cancel_export(cfg, p, path, row, native)
                        })?;
                }
                self.cloud.job_row = Some(id);
                Ok(())
            }
            "remove" => {
                if !confirm {
                    return Err("请确认从认证中心移除这个账号");
                }
                let id = id.ok_or("缺少本机账号")?.to_string();
                let a = self.db.accounts[self.index(&id)?].clone();
                let b = self
                    .cloud
                    .config
                    .bindings
                    .get(&id)
                    .ok_or("这个账号不由云端续期")?;
                if b.phase != cloud::CLOUD_MANAGED {
                    return Err(TRANSFERRING);
                }
                if a.native {
                    return Err("在用的账号不能移除，请先换上其他账号");
                }
                if self
                    .db
                    .reset_attempts
                    .iter()
                    .any(|attempt| attempt.account_id == id && attempt.outcome.unresolved())
                {
                    return Err("此账号的额度重置结果待确认，请先重试上次操作");
                }
                self.quota.poll(now());
                if self.quota.is_running(&id) {
                    return Err("此账号正在查询额度，请等查完再移除");
                }
                self.warmup_finish();
                if self.warmup.running(&id) {
                    return Err("此账号正在发送预热，请等发完再移除");
                }
                let row = id.clone();
                self.cloud
                    .start(self.platform.clone(), move |cfg, p, path| {
                        cloud::remove(cfg, p, path, row)
                    })?;
                self.cloud.job_row = Some(id);
                Ok(())
            }
            "leave" => {
                if !confirm {
                    return Err("请确认断开与认证中心的配对");
                }
                if !self.cloud.config.paired {
                    return Err("这台 Mac 还没有配对");
                }
                if ["codex", "claude"]
                    .iter()
                    .any(|p| self.cloud.config.selected.contains_key(*p))
                {
                    return Err("有云端续期的账号正在使用，请先换上本机续期的账号");
                }
                if self
                    .cloud
                    .config
                    .bindings
                    .values()
                    .any(|b| b.phase != cloud::CLOUD_MANAGED)
                    || !self.cloud.config.export_commits.is_empty()
                {
                    return Err("有没结束的转换，请先完成或放弃");
                }
                self.cloud.start(self.platform.clone(), cloud::leave)
            }
            "check" => {
                let id = id.ok_or("缺少本机账号")?;
                self.check_cloud(id)
            }
            _ => Err("不支持此云端操作"),
        }
    }
    /// 认证检查 of a cloud row: the cloud checks the login, and this Mac reads the quota with the
    /// leased access at once.
    pub(super) fn check_cloud(&mut self, id: &str) -> Result<()> {
        self.transfer_guard(id, false)?;
        let cloud_id = self
            .cloud_lease_id(id)
            .ok_or("这个账号不由云端续期")?
            .to_string();
        let tag = format!("check:{cloud_id}");
        self.cloud
            .start_light(&tag, self.platform.clone(), move |cfg, p| {
                cloud::check(cfg, p, cloud_id)
            })?;
        self.quota.request(id, now());
        Ok(())
    }
    pub(super) fn no_local_check(&self, id: &str) -> Result<()> {
        self.external_local_guard(id)?;
        if self.cloud.config.bindings.contains_key(id) {
            return Err(if self.cloud_binding(id).is_some() {
                NO_LOCAL_CHECK
            } else {
                TRANSFERRING
            });
        }
        Ok(())
    }
    /// Puts a cloud row to use: its leased access-only login goes into the native store under the
    /// official locks. The local login it replaces is kept, as a local switch keeps it; an
    /// access-only login it replaces is not kept anywhere.
    pub(super) fn activate_cloud(&mut self, id: &str) -> Result<()> {
        let row = self.db.accounts[self.index(id)?].clone();
        let cloud_id = self.cloud_lease_id(id).ok_or(CLOUD_RENEWS)?.to_string();
        let target = self.cloud.lease_bundle(&cloud_id)?;
        if target.provider != row.provider {
            return Err("云端账号类型不一致");
        }
        if target
            .access_metadata()?
            .expires_at
            .is_none_or(|t| t <= now() + 60)
        {
            return Err("云端 Access 即将到期，请稍后再试");
        }
        self.activate_leased(id, &row.provider, |m, native| {
            if let Some(b) = m.cloud.config.bindings.get_mut(id) {
                b.used = true;
            }
            m.cloud.activate(&m.platform, native, &cloud_id, &target)?;
            m.release_external_selection(&target.provider);
            Ok(())
        })
    }
    /// Puts a row whose login here is leased access to use: `publish` writes it into the native
    /// store. A local login it replaces is kept first, as a local switch keeps it; an access-only
    /// login it replaces is not kept anywhere.
    pub(super) fn activate_leased(
        &mut self,
        id: &str,
        provider: &str,
        publish: impl FnOnce(&mut Self, &NativeStore) -> Result<()>,
    ) -> Result<()> {
        let label = self.db.accounts[self.index(id)?].label.clone();
        if provider != "claude" && (self.client_running)(provider) {
            return Err("官方客户端仍在运行，请退出后再切换");
        }
        if provider == "claude" {
            for current in self
                .db
                .accounts
                .iter()
                .filter(|a| a.provider == "claude" && a.native)
            {
                if self
                    .platform
                    .vault_read_optional(&crate::claude::journal_id(&current.id))?
                    .is_some()
                {
                    return Err("当前 Claude 账号的续期结果正在自动确认，请稍后再切换");
                }
            }
        }
        // A renewal of the access in use still writing would land over this switch.
        if self.native_sync_running(provider) {
            return Err("正在写入在用账号的登录，请几秒后再试");
        }
        if provider == "codex" {
            private_dir(&self.sources.codex_home)?;
        }
        let native = self.native(provider)?;
        if let Some(old) = native
            .login_or_projection(&self.platform)?
            .filter(|b| b.metadata().is_ok())
        {
            let placed = if old.provider == "claude" {
                self.place_claude_login(old, None)
            } else {
                Ok(old)
            };
            let placed = match placed {
                Ok(placed) => placed,
                Err(message) => {
                    for a in &mut self.db.accounts {
                        if a.provider == "claude" && a.native {
                            a.next_check = now();
                        }
                    }
                    return Err(message);
                }
            };
            self.adopt_native(placed, "")?;
            self.save()?;
        }
        publish(self, &native)?;
        for a in &mut self.db.accounts {
            if a.provider == provider {
                a.native = a.id == id;
            }
        }
        let index = self.index(id)?;
        self.db.accounts[index].last_local = Some(now());
        if provider == "claude" {
            self.claude_mod.change = Some(crate::claude_mod::Change {
                at: now(),
                kind: "switch",
                label,
            });
        }
        let saved = self.save();
        if provider == "claude" {
            self.sync_claude_desktop_relay(true);
        }
        saved
    }
    /// A local login took the native store over from a cloud or external row.
    pub(super) fn release_cloud_use(&mut self, provider: &str) {
        let external = self.release_external_selection(provider);
        if self.cloud.config.selected.remove(provider).is_some() {
            let _ = self
                .platform
                .delete_key(VAULT, &format!("cloud-native-{provider}"));
            if self.cloud.save().is_err() {
                self.notice = Some("云端在用记录保存失败；下次同步会再核对".into());
            }
        } else if external {
            let _ = self
                .platform
                .delete_key(VAULT, &format!("cloud-native-{provider}"));
        }
    }
    /// pi's access for a cloud row: the leased access, never a refresh token.
    pub(super) fn pi_cloud_access(
        &mut self,
        reply_id: &str,
        account: &Account,
        cloud_id: &str,
    ) -> Result<Value> {
        if let Some(lease) = self.cloud.lease(cloud_id) {
            let bundle = cloud::projection_of(&lease)?;
            let owner = bundle.access_metadata()?.owner;
            if account.metadata.as_ref().is_some_and(|m| m.owner != owner) {
                return Err("云端 Access 与登记账号身份不一致，未向 pi 提供凭据");
            }
            let token = lease["access_token"]
                .as_str()
                .filter(|t| {
                    !t.chars().any(char::is_control)
                        && bundle.credential["tokens"]["access_token"] == *t
                })
                .ok_or("云端 Access 格式无效")?;
            self.pi_last_access = Some((account.id.clone(), now()));
            return Ok(
                json!({"state":"ready","account_id":reply_id,"label":account.label,"access_token":token,"expires_at":lease["expires_at"]}),
            );
        }
        let state = self
            .cloud
            .account(cloud_id)
            .and_then(|d| d["state"].as_str());
        if state != Some("ready") {
            return Err("云端账号需要处理，请在 AuthBaton 查看");
        }
        if self
            .cloud
            .lease_tried
            .get(cloud_id)
            .is_none_or(|t| *t + 5 <= now())
        {
            self.cloud.lease_tried.insert(cloud_id.into(), now());
            let value = cloud_id.to_string();
            self.cloud.start_light(
                &format!("lease:{cloud_id}"),
                self.platform.clone(),
                move |cfg, p| cloud::lease(cfg, p, value, None),
            )?;
        }
        Ok(json!({"state":"waiting","account_id":reply_id,"retry_after_ms":750}))
    }
    pub(super) fn cloud_tick(&mut self) {
        self.cloud.poll();
        self.reconcile_cloud_rows();
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
            self.cloud.next_poll = now() + 30;
            let _ = self
                .cloud
                .start_light("pairing", self.platform.clone(), cloud::pair_poll);
            return;
        }
        if !self.cloud.config.paired {
            return;
        }
        if self.cloud.config.pending_activation.is_some() {
            if self.cloud.next_poll > now() {
                return;
            }
            self.cloud.next_poll = now() + 30;
            let stores: HashMap<&str, NativeStore> = ["codex", "claude"]
                .into_iter()
                .filter_map(|p| self.native(p).ok().map(|n| (p, n)))
                .collect();
            if let Err(e) = self.cloud.settle(&self.platform, &stores) {
                self.cloud.error = Some(e.into());
            }
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
            if let Err(e) = self.cloud_command("import", None, None, Some(&id), true) {
                self.cloud.error = Some(e.into());
            }
            return;
        }
        // Routine reads run one at a time, beside anything a person starts.
        if !self.cloud.light.is_empty() {
            return;
        }
        if let Some(id) = self.cloud.config.export_commits.keys().next().cloned()
            && self.cloud.next_commit <= now()
        {
            self.cloud.next_commit = now() + 60;
            let tag = format!("commit:{id}");
            let _ = self
                .cloud
                .start_light(&tag, self.platform.clone(), move |cfg, p| {
                    cloud::commit_export(cfg, p, id)
                });
            return;
        }
        if self.cloud.next_directory <= now() {
            self.cloud.next_directory = now() + 30;
            let _ = self
                .cloud
                .start_light("directory", self.platform.clone(), cloud::sync_directory);
            return;
        }
        // Every cloud row keeps a lease that is good for at least 15 more minutes; one at a time.
        if let Some(id) = self
            .cloud
            .config
            .bindings
            .values()
            .filter(|b| b.cloud())
            .filter_map(|b| {
                b.cloud_id
                    .clone()
                    .map(|id| (id, b.phase == cloud::EXPORTING))
            })
            .find(|(id, ours)| {
                let state = self.cloud.account(id).and_then(|d| d["state"].as_str());
                (state == Some("ready") || (*ours && state == Some("exporting")))
                    && self.cloud.lease_due(id)
                    && self
                        .cloud
                        .lease_tried
                        .get(id)
                        .is_none_or(|t| *t + 60 <= now())
            })
            .map(|(id, _)| id)
        {
            self.cloud.lease_tried.insert(id.clone(), now());
            let tag = format!("lease:{id}");
            let _ = self
                .cloud
                .start_light(&tag, self.platform.clone(), move |cfg, p| {
                    cloud::lease(cfg, p, id, None)
                });
            return;
        }
        if self.cloud_native_next <= now() {
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
                    let tag = format!("sync:{provider}");
                    let _ = self
                        .cloud
                        .start_light(&tag, self.platform.clone(), move |cfg, p| {
                            cloud::native_sync(cfg, p, id, native)
                        });
                }
            }
        }
    }
    /// Brings the rows in line with the cloud: what finished cloud operations decided, accounts
    /// gone from the directory, accounts other Macs moved up, and which row is in use. Runs only
    /// between cloud jobs, which carry their own copy of the configuration.
    pub(super) fn reconcile_cloud_rows(&mut self) {
        if self.cloud.job.is_some() || self.storage_fault.get() {
            return;
        }
        let mut rows = false;
        let mut config = false;
        let orphans: Vec<String> = self
            .cloud
            .config
            .bindings
            .keys()
            .filter(|id| !self.db.accounts.iter().any(|a| &a.id == *id))
            .cloned()
            .collect();
        for id in orphans {
            self.cloud.config.bindings.remove(&id);
            config = true;
        }
        let mut ended: Vec<String> = vec![];
        let settled: Vec<(String, String)> = self
            .cloud
            .config
            .bindings
            .iter()
            .filter(|(_, b)| cloud::SETTLED.contains(&b.phase.as_str()))
            .map(|(id, b)| (id.clone(), b.phase.clone()))
            .collect();
        for (id, phase) in settled {
            match phase.as_str() {
                "local" => self.become_local(&id),
                "removed" => self.drop_row(&id),
                "lost" => self.mark_lost(&id, MOVED_AWAY, false),
                "left" => self.mark_lost(&id, LEFT, false),
                // The cloud renewed the login before the move back was confirmed: the copy
                // saved here is spent.
                _ => self.mark_lost(&id, cloud::EXPORT_NOT_ACTIVE, true),
            }
            ended.push(id);
        }
        if let Some(checked) = self
            .cloud
            .config
            .checked_at
            .filter(|_| self.cloud.config.paired)
        {
            let gone: Vec<(String, bool)> = self
                .cloud
                .config
                .bindings
                .iter()
                .filter(|(_, b)| {
                    b.phase == cloud::CLOUD_MANAGED
                        && b.since < checked
                        && b.cloud_id.as_ref().is_some_and(|c| {
                            self.cloud.account(c).is_none()
                                && !self.cloud.config.export_commits.contains_key(c)
                        })
                })
                .map(|(id, b)| (id.clone(), b.other()))
                .collect();
            for (id, other) in gone {
                let in_use = self.db.accounts.iter().any(|a| a.id == id && a.native);
                if other && !in_use {
                    self.drop_row(&id);
                } else {
                    self.mark_lost(&id, MOVED_AWAY, false);
                }
                ended.push(id);
            }
        }
        // 更新云端登录 makes a row of its own; once its login reaches the cloud the account's row
        // stays and that one goes.
        let bindings = &self.cloud.config.bindings;
        let replaced: Vec<String> = bindings
            .iter()
            .filter(|(id, b)| {
                b.cloud()
                    && b.replace_id.is_some()
                    && bindings.iter().any(|(other, o)| {
                        other != *id
                            && o.cloud()
                            && o.cloud_id == b.cloud_id
                            && o.replace_id.is_none()
                    })
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in replaced {
            self.drop_row(&id);
            self.cloud.config.reauthorizations.remove(&id);
            ended.push(id);
        }
        // A row from another Mac that turns out to be this Mac's own login of the account.
        let bindings = &self.cloud.config.bindings;
        let duplicates: Vec<String> = bindings
            .iter()
            .filter(|(id, b)| {
                b.other()
                    && b.phase == cloud::CLOUD_MANAGED
                    && !b.owner.is_empty()
                    && !self.db.accounts.iter().any(|a| a.id == **id && a.native)
                    && self.db.accounts.iter().any(|a| {
                        a.provider == b.provider
                            && !bindings.contains_key(&a.id)
                            && !self.is_external(&a.id)
                            && a.metadata.as_ref().is_some_and(|m| m.owner == b.owner)
                    })
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in duplicates {
            self.drop_row(&id);
            ended.push(id);
        }
        for id in ended {
            rows = true;
            config = true;
            if let Some(b) = self.cloud.config.bindings.remove(&id)
                && b.cloud_id.is_some()
                && self.cloud.config.selected.get(&b.provider) == b.cloud_id.as_ref()
            {
                self.cloud.config.selected.remove(&b.provider);
                let _ = self
                    .platform
                    .delete_key(VAULT, &format!("cloud-native-{}", b.provider));
            }
        }
        for b in self.cloud.config.bindings.values_mut() {
            // Alone with its cloud id, a renewed login's row is the account's row now.
            if b.replace_id.is_some() && b.cloud() {
                b.replace_id = None;
                config = true;
            }
        }
        if self.cloud.config.paired {
            let known: HashSet<String> = self
                .cloud
                .config
                .bindings
                .values()
                .filter_map(|b| b.cloud_id.clone())
                .collect();
            let arrivals: Vec<Value> = self
                .cloud
                .config
                .accounts
                .iter()
                .filter(|d| {
                    let Some(id) = d["id"].as_str() else {
                        return false;
                    };
                    let provider = d["provider"].as_str().unwrap_or("");
                    let owner = self.cloud.config.owners.get(id);
                    !known.contains(id)
                        && !self.cloud.config.export_commits.contains_key(id)
                        && matches!(provider, "codex" | "claude")
                        // Not yet handed over, or on its way back to some Mac.
                        && d["state"] != "awaiting_handoff"
                        && d["state"] != "exporting"
                        // Two separate logins of one account: this Mac's own stays as it is.
                        && !owner.is_some_and(|owner| {
                            self.db.accounts.iter().any(|a| {
                                a.provider == provider
                                    && !self.cloud.config.bindings.contains_key(&a.id)
                                    && !self.is_external(&a.id)
                                    && a.metadata.as_ref().is_some_and(|m| m.owner == *owner)
                            })
                        })
                })
                .cloned()
                .collect();
            for d in arrivals {
                let Ok(row) = random_id() else {
                    break;
                };
                let provider = d["provider"].as_str().unwrap_or("").to_string();
                let cloud_id = d["id"].as_str().unwrap_or("").to_string();
                self.db.accounts.push(Account {
                    id: row.clone(),
                    provider: provider.clone(),
                    label: d["label"]
                        .as_str()
                        .and_then(|l| valid_label(l).ok())
                        .unwrap_or_else(|| placeholder_label(&provider).into()),
                    metadata: None,
                    native: false,
                    automatic: false,
                    state: "cloud_managed".into(),
                    message: CLOUD_ROW.into(),
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
                self.cloud.config.bindings.insert(
                    row,
                    Binding {
                        owner: self
                            .cloud
                            .config
                            .owners
                            .get(&cloud_id)
                            .cloned()
                            .unwrap_or_default(),
                        cloud_id: Some(cloud_id),
                        provider,
                        phase: cloud::CLOUD_MANAGED.into(),
                        origin: "other".into(),
                        since: now(),
                        ..Default::default()
                    },
                );
                rows = true;
                config = true;
            }
        }
        for index in 0..self.db.accounts.len() {
            let id = self.db.accounts[index].id.clone();
            let binding = self.cloud.config.bindings.get(&id).cloned();
            let (state, message) = match &binding {
                Some(b) if b.cloud() => ("cloud_managed", CLOUD_ROW),
                Some(b) if cloud::TO_CLOUD.contains(&b.phase.as_str()) => {
                    ("cloud_transfer", TO_CLOUD_ROW)
                }
                Some(_) => continue,
                None if self.db.accounts[index].state == "cloud_transfer" => {
                    ("saved", "云端交接已取消；本地自动维护保持关闭，可手动开启")
                }
                None if self.db.accounts[index].state == "cloud_managed" => ("reauth", MOVED_AWAY),
                None => continue,
            };
            let a = &mut self.db.accounts[index];
            if a.state != state || a.message != message || (binding.is_some() && a.automatic) {
                a.state = state.into();
                a.message = message.into();
                if binding.is_some() {
                    a.automatic = false;
                }
                rows = true;
            }
            let Some(b) = binding.filter(|b| b.cloud()) else {
                continue;
            };
            let Some(metadata) = b
                .cloud_id
                .as_deref()
                .and_then(|c| self.cloud.lease_bundle(c).ok())
                .and_then(|bundle| bundle.cloud_metadata().ok())
            else {
                continue;
            };
            if b.owner.is_empty()
                && let Some(binding) = self.cloud.config.bindings.get_mut(&id)
            {
                binding.owner = metadata.owner.clone();
                config = true;
            }
            let a = &mut self.db.accounts[index];
            if serde_json::to_value(&a.metadata).ok() != serde_json::to_value(Some(&metadata)).ok()
            {
                a.metadata = Some(metadata);
                name_after_email(a);
                rows = true;
            }
        }
        for provider in ["codex", "claude"] {
            let Some(cloud_id) = self.cloud.config.selected.get(provider) else {
                continue;
            };
            let Some(row) = self
                .cloud
                .config
                .bindings
                .iter()
                .find(|(_, b)| b.cloud() && b.cloud_id.as_ref() == Some(cloud_id))
                .map(|(id, _)| id.clone())
            else {
                continue;
            };
            for a in &mut self.db.accounts {
                if a.provider == provider && a.native != (a.id == row) {
                    a.native = a.id == row;
                    rows = true;
                }
            }
        }
        // Rows first: a binding outlives a row change that was not saved, and is applied again.
        if rows && self.save().is_err() {
            return;
        }
        if config && let Err(message) = self.cloud.save() {
            self.cloud.error = Some(message.into());
        }
    }
    fn become_local(&mut self, id: &str) {
        let Ok(index) = self.index(id) else {
            return;
        };
        let saved = self
            .platform
            .vault_read(id)
            .and_then(Bundle::decode)
            .and_then(|b| b.metadata());
        let a = &mut self.db.accounts[index];
        a.automatic = true;
        a.failures = 0;
        a.next_check = now() + 30;
        match saved {
            Ok(metadata) => {
                a.metadata = Some(metadata);
                a.state = "saved".into();
                a.message = BACK_HERE.into();
                name_after_email(a);
            }
            Err(_) => {
                a.state = "reauth".into();
                a.message = "转回本机后没读到保存的登录，请重新登录".into();
            }
        }
    }
    fn drop_row(&mut self, id: &str) {
        let Ok(index) = self.index(id) else {
            return;
        };
        let _ = self.platform.delete_key(VAULT, id);
        self.db.accounts.remove(index);
        self.unsaved.remove(id);
        self.quota.forget(id);
        self.warmup.forget(id);
    }
    fn mark_lost(&mut self, id: &str, message: &str, spent: bool) {
        let Ok(index) = self.index(id) else {
            return;
        };
        if spent {
            let _ = self.platform.delete_key(VAULT, id);
        }
        let a = &mut self.db.accounts[index];
        a.state = "reauth".into();
        a.message = message.into();
        a.automatic = true;
    }
}
