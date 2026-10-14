use crate::platform::{Platform, Result, read_json, write_json};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Bundle {
    pub provider: String,
    pub credential: Value,
    pub identity: Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub owner: String,
    pub display: String,
    pub plan: String,
    pub expires_at: Option<u64>,
    pub refresh_expires_at: Option<u64>,
}
fn required<'a>(value: &'a Value, path: &str) -> Result<&'a str> {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("账号认证材料不完整")
}
pub fn claims(token: &str) -> Result<Value> {
    let payload = token.split('.').nth(1).ok_or("账号 token 格式无效")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| "账号 token 格式无效")?;
    serde_json::from_slice(&bytes).map_err(|_| "账号 token 字段无效")
}
fn masked(email: &str) -> String {
    match email.split_once('@') {
        Some((name, domain)) => format!("{}***@{}", name.chars().next().unwrap_or('*'), domain),
        None => "未提供邮箱".into(),
    }
}
impl Bundle {
    pub fn metadata(&self) -> Result<Metadata> {
        self.metadata_kind(true)
    }
    pub fn cloud_metadata(&self) -> Result<Metadata> {
        if self.credential.pointer("/tokens/refresh_token").is_some()
            || self.credential.get("refreshToken").is_some()
        {
            return Err("云端消费凭据不得包含刷新令牌");
        }
        self.metadata_kind(false)
    }
    fn metadata_kind(&self, managed: bool) -> Result<Metadata> {
        if self.provider == "codex" {
            if self.credential["OPENAI_API_KEY"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
                || self.credential["auth_mode"]
                    .as_str()
                    .is_some_and(|s| s != "chatgpt" && (managed || s != "chatgptAuthTokens"))
            {
                return Err("此入口仅纳管 ChatGPT 订阅登录");
            }
            let token = claims(required(&self.credential, "/tokens/id_token")?)?;
            let access = claims(required(&self.credential, "/tokens/access_token")?)?;
            if managed {
                required(&self.credential, "/tokens/refresh_token")?;
            }
            let auth = &token["https://api.openai.com/auth"];
            let user = auth["chatgpt_user_id"]
                .as_str()
                .or(auth["user_id"].as_str())
                .filter(|v| !v.is_empty())
                .ok_or("Codex 登录缺少用户身份")?;
            let workspace = required(&self.credential, "/tokens/account_id")?;
            if auth["chatgpt_account_id"]
                .as_str()
                .is_some_and(|v| v != workspace)
            {
                return Err("Codex 工作区身份不一致");
            }
            Ok(Metadata {
                owner: format!("codex:{user}:{workspace}"),
                display: masked(
                    token["email"]
                        .as_str()
                        .or(token["https://api.openai.com/profile"]["email"].as_str())
                        .unwrap_or(""),
                ),
                plan: auth["chatgpt_plan_type"].as_str().unwrap_or("未知").into(),
                expires_at: access["exp"].as_u64(),
                refresh_expires_at: None,
            })
        } else if self.provider == "claude" {
            required(&self.credential, "/accessToken")?;
            if managed {
                required(&self.credential, "/refreshToken")?;
            }
            if !self.credential["scopes"]
                .as_array()
                .is_some_and(|v| v.iter().any(|v| v == "user:inference"))
            {
                return Err("Claude 认证不含订阅调用权限");
            }
            Ok(Metadata {
                owner: format!(
                    "claude:{}:{}",
                    required(&self.identity, "/accountUuid")?,
                    required(&self.identity, "/organizationUuid")?
                ),
                display: masked(required(&self.identity, "/emailAddress")?),
                plan: self.credential["subscriptionType"]
                    .as_str()
                    .unwrap_or("未知")
                    .into(),
                expires_at: self.credential["expiresAt"].as_u64().map(|n| n / 1000),
                refresh_expires_at: self.credential["refreshTokenExpiresAt"]
                    .as_u64()
                    .map(|n| n / 1000),
            })
        } else {
            Err("不支持的账号类型")
        }
    }
    pub fn same_owner(&self, other: &Bundle) -> Result<bool> {
        Ok(self.metadata()?.owner == other.metadata()?.owner)
    }
    pub fn encoded(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    pub fn decode(value: Value) -> Result<Self> {
        let bundle: Self = serde_json::from_value(value).map_err(|_| "账号存储格式无效")?;
        bundle.metadata()?;
        Ok(bundle)
    }
}

#[derive(Clone)]
pub struct NativeStore {
    pub provider: String,
    pub directory: PathBuf,
    pub config: PathBuf,
    pub service: String,
    pub account: String,
    pub mode: String,
}

pub enum ClaudeRecoveryState {
    Authenticated(Bundle),
    Cleared { identity: Value },
}

fn cleared_claude_slot(credential: &Value) -> bool {
    credential["accessToken"] == ""
        && credential["refreshToken"] == ""
        && credential["expiresAt"] == 0
}
/// A Claude store that still exists but holds no subscription login: signed out, or cleared by
/// the official client after a rejected refresh. Its other entries, such as MCP logins, stay.
fn claude_signed_out(payload: &Value) -> bool {
    payload["claudeAiOauth"].is_null() || cleared_claude_slot(&payload["claudeAiOauth"])
}
pub const SIGNED_OUT: &str = "默认登录已退出或被清空；可重新登录此账号，或切换到其他账号";
impl NativeStore {
    pub fn codex(directory: &Path, force_file: bool) -> Result<Self> {
        let path = std::fs::canonicalize(directory).map_err(|_| "Codex 认证目录不存在")?;
        let mode = if force_file {
            "file".to_string()
        } else {
            codex_mode(&path)?
        };
        Ok(Self {
            provider: "codex".into(),
            directory: path.clone(),
            config: path.join("config.toml"),
            service: "Codex Auth".into(),
            account: format!(
                "cli|{}",
                &format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()))[..16]
            ),
            mode,
        })
    }
    pub fn claude(
        home: &Path,
        directory: Option<&Path>,
        storage_override: Option<&str>,
    ) -> Result<Self> {
        let directory_path = directory.unwrap_or(&home.join(".claude")).to_path_buf();
        let storage_path = match storage_override {
            Some("") => home.join(".claude"),
            Some(path) => PathBuf::from(path),
            None => directory_path.clone(),
        };
        // Relative roots depend on each client's working directory and cannot
        // identify a shared native store across the app, CLI and SDK.
        if !directory_path.is_absolute() || !storage_path.is_absolute() {
            return Err("Claude 配置和凭据目录必须使用绝对路径才能协同维护");
        }
        let string = storage_override
            .map(str::to_owned)
            .or_else(|| directory.map(|p| p.to_string_lossy().to_string()))
            .unwrap_or_default();
        if !string.is_ascii() {
            return Err("此版本尚未验证非 ASCII Claude 配置目录的 Keychain 定位");
        }
        let suffix = if string.is_empty() {
            String::new()
        } else {
            format!(
                "-{}",
                &format!("{:x}", Sha256::digest(string.as_bytes()))[..8]
            )
        };
        let config = if directory_path.join(".config.json").exists() {
            directory_path.join(".config.json")
        } else {
            directory
                .map(|p| p.join(".claude.json"))
                .unwrap_or_else(|| home.join(".claude.json"))
        };
        let user = std::env::var("USER")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                std::process::Command::new("/usr/bin/id")
                    .arg("-un")
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default()
            });
        let account = if !user.is_empty()
            && user
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            user
        } else {
            "claude-code-user".into()
        };
        Ok(Self {
            provider: "claude".into(),
            directory: storage_path,
            config,
            service: format!("Claude Code-credentials{suffix}"),
            account,
            mode: "auto".into(),
        })
    }
    fn file(&self) -> PathBuf {
        self.directory.join(if self.provider == "codex" {
            "auth.json"
        } else {
            ".credentials.json"
        })
    }
    fn payload(&self, platform: &Platform) -> Result<(Value, bool)> {
        if self.mode == "file" {
            return read_json(&self.file())?
                .map(|v| (v, false))
                .ok_or("原生凭据不存在");
        }
        if !matches!(self.mode.as_str(), "auto" | "keyring") {
            return Err("此原生凭据后端尚不支持多账号管理");
        }
        let key = if self.provider == "claude" {
            platform.native_read_key(&self.service, &self.account)?
        } else {
            platform.read_key(&self.service, &self.account)?
        };
        if let Some(key) = key {
            // Claude's official primary/fallback store treats a present Keychain
            // item as authoritative, even when an older fallback file remains.
            if self.provider == "claude" {
                return Ok((key, true));
            }
            if let Some(file) = read_json(&self.file())?
                && file != key
            {
                return Err("Keychain 与认证文件不一致，请先在官方客户端确认认证来源");
            }
            Ok((key, true))
        } else if self.mode == "auto" {
            read_json(&self.file())?
                .map(|v| (v, false))
                .ok_or("原生凭据不存在")
        } else {
            Err("原生 Keychain 凭据不存在")
        }
    }
    pub fn read(&self, platform: &Platform) -> Result<Bundle> {
        let bundle = self.read_bundle(platform)?;
        bundle.metadata()?;
        Ok(bundle)
    }
    /// The login this store holds, or None when it holds none: no credential at all, or a
    /// Claude store that was signed out or cleared.
    pub fn login(&self, platform: &Platform) -> Result<Option<Bundle>> {
        match self.payload(platform) {
            Err("原生凭据不存在" | "原生 Keychain 凭据不存在") => Ok(None),
            Ok((payload, _)) if self.provider == "claude" && claude_signed_out(&payload) => {
                Ok(None)
            }
            Ok(_) => self.read(platform).map(Some),
            Err(message) => Err(message),
        }
    }
    fn read_bundle(&self, platform: &Platform) -> Result<Bundle> {
        let (payload, _) = self.payload(platform)?;
        let bundle = if self.provider == "codex" {
            Bundle {
                provider: self.provider.clone(),
                credential: payload,
                identity: Value::Null,
            }
        } else {
            reject_mixed(&payload)?;
            let identity =
                read_json(&self.config)?.ok_or("Claude 身份配置不存在")?["oauthAccount"].clone();
            Bundle {
                provider: self.provider.clone(),
                credential: payload["claudeAiOauth"].clone(),
                identity,
            }
        };
        Ok(bundle)
    }
    pub fn cloud_read(&self, platform: &Platform) -> Result<Bundle> {
        let bundle = self.read_bundle(platform)?;
        bundle.cloud_metadata()?;
        Ok(bundle)
    }
    pub fn cloud_existing(&self, platform: &Platform) -> Result<Option<Bundle>> {
        match self.read_bundle(platform) {
            Ok(b) => {
                if b.metadata().is_err() {
                    b.cloud_metadata()?;
                }
                Ok(Some(b))
            }
            Err("原生凭据不存在" | "原生 Keychain 凭据不存在") => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub fn cloud_initialize(&self, platform: &Platform, target: &Bundle) -> Result<()> {
        if target.provider != self.provider
            || target
                .cloud_metadata()?
                .expires_at
                .is_none_or(|t| t <= crate::status::now() + 60)
        {
            return Err("云端凭据类型或有效期无效");
        }
        crate::platform::private_dir(&self.directory)?;
        let locks = if self.provider == "claude" {
            Some(crate::claude_locks::RefreshLocks::acquire(&self.directory)?)
        } else {
            None
        };
        let storage = if self.provider == "claude" {
            Some(crate::claude_locks::Lease::storage(&self.directory)?)
        } else {
            None
        };
        let guard = || -> Result<()> {
            if let Some(l) = &locks {
                l.check()?;
            }
            if let Some(l) = &storage {
                l.check()?;
            }
            Ok(())
        };
        guard()?;
        if self.cloud_existing(platform)?.is_some() {
            return Err("原生登录已存在，未覆盖");
        }
        let payload = if self.provider == "claude" {
            serde_json::json!({"claudeAiOauth":target.credential})
        } else {
            target.credential.clone()
        };
        if self.mode == "file" {
            crate::platform::write_json(&self.file(), &payload)?;
        } else if self.provider == "claude" {
            platform.native_write_key_guarded(&self.service, &self.account, &payload, &guard)?;
        } else {
            self.write_key(platform, &payload)?;
        }
        if self.provider == "claude" {
            let config =
                crate::platform::read_json(&self.config)?.unwrap_or_else(|| serde_json::json!({}));
            crate::platform::write_json(
                &self.config,
                &merge_claude_config(config, &target.identity)?,
            )?;
        }
        guard()?;
        if self.cloud_read(platform)? != *target {
            return Err("首次云端原生登录待确认");
        }
        Ok(())
    }
    /// Only the cloud owner writes access projections. The previous exact value is a CAS guard.
    pub fn cloud_publish(
        &self,
        platform: &Platform,
        expected: &Bundle,
        target: &Bundle,
    ) -> Result<()> {
        target.cloud_metadata()?;
        self.cloud_write(platform, expected, target, false)
    }
    pub fn cloud_restore(
        &self,
        platform: &Platform,
        expected: &Bundle,
        target: &Bundle,
    ) -> Result<()> {
        target.metadata()?;
        self.cloud_write(platform, expected, target, true)
    }
    fn cloud_write(
        &self,
        platform: &Platform,
        expected: &Bundle,
        target: &Bundle,
        restore: bool,
    ) -> Result<()> {
        let metadata = if restore {
            target.metadata()?
        } else {
            target.cloud_metadata()?
        };
        if target.provider != self.provider
            || (!restore
                && metadata
                    .expires_at
                    .is_none_or(|t| t <= crate::status::now() + 60))
        {
            return Err("云端凭据类型或有效期无效");
        }
        let locks = if self.provider == "claude" {
            Some(crate::claude_locks::RefreshLocks::acquire(&self.directory)?)
        } else {
            None
        };
        let storage = if self.provider == "claude" {
            Some(crate::claude_locks::Lease::storage(&self.directory)?)
        } else {
            None
        };
        let guard = || -> Result<()> {
            if let Some(l) = &locks {
                l.check()?;
            }
            if let Some(l) = &storage {
                l.check()?;
            }
            Ok(())
        };
        guard()?;
        if self.read_bundle(platform)? != *expected {
            return Err("原生登录已变化，未覆盖本机凭据");
        }
        let (mut payload, keyring) = self.payload(platform)?;
        if self.provider == "claude" {
            payload["claudeAiOauth"] = target.credential.clone();
        } else {
            payload = target.credential.clone();
        }
        guard()?;
        if keyring {
            if self.provider == "claude" {
                platform.native_write_key_guarded(
                    &self.service,
                    &self.account,
                    &payload,
                    &guard,
                )?;
            } else {
                self.write_key(platform, &payload)?;
            }
            if self.provider == "codex" && self.file().exists() {
                crate::platform::write_json(&self.file(), &payload)?;
            }
        } else {
            crate::platform::write_json(&self.file(), &payload)?;
        }
        if self.provider == "claude" {
            let config =
                crate::platform::read_json(&self.config)?.ok_or("Claude 身份配置不存在")?;
            crate::platform::write_json(
                &self.config,
                &merge_claude_config(config, &target.identity)?,
            )?;
        }
        guard()?;
        if self.read_bundle(platform)? != *target {
            return Err("云端凭据写回待确认，请恢复同步");
        }
        Ok(())
    }
    pub fn claude_recovery_state(&self, platform: &Platform) -> Result<ClaudeRecoveryState> {
        if self.provider != "claude" {
            return Err("此恢复入口仅支持 Claude");
        }
        let bundle = self.read_bundle(platform)?;
        if cleared_claude_slot(&bundle.credential) {
            return Ok(ClaudeRecoveryState::Cleared {
                identity: bundle.identity,
            });
        }
        bundle.metadata()?;
        Ok(ClaudeRecoveryState::Authenticated(bundle))
    }
    pub fn restore_cleared_claude(
        &self,
        platform: &Platform,
        expected: &Bundle,
        next: &Bundle,
        guard: &dyn Fn() -> Result<()>,
    ) -> Result<Bundle> {
        if !expected.same_owner(next)? {
            return Err("待恢复凭据身份不一致");
        }
        let ClaudeRecoveryState::Cleared { identity } = self.claude_recovery_state(platform)?
        else {
            return Err("原生登录已变化，未覆盖现有凭据");
        };
        if identity["accountUuid"] != expected.identity["accountUuid"]
            || identity["organizationUuid"] != expected.identity["organizationUuid"]
        {
            return Err("原生账号身份与待恢复记录不一致，未恢复凭据");
        }
        let (mut payload, keyring) = self.payload(platform)?;
        let mut cleared_expected = expected.credential.clone();
        cleared_expected["accessToken"] = Value::String(String::new());
        cleared_expected["refreshToken"] = Value::String(String::new());
        cleared_expected["expiresAt"] = Value::from(0);
        if payload["claudeAiOauth"] != cleared_expected {
            return Err("原生登录已变化，未覆盖现有凭据");
        }
        payload["claudeAiOauth"] = next.credential.clone();
        guard()?;
        if keyring {
            platform.native_write_key_guarded(&self.service, &self.account, &payload, guard)?;
        } else {
            write_json(&self.file(), &payload)?;
        }
        guard()?;
        let restored = self.read(platform)?;
        if restored.credential != next.credential || !restored.same_owner(next)? {
            return Err("恢复后的原生凭据未通过校验");
        }
        Ok(restored)
    }
    pub fn preflight_claude_refresh(&self, platform: &Platform) -> Result<()> {
        let (payload, keyring) = self.payload(platform)?;
        if keyring {
            platform.preflight_native_write(&self.service, &self.account, &payload)?;
        }
        Ok(())
    }
    fn write_key(&self, platform: &Platform, value: &Value) -> Result<()> {
        platform.native_write_key(&self.service, &self.account, value)
    }
    pub fn apply(&self, platform: &Platform, expected: &Bundle, target: &Bundle) -> Result<()> {
        self.apply_guarded(platform, expected, target, &|| Ok(()))
    }
    /// `guard` re-checks the caller's official leases right before each write.
    pub fn apply_guarded(
        &self,
        platform: &Platform,
        expected: &Bundle,
        target: &Bundle,
        guard: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        if target.provider != self.provider {
            return Err("账号类型不一致");
        }
        if self.read(platform)? != *expected {
            return Err("原生认证已变化，请重新检查后再切换");
        }
        let (payload, keyring) = self.payload(platform)?;
        let output = if self.provider == "codex" {
            target.credential.clone()
        } else {
            merge_claude_credentials(payload, &target.credential)?
        };
        if keyring {
            if self.provider == "claude" {
                platform.native_write_key_guarded(&self.service, &self.account, &output, guard)?;
            } else {
                self.write_key(platform, &output)?;
            }
            // A matching fallback must follow the same identity instead of reviving the previous account.
            if self.file().exists() {
                guard()?;
                write_json(&self.file(), &output)?;
            }
        } else {
            guard()?;
            write_json(&self.file(), &output)?;
        }
        if self.provider == "claude" {
            guard()?;
            let config = read_json(&self.config)?.ok_or("Claude 身份配置不存在")?;
            write_json(
                &self.config,
                &merge_claude_config(config, &target.identity)?,
            )?;
        }
        if self.read(platform)? != *target {
            return Err("原生认证写回未通过校验，请恢复当前状态");
        }
        Ok(())
    }
    pub fn refresh(&self, platform: &Platform, expected: &Bundle, target: &Bundle) -> Result<()> {
        self.refresh_guarded(platform, expected, target, &|| Ok(()))
    }
    pub fn refresh_guarded(
        &self,
        platform: &Platform,
        expected: &Bundle,
        target: &Bundle,
        guard: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        if !expected.same_owner(target)? || self.read(platform)? != *expected {
            return Err("原生认证已变化，未覆盖其凭据");
        }
        let (mut payload, keyring) = self.payload(platform)?;
        if self.provider == "claude" {
            payload["claudeAiOauth"] = target.credential.clone();
        } else {
            payload = target.credential.clone();
        }
        if keyring {
            guard()?;
            if self.provider == "claude" {
                platform.native_write_key_guarded(&self.service, &self.account, &payload, guard)?;
            } else {
                self.write_key(platform, &payload)?;
            }
            if self.provider != "claude" && self.file().exists() {
                write_json(&self.file(), &payload)?;
            }
        } else {
            guard()?;
            write_json(&self.file(), &payload)?;
        }
        guard()?;
        if self.read(platform)? != *target {
            return Err("刷新凭据尚未通过存储校验");
        }
        Ok(())
    }
    pub fn initialize(&self, platform: &Platform, target: &Bundle) -> Result<()> {
        let base = match self.payload(platform) {
            Err("原生凭据不存在" | "原生 Keychain 凭据不存在") => {
                serde_json::json!({})
            }
            Ok((payload, _)) if self.provider == "claude" && claude_signed_out(&payload) => payload,
            _ => return Err("原生认证已经存在，不能按空账号初始化"),
        };
        crate::platform::private_dir(&self.directory)?;
        let payload = if self.provider == "codex" {
            target.credential.clone()
        } else {
            merge_claude_credentials(base, &target.credential)?
        };
        if self.mode == "file" {
            write_json(&self.file(), &payload)?;
        } else {
            self.write_key(platform, &payload)?;
        }
        if self.provider == "claude" {
            let base = read_json(&self.config)?.unwrap_or_else(|| serde_json::json!({}));
            write_json(&self.config, &merge_claude_config(base, &target.identity)?)?;
        }
        if self.read(platform)? != *target {
            return Err("首次默认登录保存后校验失败");
        }
        Ok(())
    }
    /// Settles an interrupted switch from what this store holds now, or None when it holds no
    /// login. An official client may have renewed the credential since the switch stopped, so a
    /// credential matching neither side is placed by its owner: a Codex credential names it, and
    /// `claude_owner` looks a Claude one up. The pair is never guessed from the identity file,
    /// which the switch writes after the credential.
    pub fn recover_switch(
        &self,
        platform: &Platform,
        candidates: &[Bundle],
        claude_owner: &dyn Fn(&Bundle) -> Result<String>,
    ) -> Result<Option<Bundle>> {
        let payload = match self.payload(platform) {
            Ok((payload, _)) => payload,
            Err("原生凭据不存在" | "原生 Keychain 凭据不存在") => return Ok(None),
            Err(message) => return Err(message),
        };
        let credential = if self.provider == "codex" {
            payload
        } else {
            reject_mixed(&payload)?;
            if claude_signed_out(&payload) {
                return Ok(None);
            }
            payload["claudeAiOauth"].clone()
        };
        let base = if self.provider == "claude" {
            read_json(&self.config)?.unwrap_or_else(|| serde_json::json!({}))
        } else {
            Value::Null
        };
        let known = if let Some(known) = candidates
            .iter()
            .find(|b| b.provider == self.provider && b.credential == credential)
        {
            known.clone()
        } else if self.provider == "codex" {
            Bundle {
                provider: self.provider.clone(),
                credential,
                identity: Value::Null,
            }
        } else {
            let found = Bundle {
                provider: self.provider.clone(),
                credential,
                identity: base["oauthAccount"].clone(),
            };
            let owner = claude_owner(&found)?;
            let identity = candidates
                .iter()
                .chain([&found])
                .find(|b| b.metadata().is_ok_and(|m| m.owner == owner))
                .ok_or("原生 Claude 登录属于未登记的账号；请纳管当前登录后再恢复")?
                .identity
                .clone();
            Bundle { identity, ..found }
        };
        if self.provider == "claude" && base["oauthAccount"] != known.identity {
            write_json(&self.config, &merge_claude_config(base, &known.identity)?)?;
        }
        let actual = self.read(platform)?;
        if actual != known {
            return Err("切换恢复未通过身份与凭据一致性校验");
        }
        Ok(Some(actual))
    }
    pub fn remove_staging(&self, platform: &Platform) -> Result<()> {
        if self.provider == "claude" {
            platform.native_delete_key(&self.service, &self.account)?;
        }
        if self.file().exists() {
            std::fs::remove_file(self.file()).map_err(|_| "无法清理临时认证文件")?;
        }
        Ok(())
    }
}
fn codex_mode(directory: &Path) -> Result<String> {
    let content = match std::fs::read_to_string(directory.join("config.toml")) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("file".into()),
        Err(_) => return Err("无法读取 Codex 存储设置"),
    };
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            break;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim().trim_matches(['\'', '"']) == "cli_auth_credentials_store"
        {
            let value = value
                .split('#')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(['\'', '"']);
            return match value {
                "file" | "auto" | "keyring" => Ok(value.into()),
                _ => Err("Codex 存储模式不支持账号切换"),
            };
        }
    }
    Ok("file".into())
}
fn reject_mixed(payload: &Value) -> Result<()> {
    if ["enterpriseGateway", "designOauth"]
        .iter()
        .any(|key| !payload[*key].is_null())
    {
        Err("检测到混合 Claude 认证模式，不能自动切换订阅账号")
    } else {
        Ok(())
    }
}
pub fn merge_claude_credentials(mut base: Value, target: &Value) -> Result<Value> {
    reject_mixed(&base)?;
    let map = base.as_object_mut().ok_or("Claude 凭据格式无效")?;
    map.insert("claudeAiOauth".into(), target.clone());
    map.remove("organizationUuid");
    map.remove("trustedDeviceToken");
    Ok(base)
}
pub fn merge_claude_config(mut base: Value, identity: &Value) -> Result<Value> {
    let map = base.as_object_mut().ok_or("Claude 配置格式无效")?;
    for key in [
        "additionalModelOptionsCache",
        "additionalModelOptionsAnsweredAt",
        "additionalModelCostsCache",
        "modelAccessCache",
        "orgModelDefaultCache",
        "cachedArtifactRoster",
        "artifactRosterDenied",
        "lastSeenOrgDefaultUpdatedAt",
        "clientDataCache",
        "clientDataCacheSlots",
        "autoCompactWindowsCache",
        "cachedUsageUtilization",
        "githubWebConnectionStatusCache",
        "startupPrefetchedAt",
    ] {
        map.remove(key);
    }
    map.insert("oauthAccount".into(), identity.clone());
    Ok(base)
}
pub fn external_client_running(provider: &str) -> bool {
    let Ok(output) = std::process::Command::new("/bin/ps")
        .args(["-axo", "comm="])
        .output()
    else {
        return true;
    };
    if !output.status.success() {
        return true;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| official_client(provider, line.trim()))
}
// `comm` is argv[0], so a CLI typed as `Claude` keeps that spelling. Claude Desktop and the Code
// sessions it starts (`claude.app/Contents/MacOS/claude`) are app bundles that use the desktop's own
// token; only bare `claude` executables share the default login. Codex desktop's bundled CLI shares
// `~/.codex/auth.json`, so it still counts.
fn official_client(provider: &str, comm: &str) -> bool {
    let name = Path::new(comm)
        .file_name()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if provider == "codex" {
        matches!(name.as_str(), "codex" | "codex-bin" | "codex.exe")
    } else {
        matches!(name.as_str(), "claude" | "claude.exe") && !comm.contains(".app/Contents/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switching_claude_replaces_identity_but_preserves_projects() {
        let before = json!({"oauthAccount":{"accountUuid":"old","oldOnly":true},"projects":{"keep":true},"modelAccessCache":{"old":true},"hasCompletedOnboarding":true});
        let after = merge_claude_config(before, &json!({"accountUuid":"new"})).unwrap();
        assert_eq!(after["oauthAccount"], json!({"accountUuid":"new"}));
        assert!(after.get("modelAccessCache").is_none());
        assert_eq!(after["projects"]["keep"], true);
        assert_eq!(after["hasCompletedOnboarding"], true);
    }
    #[test]
    fn credentials_preserve_other_fields_and_reject_mixed_auth() {
        let after=merge_claude_credentials(json!({"claudeAiOauth":{},"coworkRemoteDevice":{"keep":true},"trustedDeviceToken":"old"}),&json!({"refreshToken":"synthetic"})).unwrap();
        assert_eq!(after["coworkRemoteDevice"]["keep"], true);
        assert!(after.get("trustedDeviceToken").is_none());
        assert!(merge_claude_credentials(json!({"enterpriseGateway":{}}), &json!({})).is_err());
    }
    #[test]
    fn claude_desktop_processes_are_not_clients_of_the_global_login() {
        // `ps -axo comm=` rows seen on 2026-10-03.
        for comm in [
            "Claude",
            "claude",
            "/opt/homebrew/bin/claude",
            "/Users/u/.vscode/extensions/anthropic.claude-code-2.1.284-darwin-arm64/resources/native-binary/claude",
        ] {
            assert!(official_client("claude", comm), "{comm}");
        }
        for comm in [
            "/Applications/Claude.app/Contents/MacOS/Claude",
            "/Users/u/Library/Application Support/Claude/claude-code/2.1.286/f2326db61802/claude.app/Contents/MacOS/claude",
            "/Applications/Claude.app/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper",
        ] {
            assert!(!official_client("claude", comm), "{comm}");
        }
        assert!(official_client(
            "codex",
            "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
        ));
        assert!(!official_client(
            "codex",
            "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"
        ));
    }
}
