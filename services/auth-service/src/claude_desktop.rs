//! Claude 桌面：各账号的本地 Code 会话列表保持一致，以及不退出登录切换桌面账号。
//!
//! 引擎是 `services/claude-desktop`（移植自 cc-same，MIT）。这里只负责在服务里跑它：
//! 后台同步线程跟着引擎 config.json 的 `background`，一屏状态由刷新线程定期读出并缓存，
//! 换号、添加账号、同步、恢复快照这类长操作各在一个工作线程里跑，界面经快照看进度。
//! 界面拿到的账号只有不透明的 handle，从不拿到账号 ID；不含任何凭据。
use claude_desktop::Ctx;
use claude_desktop::api::{
    self, AccountView, Failure, FailureReason, HeadlineKind, LoginState, Overview, PassSummary,
    SwitchProgress, SwitchStage, SwitchTarget,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, &'static str>;

/// How often the window's state is read again: an overview walks every session list.
const REFRESH: Duration = Duration::from_secs(5);
const REFRESH_BUSY: Duration = Duration::from_secs(1);
/// A background sync that stopped on its own is started again after this long.
const WATCH_RETRY: Duration = Duration::from_secs(30);

/// One action from the window (`accounts.claude_desktop`).
pub struct Command<'a> {
    pub action: &'a str,
    pub account: Option<&'a str>,
    pub label: Option<&'a str>,
    pub enabled: Option<bool>,
    pub number: Option<u32>,
    pub snapshot: Option<&'a str>,
    pub confirm: bool,
}

#[derive(Clone)]
struct Operation {
    kind: &'static str,
    target: Option<String>,
    step: &'static str,
    message: Option<String>,
    running: bool,
    ok: Option<bool>,
    /// Adding: the account Claude was signed in to before, so a new sign-in can be told apart.
    from: Option<String>,
}

struct Shared {
    ctx: Ctx,
    overview: Mutex<Option<Overview>>,
    last_pass: Mutex<Option<PassSummary>>,
    operation: Mutex<Option<Operation>>,
    stop: AtomicBool,
}

pub struct Runtime {
    shared: Option<Arc<Shared>>,
    refresher: Option<JoinHandle<()>>,
    next_refresh: Instant,
    watch: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
    watch_retry: Option<Instant>,
    job: Option<JoinHandle<()>>,
}

impl Runtime {
    /// Inert: tests and a service whose engine could not start never touch Claude's files.
    pub fn off() -> Runtime {
        Runtime {
            shared: None,
            refresher: None,
            next_refresh: Instant::now(),
            watch: None,
            watch_retry: None,
            job: None,
        }
    }

    /// The engine's state lives in `<root>/claude-desktop`. A switch cut short by a crash is
    /// finished first, while Claude is closed.
    pub fn start(root: &Path) -> Runtime {
        let dir = root.join("claude-desktop");
        if crate::platform::private_dir(&dir).is_err() {
            return Runtime::off();
        }
        let Ok(ctx) = api::context_in(dir) else {
            return Runtime::off();
        };
        let shared = Arc::new(Shared {
            ctx,
            overview: Mutex::new(None),
            last_pass: Mutex::new(None),
            operation: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        let mut runtime = Runtime {
            shared: Some(shared.clone()),
            ..Runtime::off()
        };
        runtime.job = Some(std::thread::spawn(move || {
            let _ = api::recover(&shared.ctx);
        }));
        runtime
    }

    /// A runtime over the engine `ctx` (tests: temporary folders and a fake Claude).
    #[cfg(test)]
    fn with_ctx(ctx: Ctx) -> Runtime {
        let shared = Arc::new(Shared {
            ctx,
            overview: Mutex::new(None),
            last_pass: Mutex::new(None),
            operation: Mutex::new(None),
            stop: AtomicBool::new(false),
        });
        Runtime {
            shared: Some(shared),
            ..Runtime::off()
        }
    }

    pub fn tick(&mut self) {
        let Some(shared) = self.shared.clone() else {
            return;
        };
        if self.job.as_ref().is_some_and(JoinHandle::is_finished) {
            let _ = self.job.take().map(JoinHandle::join);
            self.next_refresh = Instant::now();
        }
        if self.refresher.as_ref().is_some_and(JoinHandle::is_finished) {
            let _ = self.refresher.take().map(JoinHandle::join);
        }
        self.follow_background(&shared);
        if self.refresher.is_none() && Instant::now() >= self.next_refresh {
            let busy = self.job.is_some();
            self.next_refresh = Instant::now() + if busy { REFRESH_BUSY } else { REFRESH };
            self.refresher = Some(std::thread::spawn(move || refresh(&shared)));
        }
    }

    /// The background sync runs exactly while the engine's setting says so.
    fn follow_background(&mut self, shared: &Arc<Shared>) {
        let wanted = shared.ctx.config().background;
        if let Some((stop, handle)) = &self.watch {
            if handle.is_finished() {
                let stopped = stop.load(Ordering::Acquire);
                let _ = self.watch.take().map(|(_, handle)| handle.join());
                if !stopped {
                    self.watch_retry = Some(Instant::now() + WATCH_RETRY);
                }
            } else if !wanted {
                stop.store(true, Ordering::Release);
            }
            return;
        }
        if !wanted || self.watch_retry.is_some_and(|at| Instant::now() < at) {
            return;
        }
        self.watch_retry = None;
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, worker) = (stop.clone(), shared.clone());
        let handle = std::thread::spawn(move || {
            api::run_watch(&worker.ctx, &flag, |pass| {
                *worker.last_pass.lock().unwrap() = Some(pass.clone());
            });
        });
        self.watch = Some((stop, handle));
    }

    pub fn stop(&mut self) {
        if let Some(shared) = &self.shared {
            shared.stop.store(true, Ordering::Release);
        }
        if let Some((stop, _)) = &self.watch {
            stop.store(true, Ordering::Release);
        }
        for handle in [
            self.job.take(),
            self.refresher.take(),
            self.watch.take().map(|(_, h)| h),
        ]
        .into_iter()
        .flatten()
        {
            let _ = handle.join();
        }
    }

    pub fn command(&mut self, command: Command<'_>) -> Result<()> {
        let shared = self.shared.clone().ok_or("Claude 桌面的会话同步没有启动")?;
        let account = || -> Result<(String, String)> {
            let handle = command.account.ok_or("缺少 Claude 桌面账号")?;
            let overview = shared.overview.lock().unwrap();
            let row = overview
                .as_ref()
                .and_then(|o| {
                    o.accounts
                        .iter()
                        .find(|a| handle_of(&a.account_id) == handle)
                })
                .ok_or("找不到这个 Claude 桌面账号，请稍后再试")?;
            Ok((row.account_id.clone(), title(row)))
        };
        let quick = |result: std::result::Result<(), Failure>| -> Result<()> {
            result.map_err(|failure| failure_text(&failure))?;
            *shared.overview.lock().unwrap() = Some(api::overview(&shared.ctx));
            Ok(())
        };
        match command.action {
            "background" => {
                let on = command.enabled.ok_or("缺少后台同步开关")?;
                quick(api::set_background(&shared.ctx, on).map(drop))?;
                self.watch_retry = None;
                self.follow_background(&shared);
            }
            "alias" => {
                let (id, _) = account()?;
                let alias = command.label.map(str::trim).filter(|a| !a.is_empty());
                quick(api::set_alias(&shared.ctx, &id, alias).map(drop))?;
            }
            "enable" | "disable" => {
                let (id, _) = account()?;
                quick(if command.action == "enable" {
                    api::enable_account(&shared.ctx, &id).map(drop)
                } else {
                    api::disable_account(&shared.ctx, &id).map(drop)
                })?;
            }
            "move" => {
                let (id, _) = account()?;
                let number = command.number.ok_or("缺少编号")?;
                quick(api::move_account(&shared.ctx, &id, number).map(drop))?;
            }
            "remove" => {
                confirmed(command.confirm)?;
                let (id, _) = account()?;
                quick(api::remove_account(&shared.ctx, &id).map(drop))?;
            }
            "retention_keep" => quick(api::keep_transcripts(&shared.ctx).map(drop))?,
            "retention_undo" => quick(api::undo_retention(&shared.ctx).map(drop))?,
            "dismiss" => {
                let mut operation = shared.operation.lock().unwrap();
                if operation.as_ref().is_some_and(|o| !o.running) {
                    *operation = None;
                }
            }
            "stop" => {
                if self.job.is_some() {
                    shared.stop.store(true, Ordering::Release);
                }
            }
            "sync" => self.run(&shared, "sync", None, None, |ctx, _| {
                match api::sync_once(ctx) {
                    Ok(result) => done(
                        if !result.errors.is_empty() {
                            format!(
                                "{} 处没写成；详情在 claude-desktop/engine.log",
                                result.errors.len()
                            )
                        } else if result.applied > 0 {
                            format!("改了 {} 处", result.applied)
                        } else {
                            "没有要改的".into()
                        },
                        result.errors.is_empty(),
                    ),
                    Err(failure) => failed(&failure),
                }
            })?,
            "restore" => {
                confirmed(command.confirm)?;
                let id = command.snapshot.ok_or("缺少快照")?.to_string();
                self.run(
                    &shared,
                    "restore",
                    None,
                    None,
                    move |ctx, _| match api::restore_snapshot(ctx, &id) {
                        Ok(_) => done("已恢复；恢复前那一刻也拍了一张".into(), true),
                        Err(failure) => failed(&failure),
                    },
                )?
            }
            "fix_symlinks" => {
                confirmed(command.confirm)?;
                self.run(
                    &shared,
                    "fix",
                    None,
                    None,
                    |ctx, _| match api::fix_symlinks(ctx) {
                        Ok(fixed) => done(
                            format!("{} 个文件夹改回了真文件夹", fixed.folders.len()),
                            true,
                        ),
                        Err(failure) => failed(&failure),
                    },
                )?
            }
            "switch" => {
                confirmed(command.confirm)?;
                let (id, target) = account()?;
                let target_title = target.clone();
                self.run(&shared, "switch", Some(target), None, move |ctx, shared| {
                    let end = api::switch(
                        ctx,
                        &SwitchTarget::Account { query: id },
                        |progress| progressed(shared, &progress, &target_title),
                        &shared.stop,
                    );
                    finished(&end, &target_title)
                })?
            }
            "add" => {
                confirmed(command.confirm)?;
                let from = shared
                    .overview
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|o| o.desktop.signed_in.clone());
                self.run(&shared, "add", None, from, |ctx, shared| {
                    let end = api::add(
                        ctx,
                        |progress| progressed(shared, &progress, ""),
                        &shared.stop,
                    );
                    finished(&end, "")
                })?
            }
            _ => return Err("不支持的 Claude 桌面操作"),
        }
        Ok(())
    }

    /// One long operation at a time; its last word stays until the window dismisses it.
    fn run(
        &mut self,
        shared: &Arc<Shared>,
        kind: &'static str,
        target: Option<String>,
        from: Option<String>,
        work: impl FnOnce(&Ctx, &Shared) -> (&'static str, Option<String>, Option<bool>)
        + Send
        + 'static,
    ) -> Result<()> {
        if self.job.is_some() {
            return Err("上一项 Claude 桌面操作还没完成");
        }
        shared.stop.store(false, Ordering::Release);
        let first = if kind == "switch" || kind == "add" {
            "quitting"
        } else {
            "working"
        };
        *shared.operation.lock().unwrap() = Some(Operation {
            kind,
            target,
            step: first,
            message: None,
            running: true,
            ok: None,
            from,
        });
        let worker = shared.clone();
        self.job = Some(std::thread::spawn(move || {
            let (step, message, ok) = work(&worker.ctx, &worker);
            if let Some(operation) = worker.operation.lock().unwrap().as_mut() {
                operation.step = step;
                operation.message = message;
                operation.ok = ok;
                operation.running = false;
            }
            refresh(&worker);
        }));
        self.next_refresh = Instant::now();
        Ok(())
    }

    /// What the window shows. `linked` names the AuthBaton Claude Code account signed in as the same
    /// person, if there is one.
    pub fn snapshot(&self, linked: impl Fn(&str) -> Option<String>) -> Value {
        let Some(shared) = &self.shared else {
            return Value::Null;
        };
        let overview = shared.overview.lock().unwrap();
        let Some(ov) = overview.as_ref() else {
            return Value::Null;
        };
        let operation = shared.operation.lock().unwrap().clone();
        let last_pass = shared.last_pass.lock().unwrap().clone();
        let known = |a: &&AccountView| a.in_list;
        let current = ov.accounts.iter().find(|a| a.login == LoginState::SignedIn);
        let sessions = ov
            .accounts
            .iter()
            .filter(known)
            .map(|a| a.sessions)
            .max()
            .unwrap_or(0);
        let syncing = ov.sync.syncing
            || operation
                .as_ref()
                .is_some_and(|o| o.kind == "sync" && o.running);
        let restart_needed = ov.sync.restart_needed.is_some();
        let broken = ov.accounts.iter().any(|a| a.broken);
        let failed = operation.as_ref().is_some_and(|o| o.ok == Some(false));
        let attention = if failed {
            Some("上次操作没成功")
        } else if restart_needed {
            Some("要重启一次 Claude")
        } else if !ov.symlinked_folders.is_empty() || broken {
            Some("有会话文件夹 Claude 存不进去")
        } else {
            None
        };
        let mut last_sync = ov.sync.last_sync.as_ref().map(|l| {
            let message = if l.errors > 0 {
                format!("{} 处没写成；详情在 claude-desktop/engine.log", l.errors)
            } else if l.applied > 0 {
                format!("改了 {} 处", l.applied)
            } else {
                "没有要改的".into()
            };
            json!({"at":l.at,"ok":l.errors==0,"message":message})
        });
        if let Some(pass) = last_pass.filter(|p| !p.ok)
            && last_sync
                .as_ref()
                .is_none_or(|l| l["at"].as_f64().unwrap_or(0.0) < pass.at as f64)
        {
            last_sync = Some(
                json!({"at":pass.at,"ok":false,"message":"后台同步这一轮没做成；详情在 claude-desktop/engine.log"}),
            );
        }
        let accounts: Vec<Value> = ov
            .accounts
            .iter()
            .map(|a| {
                let usage = a.usage.as_ref();
                let window = |used: Option<u32>| {
                    used.zip(usage).map(|(used, u)| json!({"remaining":100u32.saturating_sub(used),"at":u.at,"stale":u.stale}))
                };
                json!({
                    "handle": handle_of(&a.account_id),
                    "number": a.number,
                    "display": display(a),
                    "email": a.email.as_deref().map(crate::credentials::masked),
                    "alias": a.alias,
                    "in_use": a.login == LoginState::SignedIn,
                    "login_saved": a.login != LoginState::Unknown,
                    "participates": a.in_rotation,
                    "sessions": a.sessions,
                    "behind": a.missing,
                    "five_hour": window(usage.and_then(|u| u.five_hour)),
                    "weekly": window(usage.and_then(|u| u.weekly)),
                    "linked_account": linked(&a.account_id),
                    "known": a.in_list,
                })
            })
            .collect();
        let snapshots: Vec<Value> = ov
            .snapshots
            .iter()
            .rev()
            .map(|s| {
                let kind = if s.baseline {
                    "baseline"
                } else if s.reason == "pre-restore" {
                    "pre-restore"
                } else {
                    "hourly"
                };
                json!({"id":s.id,"at":s.created,"kind":kind,"folders":s.folders})
            })
            .collect();
        let pick = |id: &Option<String>| id.as_deref().map(handle_of);
        let retention = &ov.retention;
        json!({
            "installed": ov.desktop.installed || ov.desktop.data_found,
            "running": ov.desktop.running,
            "version": ov.desktop.version,
            "switch_supported": ov.desktop.can_switch,
            "background": ov.sync.background,
            "syncing": syncing,
            "headline": headline(ov, current),
            "attention": attention,
            "sessions": sessions,
            "pending": ov.sync.pending.changes + ov.sync.pending.deferred,
            "restart_needed": restart_needed,
            "last_sync": last_sync,
            "accounts": accounts,
            "snapshots": snapshots,
            "retention": {
                "at_risk": retention.short,
                "days": retention.desktop_days.map(|d| d.round() as i64),
                "kept": retention.can_undo,
                "can_undo": retention.can_undo,
                "can_keep": retention.can_keep,
            },
            "symlinks": ov.symlinked_folders.len(),
            "operation": operation.map(|o| json!({"kind":o.kind,"target":o.target,"step":o.step,"message":o.message,"running":o.running,"ok":o.ok})),
            "picks": {"next": pick(&ov.picks.next), "next_available": pick(&ov.picks.next_available), "best": pick(&ov.picks.best)},
        })
    }
}

/// Read the window's state again; an add finishes once Claude is signed in to someone new.
fn refresh(shared: &Shared) {
    let overview = api::overview(&shared.ctx);
    if let Some(operation) = shared.operation.lock().unwrap().as_mut()
        && operation.kind == "add"
        && operation.step == "waiting_sign_in"
        && let Some(signed_in) = overview.desktop.signed_in.as_ref()
        && operation.from.as_ref() != Some(signed_in)
        && let Some(row) = overview
            .accounts
            .iter()
            .find(|a| &a.account_id == signed_in)
    {
        operation.step = "done";
        operation.ok = Some(true);
        operation.message = Some(match row.number {
            Some(number) => format!("看到 {} 登录了，已加到第 {number} 位", display(row)),
            None => format!("看到 {} 登录了", display(row)),
        });
    }
    *shared.overview.lock().unwrap() = Some(overview);
}

fn progressed(shared: &Shared, progress: &SwitchProgress, target: &str) {
    let step = step_of(progress);
    if let Some(operation) = shared.operation.lock().unwrap().as_mut() {
        operation.step = step;
        if !target.is_empty() {
            operation.target = Some(target.to_string());
        }
    }
}

fn step_of(progress: &SwitchProgress) -> &'static str {
    match progress.stage {
        SwitchStage::Starting | SwitchStage::WaitingForQuit => "quitting",
        SwitchStage::Confirming => "asking",
        SwitchStage::AfterWork => "after_work",
        SwitchStage::WaitingForUpdater => "updating",
        SwitchStage::MovingSignIn => "moving",
        SwitchStage::Launching => "launching",
        SwitchStage::Done if progress.adding => "waiting_sign_in",
        SwitchStage::Done => "done",
        SwitchStage::Failed => "failed",
        SwitchStage::Stopped => "stopped",
    }
}

fn finished(end: &SwitchProgress, target: &str) -> (&'static str, Option<String>, Option<bool>) {
    let message = match end.stage {
        SwitchStage::Done if end.adding => {
            "在 Claude 的登录页登录新账号；登录后会自动加进列表".to_string()
        }
        SwitchStage::Done if end.already => "Claude 已经登录着这个账号，什么都没改".to_string(),
        SwitchStage::Done if end.launched => format!("已换上 {target}，Claude 已重新打开"),
        SwitchStage::Done => format!("已换上 {target}；Claude 下次打开就是这个账号"),
        SwitchStage::Stopped if end.after_work => {
            "Claude 会在手上的事做完后自己退出；这次没换号".to_string()
        }
        SwitchStage::Stopped => "已停止等待，什么都没改".to_string(),
        _ => end
            .failure
            .as_ref()
            .map_or("没换成，什么都没改".into(), |f| {
                failure_text(f).to_string()
            }),
    };
    let ok = match end.stage {
        SwitchStage::Failed => Some(false),
        SwitchStage::Done if end.adding => None,
        _ => Some(true),
    };
    (step_of(end), Some(message), ok)
}

fn done(message: String, ok: bool) -> (&'static str, Option<String>, Option<bool>) {
    (if ok { "done" } else { "failed" }, Some(message), Some(ok))
}

fn failed(failure: &Failure) -> (&'static str, Option<String>, Option<bool>) {
    (
        "failed",
        Some(failure_text(failure).to_string()),
        Some(false),
    )
}

fn confirmed(confirm: bool) -> Result<()> {
    if confirm {
        Ok(())
    } else {
        Err("需要确认后才能执行")
    }
}

fn failure_text(failure: &Failure) -> &'static str {
    match failure.reason {
        FailureReason::ClaudeRunning => "先退出 Claude（⌘Q），再试一次",
        FailureReason::Busy => "正在同步或换号，稍后再试",
        FailureReason::NotFound | FailureReason::Ambiguous => "找不到这个 Claude 桌面账号",
        FailureReason::NotInList | FailureReason::NothingSaved => {
            "这个账号没保存登录；在 Claude 里登录它一次就会保存"
        }
        FailureReason::NoOther => "没有别的保存了登录、参与「下一个」的账号",
        FailureReason::Unsupported => "这台 Mac 上不能切换 Claude 桌面账号",
        FailureReason::StillOpen => "Claude 没有退出，什么都没改",
        FailureReason::Updating => "Claude 正在更新，过一会儿再试",
        FailureReason::Stopped => "已停止等待，什么都没改",
        FailureReason::SignedIn => "Claude 正登录着这个账号，不能移除",
        FailureReason::BadAlias => "名字只能用小写字母、数字和 - _ .，最多 24 个字，不能全是数字",
        FailureReason::AliasTaken => "别的账号已经叫这个名字了",
        FailureReason::BadNumber => "编号从 1 开始",
        FailureReason::Organization => "你的组织设定了对话原文的保留期，这里改不了",
        FailureReason::NoSuchSnapshot => "找不到这张快照",
        FailureReason::Error => "没做成；详情在 claude-desktop/engine.log",
    }
}

/// The window names an account by this; the account ID itself stays in the service.
fn handle_of(account_id: &str) -> String {
    format!("{:x}", Sha256::digest(account_id.as_bytes()))[..12].to_string()
}

fn display(account: &AccountView) -> String {
    if let Some(alias) = &account.alias {
        return alias.clone();
    }
    match (&account.email, account.number) {
        (Some(email), _) => crate::credentials::masked(email),
        (None, Some(number)) => format!("账号 {number}"),
        (None, None) => "没认出的账号".into(),
    }
}

fn title(account: &AccountView) -> String {
    match account.number {
        Some(number) => format!("#{number} {}", display(account)),
        None => display(account),
    }
}

fn headline(ov: &Overview, current: Option<&AccountView>) -> String {
    let h = &ov.sync.headline;
    let state = match h.kind {
        HeadlineKind::Missing => "这台 Mac 上没找到 Claude 桌面的数据".to_string(),
        HeadlineKind::Single => "只有一个账号的会话列表，暂时不用同步".to_string(),
        HeadlineKind::Linked => format!("有 {} 个会话文件夹是软链，Claude 存不进去", h.folders),
        HeadlineKind::Syncing if h.sessions > 0 => format!("正在同步 {} 个会话", h.sessions),
        HeadlineKind::Syncing => "正在同步".to_string(),
        HeadlineKind::Pending if h.sessions > 0 => format!("{} 个会话等着同步", h.sessions),
        HeadlineKind::Pending => format!("{} 处改动等着同步", h.other),
        HeadlineKind::Waiting => format!("{} 处改动等 Claude 换号或退出后补上", h.updates),
        HeadlineKind::InSync => format!("{} 个账号的会话都一样", h.accounts),
    };
    match current {
        Some(account) => format!("{state} · 在用 {}", title(account)),
        None => state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_desktop::{Config, FakeDesktop, LogSink, Paths};
    use std::fs;
    use std::path::PathBuf;

    const ADA: (&str, &str) = (
        "aaaaaaaa-0000-4000-8000-000000000001",
        "0a0a0a0a-0000-4000-8000-000000000001",
    );
    const BOB: (&str, &str) = (
        "bbbbbbbb-0000-4000-8000-000000000002",
        "0b0b0b0b-0000-4000-8000-000000000002",
    );

    struct Fixture {
        root: PathBuf,
        runtime: Runtime,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.runtime.stop();
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    impl Fixture {
        /// Claude is closed and signed in to Ada, who has two sessions; Bob's list is empty.
        fn new(name: &str) -> Fixture {
            let root = std::env::temp_dir().join(format!(
                "authmanager-claude-desktop-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            let user_data = root.join("Claude");
            for (account, org) in [ADA, BOB] {
                fs::create_dir_all(
                    user_data
                        .join("claude-code-sessions")
                        .join(account)
                        .join(org),
                )
                .unwrap();
            }
            fs::create_dir_all(root.join("projects/-tmp-proj")).unwrap();
            fs::create_dir_all(root.join("logs")).unwrap();
            fs::write(
                user_data.join("config.json"),
                format!(r#"{{"lastKnownAccountUuid":"{}"}}"#, ADA.0),
            )
            .unwrap();
            for n in 1..=2 {
                let id = format!("1111111{n}-0000-4000-8000-00000000000{n}");
                let record = json!({"sessionId": format!("local_{id}"), "cliSessionId": format!("cli-{n}"), "cwd": "/tmp/proj",
                    "originCwd": "/tmp/proj", "title": format!("session {n}"), "isArchived": false, "createdAt": 1,
                    "lastActivityAt": 1_790_000_000_000i64 + n, "permissionMode": "default"});
                let dir = user_data
                    .join("claude-code-sessions")
                    .join(ADA.0)
                    .join(ADA.1);
                fs::write(dir.join(format!("local_{id}.json")), record.to_string()).unwrap();
            }
            let mut paths = Paths::new(user_data, root.join("state"));
            paths.desktop_logs = root.join("logs");
            paths.projects = vec![root.join("projects")];
            paths.claude_settings = root.join("claude-settings.json");
            paths.claude_json = root.join("claude.json");
            paths.managed_settings = root.join("managed");
            let config = Config::load(&paths).unwrap();
            let ctx = Ctx::new(
                paths,
                config,
                FakeDesktop {
                    running: Some(false),
                    active: None,
                },
                LogSink::Silent,
            );
            Fixture {
                root,
                runtime: Runtime::with_ctx(ctx),
            }
        }

        /// The window's state once the service has read it, after any operation has finished.
        fn settle(&mut self) -> Value {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                self.runtime.tick();
                let snapshot = self.runtime.snapshot(|_| None);
                let idle = self.runtime.job.is_none() && self.runtime.refresher.is_none();
                if idle && !snapshot.is_null() && snapshot["operation"]["running"] != true {
                    return snapshot;
                }
                assert!(
                    Instant::now() < deadline,
                    "the Claude desktop runtime did not settle"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn handle(snapshot: &Value, account: &str) -> String {
            let handle = handle_of(account);
            assert!(
                snapshot["accounts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|a| a["handle"] == handle.as_str())
            );
            handle
        }

        fn command(&mut self, action: &str, account: Option<&str>, confirm: bool) -> Result<()> {
            self.runtime.command(Command {
                action,
                account,
                label: None,
                enabled: None,
                number: None,
                snapshot: None,
                confirm,
            })
        }
    }

    #[test]
    fn the_window_gets_handles_and_masked_names_never_account_ids() {
        let mut fixture = Fixture::new("view");
        let snapshot = fixture.settle();
        let text = snapshot.to_string();
        assert!(
            !text.contains(ADA.0) && !text.contains(BOB.0),
            "account IDs reached the window: {text}"
        );
        let accounts = snapshot["accounts"].as_array().unwrap();
        let ada = accounts
            .iter()
            .find(|a| a["handle"] == handle_of(ADA.0).as_str())
            .unwrap();
        assert_eq!(ada["in_use"], true);
        assert_eq!(ada["known"], true);
        assert_eq!(ada["number"], 1);
        assert_eq!(ada["sessions"], 2);
        assert_eq!(handle_of(ADA.0).len(), 12);
        assert_eq!(snapshot["installed"], true);
        assert_eq!(snapshot["background"], false);
        assert_eq!(snapshot["sessions"], 2);
        assert!(
            snapshot["headline"]
                .as_str()
                .unwrap()
                .ends_with("在用 #1 账号 1"),
            "{}",
            snapshot["headline"]
        );
        // Bob only has a session folder: shown apart, behind by Ada's two sessions.
        let bob = accounts
            .iter()
            .find(|a| a["handle"] == handle_of(BOB.0).as_str())
            .unwrap();
        assert_eq!(bob["known"], false);
        assert_eq!(bob["behind"], 2);
        assert!(Runtime::off().snapshot(|_| None).is_null());
        assert!(
            Runtime::off()
                .command(Command {
                    action: "sync",
                    account: None,
                    label: None,
                    enabled: None,
                    number: None,
                    snapshot: None,
                    confirm: false
                })
                .is_err()
        );
    }

    #[test]
    fn an_alias_renames_the_row_and_a_bad_one_is_refused_in_chinese() {
        let mut fixture = Fixture::new("alias");
        let snapshot = fixture.settle();
        let ada = Fixture::handle(&snapshot, ADA.0);
        fixture
            .runtime
            .command(Command {
                action: "alias",
                account: Some(&ada),
                label: Some("work"),
                enabled: None,
                number: None,
                snapshot: None,
                confirm: false,
            })
            .unwrap();
        let snapshot = fixture.runtime.snapshot(|_| None);
        let row = snapshot["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["handle"] == ada.as_str())
            .unwrap()
            .clone();
        assert_eq!(row["display"], "work");
        assert!(
            snapshot["headline"]
                .as_str()
                .unwrap()
                .ends_with("在用 #1 work")
        );
        let refused = fixture.runtime.command(Command {
            action: "alias",
            account: Some(&ada),
            label: Some("Not Valid!"),
            enabled: None,
            number: None,
            snapshot: None,
            confirm: false,
        });
        assert_eq!(
            refused,
            Err("名字只能用小写字母、数字和 - _ .，最多 24 个字，不能全是数字")
        );
        assert_eq!(
            fixture.command("alias", Some("000000000000"), false),
            Err("找不到这个 Claude 桌面账号，请稍后再试")
        );
    }

    #[test]
    fn a_sync_copies_the_sessions_and_reports_what_it_did() {
        let mut fixture = Fixture::new("sync");
        fixture.settle();
        fixture.command("sync", None, false).unwrap();
        assert_eq!(
            fixture.command("sync", None, false),
            Err("上一项 Claude 桌面操作还没完成")
        );
        let snapshot = fixture.settle();
        assert_eq!(snapshot["operation"]["kind"], "sync");
        assert_eq!(
            snapshot["operation"]["ok"], true,
            "{}",
            snapshot["operation"]
        );
        let bob = fixture
            .root
            .join("Claude/claude-code-sessions")
            .join(BOB.0)
            .join(BOB.1);
        let copied = fs::read_dir(&bob)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("local_"))
            .count();
        assert_eq!(copied, 2);
        assert!(
            snapshot["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["behind"] == 0)
        );
        assert_eq!(snapshot["last_sync"]["ok"], true);
        assert!(
            snapshot["snapshots"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["kind"] == "baseline")
        );
        fixture.command("dismiss", None, false).unwrap();
        assert!(fixture.runtime.snapshot(|_| None)["operation"].is_null());
    }

    #[test]
    fn a_switch_needs_confirming_and_a_failure_stays_until_dismissed() {
        let mut fixture = Fixture::new("switch");
        let snapshot = fixture.settle();
        let bob = Fixture::handle(&snapshot, BOB.0);
        assert_eq!(
            fixture.command("switch", Some(&bob), false),
            Err("需要确认后才能执行")
        );
        // Bob is only a session folder: no sign-in is kept for him, so the switch fails in its job.
        fixture.command("switch", Some(&bob), true).unwrap();
        let snapshot = fixture.settle();
        assert_eq!(
            snapshot["operation"]["ok"], false,
            "{}",
            snapshot["operation"]
        );
        assert_eq!(snapshot["attention"], "上次操作没成功");
        assert!(
            snapshot["operation"]["message"]
                .as_str()
                .unwrap()
                .contains("没保存登录"),
            "{}",
            snapshot["operation"]
        );
        fixture.command("dismiss", None, false).unwrap();
        assert!(fixture.runtime.snapshot(|_| None)["attention"].is_null());
    }

    #[test]
    fn the_background_sync_runs_exactly_while_it_is_switched_on() {
        let mut fixture = Fixture::new("background");
        fixture.settle();
        fixture
            .runtime
            .command(Command {
                action: "background",
                account: None,
                label: None,
                enabled: Some(true),
                number: None,
                snapshot: None,
                confirm: false,
            })
            .unwrap();
        assert!(fixture.runtime.watch.is_some());
        assert_eq!(fixture.runtime.snapshot(|_| None)["background"], true);
        let config: Value =
            serde_json::from_slice(&fs::read(fixture.root.join("state/config.json")).unwrap())
                .unwrap();
        assert_eq!(config["background"], true);
        fixture
            .runtime
            .command(Command {
                action: "background",
                account: None,
                label: None,
                enabled: Some(false),
                number: None,
                snapshot: None,
                confirm: false,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while fixture.runtime.watch.is_some() {
            fixture.runtime.tick();
            assert!(
                Instant::now() < deadline,
                "the background sync kept running"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fixture.runtime.snapshot(|_| None)["background"], false);
    }

    /// Reads this Mac's real Claude desktop data with the engine's state in a temporary folder.
    /// Read only: no background sync, no sync, no switch. `cargo test -- --ignored live_`.
    #[test]
    #[ignore]
    fn live_overview_reads_this_macs_claude_desktop() {
        let state = std::env::temp_dir().join(format!(
            "authmanager-claude-desktop-live-{}",
            std::process::id()
        ));
        let ctx = api::context_in(state.clone()).unwrap();
        assert!(!ctx.config().background);
        let mut runtime = Runtime::with_ctx(ctx);
        let deadline = Instant::now() + Duration::from_secs(30);
        let snapshot = loop {
            runtime.tick();
            let snapshot = runtime.snapshot(|_| None);
            if !snapshot.is_null() && runtime.refresher.is_none() {
                break snapshot;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(50));
        };
        runtime.stop();
        let _ = fs::remove_dir_all(&state);
        let accounts = snapshot["accounts"].as_array().unwrap();
        let ids: Vec<&str> = accounts
            .iter()
            .map(|a| a["handle"].as_str().unwrap())
            .collect();
        println!(
            "installed={} running={} version={} switch_supported={} sessions={} pending={} restart_needed={} symlinks={} snapshots={}",
            snapshot["installed"],
            snapshot["running"],
            snapshot["version"],
            snapshot["switch_supported"],
            snapshot["sessions"],
            snapshot["pending"],
            snapshot["restart_needed"],
            snapshot["symlinks"],
            snapshot["snapshots"].as_array().unwrap().len()
        );
        println!(
            "headline={} retention={}",
            snapshot["headline"], snapshot["retention"]
        );
        for a in accounts {
            println!(
                "account number={} display={} in_use={} known={} login_saved={} sessions={} behind={} five_hour={} weekly={}",
                a["number"],
                a["display"],
                a["in_use"],
                a["known"],
                a["login_saved"],
                a["sessions"],
                a["behind"],
                a["five_hour"],
                a["weekly"]
            );
        }
        assert_eq!(ids.len(), accounts.len());
        assert!(
            accounts
                .iter()
                .all(|a| a["handle"].as_str().unwrap().len() == 12)
        );
        // The account Claude is signed in to never reaches the window by its ID.
        let config = claude_desktop::paths::default_user_data().join("config.json");
        let config: Value =
            serde_json::from_slice(&fs::read(config).unwrap_or_default()).unwrap_or_default();
        if let Some(id) = config["lastKnownAccountUuid"].as_str() {
            assert!(
                !snapshot.to_string().contains(id),
                "the signed-in account's ID reached the window"
            );
        }
    }

    #[test]
    fn switch_messages_say_what_happened() {
        let progress = |stage, adding, already, launched| SwitchProgress {
            stage,
            adding,
            target: None,
            can_stop: false,
            from: None,
            to: None,
            launched,
            already,
            failure: None,
            after_work: false,
        };
        assert_eq!(
            finished(&progress(SwitchStage::Done, false, false, true), "#2 work")
                .1
                .unwrap(),
            "已换上 #2 work，Claude 已重新打开"
        );
        assert_eq!(
            finished(&progress(SwitchStage::Done, false, true, false), "#2 work")
                .1
                .unwrap(),
            "Claude 已经登录着这个账号，什么都没改"
        );
        let added = finished(&progress(SwitchStage::Done, true, false, true), "");
        assert_eq!((added.0, added.2), ("waiting_sign_in", None));
        assert_eq!(
            step_of(&progress(SwitchStage::Confirming, false, false, false)),
            "asking"
        );
        assert_eq!(
            finished(&progress(SwitchStage::Stopped, false, false, false), "").2,
            Some(true)
        );
    }
}
