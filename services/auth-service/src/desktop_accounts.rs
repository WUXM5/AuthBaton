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
            && self.desktop_managed()
            && !(self.desktop.other_clients)()
    }
    // AuthManager supplies the desktop's login: the desktop is connected in external mode and
    // bound to the selected account. Connected alone only means its debug port answers.
    pub(super) fn desktop_managed(&self) -> bool {
        let Some(index) = self
            .db
            .desktop
            .selected
            .as_deref()
            .and_then(|id| self.index(id).ok())
        else {
            return false;
        };
        self.desktop.connected
            && self.desktop.bound
            && self.desktop.mode.as_deref() == Some("chatgptAuthTokens")
            && self.db.accounts[index]
                .metadata
                .as_ref()
                .is_some_and(|m| Some(&m.owner) == self.desktop.owner.as_ref())
    }
    pub(super) fn desktop_snapshot(&self) -> Value {
        let mut value = self.desktop.snapshot(&self.db.desktop);
        value["managed"] = json!(self.desktop_managed());
        value
    }
    // A short local record of what AuthManager asked of the desktop and why, so a reload or a
    // second login can be traced afterwards. Only the time, the operation, the account id and a
    // fixed reason: never credentials, responses or task names.
    pub(super) fn desktop_log(&self, op: &str, account: Option<&str>, reason: &str) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.root.join("desktop-log.jsonl");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 512 * 1024) {
            let _ = std::fs::rename(&path, self.root.join("desktop-log.1.jsonl"));
        }
        let line = json!({"at":now(),"op":op,"account":account,"reason":reason}).to_string();
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut file| writeln!(file, "{line}"));
    }
    // A switch the person asks for. A poll, a scheduled check or a reset in flight only delays
    // it: it starts once they finish instead of being refused.
    pub(super) fn request_desktop_switch(&mut self, id: &str) -> Result<()> {
        if self.automation.job.is_some() || self.db.automation.run.is_some() {
            return Err("自动额度流程正在处理或待核对；请先停止并核对该流程");
        }
        if self.storage_fault.get() {
            return Err("账号数据库不可写，已停止切换");
        }
        if self.cloud.job.is_some() || self.pending_login.is_some() {
            return Err("登录或云端操作正在进行，请完成后再切换");
        }
        if self.db.switch.is_some() {
            return Err("请先恢复上次切换的当前登录状态");
        }
        if self.desktop.switching().is_some() {
            return Err("正在切换账号，请等这次切换结束");
        }
        switch_target_ready(&self.db.accounts[self.index(id)?])?;
        if self.switch_waits() {
            self.desktop.pending_switch = Some((id.into(), now() + 30));
            self.desktop.message = "等当前的检查或桌面轮询结束后开始切换".into();
            self.desktop.action_feedback(id, "checking");
            return Ok(());
        }
        self.activate_desktop(id)
    }
    // Work that holds the slots a switch needs, and finishes by itself.
    fn switch_waits(&self) -> bool {
        self.job.is_some()
            || !self.check_queue.is_empty()
            || self.reset_job.is_some()
            || self.desktop.job.is_some()
            || self.desktop.refresh.is_some()
    }
    pub(super) fn activate_desktop(&mut self, id: &str) -> Result<()> {
        self.desktop_switch_ready(id)?;
        let owner = self.db.accounts[self.index(id)?]
            .metadata
            .as_ref()
            .map(|m| m.owner.clone());
        // Already logged in to this account by AuthManager: only the default file and the record
        // are left, so there is no login and running tasks do not matter.
        if self.desktop.connected
            && self.desktop.mode.as_deref() == Some("chatgptAuthTokens")
            && owner.is_some()
            && self.desktop.owner == owner
        {
            return self.commit_desktop(id, self.desktop.observation());
        }
        let client = self.desktop.client(&self.db.desktop);
        let id = id.to_string();
        self.desktop.message = "正在检查桌面连接与任务状态；尚未开始切换".into();
        self.desktop.action_feedback(&id, "checking");
        self.desktop_log("preflight", Some(&id), "切换或接管前检查桌面");
        self.desktop.job = Some(std::thread::spawn(move || Completion::Preflight {
            id,
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
        self.desktop_log("relaunch", Some(&id), "以调试模式重启桌面，连上后切换");
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
    // No journal: a switch that stops part way leaves the desktop either on its old login or on
    // the target's external one, which only AuthManager makes; the poll finishes the latter.
    pub(super) fn commit_desktop(&mut self, id: &str, observed: Value) -> Result<()> {
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
        self.adopt_native(old.clone(), "原生账号")?;
        let previous = self
            .db
            .accounts
            .iter()
            .find(|a| a.provider == "codex" && a.native)
            .map(|a| a.id.clone())
            .ok_or("请先纳管当前原生登录")?;
        let previous_bundle = Bundle::decode(self.platform.vault_read(&previous)?)?;
        let old_owner = previous_bundle.metadata()?.owner;
        if !old.same_owner(&previous_bundle)? && !old.same_owner(&target)? {
            return Err("原生账号已由其他操作改变，未覆盖登录");
        }
        self.save()?;
        let logged_in =
            observed["owner"] == metadata.owner && observed["mode"] == "chatgptAuthTokens";
        let client = self.desktop.client(&self.db.desktop);
        let platform = self.platform.clone();
        let id = id.to_string();
        self.desktop.message = "正在确认桌面身份并更新全局默认".into();
        self.desktop.action_feedback(&id, "switching");
        if logged_in {
            self.desktop_log("finish", Some(&id), "桌面已登录这个账号，只同步默认文件");
        } else {
            self.desktop_log("apply", Some(&id), "把这个账号登录到桌面并写入默认文件");
        }
        self.desktop.job = Some(std::thread::spawn(move || {
            let result = (|| {
                let mut value = if logged_in {
                    client.call("bind", json!({"owner":metadata.owner}))?
                } else {
                    client.call(
                        "apply",
                        json!({"previousOwner":old_owner,"owner":metadata.owner,"auth":auth}),
                    )?
                };
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
                    return Err("默认文件在切换中被其他程序改动；桌面已是新账号，稍后重新核对");
                }
                if native.read(&platform)? != committed {
                    return Err("默认文件写入没有通过读回核对；稍后重新核对");
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
            self.desktop_log(
                "quit_confirmation",
                None,
                if skip {
                    "热切换开启：登录会话跳过桌面退出确认"
                } else {
                    "热切换关闭：撤回跳过退出确认"
                },
            );
        }
        if self.desktop.job.as_ref().is_some_and(|j| j.is_finished()) {
            // Retry delay starts at completion, so slow/unavailable bridges cannot
            // keep configuration controls perpetually occupied.
            self.desktop.last_poll = now();
            let completion = self.desktop.job.take().unwrap().join();
            match completion {
                Ok(Completion::Preflight { id, result }) => {
                    let (failure, declined) = match result {
                        Err(message) => (Some(message.to_string()), false),
                        Ok(value) if value["ok"] != true => (
                            Some(desktop::refusal(&value)),
                            desktop::declined_for_tasks(&value),
                        ),
                        Ok(value) => (
                            self.commit_desktop(&id, value).err().map(str::to_string),
                            false,
                        ),
                    };
                    if let Some(message) = failure {
                        self.desktop_log(
                            "preflight_failed",
                            Some(&id),
                            if declined {
                                "桌面有任务在运行"
                            } else {
                                "检查未通过"
                            },
                        );
                        if !declined {
                            self.desktop.connected = false;
                            self.desktop.target = None;
                        }
                        // Reported on the attempt itself, with its time; nothing is left standing.
                        self.desktop.message = format!("{message}；切换未开始");
                        self.desktop.action_feedback(&id, "failed");
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
                            self.desktop_log("apply_done", Some(&id), "已切换");
                            if let Ok(i) = self.index(&id) {
                                let a = &mut self.db.accounts[i];
                                a.metadata = serde_json::from_value(value["metadata"].clone()).ok();
                                a.automatic = true;
                                a.next_check = now() + 60;
                                a.message =
                                    "桌面与全局默认已切换；由 AuthManager 维护桌面凭据".into();
                                a.state = "saved".into();
                            }
                            self.desktop.message = "桌面与全局默认一致，实验连接已接管".into();
                            if let Err(e) = self.save() {
                                self.desktop.message = e.into();
                                self.desktop.action_feedback(&id, "failed");
                            } else {
                                self.desktop.action_feedback(&id, "complete");
                            }
                        } else {
                            self.desktop.connected = false;
                            self.desktop.message =
                                "桌面返回的身份不完整；切换没有完成，稍后重新核对".into();
                            self.desktop.action_feedback(&id, "failed");
                        }
                    }
                    // Nothing is held: the next poll reads the desktop again, and a login that
                    // did land is finished from there.
                    Err(message) => {
                        self.desktop_log("apply_failed", Some(&id), message);
                        self.desktop.connected = false;
                        self.desktop.message = format!("{message}；切换没有完成");
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
                        // A switch asked for meanwhile starts below, in this same tick.
                        if self.desktop.pending_switch.is_none()
                            && self.finish_desktop_login(&value)
                        {
                            return;
                        }
                        self.desktop.message = "桌面连接就绪；选择账号后将更新全局默认".into();
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
                                && self.desktop.pending_switch.is_none()
                                && self.job.is_none()
                                && self.pending_login.is_none()
                            {
                                if let Err(e) = self.activate_desktop(&id) {
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
                                self.desktop_log("bind", Some(&id), "恢复桌面续期回调");
                                return;
                            }
                            self.desktop.message = "桌面与全局默认一致，实验连接已接管".into();
                            if self.desktop.refresh.is_none()
                                && self.db.switch.is_none()
                                && let Some(request) =
                                    value["requests"].as_array().and_then(|r| r.first())
                            {
                                self.desktop_log("refresh_requested", Some(&id), "桌面请求续期");
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
                    }
                },
                Ok(Completion::Relaunch { id, result }) => match result {
                    Ok(()) => {
                        // A new desktop instance: the previous page target no longer exists, and
                        // the switch waits until the new one has been polled.
                        self.desktop.target = None;
                        self.desktop.connected = false;
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
                        self.desktop_log("restart_failed", None, message);
                        // Never retry in a loop: the person restarts it once instead.
                        self.desktop.restart_failed = true;
                        self.desktop.message =
                            format!("Codex 桌面未能以调试模式重新启动：{message}");
                    }
                },
                Ok(Completion::Reply(result)) => {
                    if let Err(e) = result {
                        self.desktop.message = e.into();
                    }
                }
                Ok(Completion::Update(result)) => match result {
                    Ok(value) if value["confirmed"] == true => {
                        let _ = self.desktop.observed(&value);
                    }
                    _ => {
                        self.desktop_log("update_unconfirmed", None, "桌面更新尚未确认");
                        self.desktop.connected = false;
                        self.desktop.message = "新凭据已保存，桌面更新尚未确认".into();
                    }
                },
                Err(_) => {
                    self.desktop.connected = false;
                    self.desktop.message = "桌面操作中断；下次轮询会重新核对".into();
                    if let Some(id) = self.desktop.switching().map(String::from) {
                        self.desktop.pending_switch = None;
                        self.desktop.action_feedback(&id, "failed");
                    }
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
            let Ok(bundle) = self
                .index(&refresh.account_id)
                .and_then(|index| self.bundle(&self.db.accounts[index]))
            else {
                self.reply_refresh(None);
                return;
            };
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
            // A renewal is never pushed to the desktop: logging it in again rebuilds its windows.
            // Codex asks only after a 401 and then retries once, so it gets the newer token here,
            // at once and offline, even while another account's check holds the job slot.
            if hash != refresh.hash && self.desktop_owns(&refresh.account_id) {
                self.reply_refresh(desktop::external_auth(&bundle).ok());
                return;
            }
            if self.job.is_none() {
                if refresh.attempted {
                    self.reply_refresh(None);
                    return;
                }
                if let Some(r) = self.desktop.refresh.as_mut() {
                    r.attempted = true;
                }
                self.start_desktop_refresh(&refresh.account_id);
            }
            return;
        }
        if let Some((id, deadline)) = self.desktop.pending_switch.clone() {
            if self.desktop.connected && !self.switch_waits() {
                self.desktop.pending_switch = None;
                if let Err(message) = self.activate_desktop(&id) {
                    self.desktop.message = format!("{message}；切换未开始");
                    self.desktop.action_feedback(&id, "failed");
                }
                return;
            }
            if now() >= deadline {
                self.desktop.pending_switch = None;
                self.desktop.message = if self.desktop.connected {
                    "等其他检查结束超时；切换未开始，可以再试一次".into()
                } else {
                    format!("桌面没有连上：{}；切换未开始", self.desktop.message)
                };
                self.desktop.action_feedback(&id, "failed");
                return;
            }
            // Connected: wait for the work in the way. Otherwise poll until the desktop answers.
            if self.desktop.connected {
                return;
            }
        }
        if self.restart_undebugged_desktop() {
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
    // Codex itself never logs in with external tokens; only AuthManager does. So a desktop on such
    // a login whose account is not the selected one is a switch that stopped part way, or outlived
    // a restart of the service: finish it from what the desktop shows. True while that is handled.
    fn finish_desktop_login(&mut self, observed: &Value) -> bool {
        if self.desktop.mode.as_deref() != Some("chatgptAuthTokens") {
            return false;
        }
        let Some(id) = self
            .db
            .accounts
            .iter()
            .find(|a| {
                a.provider == "codex"
                    && a.metadata.as_ref().map(|m| &m.owner) == self.desktop.owner.as_ref()
            })
            .map(|a| a.id.clone())
        else {
            return false;
        };
        if self.db.desktop.selected.as_deref() == Some(&id) {
            return false;
        }
        // The default file has a writer of its own while a check or login runs; wait for it.
        if now() < self.desktop.finish_after
            || self.job.is_some()
            || self.reset_job.is_some()
            || self.pending_login.is_some()
            || self.db.switch.is_some()
        {
            return true;
        }
        self.desktop.finish_after = now() + 30;
        if let Err(message) = self.commit_desktop(&id, observed.clone()) {
            self.desktop_log("finish_failed", Some(&id), message);
            self.desktop.message = format!("桌面已登录另一个账号，默认文件还没同步：{message}");
        }
        true
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
        self.desktop_log(
            "restart",
            None,
            "桌面没带调试端口，启动完成且没有任务，以调试模式重开",
        );
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
        match result {
            Ok(()) => self.desktop_log("refresh_start", Some(id), "联网续期以回复桌面"),
            Err(e) => {
                self.desktop_log("refresh_unavailable", Some(id), e);
                self.desktop.message = e.into();
                self.reply_refresh(None);
            }
        }
    }
    fn reply_refresh(&mut self, auth: Option<Value>) {
        let Some(r) = self.desktop.refresh.take() else {
            return;
        };
        self.desktop_log(
            "refresh_reply",
            Some(&r.account_id),
            if auth.is_some() {
                "回复新令牌"
            } else {
                "没有回复令牌（超时、账号已变或续期失败）"
            },
        );
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
