use super::*;
use crate::desktop::{self, Completion, Refresh};

impl Accounts {
    pub fn configure_desktop(&mut self, enabled: Option<bool>, port: Option<u16>) -> Result<()> {
        self.ensure_idle()?;
        if let Some(port) = port {
            if port < 1024 {
                return Err("桌面端口必须在 1024–65535 之间");
            }
            if port != self.db.desktop.port
                && self.db.desktop.selected.is_some()
                && (self.client_running)("codex")
            {
                return Err("更改已接管桌面的端口前，请先退出 Codex 桌面");
            }
        }
        if enabled == Some(false)
            && self.db.desktop.selected.is_some()
            && (self.client_running)("codex")
        {
            return Err("关闭实验连接前请退出 Codex 桌面；不会自动注销账号");
        }
        if let Some(enabled) = enabled {
            self.db.desktop.enabled = enabled;
        }
        if let Some(port) = port {
            self.db.desktop.port = port;
        }
        self.desktop = desktop::Runtime::default();
        self.save()?;
        if self.db.desktop.enabled {
            self.desktop.message = "正在检查桌面连接".into();
        }
        Ok(())
    }
    pub fn launch_desktop(&self) -> Result<()> {
        if !self.db.desktop.enabled {
            return Err("请先启用实验连接");
        }
        if desktop::desktop_running()? {
            return Err("首次接入请先手动退出 Codex 桌面，再点击启动；不会结束你的任务");
        }
        let app = desktop::DESKTOP_APPS
            .into_iter()
            .find(|p| Path::new(p).exists())
            .ok_or("未找到官方桌面应用")?;
        let status = desktop::open_desktop(app, self.db.desktop.port)
            .status()
            .map_err(|_| "无法启动官方桌面")?;
        if status.success() {
            Ok(())
        } else {
            Err("无法启动官方桌面")
        }
    }
    pub(super) fn desktop_owns(&self, id: &str) -> bool {
        let Ok(index) = self.index(id) else {
            return false;
        };
        self.db.accounts[index].native
            && self.db.desktop.enabled
            && self.db.desktop.selected.as_deref() == Some(id)
            && self.desktop.connected
            && self.desktop.bound
            && self.desktop.mode.as_deref() == Some("chatgptAuthTokens")
            && self.db.accounts[index]
                .metadata
                .as_ref()
                .is_some_and(|m| Some(&m.owner) == self.desktop.owner.as_ref())
            && !(self.desktop.other_clients)()
    }
    pub(super) fn activate_desktop(&mut self, id: &str, recover: bool) -> Result<()> {
        self.desktop_switch_ready(id)?;
        let client = self.desktop.client(&self.db.desktop);
        let id = id.to_string();
        self.desktop.message = "正在检查桌面连接与任务状态；尚未开始切换".into();
        self.desktop.action_feedback(&id, "checking");
        self.desktop.job = Some(std::thread::spawn(move || Completion::Preflight {
            id,
            recover,
            result: client.raw("preflight", Value::Null),
        }));
        Ok(())
    }
    // Hot switch is on but the desktop runs without its debug port: restart it in debug mode
    // when no Codex turn is running, and switch once it connects.
    pub fn relaunch_desktop_and_activate(&mut self, id: &str) -> Result<()> {
        if !self.db.desktop.enabled {
            return Err("请先启用实验连接");
        }
        if self.automation.job.is_some() || self.db.automation.run.is_some() {
            return Err("请等待自动额度流程完成");
        }
        if self.db.switch.is_some() {
            return Err("请先处理上次未完成的切换");
        }
        self.desktop_switch_ready(id)?;
        if self.db.accounts[self.index(id)?].provider != "codex" {
            return Err("只有 Codex 账号可以通过桌面切换");
        }
        if (self.desktop.turns_running)(&self.sources.codex_home)? {
            return Err("Codex 里还有任务在运行；等它结束后再切换，桌面不会被重启");
        }
        let (relaunch, port, id) = (self.desktop.relaunch, self.db.desktop.port, id.to_string());
        self.desktop.message = "正在以调试模式重启 Codex 桌面；没有任务在运行".into();
        self.desktop.action_feedback(&id, "relaunching");
        self.desktop.job = Some(std::thread::spawn(move || Completion::Relaunch {
            id,
            result: relaunch(port),
        }));
        Ok(())
    }
    fn desktop_switch_ready(&self, id: &str) -> Result<()> {
        if self.job.is_some()
            || !self.check_queue.is_empty()
            || self.reset_job.is_some()
            || self.pending_login.is_some()
            || self.desktop.job.is_some()
            || self.desktop.refresh.is_some()
        {
            return Err("请等待当前检查或桌面操作完成");
        }
        if self.storage_fault.get() {
            return Err("账号数据库不可写，已停止切换");
        }
        if (self.desktop.other_clients)() {
            return Err(
                "VS Code 的 Codex 插件或独立 Codex CLI 正在运行；请在 VS Code 里停用该插件（或退出 VS Code）并退出 CLI 后再切换",
            );
        }
        let index = self.index(id)?;
        switch_target_ready(&self.db.accounts[index])?;
        let target = self.bundle(&self.db.accounts[index])?;
        let metadata = target.metadata()?;
        if metadata.expires_at.is_none_or(|t| t <= now() + 60) {
            return Err("目标 Access token 已到期或即将到期，请先在线检查或重新登录");
        }
        Ok(())
    }
    pub(super) fn commit_desktop(
        &mut self,
        id: &str,
        recover: bool,
        observed: Value,
    ) -> Result<()> {
        if (self.desktop.other_clients)() {
            return Err("检测到 VS Code 的 Codex 插件或独立 Codex CLI，切换未开始");
        }
        let target = self.bundle(&self.db.accounts[self.index(id)?])?;
        let metadata = target.metadata()?;
        if metadata.expires_at.is_none_or(|t| t <= now() + 60) {
            return Err("目标 Access token 已到期或即将到期，切换未开始");
        }
        let auth = desktop::external_auth(&target)?;
        let native = self.native("codex")?;
        if native.mode != "file" {
            return Err("桌面实验切换暂只支持 Codex file 认证后端");
        }
        let old = native.read(&self.platform)?;
        if observed["owner"] != old.metadata()?.owner && observed["owner"] != metadata.owner {
            return Err("桌面与默认文件身份不一致，切换未开始");
        }
        self.desktop.observed(&observed)?;
        let previous = if recover {
            self.db
                .switch
                .as_ref()
                .and_then(|s| s.previous.clone())
                .ok_or("缺少切换恢复记录")?
        } else {
            self.adopt_native(old.clone(), "原生账号")?;
            self.db
                .accounts
                .iter()
                .find(|a| a.provider == "codex" && a.native)
                .map(|a| a.id.clone())
                .ok_or("请先纳管当前原生登录")?
        };
        let previous_bundle = Bundle::decode(self.platform.vault_read(&previous)?)?;
        let old_owner = previous_bundle.metadata()?.owner;
        if !old.same_owner(&previous_bundle)? && !old.same_owner(&target)? {
            return Err("原生账号已由其他操作改变，未覆盖登录");
        }
        self.db.switch = Some(Switch {
            provider: "codex".into(),
            target: id.into(),
            previous: Some(previous.clone()),
            desktop: true,
        });
        self.save()?;
        let client = self.desktop.client(&self.db.desktop);
        let platform = self.platform.clone();
        let id = id.to_string();
        self.desktop.message = "正在确认桌面身份并更新全局默认".into();
        self.desktop.action_feedback(&id, "switching");
        self.desktop.job = Some(std::thread::spawn(move || {
            let result = (|| {
                let mut value = client.call(
                    "apply",
                    json!({"previousOwner":old_owner,"owner":metadata.owner,"auth":auth}),
                )?;
                validate_applied(&value, &metadata.owner)?;
                // Re-read after the desktop relinquishes managed auth, preserving its latest generation.
                let latest = native.read(&platform)?;
                let mut committed = target.clone();
                if latest.same_owner(&previous_bundle)? {
                    platform.vault_write(&previous, &latest.encoded())?;
                    if previous_bundle.same_owner(&target)? {
                        // A→A adoption must never roll back a generation refreshed by the desktop
                        // during its preflight reads. External mode has now relinquished that writer.
                        if latest != target {
                            value=client.call("update",json!({"owner":metadata.owner,"previousOwner":metadata.owner,"auth":desktop::external_auth(&latest)?}))?;
                            validate_applied(&value, &metadata.owner)?;
                        }
                        committed = latest;
                    } else {
                        native.apply(&platform, &latest, &target)?;
                    }
                } else if latest != target {
                    return Err("原生凭据已变化；桌面已切换，文件等待恢复确认");
                }
                if native.read(&platform)? != committed {
                    return Err("默认文件写入尚未确认，请恢复切换");
                }
                value["metadata"] = serde_json::to_value(committed.metadata()?).unwrap();
                Ok(value)
            })();
            Completion::Activate { id, result }
        }));
        Ok(())
    }
    pub(super) fn desktop_tick(&mut self) {
        let skip = self.db.desktop.enabled;
        if self.desktop.quit_confirmation_skipped != Some(skip) {
            // Tried once per change: if it fails, a desktop about to be restarted still asks
            // before quitting, as it did before.
            let _ = (self.desktop.skip_quit_confirmation)(skip);
            self.desktop.quit_confirmation_skipped = Some(skip);
        }
        if self.desktop.job.as_ref().is_some_and(|j| j.is_finished()) {
            // Retry delay starts at completion, so slow/unavailable bridges cannot
            // keep configuration controls perpetually occupied.
            self.desktop.last_poll = now();
            let completion = self.desktop.job.take().unwrap().join();
            match completion {
                Ok(Completion::Preflight {
                    id,
                    recover,
                    result,
                }) => {
                    let (failure, declined) = match result {
                        Err(message) => (Some(message.to_string()), false),
                        Ok(value) if value["ok"] != true => (
                            Some(desktop::refusal(&value)),
                            desktop::declined_for_tasks(&value),
                        ),
                        Ok(value) => (
                            self.commit_desktop(&id, recover, value)
                                .err()
                                .map(str::to_string),
                            false,
                        ),
                    };
                    if let Some(message) = failure {
                        if !declined {
                            self.desktop.connected = false;
                            self.desktop.target = None;
                        }
                        self.desktop.message = if self.db.switch.is_some() {
                            format!("{message}；上次切换记录仍保留")
                        } else {
                            format!("{message}；切换未开始")
                        };
                        // Only an unfinished switch is a standing condition; a switch that never
                        // started is reported on the attempt itself, with its time.
                        if self.db.switch.is_some() {
                            self.notice = Some(self.desktop.message.clone());
                        }
                        self.desktop.action_feedback(&id, "failed");
                    }
                }
                Ok(Completion::Reconcile(result)) => {
                    if let Err(message) =
                        result.and_then(|value| self.finish_desktop_reconcile(value))
                    {
                        self.desktop.message = format!("{message}；上次切换记录仍保留");
                    }
                }
                Ok(Completion::Activate { id, result }) => match result {
                    Ok(value) => {
                        if self.desktop.observed(&value).is_ok() {
                            for a in &mut self.db.accounts {
                                if a.provider == "codex" {
                                    a.native = a.id == id;
                                }
                            }
                            self.db.desktop.selected = Some(id.clone());
                            self.db.switch = None;
                            if let Ok(i) = self.index(&id) {
                                let a = &mut self.db.accounts[i];
                                a.metadata = serde_json::from_value(value["metadata"].clone()).ok();
                                a.automatic = true;
                                a.next_check = now() + 60;
                                a.message =
                                    "桌面与全局默认已切换；由 AuthManager 维护桌面凭据".into();
                                a.state = "saved".into();
                            }
                            self.desktop.message = if value["contextPreserved"] == false {
                                "账号与默认文件已切换，但部分页面状态未能保留；请核对草稿".into()
                            } else {
                                "桌面与全局默认一致，实验连接已接管".into()
                            };
                            if value["contextPreserved"] == false {
                                self.notice = Some(self.desktop.message.clone());
                            }
                            if let Err(e) = self.save() {
                                self.desktop.message = e.into();
                                self.desktop.action_feedback(&id, "failed");
                            } else {
                                self.desktop.action_feedback(&id, "complete");
                            }
                        } else {
                            self.desktop.connected = false;
                            self.desktop.message =
                                "桌面返回的身份不完整；请核对并恢复当前登录状态".into();
                            self.desktop.action_feedback(&id, "failed");
                        }
                    }
                    Err(message) => {
                        self.desktop.connected = false;
                        self.desktop.message = format!("{message}；请恢复当前登录状态");
                        self.desktop.action_feedback(&id, "failed");
                    }
                },
                Ok(Completion::Observe(result)) => match result {
                    Ok(value) => {
                        self.observe_strategy_usage(&value);
                        if let Err(message) = self.desktop.observed(&value) {
                            self.desktop.connected = false;
                            self.desktop.message = message.into();
                            return;
                        }
                        self.desktop.message = "桌面连接就绪；选择账号后将更新全局默认".into();
                        if let Some((id, _)) = self.desktop.pending_switch.take() {
                            if let Err(message) = self.activate_desktop(&id, false) {
                                self.desktop.message = format!("{message}；切换未开始");
                                self.desktop.action_feedback(&id, "failed");
                            }
                            return;
                        }
                        if let Some(id) = self.db.desktop.selected.clone() {
                            let owned = self
                                .index(&id)
                                .ok()
                                .and_then(|i| self.db.accounts[i].metadata.as_ref())
                                .is_some_and(|m| Some(&m.owner) == self.desktop.owner.as_ref());
                            if !owned {
                                self.desktop.message =
                                    "桌面身份与已选账号不同，已暂停自动接管".into();
                                return;
                            }
                            if self.desktop.mode.as_deref() == Some("chatgpt")
                                && self.db.switch.is_none()
                                && self.job.is_none()
                                && self.pending_login.is_none()
                            {
                                if let Err(e) = self.activate_desktop(&id, false) {
                                    self.desktop.message = e.into();
                                }
                                return;
                            }
                            if !self.desktop.bound {
                                let client = self.desktop.client(&self.db.desktop);
                                let owner = self.desktop.owner.clone().unwrap();
                                self.desktop.job = Some(std::thread::spawn(move || {
                                    Completion::Update(client.call("bind", json!({"owner":owner})))
                                }));
                                self.desktop.message = "正在恢复桌面续期回调".into();
                                return;
                            }
                            self.desktop.message = "桌面与全局默认一致，实验连接已接管".into();
                            if self.desktop.refresh.is_none()
                                && self.db.switch.is_none()
                                && let Some(request) =
                                    value["requests"].as_array().and_then(|r| r.first())
                            {
                                self.desktop.refresh = Some(Refresh {
                                    id: request["id"].clone(),
                                    epoch: request["epoch"].clone(),
                                    instance: value["instance"].as_str().unwrap_or("").into(),
                                    account_id: id,
                                    deadline: now() + 7
                                        - (request["ageMs"].as_u64().unwrap_or(7000) / 1000).min(7),
                                    hash: value["hash"].as_str().unwrap_or("").into(),
                                    attempted: false,
                                });
                            }
                        }
                    }
                    Err(e) => {
                        self.desktop.connected = false;
                        self.desktop.message = e.into();
                        self.desktop.target = None;
                        // A just restarted desktop may need a few polls before its page is ready.
                        if let Some((id, deadline)) = self.desktop.pending_switch.clone()
                            && now() >= deadline
                        {
                            self.desktop.pending_switch = None;
                            self.desktop.message = format!("桌面已重启但没有连上：{e}；切换未开始");
                            self.desktop.action_feedback(&id, "failed");
                        }
                    }
                },
                Ok(Completion::Relaunch { id, result }) => match result {
                    Ok(()) => {
                        // A new desktop instance: the previous page target no longer exists.
                        self.desktop.target = None;
                        self.desktop.pending_switch = Some((id.clone(), now() + 60));
                        self.desktop.last_poll = 0;
                        self.desktop.message = "桌面已以调试模式重启，正在连接后切换".into();
                        self.desktop.action_feedback(&id, "relaunching");
                    }
                    Err(message) => {
                        self.desktop.message = format!("{message}；切换未开始");
                        self.desktop.action_feedback(&id, "failed");
                    }
                },
                Ok(Completion::Restart(result)) => match result {
                    Ok(()) => {
                        self.desktop.target = None;
                        self.desktop.last_poll = 0;
                        self.desktop.message = "Codex 桌面已以调试模式重新启动".into();
                    }
                    Err(message) => {
                        // Never retry in a loop: the person restarts it once instead.
                        self.desktop.restart_failed = true;
                        self.desktop.message =
                            format!("Codex 桌面未能以调试模式重新启动：{message}");
                    }
                },
                Ok(Completion::Reply(result)) => {
                    if let Err(e) = result {
                        self.desktop.message = e.into();
                    } else {
                        self.desktop.dirty = false;
                    }
                }
                Ok(Completion::Update(result)) => match result {
                    Ok(value) if value["confirmed"] == true => {
                        let _ = self.desktop.observed(&value);
                        self.desktop.dirty = false;
                    }
                    _ => {
                        self.desktop.connected = false;
                        self.desktop.message = "新凭据已保存，桌面更新尚未确认".into();
                    }
                },
                Err(_) => {
                    self.desktop.connected = false;
                    self.desktop.message = "桌面操作中断，请核对并恢复".into();
                }
            }
        }
        if !self.db.desktop.enabled
            || self.storage_fault.get()
            || self.desktop.job.is_some()
            || self.automation.job.is_some()
            || self.db.automation.run.is_some()
        {
            return;
        }
        if let Some(refresh) = self.desktop.refresh.clone() {
            // Never send a generation from a different binding, even when work finished later.
            let bound = self.db.desktop.selected.as_deref() == Some(&refresh.account_id)
                && self.desktop.instance.as_deref() == Some(&refresh.instance)
                && self.db.switch.is_none();
            if !bound || now() >= refresh.deadline {
                self.reply_refresh(None);
                return;
            }
            if self.job.is_none() {
                if let Ok(index) = self.index(&refresh.account_id)
                    && let Ok(bundle) = self.bundle(&self.db.accounts[index])
                {
                    use sha2::{Digest, Sha256};
                    let hash = format!(
                        "{:x}",
                        Sha256::digest(
                            bundle.credential["tokens"]["access_token"]
                                .as_str()
                                .unwrap_or("")
                                .as_bytes()
                        )
                    );
                    if hash != refresh.hash && self.desktop_owns(&refresh.account_id) {
                        self.reply_refresh(desktop::external_auth(&bundle).ok());
                        return;
                    }
                    if refresh.attempted {
                        self.reply_refresh(None);
                        return;
                    }
                    if let Some(r) = self.desktop.refresh.as_mut() {
                        r.attempted = true;
                    }
                    self.start_desktop_refresh(&refresh.account_id);
                } else {
                    self.reply_refresh(None);
                }
            }
            return;
        }
        if self.desktop.dirty
            && self.job.is_none()
            && self.db.switch.is_none()
            && let Some(id) = self.db.desktop.selected.clone()
            && self.desktop_owns(&id)
            && let Ok(i) = self.index(&id)
            && let Ok(bundle) = self.bundle(&self.db.accounts[i])
            && let Ok(auth) = desktop::external_auth(&bundle)
        {
            let owner = bundle.metadata().unwrap().owner;
            let client = self.desktop.client(&self.db.desktop);
            self.desktop.job = Some(std::thread::spawn(move || {
                Completion::Update(client.call(
                    "update",
                    json!({"owner":owner,"previousOwner":owner,"auth":auth}),
                ))
            }));
            return;
        }
        // Also while a switch waits for recovery: recovering it needs the debug port.
        if self.restart_undebugged_desktop() || self.db.switch.is_some() {
            return;
        }
        if now() < self.desktop.last_poll + 2 {
            return;
        }
        self.desktop.last_poll = now();
        let client = self.desktop.client(&self.db.desktop);
        self.desktop.job = Some(std::thread::spawn(move || {
            Completion::Observe(client.call("poll", Value::Null))
        }));
    }
    // While AuthManager runs, a desktop that starts without the debug port (opened from the Dock,
    // or restarted by its updater) is restarted in debug mode once it has finished starting and
    // before any turn starts in it. One that has run for a while is left alone: the person may be
    // in the middle of work.
    fn restart_undebugged_desktop(&mut self) -> bool {
        if self.desktop.restart_failed
            || self.desktop.pending_switch.is_some()
            || now() < self.desktop.last_watch + 2
        {
            return false;
        }
        self.desktop.last_watch = now();
        let Ok(Some((pid, age))) = (self.desktop.undebugged)(self.db.desktop.port) else {
            return false;
        };
        if self.desktop.settled_pid == Some(pid) {
            return false;
        }
        // Quitting a desktop before its startup finishes makes it show "failed to start" and stay
        // open, so look again on the next round.
        let Some(started) = (self.desktop.startup_finished)(&self.home, pid, age) else {
            if age > desktop::STARTUP_WAIT_SECS {
                self.desktop.settled_pid = Some(pid);
            }
            return false;
        };
        self.desktop.settled_pid = Some(pid);
        if now().saturating_sub(started) > desktop::FRESH_DESKTOP_SECS
            // `ps` rounds the age down to whole seconds.
            || desktop::turns_started_since(&self.sources.codex_home, now().saturating_sub(age + 1))
                .unwrap_or(true)
        {
            return false;
        }
        let (relaunch, port) = (self.desktop.relaunch, self.db.desktop.port);
        self.desktop.message = "Codex 桌面没有开启热切换，正在以调试模式重新启动".into();
        self.desktop.job = Some(std::thread::spawn(move || {
            Completion::Restart(relaunch(port))
        }));
        true
    }
    fn start_desktop_refresh(&mut self, id: &str) {
        let result = (|| -> Result<()> {
            if !self.desktop_owns(id) {
                return Err("当前登录由其他 Codex 客户端占用，桌面续期已延后");
            }
            let index = self.index(id)?;
            if matches!(
                self.db.accounts[index].state.as_str(),
                "reauth" | "unsaved" | "uncertain" | "attention"
            ) {
                return Err("账号需处理，桌面续期已停止");
            }
            let bundle = self.bundle(&self.db.accounts[index])?;
            let cancel = Arc::new(AtomicBool::new(false));
            let work = Work {
                transport: crate::maintenance::http,
                cancel: cancel.clone(),
                id: id.into(),
                bundle,
                platform: self.platform.clone(),
                profile: self.root.join("profiles").join(format!("check-{id}")),
                codex: self.codex.clone(),
                native: Some(self.native("codex")?),
            };
            self.db.accounts[index].state = "checking".into();
            self.save()?;
            self.job = Some(Job {
                id: id.into(),
                cancel,
                handle: std::thread::spawn(move || {
                    JobResult::Check(Box::new(work.run_force_refresh()))
                }),
            });
            Ok(())
        })();
        if let Err(e) = result {
            self.desktop.message = e.into();
            self.reply_refresh(None);
        }
    }
    fn reply_refresh(&mut self, auth: Option<Value>) {
        let Some(r) = self.desktop.refresh.take() else {
            return;
        };
        let client = self.desktop.client(&self.db.desktop);
        self.desktop.job = Some(std::thread::spawn(move || {
            Completion::Reply(client.call(
                "reply",
                json!({"id":r.id,"epoch":r.epoch,"instance":r.instance,"auth":auth}),
            ))
        }));
    }
    pub(super) fn desktop_stop(&mut self) {
        if let Some(job) = self.desktop.job.take() {
            let _ = job.join();
        }
        // Without AuthManager nothing restarts a desktop, so every desktop keeps its confirmation.
        if self.desktop.quit_confirmation_skipped == Some(true) {
            let _ = (self.desktop.skip_quit_confirmation)(false);
        }
        if self.db.desktop.enabled && self.desktop.connected {
            let _ = self
                .desktop
                .client(&self.db.desktop)
                .call("release", Value::Null);
        }
    }
    pub fn recover_desktop_native(&mut self, confirmed: bool) -> Result<()> {
        if self.automation.job.is_some() || self.db.automation.run.is_some() {
            return Err("请先停止并核对自动额度流程");
        }
        if !confirmed {
            return Err("请确认已完全退出相关 Codex 客户端");
        }
        if self.job.is_some()
            || self.desktop.job.is_some()
            || self.pending_login.is_some()
            || self.storage_fault.get()
        {
            return Err("请等待当前操作完成");
        }
        if (self.client_running)("codex") {
            return Err("请完全退出 Codex 桌面和 CLI 后再恢复默认文件状态");
        }
        let operation = self
            .db
            .switch
            .clone()
            .filter(|s| s.desktop)
            .ok_or("没有待恢复的桌面切换")?;
        let native = self.native("codex")?.read(&self.platform)?;
        let owner = native.metadata()?.owner;
        let known = [Some(operation.target), operation.previous]
            .into_iter()
            .flatten()
            .any(|id| {
                self.index(&id)
                    .ok()
                    .and_then(|i| self.db.accounts[i].metadata.as_ref())
                    .is_some_and(|m| m.owner == owner)
            });
        if !known {
            return Err("默认文件是未知账号，未修改记录或登录");
        }
        self.adopt_native(native, "原生账号")?;
        self.db.switch = None;
        self.db.desktop.enabled = false;
        self.db.desktop.selected = None;
        self.desktop = desktop::Runtime::default();
        self.notice =
            Some("已按默认文件恢复，实验连接已关闭；可以检查或重新登录账号后再开启".into());
        self.save()
    }

    pub fn reconcile_desktop(&mut self) -> Result<()> {
        if self.automation.job.is_some() || self.db.automation.run.is_some() {
            return Err("请先停止并核对自动额度流程");
        }
        if self.job.is_some()
            || self.reset_job.is_some()
            || self.pending_login.is_some()
            || self.desktop.job.is_some()
            || self.desktop.refresh.is_some()
            || self.storage_fault.get()
        {
            return Err("请等待当前操作完成");
        }
        if !self.db.switch.as_ref().is_some_and(|s| s.desktop) || self.db.desktop.selected.is_some()
        {
            return Err("此切换不能仅通过核对原账号取消，请使用原恢复入口");
        }
        let client = self.desktop.client(&self.db.desktop);
        self.desktop.message = "正在核对桌面和默认文件；不会重试账号切换".into();
        self.desktop.job = Some(std::thread::spawn(move || {
            Completion::Reconcile(client.call("probe", Value::Null))
        }));
        Ok(())
    }

    fn finish_desktop_reconcile(&mut self, value: Value) -> Result<()> {
        use sha2::{Digest, Sha256};
        let operation = self
            .db
            .switch
            .clone()
            .filter(|s| s.desktop)
            .ok_or("没有待核对的桌面切换")?;
        let previous = operation.previous.as_deref().ok_or("缺少原账号记录")?;
        let account = &self.db.accounts[self.index(previous)?];
        let native = self.native("codex")?.read(&self.platform)?;
        let owner = native.metadata()?.owner;
        let hash = format!(
            "{:x}",
            Sha256::digest(
                native.credential["tokens"]["access_token"]
                    .as_str()
                    .ok_or("原生认证缺少 Access token")?
            )
        );
        if self.db.desktop.selected.is_some()
            || previous == operation.target
            || !account.native
            || account.metadata.as_ref().is_none_or(|m| m.owner != owner)
            || value["owner"] != owner
            || value["mode"] != "chatgpt"
            || !value["boundOwner"].is_null()
            || value["hash"] != hash
        {
            return Err("尚不能确认桌面和文件均保持原账号；未取消切换记录");
        }
        self.desktop.observed(&value)?;
        self.db.switch = None;
        if let Err(message) = self.save() {
            self.db.switch = Some(operation);
            return Err(message);
        }
        self.desktop.message = "桌面连接就绪；已确认原账号保持，未执行的切换已取消".into();
        self.notice = Some("已核对桌面与默认文件仍为原账号；未切换账号，未改写认证".into());
        Ok(())
    }
}
fn validate_applied(value: &Value, owner: &str) -> Result<()> {
    if value["confirmed"] != true || value["owner"] != owner || value["mode"] != "chatgptAuthTokens"
    {
        return Err("桌面凭据可能已应用，但登录完成尚未确认");
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ack_and_changed_memory_do_not_commit_global_selection() {
        assert!(
            validate_applied(
                &json!({"owner":"target","mode":"chatgptAuthTokens","confirmed":false}),
                "target"
            )
            .is_err()
        );
        assert!(
            validate_applied(
                &json!({"owner":"other","mode":"chatgptAuthTokens","confirmed":true}),
                "target"
            )
            .is_err()
        );
        assert!(
            validate_applied(
                &json!({"owner":"target","mode":"chatgptAuthTokens","confirmed":true}),
                "target"
            )
            .is_ok()
        );
    }
}
