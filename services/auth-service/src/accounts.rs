use crate::credentials::{Bundle, Metadata, NativeStore, SIGNED_OUT, external_client_running};
use crate::login::LoginManager;
use crate::maintenance::{Outcome, Quota, Work};
use crate::platform::{Platform, Result, private_dir, random_id};
use crate::status::{Sources, now};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub metadata: Option<Metadata>,
    pub native: bool,
    pub automatic: bool,
    pub state: String,
    pub message: String,
    pub last_local: Option<u64>,
    pub last_online: Option<u64>,
    pub last_refresh: Option<u64>,
    pub next_check: u64,
    pub failures: u32,
    pub quota: Vec<Quota>,
    pub pending_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warmup: Option<crate::warmup::Plan>,
    /// A switched-off plan keeps its settled slot here, under its own key so a build without this
    /// field reads warm-up as off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warmup_off: Option<crate::warmup::Plan>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Switch {
    provider: String,
    target: String,
    previous: Option<String>,
    #[serde(default)]
    desktop: bool,
}
#[derive(Serialize, Deserialize)]
struct Database {
    version: u8,
    accounts: Vec<Account>,
    switch: Option<Switch>,
    #[serde(default)]
    desktop: crate::desktop::Config,
    #[serde(default)]
    reset_attempts: Vec<crate::resets::Attempt>,
    #[serde(default)]
    automation: crate::automation::Store,
}
struct Job {
    id: String,
    handle: JoinHandle<JobResult>,
    cancel: Arc<AtomicBool>,
}
enum JobResult {
    Check(Box<Outcome>),
    Observed(Bundle, Option<&'static str>),
    ReadError(&'static str),
    Recovery(Result<(Bundle, bool)>),
    Switch(Result<Option<Bundle>>),
}
#[derive(Clone)]
struct CheckRequest {
    id: String,
    online: bool,
}
pub struct Accounts {
    root: PathBuf,
    platform: Platform,
    db: Database,
    home: PathBuf,
    sources: Sources,
    codex: Option<PathBuf>,
    job: Option<Job>,
    active_check: Option<CheckRequest>,
    check_queue: VecDeque<CheckRequest>,
    pending_login: Option<String>,
    last_scan: u64,
    pub notice: Option<String>,
    unsaved: HashMap<String, (Bundle, Bundle)>,
    storage_fault: Cell<bool>,
    client_running: fn(&str) -> bool,
    desktop: crate::desktop::Runtime,
    quota: crate::quota::Runtime,
    warmup: crate::warmup::Runtime,
    client_sources: Vec<crate::clients::Source>,
    clients_checked_at: Option<u64>,
    pi_last_access: Option<(String, u64)>,
    cloud: crate::cloud::Runtime,
    cloud_native_next: u64,
    reset_job: Option<(String, JoinHandle<crate::resets::Outcome>)>,
    automation: crate::automation::Runtime,
    claude_mod: crate::claude_mod::Runtime,
    #[cfg(test)]
    reset_transport: Option<crate::codex_api::Transport>,
    #[cfg(test)]
    claude_reset_transport: Option<crate::claude_resets::Transport>,
    #[cfg(test)]
    native_override: Option<NativeStore>,
    #[cfg(test)]
    profile_transport: Option<crate::maintenance::Transport>,
}
impl Accounts {
    pub fn open(
        root: &Path,
        home: &Path,
        sources: Sources,
        codex: Option<PathBuf>,
    ) -> Result<Self> {
        private_dir(root)?;
        let bundled = std::env::current_exe()
            .map_err(|_| "无法定位原生存储工具")?
            .with_file_name("credential-store");
        let platform = Platform {
            helper: crate::platform::install_helper(root, &bundled)?,
            recovery_wait: None,
            #[cfg(test)]
            fixture_keychain: false,
        };
        Self::with_platform(root, home, sources, codex, platform)
    }
    fn with_platform(
        root: &Path,
        home: &Path,
        sources: Sources,
        codex: Option<PathBuf>,
        platform: Platform,
    ) -> Result<Self> {
        let value = platform.load_db(&root.join("accounts.sqlite3"))?;
        let mut db: Database = match value {
            Some(mut value) => {
                let version = value["version"].as_u64().ok_or("账号数据库格式无效")?;
                if !(1..=8).contains(&version) {
                    return Err("账号数据库版本不受支持，未覆盖其内容");
                }
                if version <= 7
                    && let Some(config) = value.pointer_mut("/automation/config")
                {
                    *config = serde_json::to_value(crate::automation::Config::from_legacy(
                        config.clone(),
                        version,
                    )?)
                    .map_err(|_| "无法迁移旧版额度策略")?;
                }
                serde_json::from_value(value).map_err(|_| "账号数据库格式无效")?
            }
            None => Database {
                version: 8,
                accounts: vec![],
                switch: None,
                desktop: crate::desktop::Config::default(),
                reset_attempts: vec![],
                automation: crate::automation::Store::default(),
            },
        };
        db.version = 8;
        db.automation.config.validate()?;
        // A POST is only possible after its Attempt is durable. Orphan admission, NotSent and
        // NoCredit receipts prove no consumption and must not lock an account's reset indefinitely.
        // A switch latch has no Attempt; it holds until the account's quota recovers.
        db.automation.latches.retain(|id, latch| {
            latch.kind == crate::automation::LatchKind::Switch
                || db.reset_attempts.iter().any(|a| {
                    a.account_id == *id
                        && a.request_id == latch.request_id
                        && !matches!(
                            a.outcome,
                            crate::resets::Outcome::NotSent | crate::resets::Outcome::NoCredit
                        )
                })
        });
        if let Some(run) = &mut db.automation.run {
            run.phase = crate::automation::Phase::Attention;
            db.automation.config.enabled = false;
            db.automation.last_message =
                "上次自动流程被中断；请核对桌面会话后结束此流程，不会自动重放中断或接续".into();
        }
        for attempt in &mut db.reset_attempts {
            if attempt.outcome == crate::resets::Outcome::Pending {
                attempt.outcome = crate::resets::Outcome::Unknown;
                if let Some(execution) = attempt.executions.last_mut() {
                    execution.outcome = crate::resets::Outcome::Unknown;
                    // A restart is not evidence of when the remote request finished.
                    execution.finished_at = None;
                }
            }
        }
        let mut manager = Self {
            root: root.into(),
            platform,
            db,
            home: home.into(),
            sources,
            codex,
            job: None,
            active_check: None,
            check_queue: VecDeque::new(),
            pending_login: None,
            last_scan: 0,
            notice: None,
            unsaved: HashMap::new(),
            storage_fault: Cell::new(false),
            client_running: external_client_running,
            desktop: crate::desktop::Runtime::default(),
            quota: crate::quota::Runtime::default(),
            warmup: crate::warmup::Runtime::default(),
            client_sources: vec![],
            clients_checked_at: None,
            pi_last_access: None,
            cloud: crate::cloud::Runtime::open(root)?,
            cloud_native_next: 0,
            reset_job: None,
            automation: crate::automation::Runtime::default(),
            claude_mod: crate::claude_mod::Runtime::default(),
            #[cfg(test)]
            reset_transport: None,
            #[cfg(test)]
            claude_reset_transport: None,
            #[cfg(test)]
            native_override: None,
            #[cfg(test)]
            profile_transport: None,
        };
        for account in &mut manager.db.accounts {
            if account.provider == "claude" && account.native && account.state == "delegated" {
                account.state = if account.last_online.is_some() {
                    "healthy"
                } else {
                    "saved"
                }
                .into();
                account.message = "原生账号已支持协同维护；按自动维护开关执行".into();
                account.next_check = now() + 1;
            }
            // An interrupted Claude check, or a result that only lived in memory, may have consumed
            // a grant whose response is gone; reconcile it like any unanswered request.
            if account.provider == "claude"
                && (account.state == "checking" || (account.state == "unsaved" && !account.native))
            {
                account.state = "uncertain".into();
                account.message = crate::claude::UNCERTAIN.into();
            } else if account.state == "checking" {
                account.state = "attention".into();
                account.message = "上次检查被中断，请手动核对后恢复维护".into();
            }
        }
        // Desktop switches no longer keep a journal (an older version may have left one): the poll
        // reads the desktop and the default file again and finishes a login the desktop took.
        if manager.db.switch.as_ref().is_some_and(|s| s.desktop) {
            manager.db.switch = None;
        }
        if manager.db.switch.is_some() {
            manager.notice = Some("上次切换尚未确认，请结束任务后重试上次切换".into());
        }
        // No login survives a restart: keep a staged result that can still be saved, drop the rest.
        let pending: Vec<String> = manager
            .db
            .accounts
            .iter()
            .filter(|a| a.pending_profile.is_some())
            .map(|a| a.id.clone())
            .collect();
        for id in pending {
            if matches!(manager.staged_login(&id), Ok(Some(_))) {
                let a = manager.db.accounts.iter_mut().find(|a| a.id == id).unwrap();
                a.state = "attention".into();
                a.message = "上次登录已完成但尚未保存；请点「本地检查」完成保存".into();
            } else if let Err(message) = manager.discard_login(&id, None) {
                manager.notice = Some(message.into());
            }
        }
        Ok(manager)
    }
    fn save(&self) -> Result<()> {
        let result = self.platform.save_db(
            &self.root.join("accounts.sqlite3"),
            &serde_json::to_value(&self.db).unwrap(),
        );
        if result.is_err() {
            self.storage_fault.set(true);
        }
        result
    }
    fn index(&self, id: &str) -> Result<usize> {
        if self.cloud.config.bindings.contains_key(id) {
            return Err("此账号正在交接或已由云端维护，请使用云端模式操作");
        }
        self.db
            .accounts
            .iter()
            .position(|a| a.id == id)
            .ok_or("账号不存在")
    }
    fn ensure_idle(&self) -> Result<()> {
        self.ensure_check_admission()?;
        if self.job.is_some() || !self.check_queue.is_empty() {
            return Err("已有账号操作正在进行，请稍后重试");
        }
        Ok(())
    }
    fn ensure_check_admission(&self) -> Result<()> {
        if self.automation.job.is_some() || self.db.automation.run.is_some() {
            return Err("自动额度流程正在处理或待核对；请先停止并核对该流程");
        }
        if self.cloud.job.is_some() {
            return Err("云端操作正在进行，请稍后重试");
        }
        if self.storage_fault.get() {
            return Err("账号数据库保存失败，已暂停变更；修复存储后重新连接服务");
        }
        if (self.job.is_some() && self.active_check.is_none())
            || self.reset_job.is_some()
            || self.pending_login.is_some()
            || self.desktop.job.is_some()
            || self.desktop.refresh.is_some()
        {
            return Err("已有账号操作正在进行，请稍后重试");
        }
        if self.db.switch.is_some() {
            return Err("请先恢复上次切换的当前登录状态");
        }
        Ok(())
    }
    fn native(&self, provider: &str) -> Result<NativeStore> {
        #[cfg(test)]
        if let Some(native) = &self.native_override
            && native.provider == provider
        {
            return Ok(native.clone());
        }
        match provider {
            "codex" => NativeStore::codex(&self.sources.codex_home, false),
            "claude" => NativeStore::claude(
                &self.home,
                std::env::var_os("CLAUDE_CONFIG_DIR")
                    .as_deref()
                    .map(Path::new),
                std::env::var("CLAUDE_SECURESTORAGE_CONFIG_DIR")
                    .ok()
                    .as_deref(),
            ),
            _ => Err("不支持的账号类型"),
        }
    }
    fn staging(&self, provider: &str, id: &str) -> Result<NativeStore> {
        let path = self.root.join("profiles").join(id);
        private_dir(&path)?;
        if provider == "codex" {
            NativeStore::codex(&path, true)
        } else {
            NativeStore::claude(&self.home, Some(&path), None)
        }
    }
    fn bundle(&self, account: &Account) -> Result<Bundle> {
        let native = account
            .native
            .then(|| self.native(&account.provider))
            .transpose()?;
        read_account_bundle(&self.platform, account, native.as_ref())
    }
    pub fn snapshot(&self) -> Value {
        // Persisted identity keys never cross the UI boundary.
        let paused = self.quota_paused().is_some();
        let accounts:Vec<Value>=self.db.accounts.iter().map(|a|json!({"id":a.id,"provider":a.provider,"label":a.label,"identity":a.metadata.as_ref().map(|m|&m.display),"plan":a.metadata.as_ref().map(|m|&m.plan),"expires_at":a.metadata.as_ref().and_then(|m|m.expires_at),"refresh_expires_at":a.metadata.as_ref().and_then(|m|m.refresh_expires_at),"native":a.native,"automatic":a.automatic,"renewal_mode":if a.provider=="claude" && a.native{Some("claude_coordinated")}else{None},"state":a.state,"message":a.message,"check_status":if self.active_check.as_ref().is_some_and(|r|r.id==a.id){Some("running")}else if self.check_queue.iter().any(|r|r.id==a.id){Some("queued")}else{None},"check_online":self.active_check.as_ref().filter(|r|r.id==a.id).or_else(||self.check_queue.iter().find(|r|r.id==a.id)).map(|r|r.online),"last_local":a.last_local,"last_online":a.last_online,"last_refresh":a.last_refresh,"next_check":a.next_check,"quota":a.quota,"login_notice":crate::clients::login_notice(&a.provider,a.metadata.as_ref(),now()),"reset":self.db.reset_attempts.iter().rev().find(|attempt|attempt.account_id==a.id).map(|attempt|attempt.snapshot(a.provider=="codex")),"usage":Some(self.quota.snapshot(&a.id,now(),quota_eligible(a)&&!paused)),"warmup":a.warmup.as_ref().map(|p|(p,true)).or(a.warmup_off.as_ref().map(|p|(p,false))).map(|(p,enabled)|json!({"enabled":enabled,"hour":p.hour,"running":self.warmup.running(&a.id),"last":p.last}))})).collect();
        json!({"accounts":accounts,"clients":self.clients_snapshot(),"busy":self.job.is_some()||!self.check_queue.is_empty()||self.pending_login.is_some()||self.reset_job.is_some()||self.automation.job.is_some()||self.db.automation.run.is_some(),"checks_blocked":self.ensure_check_admission().is_err(),"recovery_needed":self.db.switch.is_some(),"notice":if self.storage_fault.get(){Some("账号数据库保存失败；已暂停账号变更和自动维护")}else{self.notice.as_deref()},"maintenance_running":!self.storage_fault.get(),"desktop":self.desktop_snapshot(),"strategy":self.strategy_snapshot(),"claude_mod":self.claude_mod.snapshot()})
    }
    pub fn import(&mut self, provider: &str, label: &str) -> Result<()> {
        self.ensure_idle()?;
        let bundle = self.native(provider)?.read(&self.platform)?;
        self.adopt_native(bundle, label)?;
        self.save()
    }
    pub fn import_codex_file(&mut self, path: &Path, label: &str) -> Result<()> {
        self.ensure_idle()?;
        if !path.is_absolute() {
            return Err("请提供认证 JSON 文件的绝对路径");
        }
        let path = std::fs::canonicalize(path).map_err(|_| "认证文件不存在或不可访问")?;
        if path.extension().is_none_or(|extension| extension != "json") {
            return Err("仅支持导入认证 JSON 文件");
        }
        let bundle = Bundle {
            provider: "codex".into(),
            credential: crate::platform::read_json(&path)?.ok_or("认证文件不存在")?,
            identity: Value::Null,
        };
        let metadata = bundle.metadata()?;
        let native = std::fs::canonicalize(self.sources.codex_home.join("auth.json"))
            .is_ok_and(|current| current == path)
            && self.native("codex")?.mode == "file";
        if !native
            && let Some(account) = self.db.accounts.iter_mut().find(|a| {
                a.metadata
                    .as_ref()
                    .is_some_and(|m| m.owner == metadata.owner)
            })
        {
            // A backup file must not replace a previously managed credential generation.
            account.automatic = false;
            return self.save();
        }
        self.save_import(bundle, label, native, Some(false))?;
        self.save()
    }
    fn adopt_native(&mut self, bundle: Bundle, label: &str) -> Result<()> {
        self.save_import(bundle, label, true, None)
    }
    fn save_import(
        &mut self,
        bundle: Bundle,
        label: &str,
        native: bool,
        automatic: Option<bool>,
    ) -> Result<()> {
        let metadata = bundle.metadata()?;
        let found = self.db.accounts.iter().position(|a| {
            a.metadata
                .as_ref()
                .is_some_and(|m| m.owner == metadata.owner)
        });
        let mut account = if let Some(index) = found {
            self.db.accounts[index].clone()
        } else {
            Account {
                id: random_id()?,
                provider: bundle.provider.clone(),
                // A blank import name defaults to the login email, which stays true after switching.
                label: valid_label(if label.trim().is_empty() {
                    &metadata.display
                } else {
                    label
                })?,
                metadata: None,
                native: false,
                automatic: true,
                state: "saved".into(),
                message: String::new(),
                last_local: None,
                last_online: None,
                last_refresh: None,
                next_check: now() + 60,
                failures: 0,
                quota: vec![],
                pending_profile: None,
                warmup: None,
                warmup_off: None,
            }
        };
        account.native = native;
        if let Some(automatic) = automatic {
            account.automatic = automatic;
        }
        account.metadata = Some(metadata);
        account.last_local = Some(now());
        account.state = "saved".into();
        account.message = if automatic == Some(false) {
            "已从文件导入；自动维护已关闭，未执行在线检查或刷新"
        } else {
            "当前原生登录已纳管；尚未在线验证"
        }
        .into();
        // Publish only after the credential commit and readback have succeeded.
        self.platform.vault_write(&account.id, &bundle.encoded())?;
        for a in &mut self.db.accounts {
            if native && a.provider == bundle.provider {
                a.native = false;
            }
        }
        if let Some(index) = found {
            self.db.accounts[index] = account;
        } else {
            self.db.accounts.push(account);
        }
        Ok(())
    }
    pub fn begin_login(
        &mut self,
        provider: &str,
        label: &str,
        existing: Option<&str>,
        login: &mut LoginManager,
    ) -> Result<()> {
        self.ensure_idle()?;
        if !matches!(provider, "codex" | "claude") {
            return Err("不支持的账号类型");
        }
        let binary = if provider == "codex" {
            self.codex.as_deref()
        } else {
            self.sources.claude_bin.as_deref()
        }
        .ok_or("未安装对应的官方 CLI")?
        .to_path_buf();
        let profile = random_id()?;
        let location = self.staging(provider, &profile)?;
        let index = if let Some(id) = existing {
            let i = self.index(id)?;
            if self.db.accounts[i].provider != provider {
                return Err("账号类型不一致");
            }
            if self.db.accounts[i].native && (self.client_running)(provider) {
                return Err("重新登录当前账号前，请先结束任务并退出相关官方客户端");
            }
            i
        } else {
            self.db.accounts.push(Account {
                id: random_id()?,
                provider: provider.into(),
                label: valid_label(label)?,
                metadata: None,
                native: false,
                automatic: true,
                state: "pending".into(),
                message: "等待官方登录".into(),
                last_local: None,
                last_online: None,
                last_refresh: None,
                next_check: now() + 60,
                failures: 0,
                quota: vec![],
                pending_profile: None,
                warmup: None,
                warmup_off: None,
            });
            self.db.accounts.len() - 1
        };
        let id = self.db.accounts[index].id.clone();
        let previous = self.db.accounts[index].pending_profile.replace(profile);
        self.save()?;
        if let Err(message) = login.start(provider, &binary, &location.directory) {
            // The official login never started, so nothing may wait on it.
            self.discard_login(&id, previous)?;
            return Err(message);
        }
        self.pending_login = Some(id);
        Ok(())
    }
    /// Ends a login attempt that left nothing to save: its staging goes, and an account that
    /// never received an identity goes with it, so no account is left waiting on the attempt.
    fn discard_login(&mut self, id: &str, restore: Option<String>) -> Result<()> {
        let index = self
            .db
            .accounts
            .iter()
            .position(|a| a.id == id)
            .ok_or("账号不存在")?;
        let account = self.db.accounts[index].clone();
        if let Some(profile) = &account.pending_profile {
            let staging = self.staging(&account.provider, profile)?;
            staging.remove_staging(&self.platform)?;
            remove_owned_profile(&staging.directory)?;
        }
        if account.metadata.is_none() {
            self.db.accounts.remove(index);
            if self.cloud.config.reauthorizations.remove(id).is_some() {
                self.cloud.save()?;
            }
        } else {
            self.db.accounts[index].pending_profile = restore;
        }
        self.save()
    }
    pub fn finish_login(&mut self, login: &mut LoginManager) {
        let Some(id) = self.pending_login.clone() else {
            return;
        };
        if login.state.as_ref().is_some_and(|s| s.active()) {
            return;
        }
        self.pending_login = None;
        let result = self.capture_login(&id);
        if let Err(message) = result {
            // A staged login is kept only while it can still be saved, e.g. once the official
            // client quits; a cancelled, failed or foreign login leaves the account as it was.
            if matches!(self.staged_login(&id), Ok(Some(_))) {
                if let Ok(index) = self.index(&id) {
                    self.db.accounts[index].state = "attention".into();
                    self.db.accounts[index].message = message.into();
                }
            } else if let Err(cleanup) = self.discard_login(&id, None) {
                self.notice = Some(cleanup.into());
            }
            if let Some(state) = &mut login.state {
                state.phase = "failed".into();
                state.message = message.into();
            }
        } else if let Some(state) = &mut login.state {
            state.phase = "succeeded".into();
            state.message = if self.cloud.config.reauthorizations.contains_key(&id) {
                "官方登录已完成；后台将核对身份并更新同一云端账号，请在云端页查看交接结果。"
            } else if self.db.accounts.iter().any(|a| a.id == id && a.native) {
                "当前账号已重新登录并完成存储校验，请重新打开官方客户端。"
            } else {
                "账号已独立保存，未替换当前默认登录。可在账号列表中选择切换。"
            }
            .into();
        }
        if let Err(message) = self.save() {
            self.notice = Some(message.into());
        }
    }
    /// The login waiting in this account's staging, when it can still be saved to the account.
    fn staged_login(&self, id: &str) -> Result<Option<Bundle>> {
        let account = &self.db.accounts[self.index(id)?];
        let profile = account.pending_profile.as_ref().ok_or("没有待确认的登录")?;
        let Some(bundle) = self
            .staging(&account.provider, profile)?
            .login(&self.platform)?
        else {
            return Ok(None);
        };
        let metadata = bundle.metadata()?;
        if account
            .metadata
            .as_ref()
            .is_some_and(|m| m.owner != metadata.owner)
        {
            return Err("登录身份与所选账号不同；未覆盖原账号，请按新账号添加");
        }
        if self.db.accounts.iter().any(|a| {
            a.id != id
                && a.metadata
                    .as_ref()
                    .is_some_and(|m| m.owner == metadata.owner)
                && !self
                    .cloud
                    .config
                    .reauthorizations
                    .get(id)
                    .is_some_and(|cloud_id| {
                        self.cloud
                            .config
                            .bindings
                            .get(&a.id)
                            .and_then(|b| b.cloud_id.as_ref())
                            == Some(cloud_id)
                    })
        }) {
            return Err("该身份已在列表中，请使用原账号的重新登录操作");
        }
        Ok(Some(bundle))
    }
    fn capture_login(&mut self, id: &str) -> Result<()> {
        let index = self.index(id)?;
        let account = self.db.accounts[index].clone();
        let bundle = self
            .staged_login(id)?
            .ok_or("官方登录没有完成，未取得新授权")?;
        let metadata = bundle.metadata()?;
        let native = self.staging(&account.provider, account.pending_profile.as_ref().unwrap())?;
        let mut backup = None;
        let replacing = if account.native {
            if (self.client_running)(&account.provider) {
                return Err("新授权暂存成功；请退出官方客户端后点本地检查完成保存");
            }
            let target = self.native(&account.provider)?;
            let old = target.login(&self.platform)?;
            if let Some(old) = &old {
                if !old.same_owner(&bundle)? {
                    return Err("原生账号已变化，未应用重新登录结果");
                }
                let key = format!("backup-{}", random_id()?);
                self.platform.vault_write(&key, &old.encoded())?;
                backup = Some(key);
            }
            Some((target, old))
        } else {
            None
        };
        self.platform.vault_write(id, &bundle.encoded())?;
        if let Some((target, old)) = replacing {
            self.db.switch = Some(Switch {
                desktop: false,
                provider: account.provider.clone(),
                target: id.into(),
                previous: backup.clone(),
            });
            self.save()?;
            if let Some(old) = old {
                target.apply(&self.platform, &old, &bundle)?;
            } else {
                target.initialize(&self.platform, &bundle)?;
            }
            self.db.switch = None;
        }
        let a = &mut self.db.accounts[index];
        a.metadata = Some(metadata);
        a.last_local = Some(now());
        a.state = "saved".into();
        a.message = "账号已保存，等待在线检查".into();
        a.next_check = now() + 30;
        // The credential commits succeeded; the staging reference goes with the metadata commit.
        a.pending_profile = None;
        self.unsaved.remove(id);
        self.save()?;
        if let Some(key) = backup {
            let _ = self
                .platform
                .delete_key("io.github.wuxm5.authbaton.vault", &key);
        }
        // The login is saved; a staging copy that fails to clear is leftover, not pending work.
        if native.remove_staging(&self.platform).is_err() {
            self.notice = Some("新登录已保存，但临时授权未能清理；可在钥匙串中手动删除".into());
        }
        Ok(())
    }
    pub fn local_check(&mut self, id: &str) -> Result<()> {
        if self.db.accounts[self.index(id)?].pending_profile.is_some() {
            self.ensure_idle()?;
            self.capture_login(id)?;
            return self.save();
        }
        self.request_check(id, false)
    }
    pub fn request_check(&mut self, id: &str, online: bool) -> Result<()> {
        self.ensure_check_admission()?;
        self.validate_check(id, online)?;
        if self
            .active_check
            .as_ref()
            .is_some_and(|request| request.id == id)
            || self.check_queue.iter().any(|request| request.id == id)
        {
            return Ok(());
        }
        if self.job.is_none() && self.check_queue.is_empty() {
            return self.start_check(id, online, true);
        }
        self.check_queue.push_back(CheckRequest {
            id: id.into(),
            online,
        });
        Ok(())
    }
    fn validate_check(&self, id: &str, online: bool) -> Result<()> {
        let account = &self.db.accounts[self.index(id)?];
        // A newer credential held in memory must be written back, never superseded by a new grant.
        if online
            && (self.unsaved.contains_key(id)
                || (account.provider == "codex" && account.state == "unsaved"))
        {
            return Err("请先重试保存未提交的凭据，或重新登录；不会重复提交旧刷新凭据");
        }
        if account.pending_profile.is_some() {
            return Err("请先完成或读取此账号的登录结果");
        }
        Ok(())
    }
    fn start_next_check(&mut self) {
        if self.storage_fault.get()
            || self.db.switch.is_some()
            || self.db.automation.run.is_some()
            || self.automation.job.is_some()
        {
            self.check_queue.clear();
            return;
        }
        if self.job.is_some() || self.ensure_check_admission().is_err() {
            return;
        }
        if let Some(request) = self.check_queue.pop_front()
            && let Err(message) = self.start_check(&request.id, request.online, true)
        {
            self.notice = Some(message.into());
        }
    }
    pub fn configure(
        &mut self,
        id: &str,
        label: Option<&str>,
        automatic: Option<bool>,
    ) -> Result<()> {
        self.ensure_idle()?;
        let index = self.index(id)?;
        if let Some(label) = label {
            self.db.accounts[index].label = valid_label(label)?;
        }
        if let Some(value) = automatic {
            let a = &mut self.db.accounts[index];
            a.automatic = value;
            if value && !matches!(a.state.as_str(), "unsaved" | "uncertain" | "reauth") {
                a.next_check = now() + 1;
            }
        }
        self.save()
    }
    pub fn remove(&mut self, id: &str, confirmed: bool) -> Result<()> {
        if !confirmed {
            return Err("请先确认从 AuthBaton 移除账号及其保存的凭据");
        }
        self.ensure_idle()?;
        let Some(index) = self.db.accounts.iter().position(|account| account.id == id) else {
            return Ok(());
        };
        let account = self.db.accounts[index].clone();
        if account.native || self.db.desktop.selected.as_deref() == Some(id) {
            return Err("当前账号不能直接移除，请先切换到其他账号");
        }
        if self
            .db
            .reset_attempts
            .iter()
            .any(|attempt| attempt.account_id == id && attempt.outcome.unresolved())
        {
            return Err("此账号有结果待确认的额度重置，请先重试上次操作确认结果");
        }
        self.quota.poll(now());
        if self.quota.is_running(id) {
            return Err("此账号正在查询额度，请在查询结束后移除");
        }
        self.warmup_finish();
        if self.warmup.running(id) {
            return Err("此账号正在发送预热，请在发送结束后移除");
        }
        // Keep the durable account ID until all owned secrets are gone. Missing Vault items
        // are already a successful delete, so a partial removal can be retried after restart.
        if let Some(profile) = &account.pending_profile {
            let staging = self.staging(&account.provider, profile)?;
            staging.remove_staging(&self.platform)?;
            remove_owned_profile(&staging.directory)?;
        }
        if account.provider == "codex" {
            remove_owned_profile(&self.root.join("profiles").join(format!("check-{id}")))?;
        }
        self.platform
            .delete_key("io.github.wuxm5.authbaton.vault", id)?;
        if account.provider == "claude" {
            self.platform.delete_key(
                "io.github.wuxm5.authbaton.vault",
                &crate::claude::journal_id(id),
            )?;
        }
        self.db.accounts.remove(index);
        let reset_attempts = self.db.reset_attempts.clone();
        self.db
            .reset_attempts
            .retain(|attempt| attempt.account_id != id);
        if self.save().is_err() {
            self.db.accounts.insert(index, account);
            self.db.reset_attempts = reset_attempts;
            return Err("凭据已清理，但账号目录保存失败；修复存储并重新连接服务后，再次移除此账号");
        }
        self.unsaved.remove(id);
        self.quota.forget(id);
        self.warmup.forget(id);
        Ok(())
    }
    pub fn check(&mut self, id: &str, manual: bool) -> Result<()> {
        self.ensure_idle()?;
        self.start_check(id, true, manual)
    }
    fn start_check(&mut self, id: &str, online: bool, manual: bool) -> Result<()> {
        self.validate_check(id, online)?;
        let index = self.index(id)?;
        let account = self.db.accounts[index].clone();
        let native = account
            .native
            .then(|| self.native(&account.provider))
            .transpose()?;
        let external = account.provider == "codex" && self.desktop_owns(id);
        let client_running = self.client_running;
        let codex = self.codex.clone();
        let platform = self.platform.clone();
        let profile = self.root.join("profiles").join(format!("check-{id}"));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        if online {
            self.db.accounts[index].state = "checking".into();
            self.db.accounts[index].message = "正在读取凭据并检查认证，必要时续期".into();
        }
        self.save()?;
        self.active_check = Some(CheckRequest {
            id: id.into(),
            online,
        });
        self.job = Some(Job {
            id: id.into(),
            cancel,
            handle: std::thread::spawn(move || {
                let prepare = || -> Result<JobResult> {
                    let bundle = read_account_bundle(&platform, &account, native.as_ref())?;
                    if worker_cancel.load(Ordering::Acquire) {
                        return Err("检查已停止；未继续认证或续期");
                    }
                    if !online {
                        return Ok(JobResult::Observed(bundle, None));
                    }
                    if account.native
                        && account.provider != "claude"
                        && !external
                        && (!manual || client_running(&account.provider))
                    {
                        let message = if manual {
                            "认证检查可能续期，请退出官方客户端后操作；查看额度请使用“检查用量”"
                        } else {
                            "当前默认账号由官方客户端续期；额度可独立查询，无需退出客户端"
                        };
                        return Ok(JobResult::Observed(bundle, Some(message)));
                    }
                    let work = Work {
                        transport: crate::maintenance::http,
                        cancel: worker_cancel.clone(),
                        id: account.id.clone(),
                        bundle,
                        platform: platform.clone(),
                        profile: profile.clone(),
                        codex: codex.clone(),
                        native: native.clone(),
                    };
                    Ok(JobResult::Check(Box::new(work.run())))
                };
                prepare().unwrap_or_else(JobResult::ReadError)
            }),
        });
        Ok(())
    }
    pub fn activate(&mut self, id: &str, confirmed: bool) -> Result<()> {
        if !confirmed {
            return Err("请确认已结束相关任务后再切换");
        }
        let index = self.index(id)?;
        if self.db.accounts[index].provider == "codex" && self.db.desktop.enabled {
            return self.request_desktop_switch(id);
        }
        self.ensure_idle()?;
        self.activate_native(id)
    }
    // Called only under the service's writer admission. Codex clients must have stopped;
    // running Claude clients pick the new login up within 30 s, on a new request.
    fn activate_native(&mut self, id: &str) -> Result<()> {
        let target = self.db.accounts[self.index(id)?].clone();
        if target.native {
            return Ok(());
        }
        switch_target_ready(&target)?;
        if target.provider != "claude" && (self.client_running)(&target.provider) {
            return Err("检测到官方客户端仍在运行，请先退出后再切换");
        }
        if target.provider == "claude" {
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
        if target.provider == "codex" {
            private_dir(&self.sources.codex_home)?;
        }
        let native = self.native(&target.provider)?;
        // Held from the read of the current login until the new one is written and read back.
        let locks = (target.provider == "claude")
            .then(|| crate::claude::SwitchLocks::acquire(&native))
            .transpose()?;
        let guard = || locks.as_ref().map_or(Ok(()), |l| l.check());
        let old = native.login(&self.platform)?;
        let next = Bundle::decode(self.platform.vault_read(&target.id)?)?;
        if let Some(old) = &old {
            // Keeps the source's latest material, including a refresh an official client just saved.
            self.adopt_native(old.clone(), "原生账号")?;
        }
        let previous = self
            .db
            .accounts
            .iter()
            .find(|a| a.provider == target.provider && a.native)
            .map(|a| a.id.clone());
        self.db.switch = Some(Switch {
            desktop: false,
            provider: target.provider.clone(),
            target: target.id.clone(),
            previous,
        });
        self.save()?;
        if let Some(old) = old {
            native.apply_guarded(&self.platform, &old, &next, &guard)?;
        } else {
            guard()?;
            native.initialize(&self.platform, &next)?;
        }
        guard()?;
        drop(locks);
        for a in &mut self.db.accounts {
            if a.provider == target.provider {
                a.native = a.id == id;
            }
        }
        let i = self.index(id)?;
        let a = &mut self.db.accounts[i];
        a.metadata = Some(next.metadata()?);
        a.state = "saved".into();
        a.message = if target.provider == "claude" {
            "默认认证已更新；运行中的 Claude Code 约 30 秒内改用此账号"
        } else {
            "默认认证已更新；请重新打开客户端确认账号"
        }
        .into();
        a.last_local = Some(now());
        if target.provider == "claude" {
            self.claude_mod.change = Some(crate::claude_mod::Change {
                at: now(),
                kind: "switch",
                label: target.label.clone(),
            });
        }
        self.db.switch = None;
        self.save()
    }
    pub fn retry_save(&mut self, id: &str) -> Result<()> {
        self.ensure_idle()?;
        let index = self.index(id)?;
        let account = &self.db.accounts[index];
        if account.provider == "claude" && account.native {
            let owner = account
                .metadata
                .as_ref()
                .ok_or("缺少登记身份")?
                .owner
                .clone();
            let native = self.native("claude")?;
            let id = id.to_string();
            let hint = self.unsaved.get(&id).cloned();
            let cancel = Arc::new(AtomicBool::new(false));
            let platform = self.platform.for_recovery(cancel.clone());
            // Keep the durable unsaved state while the HTTP-free worker runs.
            self.db.accounts[index].message = "正在恢复已保存的新凭据；不会发送刷新请求".into();
            self.save()?;
            self.job = Some(Job {
                id: id.clone(),
                cancel,
                handle: std::thread::spawn(move || {
                    let result = (|| {
                        let expected = if let Some(journal) =
                            platform.vault_read_optional(&crate::claude::journal_id(&id))?
                        {
                            Bundle::decode(journal["expected"].clone())?
                        } else {
                            native.read(&platform)?
                        };
                        if expected.metadata()?.owner != owner {
                            return Err("待恢复记录与登记账号不一致");
                        }
                        crate::claude::retry(&native, &platform, &id, &expected, hint.as_ref())
                    })();
                    JobResult::Recovery(result)
                }),
            });
            return Ok(());
        }
        let account = &self.db.accounts[index];
        let (previous, next) = match self.unsaved.get(id).cloned() {
            Some(held) => held,
            // After a restart, a Codex check's renewed credential may still be in its profile.
            None if account.provider == "codex" && !account.native => {
                let current = self.bundle(account)?;
                let profile = self.root.join("profiles").join(format!("check-{id}"));
                let next = crate::maintenance::codex_leftover(&profile, &current)
                    .ok_or("未保留可恢复的新凭据；请重新登录此账号")?;
                (current, next)
            }
            None => return Err("未保留可恢复的新凭据；请重新登录此账号"),
        };
        let current = self.bundle(&self.db.accounts[index])?;
        if current != next {
            if current != previous {
                return Err("账号凭据已由其他操作更新，未覆盖其内容");
            }
            if self.db.accounts[index].native {
                self.native(&next.provider)?
                    .refresh(&self.platform, &previous, &next)?;
            } else {
                self.platform.vault_write(id, &next.encoded())?;
            }
        }
        self.unsaved.remove(id);
        let a = &mut self.db.accounts[index];
        a.metadata = Some(next.metadata()?);
        a.last_refresh = Some(now());
        a.state = "saved".into();
        a.message = "新的续期凭据已保存；可继续在线检查".into();
        a.next_check = now() + 60;
        self.save()
    }
    pub fn recover(&mut self) -> Result<()> {
        if self.job.is_some() || self.pending_login.is_some() {
            return Err("请等待当前操作结束");
        }
        let operation = self.db.switch.clone().ok_or("没有待恢复的切换")?;
        if operation.provider != "claude" && (self.client_running)(&operation.provider) {
            return Err("请先退出相关官方客户端再恢复切换");
        }
        let mut candidates = vec![Bundle::decode(
            self.platform.vault_read(&operation.target)?,
        )?];
        if let Some(id) = &operation.previous {
            candidates.push(Bundle::decode(self.platform.vault_read(id)?)?);
        }
        let native = self.native(&operation.provider)?;
        let platform = self.platform.clone();
        #[cfg(test)]
        let transport = self
            .profile_transport
            .unwrap_or(|_, _, _, _| Err("test transport not configured"));
        #[cfg(not(test))]
        let transport = crate::maintenance::http;
        // Placing a Claude login an official client renewed meanwhile may take a profile request.
        self.job = Some(Job {
            id: operation.target.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            handle: std::thread::spawn(move || {
                JobResult::Switch((|| {
                    let locks = (native.provider == "claude")
                        .then(|| crate::claude::SwitchLocks::acquire(&native))
                        .transpose()?;
                    let found = native.recover_switch(&platform, &candidates, &|bundle| {
                        crate::maintenance::claude_owner(transport, bundle)
                    })?;
                    if let Some(locks) = &locks {
                        locks.check()?;
                    }
                    Ok(found)
                })())
            }),
        });
        Ok(())
    }
    /// The official client renews the default Codex login in place. Keeping the account's own
    /// copy current means switching back later never writes a refresh token it already rotated.
    fn sync_native_codex(&mut self, index: usize, bundle: &Bundle) {
        let account = &self.db.accounts[index];
        if !account.native
            || account.provider != "codex"
            || account.metadata.as_ref().map(|m| &m.owner)
                != bundle.metadata().ok().as_ref().map(|m| &m.owner)
        {
            return;
        }
        let saved = self
            .platform
            .vault_read(&account.id)
            .and_then(Bundle::decode);
        if saved.is_ok_and(|saved| saved == *bundle) {
            return;
        }
        if let Err(message) = self.platform.vault_write(&account.id, &bundle.encoded()) {
            self.notice = Some(format!("未能保存官方客户端续期后的 Codex 凭据：{message}"));
        }
    }
    fn finish_recover(&mut self, found: Result<Option<Bundle>>) -> Result<()> {
        let operation = self.db.switch.clone().ok_or("没有待恢复的切换")?;
        match found? {
            Some(bundle) => self.adopt_native(bundle, "恢复的原生账号")?,
            // The store holds no login, so no account is the default one.
            None => {
                for a in &mut self.db.accounts {
                    if a.provider == operation.provider {
                        a.native = false;
                    }
                }
            }
        }
        self.db.switch = None;
        self.notice = None;
        self.save()?;
        if let Some(id) = operation.previous.filter(|id| id.starts_with("backup-")) {
            let _ = self.platform.delete_key("io.github.wuxm5.authbaton.vault", &id);
        }
        Ok(())
    }
    pub fn tick(&mut self) {
        self.cloud_tick();
        if self
            .reset_job
            .as_ref()
            .is_some_and(|(_, job)| job.is_finished())
        {
            self.finish_reset();
        }
        if self
            .job
            .as_ref()
            .is_some_and(|job| job.handle.is_finished())
        {
            self.finish_job();
        }
        self.desktop_tick();
        self.automation_tick();
        self.start_next_check();
        self.quota_tick();
        if self.job.is_some()
            || self.reset_job.is_some()
            || self.pending_login.is_some()
            || self.db.switch.is_some()
            || self.storage_fault.get()
            || self.automation.job.is_some()
            || self.db.automation.run.is_some()
            || self.desktop.job.is_some()
            || self.desktop.refresh.is_some()
            || self.desktop.pending_switch.is_some()
            || !self.check_queue.is_empty()
            || now() < self.last_scan + 15
        {
            return;
        }
        self.last_scan = now();
        let id = self
            .db
            .accounts
            .iter()
            .filter(|a| due(a, now()))
            .min_by_key(|a| a.next_check)
            .map(|a| a.id.clone());
        let Some(id) = id else {
            return;
        };
        // Write back a held result without HTTP; otherwise a check reconciles from the stores.
        let pending = self.unsaved.contains_key(&id);
        let result = if pending {
            self.retry_save(&id)
        } else {
            self.check(&id, false)
        };
        if let Err(message) = result {
            if let Ok(index) = self.index(&id) {
                let a = &mut self.db.accounts[index];
                if pending {
                    a.failures = a.failures.saturating_add(1);
                    a.next_check = now() + backoff(a.failures);
                } else {
                    a.state = "attention".into();
                    a.next_check = now() + 300;
                }
                a.message = message.into();
            }
            let _ = self.save();
        }
    }
    fn finish_job(&mut self) {
        let Some(job) = self.job.take() else {
            return;
        };
        self.active_check = None;
        let result = match job.handle.join() {
            Ok(JobResult::Switch(found)) => {
                if let Err(message) = self.finish_recover(found) {
                    self.notice = Some(format!("上次切换仍待恢复：{message}"));
                }
                return;
            }
            result => result,
        };
        let Ok(index) = self.index(&job.id) else {
            return;
        };
        match result {
            Ok(JobResult::Check(result)) => {
                if let Some(pending) = result.pending {
                    self.unsaved.insert(job.id.clone(), pending);
                }
                self.sync_native_codex(index, &result.bundle);
                let a = &mut self.db.accounts[index];
                a.metadata = result.bundle.metadata().ok();
                a.last_local = Some(now());
                if result.online {
                    a.last_online = Some(now());
                    a.failures = 0;
                    a.quota = result.quota;
                } else {
                    a.failures = a.failures.saturating_add(1);
                }
                if result.refreshed {
                    a.last_refresh = Some(now());
                }
                a.state = result.state;
                a.message = result.message;
                a.next_check = if result.online || a.state == "limited" {
                    next_due(a.metadata.as_ref(), now())
                } else {
                    now() + backoff(a.failures)
                };
                if result.refreshed && self.db.desktop.selected.as_deref() == Some(&job.id) {
                    self.desktop_log(
                        "renewed",
                        Some(&job.id),
                        "AuthBaton 续期了桌面绑定的账号；桌面下次要令牌时直接给它",
                    );
                }
            }
            Ok(JobResult::Observed(bundle, delegated)) => {
                self.sync_native_codex(index, &bundle);
                let a = &mut self.db.accounts[index];
                a.metadata = Some(bundle.metadata().expect("observed bundle was validated"));
                a.last_local = Some(now());
                if let Some(message) = delegated {
                    a.state = "delegated".into();
                    a.message = message.into();
                    a.next_check = now() + 300;
                } else if !matches!(
                    a.state.as_str(),
                    "reauth" | "attention" | "unsaved" | "uncertain" | "backup_pending"
                ) {
                    // A preserved state keeps the message that explains it.
                    a.state = "saved".into();
                    a.message = "本地凭据已读取；不代表远端可用".into();
                }
            }
            Ok(JobResult::ReadError(message)) => {
                let a = &mut self.db.accounts[index];
                // Preparation did not produce a grant; keep identity and unresolved recovery state.
                if message == "原生登录身份已变化，请重新纳管当前登录" {
                    a.native = false;
                }
                if !matches!(
                    a.state.as_str(),
                    "reauth" | "unsaved" | "uncertain" | "backup_pending"
                ) {
                    a.state = "attention".into();
                }
                a.message = message.into();
                // Preserved recovery states stay scheduled; back off instead of rereading every scan.
                a.failures = a.failures.saturating_add(1);
                a.next_check = now() + backoff(a.failures);
            }
            Ok(JobResult::Switch(_)) => unreachable!("handled before the account lookup"),
            Ok(JobResult::Recovery(result)) => {
                let a = &mut self.db.accounts[index];
                match result {
                    Ok((bundle, refreshed)) => {
                        self.unsaved.remove(&job.id);
                        a.metadata = bundle.metadata().ok();
                        if refreshed {
                            a.last_refresh = Some(now());
                        }
                        a.last_local = Some(now());
                        a.state = "saved".into();
                        a.message = "原生凭据与备份已同步；恢复过程未发送刷新请求".into();
                        a.next_check = now() + 60;
                    }
                    Err(message) => {
                        // A timeout can happen after the native commit: retain the journal and identity.
                        a.state = if message == crate::claude::UNCERTAIN {
                            "uncertain"
                        } else {
                            "unsaved"
                        }
                        .into();
                        a.message = message.into();
                        a.failures = a.failures.saturating_add(1);
                        a.next_check = now() + backoff(a.failures);
                    }
                }
            }
            Err(_) => {
                let a = &mut self.db.accounts[index];
                a.state = "attention".into();
                a.message = "检查任务中断，请手动核对后恢复维护".into();
            }
        }
        if let Err(message) = self.save() {
            self.notice = Some(message.into());
        }
    }
    pub fn stop(&mut self) {
        self.automation_shutdown();
        self.cloud.finish();
        self.check_queue.clear();
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Release);
        }
        if self.job.is_some() {
            self.finish_job();
        }
        self.desktop_stop();
        self.finish_reset();
        self.warmup_stop();
        self.quota.stop(now());
    }

    /// A running Claude Code session's rate-limit reading, relayed by the companion mod. It only
    /// brings the current account's quota GET forward; the strategy still decides on that GET.
    pub fn mod_report(&mut self, report: crate::claude_mod::Report) -> Value {
        let time = now();
        self.claude_mod.last_report = Some(time);
        let current = self
            .db
            .accounts
            .iter()
            .find(|a| a.provider == "claude" && a.native)
            .map(|a| (a.id.clone(), a.label.clone()));
        if let Some((id, _)) = &current
            && self.db.automation.config.enabled
            && let Some(rule) = self
                .db
                .automation
                .config
                .accounts
                .iter()
                .find(|r| r.id == *id && r.participate)
            && (report.limited || report.primary_used() >= 100.0 - f64::from(rule.below))
            && time >= self.claude_mod.last_query + 30
        {
            self.claude_mod.last_query = time;
            self.quota.request(id, time);
        }
        json!({"account": current.map(|(_, label)| label), "change": self.claude_mod.change, "now": time})
    }
    pub fn request_quota(&mut self, id: Option<&str>) -> Result<()> {
        let Some(id) = id else {
            self.quota_tick();
            return Ok(());
        };
        let index = self.index(id)?;
        if !matches!(
            self.db.accounts[index].provider.as_str(),
            "codex" | "claude"
        ) {
            return Err("此账号尚不支持独立额度查询");
        }
        // The scheduler never picks these, so a request would stay queued forever.
        if !quota_eligible(&self.db.accounts[index]) {
            return Err("此账号的认证状态待处理，暂不能查询额度");
        }
        if self.quota_paused().is_some() {
            return Err(
                "AuthBaton 有待处理的问题（数据库故障、换号恢复或自动换号流程），暂不能查询额度",
            );
        }
        self.quota.request(id, now());
        Ok(())
    }
    /// The line at which the local strategy resets this account. Its quota is queried sooner as
    /// it nears the line; other accounts keep the plain interval.
    fn quota_line(&self, id: &str) -> Option<f64> {
        let config = &self.db.automation.config;
        if !config.enabled || self.cloud.config.mode != "local" {
            return None;
        }
        config
            .accounts
            .iter()
            .find(|r| r.id == id && r.participate && r.order.resets())
            .map(|r| f64::from(r.below))
    }
    /// Pauses that last until the user acts. Quota and warm-up work waits for them, so a request
    /// made meanwhile would not run.
    fn quota_paused(&self) -> Option<&'static str> {
        if self.storage_fault.get() {
            Some("账号数据库保存失败")
        } else if self.db.switch.is_some() {
            Some("有中断的换号待恢复")
        } else if self.db.automation.run.is_some() {
            Some("自动换号流程正在处理或待核对")
        } else {
            None
        }
    }
    fn quota_tick(&mut self) {
        self.quota.poll(now());
        self.warmup_finish();
        let warmup = self.warmup.check_due(now());
        if warmup {
            self.warmup_note_blocked();
        }
        // Quota work does not reserve the authentication job or require official clients to exit.
        // Start after our credential commits finish; the network worker receives no store/writer.
        if self.storage_fault.get()
            || self.automation.job.is_some()
            || self.db.automation.run.is_some()
            || self.reset_job.is_some()
            || self.db.switch.is_some()
            || self.pending_login.is_some()
            || self.job.is_some()
            || self.desktop.job.is_some()
            || self.desktop.refresh.is_some()
        {
            return;
        }
        if warmup {
            self.warmup_start();
        }
        if let Some(account) = self
            .db
            .accounts
            .iter()
            .find(|a| quota_eligible(a) && self.quota.due(&a.id, now()))
        {
            let bundle = self.bundle(account);
            let id = account.id.clone();
            let line = self.quota_line(&id);
            if account.provider == "claude" {
                self.quota.start_claude(
                    &id,
                    bundle.and_then(|b| crate::quota::ClaudeAccess::from_bundle(&b)),
                    line,
                    now(),
                );
            } else {
                self.quota.start(
                    &id,
                    bundle.and_then(|b| crate::codex_api::Access::from_bundle(&b)),
                    line,
                    now(),
                );
            }
        }
    }
}
fn quota_eligible(a: &Account) -> bool {
    matches!(a.provider.as_str(), "codex" | "claude")
        && a.pending_profile.is_none()
        && (!matches!(a.state.as_str(), "unsaved" | "uncertain")
            || (a.provider == "claude" && a.native && a.metadata.is_some()))
}
#[path = "accounts_clients.rs"]
mod accounts_clients;
#[path = "automation_accounts.rs"]
mod automation_accounts;
#[path = "cloud_accounts.rs"]
mod cloud_accounts;
#[path = "desktop_accounts.rs"]
mod desktop_accounts;
#[path = "pi_accounts.rs"]
mod pi_accounts;
#[path = "reset_accounts.rs"]
mod reset_accounts;
#[path = "warmup_accounts.rs"]
mod warmup_accounts;
fn read_account_bundle(
    platform: &Platform,
    account: &Account,
    native: Option<&NativeStore>,
) -> Result<Bundle> {
    let bundle = if let Some(native) = native {
        native.login(platform)?.ok_or(SIGNED_OUT)?
    } else {
        Bundle::decode(platform.vault_read(&account.id)?)?
    };
    let metadata = bundle.metadata()?;
    if account
        .metadata
        .as_ref()
        .is_some_and(|m| m.owner != metadata.owner)
    {
        return Err("原生登录身份已变化，请重新纳管当前登录");
    }
    Ok(bundle)
}
/// A target whose saved login is known to be stale or rejected would replace a working default
/// login and hide the problem.
fn switch_target_ready(account: &Account) -> Result<()> {
    if account.metadata.is_none() || account.pending_profile.is_some() {
        return Err("目标账号的登录尚未完成");
    }
    if matches!(account.state.as_str(), "reauth" | "unsaved" | "uncertain") {
        return Err("请先处理目标账号的登录或凭据保存状态");
    }
    Ok(())
}
fn remove_owned_profile(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("无法清理此账号的 AuthBaton 临时目录，请重试移除"),
    }
}
fn valid_label(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 60 || value.chars().any(char::is_control) {
        Err("账号名称须为 1–60 个可见字符")
    } else {
        Ok(value.into())
    }
}
fn due(account: &Account, time: u64) -> bool {
    account.automatic
        && account.metadata.is_some()
        && account.pending_profile.is_none()
        && account.next_check <= time
        && !matches!(account.state.as_str(), "reauth" | "attention" | "checking")
        // Claude results of unknown outcome or not yet written back reconcile automatically.
        && !(account.provider == "codex" && account.state == "unsaved")
}
fn next_due(metadata: Option<&Metadata>, time: u64) -> u64 {
    metadata
        .and_then(|m| m.expires_at)
        .map(|t| (time + 1800).min(t.saturating_sub(600).max(time + 30)))
        .unwrap_or(time + 1800)
}
fn backoff(failures: u32) -> u64 {
    match failures {
        0 | 1 => 60,
        2 => 300,
        3 => 900,
        _ => 3600,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scheduler_does_not_retry_uncertain_refresh_or_override_pause() {
        let mut a = Account {
            id: "id".into(),
            provider: "claude".into(),
            label: "test".into(),
            metadata: Some(Metadata {
                owner: "test".into(),
                display: "test".into(),
                plan: "pro".into(),
                expires_at: None,
                refresh_expires_at: None,
            }),
            native: false,
            automatic: true,
            state: "attention".into(),
            message: String::new(),
            last_local: None,
            last_online: None,
            last_refresh: None,
            next_check: 1,
            failures: 0,
            quota: vec![],
            pending_profile: None,
            warmup: None,
            warmup_off: None,
        };
        assert!(!due(&a, 2));
        a.state = "healthy".into();
        assert!(due(&a, 2));
        a.automatic = false;
        assert!(!due(&a, 2));
        assert_eq!(backoff(99), 3600);
    }
}

#[cfg(test)]
#[path = "native_smoke_tests.rs"]
mod native_smoke_tests;

#[cfg(test)]
#[path = "claude_live_switch_tests.rs"]
mod claude_live_switch_tests;

#[cfg(test)]
#[path = "boundary_tests.rs"]
mod boundary_tests;
