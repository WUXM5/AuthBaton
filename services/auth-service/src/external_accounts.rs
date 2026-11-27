//! External rows: accounts a credential center lists, shown and used here with access it lends.
use super::*;
use crate::external::{self, Follow};
use std::collections::HashSet;

const EXTERNAL_RENEWS: &str = "外部账号由来源续期，本机不检查也不续期";
pub(super) const SOURCE_ONLY: &str = "外部账号只能在来源网页上登录、改名或移除";
const NOT_IN_RELAY: &str = "这个外部账号还不能加入接力：先把它设为可换上本机";
const NO_SWITCH: &str = "这个外部账号还不能换上本机：先把它设为可换上本机";
const NO_RESET: &str = "这个外部账号只能看额度：先把它设为可用重置卡";
const NO_AUTO_RESET: &str = "只有可换上本机、在接力队列里的外部账号才会自动用卡";
const SOURCE_DOWN: &str = "外部来源连不上或配对已失效，暂不能用这个账号";
const NOT_READY: &str = "来源那边这个账号现在不能用（需要重新登录、已停用或已删除）";
const UNREADABLE: &str = "只有 setup-token，读不了额度和重置卡";
const LEASE_PENDING: &str = "外部账号的 Access 还在领取，请稍后再试";
const USE_PENDING: &str = "正在向外部来源领取这次要用的 Access，稍后自动重试";
const IN_USE_CODE: &str = "Code 正在用这个号，先换到其他账号再降档";
const IN_USE_CODEX: &str = "Codex 正在用这个号，先换到其他账号再降档";
const LEAVES_RELAY: &str = "这个号在自动接力队列里：确认降档后会移出队列，见底线与规则不保留";
const SYNCING: &str = "正在写入在用账号的登录，请几秒后再试";

impl Accounts {
    pub(super) fn is_external(&self, id: &str) -> bool {
        self.external.config.rows.contains_key(id)
    }
    pub(super) fn external_key(&self, id: &str) -> Option<String> {
        self.external
            .config
            .rows
            .get(id)
            .map(|r| external::key(&r.source, &r.account_ref))
    }
    /// An external row's source, and its directory entry while the center still lists it.
    fn external_entry(&self, id: &str) -> Option<(&external::Source, Option<&Value>)> {
        let row = self.external.config.rows.get(id)?;
        let source = self.external.source(&row.source)?;
        Some((source, source.entry(&row.account_ref)))
    }
    /// The access-only login leased for an external row; none for a row of this Mac or the cloud.
    pub(super) fn external_bundle(&self, id: &str) -> Option<Result<Bundle>> {
        let key = self.external_key(id)?;
        Some(
            self.external
                .lease(&key)
                .map(|l| l.bundle.clone())
                .ok_or(LEASE_PENDING),
        )
    }
    fn external_readable(&self, id: &str) -> bool {
        self.external_entry(id)
            .and_then(|(_, entry)| entry)
            .is_none_or(|e| e["quota_readable"] == true)
    }
    /// Quota of an external row waits for its leased access, and is never read without one.
    pub(super) fn external_quota_ready(&self, id: &str) -> Option<bool> {
        let key = self.external_key(id)?;
        Some(self.external_readable(id) && self.external.lease(&key).is_some())
    }
    /// What the source says about using the account now: paired, listed, ready and readable.
    fn external_usable(&self, id: &str) -> Result<()> {
        let (source, entry) = self.external_entry(id).ok_or(SOURCE_DOWN)?;
        if source.state != "connected" {
            return Err(SOURCE_DOWN);
        }
        let entry = entry.ok_or(NOT_READY)?;
        if entry["status"] != "ready"
            || self
                .db
                .accounts
                .iter()
                .any(|a| a.id == id && a.state != "external")
        {
            return Err(NOT_READY);
        }
        if entry["quota_readable"] != true {
            return Err(UNREADABLE);
        }
        Ok(())
    }
    /// An external row can be put in use, and is a relay target: it may be switched to, its
    /// source is connected and its account ready.
    pub(super) fn external_switchable(&self, id: &str) -> Result<()> {
        if self
            .external
            .config
            .rows
            .get(id)
            .is_some_and(|r| r.mode != "switch")
        {
            return Err(NO_SWITCH);
        }
        self.external_usable(id)
    }
    /// A card of an external row may be spent: by hand from mode reset on, by a rule only in mode
    /// switch. None for any other row.
    pub(super) fn external_reset_lender(
        &self,
        id: &str,
        automatic: bool,
    ) -> Result<Option<(external::Source, Value)>> {
        let Some(row) = self.external.config.rows.get(id) else {
            return Ok(None);
        };
        match (row.mode.as_str(), automatic) {
            ("switch", _) | ("reset", false) => {}
            (_, true) => return Err(NO_AUTO_RESET),
            _ => return Err(NO_RESET),
        }
        self.external_usable(id)?;
        let (source, entry) = self.external_entry(id).ok_or(SOURCE_DOWN)?;
        Ok(Some((source.clone(), entry.ok_or(NOT_READY)?.clone())))
    }
    /// In use for Code or Codex, or holding a reset whose outcome is not settled.
    fn external_held(&self, id: &str) -> bool {
        self.db.accounts.iter().any(|a| a.id == id && a.native)
            || self.db.desktop.selected.as_deref() == Some(id)
            || self
                .db
                .reset_attempts
                .iter()
                .any(|a| a.account_id == id && a.outcome.unresolved())
    }
    /// A renewal of the access in use still writing, which would land over a switch.
    pub(super) fn native_sync_running(&self, provider: &str) -> bool {
        let tag = format!("sync:{provider}");
        self.cloud
            .light
            .iter()
            .any(|l| l.tag == tag && !l.handle.is_finished())
            || self.external.busy(&format!("native:{provider}"))
    }
    /// Asks for an external account's access in the background.
    fn start_external_lease(
        &mut self,
        id: &str,
        purpose: &'static str,
        rejected: Option<u64>,
    ) -> Result<()> {
        let row = self
            .external
            .config
            .rows
            .get(id)
            .ok_or(SOURCE_DOWN)?
            .clone();
        let source = self
            .external
            .source(&row.source)
            .ok_or(SOURCE_DOWN)?
            .clone();
        let entry = source.entry(&row.account_ref).ok_or(NOT_READY)?.clone();
        let key = external::key(&row.source, &row.account_ref);
        self.external.tried.insert(key.clone(), now());
        let tag = format!("{}:{key}", if purpose == "use" { "use" } else { "lease" });
        let platform = self.platform.clone();
        self.external.start(&tag, &row.source, move || {
            external::lease(&platform, &source, &entry, purpose, rejected)
        });
        Ok(())
    }
    /// An external row is put to use only with access leased for that use a moment ago. Without
    /// one it is asked for in the background, and the caller tries again once it is in.
    pub(super) fn external_use_gate(&mut self, id: &str) -> Result<()> {
        self.external_switchable(id)?;
        let key = self.external_key(id).ok_or(SOURCE_DOWN)?;
        if self.external.use_ready(&key) {
            return Ok(());
        }
        if !self.external.running(&format!("use:{key}")) {
            self.start_external_lease(id, "use", None)?;
        }
        Err(USE_PENDING)
    }
    /// A switch a person asked for: true when it waits for access leased for it, and goes ahead
    /// once that is in.
    pub(super) fn external_switch_later(&mut self, id: &str) -> Result<bool> {
        self.external_switchable(id)?;
        let key = self.external_key(id).ok_or(SOURCE_DOWN)?;
        if self.external.use_ready(&key) {
            return Ok(false);
        }
        if !self.external.running(&format!("use:{key}")) {
            self.start_external_lease(id, "use", None)?;
        }
        self.external.follow = Some(Follow {
            row: id.into(),
            key,
            started: now(),
            until: now() + 30,
        });
        Ok(true)
    }
    fn external_follow(&mut self) {
        let Some(follow) = self.external.follow.as_ref() else {
            return;
        };
        let (row, key, started, until) = (
            follow.row.clone(),
            follow.key.clone(),
            follow.started,
            follow.until,
        );
        if !self.is_external(&row) {
            self.external.follow = None;
            return;
        }
        let failed = self
            .external
            .failed
            .get(&key)
            .filter(|(at, _)| *at >= started)
            .map(|(_, message)| message.clone());
        if failed.is_none() && self.external.use_ready(&key) {
            // A switch waits for the work it would refuse to run beside, up to its deadline.
            let provider = self
                .db
                .accounts
                .iter()
                .find(|a| a.id == row)
                .map(|a| a.provider.clone())
                .unwrap_or_default();
            if (self.ensure_idle().is_err() || self.native_sync_running(&provider)) && now() < until
            {
                return;
            }
            self.external.follow = None;
            if let Err(message) = self.activate(&row, true) {
                self.notice = Some(format!("没有换上外部账号：{message}"));
            }
            return;
        }
        if failed.is_some() || now() >= until || !self.external.running(&format!("use:{key}")) {
            self.external.follow = None;
            self.notice = Some(format!(
                "没有换上外部账号：{}",
                failed.unwrap_or_else(|| "没能及时领到 Access，请重试".into())
            ));
        }
    }
    /// Puts an external row to use: access leased for it goes into the native store under the
    /// official locks, as a cloud row's does.
    pub(super) fn activate_external(&mut self, id: &str) -> Result<()> {
        self.external_use_gate(id)?;
        let row = self.db.accounts[self.index(id)?].clone();
        let key = self.external_key(id).ok_or(SOURCE_DOWN)?;
        let target = self
            .external
            .lease(&key)
            .ok_or(LEASE_PENDING)?
            .bundle
            .clone();
        if target.provider != row.provider {
            return Err("外部账号类型不一致");
        }
        self.activate_leased(id, &row.provider, |m, native| {
            m.publish_external(id, native)
        })
    }
    /// Writes an external row's leased login into the native store and records the row as the one
    /// in use there, and the row of this Mac it replaced.
    pub(super) fn publish_external(&mut self, id: &str, native: &NativeStore) -> Result<()> {
        let key = self.external_key(id).ok_or(SOURCE_DOWN)?;
        let lease = self.external.lease(&key).ok_or(LEASE_PENDING)?.clone();
        let provider = lease.bundle.provider.clone();
        let previous = self
            .db
            .accounts
            .iter()
            .find(|a| {
                a.provider == provider
                    && a.native
                    && !self.is_external(&a.id)
                    && self.cloud_binding(&a.id).is_none()
            })
            .map(|a| a.id.clone());
        if self.native_sync_running(&provider) {
            return Err(SYNCING);
        }
        let platform = self.platform.clone();
        let runtime = &mut self.external;
        crate::cloud::write_projection(
            &platform,
            native,
            id,
            &lease.bundle,
            Some(&mut |landed| {
                if landed {
                    runtime.config.selected.insert(provider.clone(), id.into());
                    runtime.config.pending_activation = None;
                    if let Some(previous) = &previous {
                        runtime
                            .config
                            .previous
                            .insert(provider.clone(), previous.clone());
                    }
                } else {
                    runtime.config.pending_activation = Some(id.into());
                }
                runtime.save()
            }),
        )?;
        self.external
            .written
            .insert(provider.clone(), (lease.expires_at, lease.version));
        // The store holds this row's access now, not a cloud row's.
        if self.cloud.config.selected.remove(&provider).is_some()
            && let Err(message) = self.cloud.save()
        {
            self.notice = Some(message.into());
        }
        Ok(())
    }
    /// The store no longer holds an external row's access. True when one was recorded.
    pub(super) fn release_external_selection(&mut self, provider: &str) -> bool {
        self.external.written.remove(provider);
        if self.external.config.selected.remove(provider).is_none() {
            return false;
        }
        if self.external.save().is_err() {
            self.notice = Some("外部账号在用记录保存失败；下次同步会再核对".into());
        }
        true
    }
    /// Access for an external row is on its way.
    pub(super) fn external_refreshing(&self, account_id: &str) -> bool {
        self.external_key(account_id)
            .is_some_and(|key| self.external.running(&format!("use:{key}")))
    }
    /// A refused desktop asks for access again, naming the version it was refused with; the
    /// center renews it first. The desktop gets it on the tick after it is in.
    pub(super) fn refresh_from_external(&mut self, account_id: &str) -> bool {
        let Some(key) = self.external_key(account_id) else {
            return false;
        };
        let rejected = self.external.last_lease(&key).map(|l| l.version);
        self.start_external_lease(account_id, "use", rejected)
            .is_ok()
    }
    /// Settles an activation a restart interrupted: the native store says whether it landed.
    fn settle_external(&mut self) {
        let Some(id) = self.external.config.pending_activation.clone() else {
            return;
        };
        if self.external.next_settle > now() {
            return;
        }
        self.external.next_settle = now() + 30;
        let stores: HashMap<&str, NativeStore> = ["codex", "claude"]
            .into_iter()
            .filter_map(|p| self.native(p).ok().map(|n| (p, n)))
            .collect();
        let landed = match crate::cloud::settle_projection(&self.platform, &stores, &id) {
            Ok(landed) => landed,
            Err(message) => {
                self.notice = Some(message.into());
                return;
            }
        };
        for provider in landed {
            self.external
                .config
                .selected
                .insert(provider.into(), id.clone());
            self.cloud.config.selected.remove(provider);
            for a in &mut self.db.accounts {
                if a.provider == provider {
                    a.native = a.id == id;
                }
            }
        }
        self.external.config.pending_activation = None;
        if self.save().is_ok() {
            let _ = self.cloud.save();
            let _ = self.external.save();
        }
    }
    pub(super) fn external_view(&self, account: &Account) -> Value {
        let Some(row) = self.external.config.rows.get(&account.id) else {
            return Value::Null;
        };
        let source = self.external.source(&row.source);
        let entry = source.and_then(|s| s.entry(&row.account_ref));
        let label = source.map_or(external::PLACEHOLDER, |s| s.label.as_str());
        let status = entry.map_or("gone", |e| {
            e["status"].as_str().unwrap_or("needs_reauthorization")
        });
        let readable = entry.is_some_and(|e| e["quota_readable"] == true);
        let reason = match status {
            "gone" => Some(format!("已从 {label} 删除")),
            "disabled" => Some(format!("这个号在 {label} 已停用")),
            "needs_reauthorization" => Some(format!("{label} 那边需要重新登录这个号")),
            _ if !readable => Some(UNREADABLE.to_string()),
            _ => None,
        };
        json!({
            "source": row.source,
            "source_label": label,
            "account_ref": row.account_ref,
            "mode": row.mode,
            "quota_readable": readable,
            "status": status,
            "source_state": source.map_or("error", |s| s.state.as_str()),
            "manage_url": source.map(|s| &s.manage_url),
            "reason": reason,
        })
    }
    pub(super) fn external_snapshot(&self) -> Value {
        let sources: Vec<Value> = self
            .external
            .config
            .sources
            .iter()
            .map(|s| {
                let count = |p: &str| {
                    s.directory
                        .iter()
                        .filter(|e| external::provider(e) == p)
                        .count()
                };
                json!({
                    "id": s.id,
                    "label": s.label,
                    "site_url": s.site_url,
                    "manage_url": s.manage_url,
                    "device_label": s.device_label,
                    // Stored as "error" until the center accepts the pairing, so nothing from it is used and
                    // a restart midway leaves it retryable; while the pairing runs it reads as connecting.
                    "state": if self.external.job(&s.id) == Some("connecting") { "connecting" } else { s.state.as_str() },
                    "error": s.error,
                    "read_at": s.read_at,
                    "counts": {"codex": count("codex"), "claude": count("claude")},
                    "default_mode": s.default_mode,
                    "merged": self.external.config.merged.get(&s.id).cloned().unwrap_or_default(),
                    "job": self.external.job(&s.id),
                })
            })
            .collect();
        json!({"sources": sources})
    }
    /// accounts.external: connect, sync, disconnect, set_mode, set_default. A disconnect while an
    /// external row is in use answers with the row to switch away from first.
    pub fn external_command(
        &mut self,
        action: &str,
        enrollment: Option<&str>,
        id: Option<&str>,
        mode: Option<&str>,
        confirm: bool,
        switch_back: bool,
    ) -> Result<Option<Value>> {
        if self.storage_fault.get() {
            return Err("账号数据库保存失败，已暂停变更；修复存储后重新连接服务");
        }
        match action {
            "connect" => self.connect_external(enrollment.ok_or("缺少配对串")?)?,
            "sync" => {
                let source = self
                    .external
                    .config
                    .sources
                    .iter()
                    .find(|s| s.connected_at > 0)
                    .ok_or("还没有连上外部来源")?
                    .clone();
                self.start_external_directory(source);
            }
            "disconnect" => return self.disconnect_external(confirm, switch_back),
            "set_mode" => self.set_external_mode(
                id.ok_or("缺少账号标识")?,
                mode.ok_or("缺少权限档位")?,
                confirm,
            )?,
            "set_default" => {
                let mode = mode
                    .filter(|m| external::MODES.contains(m))
                    .ok_or("权限档位无效")?;
                let source = self
                    .external
                    .config
                    .sources
                    .first_mut()
                    .ok_or("还没有连上外部来源")?;
                source.default_mode = mode.into();
                self.external.save()?;
            }
            _ => return Err("外部来源不支持这个操作"),
        }
        Ok(None)
    }
    fn connect_external(&mut self, text: &str) -> Result<()> {
        let enrollment = external::Enrollment::decode(text)?;
        let existing = self.external.config.sources.first().cloned();
        if let Some(s) = &existing {
            if self.external.job(&s.id).is_some() {
                return Err("正在连接外部来源，请稍等");
            }
            if s.state == "connected" {
                return Err("已经连着一个外部来源，先断开");
            }
            if s.site_url != enrollment.site_url
                && self.external.config.rows.values().any(|r| r.source == s.id)
            {
                return Err("这是另一个来源的配对串；先断开现有来源再连接");
            }
        }
        let mut source = match existing {
            Some(s) if s.site_url == enrollment.site_url => s,
            other => {
                // Another center replaces a source that never got going here.
                if let Some(old) = other {
                    self.external.forget_pairing(&self.platform, &old.id)?;
                    self.external.config.merged.remove(&old.id);
                }
                external::Source {
                    id: external::source_id(&enrollment.site_url),
                    label: external::PLACEHOLDER.into(),
                    site_url: enrollment.site_url.clone(),
                    manage_url: enrollment.site_url.clone(),
                    default_mode: "view".into(),
                    ..Default::default()
                }
            }
        };
        // Until the center accepts the pairing nothing from this source can be used.
        source.state = "error".into();
        source.error = None;
        let previous = self.external.config.sources.clone();
        self.external.config.sources = vec![source.clone()];
        if let Err(message) = self.external.save() {
            self.external.config.sources = previous;
            return Err(message);
        }
        // Requests made with an earlier pairing describe what no longer holds.
        self.external.renew_generation();
        let platform = self.platform.clone();
        let id = source.id.clone();
        self.external.start(&format!("connect:{id}"), &id, move || {
            external::connect(&platform, &source, enrollment)
        });
        Ok(())
    }
    fn start_external_directory(&mut self, source: external::Source) {
        self.external.next_directory = now() + external::DIRECTORY_EVERY;
        let platform = self.platform.clone();
        let id = source.id.clone();
        self.external
            .start(&format!("directory:{id}"), &id, move || {
                external::sync(&platform, &source)
            });
    }
    fn set_external_mode(&mut self, id: &str, mode: &str, confirm: bool) -> Result<()> {
        if !external::MODES.contains(&mode) {
            return Err("权限档位无效");
        }
        let index = self.index(id)?;
        let row = self
            .external
            .config
            .rows
            .get(id)
            .ok_or("这个账号不是外部账号")?
            .clone();
        if row.mode == mode {
            return Ok(());
        }
        let ruled = self
            .db
            .automation
            .config
            .accounts
            .iter()
            .any(|r| r.id == id);
        if row.mode == "switch" {
            if self.db.accounts[index].native || self.db.desktop.selected.as_deref() == Some(id) {
                return Err(if self.db.accounts[index].provider == "codex" {
                    IN_USE_CODEX
                } else {
                    IN_USE_CODE
                });
            }
            if ruled && !confirm {
                return Err(LEAVES_RELAY);
            }
        }
        let rules = self.db.automation.config.accounts.clone();
        if mode != "switch" && ruled {
            self.db.automation.config.accounts.retain(|r| r.id != id);
            if let Err(message) = self.save() {
                self.db.automation.config.accounts = rules;
                return Err(message);
            }
        }
        self.external.config.rows.get_mut(id).unwrap().mode = mode.into();
        self.external.save()
    }
    fn disconnect_external(&mut self, confirm: bool, switch_back: bool) -> Result<Option<Value>> {
        if !confirm {
            return Err("请确认断开外部来源");
        }
        let source = self
            .external
            .config
            .sources
            .first()
            .ok_or("没有连着外部来源")?
            .clone();
        let rows: Vec<String> = self
            .external
            .config
            .rows
            .iter()
            .filter(|(_, r)| r.source == source.id)
            .map(|(id, _)| id.clone())
            .collect();
        // A reset in flight is pending, so unresolved too.
        if self
            .db
            .reset_attempts
            .iter()
            .any(|a| rows.contains(&a.account_id) && a.outcome.unresolved())
        {
            return Err("外部账号有正在进行或待确认的重置，请等它结束或重试上次操作");
        }
        for id in rows
            .iter()
            .filter(|id| self.external_held(id))
            .cloned()
            .collect::<Vec<_>>()
        {
            if !switch_back {
                return Ok(Some(json!({"needs_switch": id})));
            }
            let provider = self.db.accounts[self.index(&id)?].provider.clone();
            let back = self
                .external
                .config
                .previous
                .get(&provider)
                .filter(|p| self.db.accounts.iter().any(|a| a.id == **p) && !self.is_external(p))
                .cloned()
                .ok_or("没有记下上一个本机账号，请先在页面上换到其他账号")?;
            self.activate(&back, true)?;
            if self.external_held(&id) {
                return Err("正在换回本机账号，换好后再断开来源");
            }
        }
        let rules = self.db.automation.config.accounts.clone();
        let accounts = self.db.accounts.clone();
        for id in &rows {
            self.drop_external_row(id);
        }
        if let Err(message) = self.save() {
            self.db.automation.config.accounts = rules;
            self.db.accounts = accounts;
            return Err(message);
        }
        self.external.forget_pairing(&self.platform, &source.id)?;
        self.external.config.sources.retain(|s| s.id != source.id);
        self.external.config.merged.remove(&source.id);
        self.external.config.selected.clear();
        self.external.config.previous.clear();
        self.external.clear();
        self.external.renew_generation();
        self.external.save()?;
        Ok(None)
    }
    /// Drops an external row; its mode waits 30 days for the account to come back.
    fn drop_external_row(&mut self, id: &str) {
        if let Some(row) = self.external.config.rows.remove(id) {
            let key = external::key(&row.source, &row.account_ref);
            self.external.config.parked.insert(
                key.clone(),
                external::Parked {
                    mode: row.mode,
                    until: now() + external::PARKED_FOR,
                },
            );
            self.external.forget(&key);
        }
        self.external.reported.remove(id);
        if let Ok(index) = self.index(id) {
            self.db.accounts.remove(index);
        }
        self.db.automation.config.accounts.retain(|r| r.id != id);
        self.unsaved.remove(id);
        self.quota.forget(id);
        self.warmup.forget(id);
    }
    /// What the row of an account in the directory says: its name, whose it is and its state.
    fn external_account(&self, key: &str, entry: &Value) -> (String, Option<Metadata>) {
        let email = entry["identity"]["email"].as_str();
        let provider = external::provider(entry);
        let label = email
            .or(entry["label"].as_str())
            .and_then(|l| valid_label(l).ok())
            .unwrap_or_else(|| placeholder_label(provider).into());
        let leased = self.external.last_lease(key).map(|l| l.metadata.clone());
        let metadata = leased
            .as_ref()
            .map(|m| m.owner.clone())
            .or_else(|| external::owner(entry))
            .map(|owner| Metadata {
                owner,
                display: leased
                    .as_ref()
                    .map(|m| m.display.clone())
                    .or_else(|| email.map(str::to_owned))
                    .unwrap_or_else(|| "未提供邮箱".into()),
                plan: leased
                    .as_ref()
                    .map(|m| m.plan.clone())
                    .or_else(|| entry["plan"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "未知".into()),
                expires_at: leased.as_ref().and_then(|m| m.expires_at),
                // The login the center keeps, shown as the card's login expiry.
                refresh_expires_at: entry["login_expires_at"].as_u64(),
            });
        (label, metadata)
    }
    /// Brings the rows in line with each source's directory: new accounts get a row, accounts this
    /// Mac has its own row of are merged, accounts gone from the directory go once not in use.
    pub(super) fn reconcile_external_rows(&mut self) {
        if self.storage_fault.get() {
            return;
        }
        let time = now();
        let (mut rows, mut config) = (false, false);
        let orphans: Vec<String> = self
            .external
            .config
            .rows
            .keys()
            .filter(|id| !self.db.accounts.iter().any(|a| &a.id == *id))
            .cloned()
            .collect();
        for id in orphans {
            self.external.config.rows.remove(&id);
            config = true;
        }
        let parked = self.external.config.parked.len();
        self.external.config.parked.retain(|_, p| p.until > time);
        config |= parked != self.external.config.parked.len();
        for source in self.external.config.sources.clone() {
            if source.read_at.is_none() {
                continue;
            }
            let mut merged = vec![];
            let mut listed = HashSet::new();
            for entry in &source.directory {
                let Some(account_ref) = entry["account_ref"].as_str() else {
                    continue;
                };
                listed.insert(account_ref.to_string());
                let key = external::key(&source.id, account_ref);
                let provider = external::provider(entry);
                let row = self
                    .external
                    .config
                    .rows
                    .iter()
                    .find(|(_, r)| r.source == source.id && r.account_ref == account_ref)
                    .map(|(id, _)| id.clone());
                let (label, metadata) = self.external_account(&key, entry);
                // The same person as a row of this Mac or the cloud: only that row is shown.
                let local = metadata.as_ref().and_then(|m| {
                    self.db.accounts.iter().find(|a| {
                        a.provider == provider
                            && !self.is_external(&a.id)
                            && a.metadata.as_ref().is_some_and(|l| l.owner == m.owner)
                    })
                });
                if let Some(local) = local {
                    let email = local.metadata.as_ref().map(|m| m.display.clone());
                    match &row {
                        Some(id) if self.external_held(id) => {}
                        Some(id) => {
                            let id = id.clone();
                            merged.push(json!({"provider": provider, "email": email}));
                            self.drop_external_row(&id);
                            (rows, config) = (true, true);
                            continue;
                        }
                        None => {
                            merged.push(json!({"provider": provider, "email": email}));
                            continue;
                        }
                    }
                }
                let Some(id) = row else {
                    let Ok(id) = random_id() else {
                        break;
                    };
                    let mode = self
                        .external
                        .config
                        .parked
                        .remove(&key)
                        .map(|p| p.mode)
                        .filter(|m| external::MODES.contains(&m.as_str()))
                        .unwrap_or_else(|| {
                            if external::MODES.contains(&source.default_mode.as_str()) {
                                source.default_mode.clone()
                            } else {
                                "view".into()
                            }
                        });
                    self.external.config.rows.insert(
                        id.clone(),
                        external::Row {
                            source: source.id.clone(),
                            account_ref: account_ref.into(),
                            mode,
                        },
                    );
                    self.db.accounts.push(Account {
                        id,
                        provider: provider.into(),
                        label,
                        metadata,
                        native: false,
                        automatic: false,
                        state: "external".into(),
                        message: external::ROW_MESSAGE.into(),
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
                    (rows, config) = (true, true);
                    continue;
                };
                let Ok(index) = self.index(&id) else {
                    continue;
                };
                let a = &mut self.db.accounts[index];
                if a.label != label
                    || a.state != "external"
                    || a.message != external::ROW_MESSAGE
                    || a.automatic
                    || serde_json::to_value(&a.metadata).ok()
                        != serde_json::to_value(&metadata).ok()
                {
                    a.label = label;
                    a.metadata = metadata;
                    a.state = "external".into();
                    a.message = external::ROW_MESSAGE.into();
                    a.automatic = false;
                    rows = true;
                }
            }
            let gone: Vec<String> = self
                .external
                .config
                .rows
                .iter()
                .filter(|(_, r)| r.source == source.id && !listed.contains(&r.account_ref))
                .map(|(id, _)| id.clone())
                .collect();
            for id in gone {
                if !self.external_held(&id) {
                    self.drop_external_row(&id);
                    (rows, config) = (true, true);
                    continue;
                }
                // In use: it stays until switched away, as a cloud row moved away does.
                let message = format!("已从 {} 删除；换到其他账号后会移除", source.label);
                if let Ok(index) = self.index(&id) {
                    let a = &mut self.db.accounts[index];
                    if a.state != "reauth" || a.message != message {
                        a.state = "reauth".into();
                        a.message = message;
                        rows = true;
                    }
                }
            }
            if self.external.config.merged.get(&source.id) != Some(&merged) {
                self.external
                    .config
                    .merged
                    .insert(source.id.clone(), merged);
                config = true;
            }
        }
        // Rows first: a mapping outlives a row change that was not saved, and is applied again.
        if rows && self.save().is_err() {
            return;
        }
        if config && let Err(message) = self.external.save() {
            self.notice = Some(message.into());
        }
    }
    /// Background work for external sources: one routine request at a time, beside anything a
    /// person starts.
    pub(super) fn external_tick(&mut self) {
        if self.external.poll()
            && let Err(message) = self.external.save()
        {
            self.notice = Some(message.into());
        }
        self.reconcile_external_rows();
        if self.storage_fault.get() {
            return;
        }
        self.settle_external();
        self.external_follow();
        if !self.external.light.is_empty() {
            return;
        }
        let Some(source) = self
            .external
            .config
            .sources
            .iter()
            .find(|s| s.connected_at > 0)
            .cloned()
        else {
            return;
        };
        let time = now();
        let retry = |m: &Self, key: &str| {
            m.external
                .tried
                .get(key)
                .is_none_or(|at| *at + external::RETRY <= time)
        };
        // The access in the native store, renewed before it runs out or once a newer one is in.
        let due = self
            .external
            .config
            .selected
            .iter()
            .map(|(p, id)| (p.clone(), id.clone()))
            .find(|(provider, id)| {
                let Some(key) = self.external_key(id) else {
                    return false;
                };
                self.db.accounts.iter().any(|a| a.id == *id && a.native)
                    && self.external_usable(id).is_ok()
                    && self
                        .external
                        .written
                        .get(provider)
                        .is_none_or(|(expires, version)| {
                            *expires <= time + external::NATIVE_LEAD
                                || self
                                    .external
                                    .lease(&key)
                                    .is_some_and(|l| l.version > *version)
                        })
                    && retry(self, &format!("native:{provider}"))
            });
        if let Some((provider, id)) = due {
            self.external
                .tried
                .insert(format!("native:{provider}"), time);
            if let (Ok(native), Some((source, Some(entry)))) = (
                self.native(&provider),
                self.external_entry(&id)
                    .map(|(s, e)| (s.clone(), e.cloned())),
            ) {
                let platform = self.platform.clone();
                self.external.start(
                    &format!("native:{provider}"),
                    &source.id.clone(),
                    move || external::native_sync(&platform, &source, &entry, &native, &id),
                );
                return;
            }
        }
        if self.external.next_directory <= time {
            self.start_external_directory(source);
            return;
        }
        if source.state != "connected" {
            return;
        }
        // Access for quota, asked for when a reading is due.
        let wanted = self
            .external
            .config
            .rows
            .keys()
            .find(|id| {
                let Some(key) = self.external_key(id) else {
                    return false;
                };
                self.external.lease(&key).is_none()
                    && self.external_usable(id).is_ok()
                    && self.quota.due(id, time)
                    && retry(self, &key)
            })
            .cloned();
        if let Some(id) = wanted {
            let _ = self.start_external_lease(&id, "quota", None);
            return;
        }
        // Each successful reading goes back to the center once.
        let reading = self.external.config.rows.iter().find_map(|(id, row)| {
            let (at, windows, credits) = self.quota.report(id)?;
            if self.external.reported.get(id).is_some_and(|r| *r >= at) {
                return None;
            }
            let key = external::key(&row.source, &row.account_ref);
            let version = self.external.last_lease(&key).map(|l| l.version);
            Some((
                id.clone(),
                at,
                json!({"account_ref": row.account_ref, "credential_version": version,
                    "observed_at": at, "windows": windows, "reset_credits": credits}),
            ))
        });
        if let Some((id, at, body)) = reading {
            self.external.reported.insert(id.clone(), at);
            let platform = self.platform.clone();
            let source_id = source.id.clone();
            self.external
                .start(&format!("report:{id}"), &source_id, move || {
                    external::report(&platform, &source, body)
                });
        }
    }
    /// A rule may name an external row only while it may be switched to.
    pub(super) fn external_rule_allowed(&self, id: &str) -> Result<()> {
        match self.external.config.rows.get(id) {
            Some(row) if row.mode != "switch" => Err(NOT_IN_RELAY),
            _ => Ok(()),
        }
    }
    pub(super) fn external_local_guard(&self, id: &str) -> Result<()> {
        if self.is_external(id) {
            Err(EXTERNAL_RENEWS)
        } else {
            Ok(())
        }
    }
    pub(super) fn external_unreadable(&self, id: &str) -> Option<&'static str> {
        (self.is_external(id) && !self.external_readable(id)).then_some(UNREADABLE)
    }
}
