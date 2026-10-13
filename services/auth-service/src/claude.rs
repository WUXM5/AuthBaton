//! Coordinated renewal of the existing native login; never invokes auth login.
use crate::claude_locks::{Lease, RefreshLocks};
use crate::credentials::{Bundle, ClaudeRecoveryState, NativeStore};
use crate::maintenance::{Outcome, Work, merge_refresh};
use crate::platform::{Platform, Result};
use crate::status::now;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

pub(crate) const UNCERTAIN: &str = "续期请求没有收到回应；稍后自动用同一凭据重新确认，无需操作";
pub fn journal_id(id: &str) -> String {
    format!("{id}-claude-refresh")
}

/// Every official lock a running Claude client takes before touching the default login.
/// Clients re-check the store before each new request (Keychain reads are cached for 30 s), so
/// switching under these locks needs no restart: no sibling refresh or config save interleaves.
pub struct SwitchLocks {
    refresh: RefreshLocks,
    storage: Lease,
    config: Lease,
}
impl SwitchLocks {
    pub fn acquire(native: &NativeStore) -> Result<Self> {
        let refresh = RefreshLocks::acquire(&native.directory)?;
        let storage = Lease::storage(&native.directory)?;
        let config = Lease::config(&native.config)?;
        Ok(Self {
            refresh,
            storage,
            config,
        })
    }
    pub fn check(&self) -> Result<()> {
        self.refresh.check()?;
        self.storage.check()?;
        self.config.check()
    }
}

struct Coordinator<'a> {
    native: &'a NativeStore,
    platform: &'a Platform,
    id: &'a str,
    owner: &'a Bundle,
    locks: RefreshLocks,
}
impl<'a> Coordinator<'a> {
    fn new(
        native: &'a NativeStore,
        platform: &'a Platform,
        id: &'a str,
        owner: &'a Bundle,
    ) -> Result<Self> {
        Ok(Self {
            native,
            platform,
            id,
            owner,
            locks: RefreshLocks::acquire(&native.directory)?,
        })
    }
    fn read(&self) -> Result<Bundle> {
        self.locks.check()?;
        let bundle = self.native.read(self.platform)?;
        if !self.owner.same_owner(&bundle)? {
            return Err("原生登录身份已变化，已停止协同维护");
        }
        self.locks.check()?;
        Ok(bundle)
    }
    fn record(&self, expected: &Bundle, next: Option<&Bundle>) -> Result<()> {
        self.platform.vault_write(
            &journal_id(self.id),
            &json!({"expected":expected.encoded(),"next":next.map(Bundle::encoded)}),
        )
    }
    fn clear(&self) -> Result<()> {
        self.platform
            .delete_key("io.github.wuxm5.authbaton.vault", &journal_id(self.id))
    }
    fn commit(&self, expected: &Bundle, next: &Bundle) -> Result<Bundle> {
        let storage = Lease::storage(&self.native.directory)?;
        let mut current = self.read()?;
        if current.credential == expected.credential {
            // Preserve current identity/config metadata; only renew this OAuth credential.
            let target = Bundle {
                credential: next.credential.clone(),
                ..current.clone()
            };
            storage.check()?;
            self.locks.check()?;
            self.native
                .refresh_guarded(self.platform, &current, &target, &|| {
                    storage.check()?;
                    self.locks.check()
                })?;
            storage.check()?;
            current = self.read()?;
            if current != target {
                return Err("刷新凭据尚未通过存储校验");
            }
        }
        // An official sibling may have committed first; the native store wins.
        Ok(current)
    }
    fn sync(&self) -> Result<Bundle> {
        let storage = Lease::storage(&self.native.directory)?;
        let current = self.read()?;
        self.platform.vault_write(self.id, &current.encoded())?;
        storage.check()?;
        self.locks.check()?;
        Ok(current)
    }
    fn recover(
        &self,
        hint: Option<&(Bundle, Bundle)>,
        allow_cleared: bool,
    ) -> Result<Option<(Bundle, bool)>> {
        let Some(record) = self.platform.vault_read_optional(&journal_id(self.id))? else {
            return Ok(None);
        };
        let expected = Bundle::decode(record["expected"].clone())?;
        if !self.owner.same_owner(&expected)? {
            return Err("待恢复续期属于其他身份，未修改原生登录");
        }
        let mut next = if record["next"].is_null() {
            None
        } else {
            Some(Bundle::decode(record["next"].clone())?)
        };
        if let Some((old, rotated)) = hint
            && *old == expected
            && next.is_none()
        {
            self.record(old, Some(rotated))?;
            next = Some(rotated.clone());
        }
        if let Some(next) = &next
            && !expected.same_owner(next)?
        {
            return Err("待恢复凭据身份不一致");
        }
        let current = {
            let storage = Lease::storage(&self.native.directory)?;
            self.locks.check()?;
            let current = match self.native.claude_recovery_state(self.platform)? {
                ClaudeRecoveryState::Authenticated(bundle) => {
                    if !self.owner.same_owner(&bundle)? {
                        return Err("原生登录身份已变化，已停止恢复");
                    }
                    bundle
                }
                ClaudeRecoveryState::Cleared { .. } if allow_cleared => {
                    self.native.restore_cleared_claude(
                        self.platform,
                        &expected,
                        next.as_ref().ok_or(UNCERTAIN)?,
                        &|| {
                            storage.check()?;
                            self.locks.check()
                        },
                    )?
                }
                ClaudeRecoveryState::Cleared { .. } => {
                    return Err(
                        "Claude 已清空失效凭据；请手动恢复已保存的续期结果，不会自动恢复登录",
                    );
                }
            };
            storage.check()?;
            current
        };
        let current = if current.credential == expected.credential {
            self.commit(&expected, next.as_ref().ok_or(UNCERTAIN)?)?
        } else {
            current
        };
        let refreshed = next
            .as_ref()
            .is_some_and(|n| n.credential == current.credential);
        let current = self.sync()?;
        self.clear()?;
        Ok(Some((current, refreshed)))
    }
}

/// Recovery is deliberately HTTP-free, including after a service restart.
pub fn retry(
    native: &NativeStore,
    platform: &Platform,
    id: &str,
    owner: &Bundle,
    hint: Option<&(Bundle, Bundle)>,
) -> Result<(Bundle, bool)> {
    let coordinator = Coordinator::new(native, platform, id, owner)?;
    match coordinator.recover(hint, true)? {
        Some(result) => Ok(result),
        None => Ok((coordinator.sync()?, false)),
    }
}

pub fn prepare(work: &Work, force_refresh: bool) -> Outcome {
    let mut outcome = Outcome::failed(work.bundle.clone(), "Claude 协同维护未完成");
    let result = (|| -> Result<()> {
        if work.cancel.load(Ordering::Acquire) {
            return Err("检查已停止");
        }
        let coordinator = Coordinator::new(
            work.native.as_ref().unwrap(),
            &work.platform,
            &work.id,
            &work.bundle,
        )?;
        // A persisted intent is checked before any new grant, even after restart.
        if coordinator
            .platform
            .vault_read_optional(&journal_id(&work.id))?
            .is_some()
        {
            outcome.state = "unsaved".into();
            match coordinator.recover(None, false) {
                Ok(Some((bundle, refreshed))) => {
                    outcome.bundle = bundle;
                    outcome.refreshed = refreshed;
                    outcome.state = "ready".into();
                    return Ok(());
                }
                Ok(None) => {}
                // The lost response's credential is unrecoverable by anyone, and the native store
                // still holds the one that was sent. Re-granting it can only succeed or be
                // definitively rejected; it cannot invalidate a credential anyone holds.
                Err(UNCERTAIN) => coordinator.clear()?,
                Err(message) => return Err(message),
            }
        }
        outcome.state = "backup_pending".into();
        let current = coordinator.sync()?;
        outcome.bundle = current.clone();
        outcome.state = "retry".into();
        let metadata = current.metadata()?;
        // A 401 may refer to an older access token; adopt a sibling's latest token first.
        let force_refresh = force_refresh
            && current.credential["accessToken"] == work.bundle.credential["accessToken"];
        if !force_refresh && metadata.expires_at.is_some_and(|t| t > now() + 600) {
            outcome.state = "ready".into();
            return Ok(());
        }
        if metadata.refresh_expires_at.is_some_and(|t| t <= now()) {
            outcome.state = "reauth".into();
            return Err("刷新凭据已到期，请重新登录");
        }
        {
            let storage = Lease::storage(&coordinator.native.directory)?;
            coordinator
                .native
                .preflight_claude_refresh(coordinator.platform)?;
            storage.check()?;
            coordinator.locks.check()?;
        }
        let credential = &current.credential;
        let scopes = credential["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        let body = json!({"grant_type":"refresh_token","refresh_token":credential["refreshToken"],"client_id":credential["clientId"].as_str().unwrap_or("9d1c250a-e61b-44d9-88ed-5944d1962f5e"),"scope":scopes});
        coordinator.record(&current, None)?;
        // An aborted attempt remains explicitly recoverable; never silently regrant.
        outcome.state = "uncertain".into();
        coordinator.locks.check()?;
        if work.cancel.load(Ordering::Acquire) {
            coordinator.clear()?;
            outcome.state = "retry".into();
            return Err("检查已停止");
        }
        let response = (work.transport)(
            "https://platform.claude.com/v1/oauth/token",
            None,
            Some(&body),
            false,
        );
        let next = match response {
            Ok((200, value)) => merge_refresh(&current, &value, now())?,
            Ok((status, value)) if (400..500).contains(&status) => {
                coordinator.clear()?;
                if value["error"] == "invalid_grant" || value["error"]["type"] == "invalid_grant" {
                    outcome.state = "reauth".into();
                    return Err("Claude 刷新凭据已失效，请重新登录");
                }
                outcome.state = "retry".into();
                return Err("Claude 刷新被拒绝；保留账号，稍后检查");
            }
            Err(crate::network::NOT_SENT) => {
                coordinator.clear()?;
                outcome.state = "retry".into();
                return Err(crate::network::NOT_SENT);
            }
            _ => return Err(UNCERTAIN),
        };
        outcome.state = "unsaved".into();
        outcome.pending = Some((current.clone(), next.clone()));
        // Keep the rotated token durably when possible. Even if Vault fails here,
        // still attempt the authoritative native commit before returning to the UI.
        let _ = coordinator.record(&current, Some(&next));
        let committed = coordinator.commit(&current, &next)?;
        outcome.refreshed = committed.credential == next.credential;
        outcome.bundle = committed;
        outcome.pending = None;
        outcome.state = "backup_pending".into();
        outcome.bundle = coordinator.sync()?;
        coordinator.clear()?;
        outcome.state = "ready".into();
        Ok(())
    })();
    if let Err(message) = result {
        if message == UNCERTAIN {
            outcome.state = "uncertain".into();
        }
        outcome.message = if outcome.state == "unsaved" && outcome.pending.is_some() {
            format!("续期已返回新凭据，写回尚未完成；会自动重试写回。{message}")
        } else {
            message.into()
        };
    }
    outcome
}
