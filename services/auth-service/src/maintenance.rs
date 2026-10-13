use crate::credentials::{Bundle, NativeStore};
use crate::platform::{Platform, Result, private_dir, read_json, write_json};
use crate::status::now;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::time::Duration;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Quota {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<u64>,
}
pub type Transport = fn(&str, Option<&str>, Option<&Value>, bool) -> Result<(u16, Value)>;

#[derive(Clone)]
pub struct Work {
    pub id: String,
    pub bundle: Bundle,
    pub platform: Platform,
    pub profile: PathBuf,
    pub codex: Option<PathBuf>,
    pub native: Option<NativeStore>,
    pub cancel: Arc<AtomicBool>,
    pub transport: Transport,
}
pub struct Outcome {
    pub bundle: Bundle,
    pub online: bool,
    pub refreshed: bool,
    pub state: String,
    pub message: String,
    pub quota: Vec<Quota>,
    pub pending: Option<(Bundle, Bundle)>,
}
impl Outcome {
    pub(crate) fn failed(bundle: Bundle, message: &str) -> Self {
        Self {
            bundle,
            online: false,
            refreshed: false,
            state: "retry".into(),
            message: message.into(),
            quota: vec![],
            pending: None,
        }
    }
}
impl Work {
    pub fn run(self) -> Outcome {
        if self.cancel.load(Ordering::Acquire) {
            return Outcome::failed(self.bundle.clone(), "检查已停止");
        }
        if self.bundle.provider == "codex" {
            self.codex_check(false)
        } else {
            if self.native.is_none()
                && !matches!(
                    self.platform
                        .vault_read_optional(&crate::claude::journal_id(&self.id)),
                    Ok(None)
                )
            {
                let mut result = Outcome::failed(
                    self.bundle.clone(),
                    "此账号的原生续期待确认，请纳管实际登录后恢复；未刷新备份副本",
                );
                result.state = "attention".into();
                return result;
            }
            self.claude_check()
        }
    }
    fn save(&self, expected: &Bundle, next: &Bundle) -> Result<()> {
        if !expected.same_owner(next)? {
            return Err("刷新后的账号身份不一致");
        }
        if let Some(native) = &self.native {
            native.refresh(&self.platform, expected, next)
        } else {
            if Bundle::decode(self.platform.vault_read(&self.id)?)? != *expected {
                return Err("凭据已被其他操作更新");
            }
            self.platform.vault_write(&self.id, &next.encoded())
        }
    }
    pub fn run_force_refresh(self) -> Outcome {
        self.codex_check(true)
    }
    fn codex_check(&self, force_refresh: bool) -> Outcome {
        let mut outcome = Outcome::failed(self.bundle.clone(), "Codex 在线检查未完成，稍后重试");
        if self.native.is_none()
            && let Some(left) = codex_leftover(&self.profile, &self.bundle)
        {
            if let Err(message) = self.save(&self.bundle, &left) {
                outcome.state = "unsaved".into();
                outcome.message = message.into();
                outcome.pending = Some((self.bundle.clone(), left));
                return outcome;
            }
            let renewed = Self {
                bundle: left,
                ..self.clone()
            };
            let mut outcome = renewed.codex_check(force_refresh);
            outcome.refreshed = true;
            return outcome;
        }
        let result = (|| -> Result<Vec<Quota>> {
            let binary = self.codex.as_ref().ok_or("未找到 Codex CLI")?;
            let home = if let Some(native) = &self.native {
                native.directory.clone()
            } else {
                private_dir(&self.profile)?;
                // Names the vault generation this check starts from; see `codex_leftover`.
                std::fs::write(self.profile.join("base"), generation(&self.bundle))
                    .map_err(|_| "无法写入检查目录")?;
                write_json(&self.profile.join("auth.json"), &self.bundle.credential)?;
                self.profile.clone()
            };
            let mut rpc = CodexRpc::start(binary, &home, self.native.is_none())?;
            if self.cancel.load(Ordering::Acquire) {
                return Err("检查已停止");
            }
            let refresh = force_refresh
                || self
                    .bundle
                    .metadata()?
                    .expires_at
                    .is_some_and(|t| t <= now() + 600);
            let account = rpc.call("account/read", json!({"refreshToken":refresh}))?;
            validate_codex_account(&account)?;
            if self.cancel.load(Ordering::Acquire) {
                return Err("检查已停止");
            }
            let value = match rpc.call("account/rateLimits/read", Value::Null) {
                Ok(value) => value,
                Err(message) => {
                    if !self.cancel.load(Ordering::Acquire) {
                        // A quota request can refresh internally. Read the same instance's
                        // cached auth state instead of inferring expiry from a generic 401.
                        let account = rpc.call("account/read", json!({"refreshToken":false}))?;
                        validate_codex_account(&account)?;
                    }
                    return Err(message);
                }
            };
            let bucket = &value["rateLimits"];
            let mut quota = Vec::new();
            for (field, label) in [("primary", "主要额度"), ("secondary", "次要额度")] {
                if let Some(used) = bucket[field]["usedPercent"]
                    .as_f64()
                    .filter(|v| v.is_finite())
                {
                    quota.push(Quota {
                        label: label.into(),
                        used_percent: used.clamp(0.0, 100.0),
                        resets_at: bucket[field]["resetsAt"].as_u64(),
                    });
                }
            }
            Ok(quota)
        })();
        // Even a quota request can refresh; recover its latest persisted credentials on failure.
        let updated = if let Some(native) = &self.native {
            native.read(&self.platform)
        } else {
            read_json(&self.profile.join("auth.json"))
                .and_then(|v| v.ok_or("临时认证读取失败"))
                .map(|v| Bundle {
                    provider: "codex".into(),
                    credential: v,
                    identity: Value::Null,
                })
        };
        if let Ok(updated) = updated {
            if !self.bundle.same_owner(&updated).unwrap_or(false) {
                outcome.state = "attention".into();
                outcome.message = "官方检查返回了不同账号，已停止维护".into();
                return outcome;
            }
            if self.native.is_none()
                && let Err(message) = self.save(&self.bundle, &updated)
            {
                outcome.state = "unsaved".into();
                outcome.message = message.into();
                outcome.pending = Some((self.bundle.clone(), updated));
                return outcome;
            }
            outcome.refreshed = updated.credential["tokens"] != self.bundle.credential["tokens"];
            outcome.bundle = updated;
            if self.native.is_none() {
                let _ = std::fs::remove_file(self.profile.join("auth.json"));
            }
        } else {
            outcome.state = "attention".into();
            outcome.message = "检查后的凭据无法读取；未确认续期结果".into();
            return outcome;
        }
        if outcome.bundle.metadata().is_err() {
            outcome.state = "attention".into();
            outcome.message = "检查后的凭据未通过校验".into();
            return outcome;
        }
        match result {
            Ok(quota) => {
                outcome.online = true;
                outcome.state = "healthy".into();
                outcome.message = "Codex 在线检查成功".into();
                outcome.quota = quota;
            }
            Err(message) => {
                outcome.message = message.into();
                if message == "官方认证需要重新登录" {
                    outcome.state = "reauth".into();
                } else if message == "Codex 未返回可确认的订阅账号，已停止检查" {
                    outcome.state = "attention".into();
                }
            }
        }
        outcome
    }
    fn claude_check(&self) -> Outcome {
        self.claude_check_inner(false)
    }
    fn claude_prepare(&self, force_refresh: bool) -> Outcome {
        let mut result = Outcome::failed(self.bundle.clone(), "Claude 在线检查未完成");
        if self.cancel.load(Ordering::Acquire) {
            result.message = "检查已停止".into();
            return result;
        }
        let mut current = self.bundle.clone();
        if current
            .metadata()
            .ok()
            .and_then(|m| m.refresh_expires_at)
            .is_some_and(|t| t <= now())
        {
            result.state = "reauth".into();
            result.message = "刷新凭据已到期，请重新登录".into();
            return result;
        }
        if force_refresh
            || current
                .metadata()
                .ok()
                .and_then(|m| m.expires_at)
                .is_none_or(|t| t <= now() + 600)
        {
            let credential = &current.credential;
            let scopes = credential["scopes"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ");
            let body = json!({"grant_type":"refresh_token","refresh_token":credential["refreshToken"],"client_id":credential["clientId"].as_str().unwrap_or("9d1c250a-e61b-44d9-88ed-5944d1962f5e"),"scope":scopes});
            if self.cancel.load(Ordering::Acquire) {
                result.message = "检查已停止".into();
                return result;
            }
            match (self.transport)(
                "https://platform.claude.com/v1/oauth/token",
                None,
                Some(&body),
                false,
            ) {
                Ok((200, value)) => match merge_refresh(&current, &value, now()) {
                    Ok(next) => {
                        // Commit rotated material before optional profile/usage requests.
                        result.bundle = next.clone();
                        if let Err(message) = self.save(&current, &next) {
                            result.state = "unsaved".into();
                            result.message = message.into();
                            result.pending = Some((current.clone(), next));
                            result.bundle = current;
                            return result;
                        }
                        current = next;
                        result.refreshed = true;
                        result.bundle = current.clone();
                    }
                    Err(message) => {
                        result.state = "attention".into();
                        result.message = message.into();
                        return result;
                    }
                },
                Ok((_, value)) => {
                    if value["error"] == "invalid_grant"
                        || value["error"]["type"] == "invalid_grant"
                    {
                        result.state = "reauth".into();
                        result.message = "Claude 刷新凭据已失效，请重新登录".into();
                    } else {
                        result.message = "Claude 刷新被拒绝；保留账号，稍后检查".into();
                    }
                    return result;
                }
                Err(message) if crate::network::not_sent(message) => {
                    result.message = message.into();
                    return result;
                }
                Err(_) => {
                    // No newer credential exists anywhere: the next check re-grants this one.
                    result.state = "uncertain".into();
                    result.message = crate::claude::UNCERTAIN.into();
                    return result;
                }
            }
        }
        result.state = "ready".into();
        result
    }
    fn claude_check_inner(&self, force_refresh: bool) -> Outcome {
        let mut result = if self.native.is_some() {
            crate::claude::prepare(self, force_refresh)
        } else {
            self.claude_prepare(force_refresh)
        };
        if result.state != "ready" {
            return result;
        }
        result.state = "retry".into();
        let current = result.bundle.clone();
        if self.cancel.load(Ordering::Acquire) {
            result.message = "维护已停止；已提交的续期结果保留".into();
            return result;
        }
        if !current.credential["scopes"]
            .as_array()
            .is_some_and(|scopes| scopes.iter().any(|s| s == "user:profile"))
        {
            result.state = "limited".into();
            result.message = "授权未包含身份查询权限；到期续期仍会按计划进行".into();
            return result;
        }
        let token = current.credential["accessToken"].as_str().unwrap();
        match (self.transport)(
            "https://api.anthropic.com/api/oauth/profile",
            Some(token),
            None,
            false,
        ) {
            Ok((200, profile)) => {
                if profile["account"]["uuid"] != current.identity["accountUuid"]
                    || profile["organization"]["uuid"] != current.identity["organizationUuid"]
                {
                    result.state = "attention".into();
                    result.message = "Claude 在线身份与保存账号不一致".into();
                    return result;
                }
                result.online = true;
                result.state = "healthy".into();
                result.message = "Claude 在线身份验证成功".into();
            }
            Ok((401, _)) if !result.refreshed && !force_refresh => {
                let mut latest = self.clone();
                latest.bundle = current;
                return latest.claude_check_inner(true);
            }
            Ok((403, _)) => {
                result.state = "limited".into();
                result.message = "在线身份接口无访问权限；未将登录判为失效".into();
                return result;
            }
            Ok((401, _)) => {
                result.state = "reauth".into();
                result.message = "Claude 在线认证未通过，请重新登录".into();
                return result;
            }
            _ => {
                result.message = "Claude 在线检查暂不可用；未将账号判为失效".into();
                return result;
            }
        }
        if !self.cancel.load(Ordering::Acquire)
            && let Ok((200, usage)) = (self.transport)(
                "https://api.anthropic.com/api/oauth/usage",
                Some(token),
                None,
                true,
            )
        {
            for (field, label) in [("five_hour", "5 小时额度"), ("seven_day", "7 天额度")] {
                if let Some(used) = usage[field]["utilization"]
                    .as_f64()
                    .filter(|v| v.is_finite())
                {
                    result.quota.push(Quota {
                        label: label.into(),
                        used_percent: used.clamp(0.0, 100.0),
                        resets_at: None,
                    });
                }
            }
        }
        result
    }
}
pub(crate) fn generation(bundle: &Bundle) -> String {
    format!("{:x}", Sha256::digest(bundle.credential.to_string()))
}
/// The renewed credential that an interrupted Codex check left in its profile: the official CLI
/// rotates it there, and a crash before the save leaves it as the only copy. It is newer than the
/// vault only while the vault still holds the generation that check started from.
pub fn codex_leftover(profile: &Path, current: &Bundle) -> Option<Bundle> {
    if std::fs::read_to_string(profile.join("base")).ok()? != generation(current) {
        return None;
    }
    let left = Bundle {
        provider: "codex".into(),
        credential: read_json(&profile.join("auth.json")).ok()??,
        identity: Value::Null,
    };
    (left != *current && current.same_owner(&left).unwrap_or(false)).then_some(left)
}
/// The Claude account that owns `bundle`'s access token, as its `claude:<account>:<organization>`
/// owner key. A read-only profile request; used only when local evidence cannot tell.
pub fn claude_owner(transport: Transport, bundle: &Bundle) -> Result<String> {
    let token = bundle.credential["accessToken"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or("原生 Claude 登录缺少访问凭据")?;
    match transport(
        "https://api.anthropic.com/api/oauth/profile",
        Some(token),
        None,
        false,
    ) {
        Ok((200, profile)) => match (
            profile["account"]["uuid"].as_str(),
            profile["organization"]["uuid"].as_str(),
        ) {
            (Some(account), Some(organization)) => Ok(format!("claude:{account}:{organization}")),
            _ => Err("Claude 身份接口未返回账号；稍后重试恢复"),
        },
        Ok((401, _)) => {
            Err("原生 Claude 登录已失效，无法确认属于哪个账号；请在官方客户端重新登录后再恢复")
        }
        _ => Err("暂时无法确认原生 Claude 登录属于哪个账号；稍后重试恢复"),
    }
}
fn validate_codex_account(value: &Value) -> Result<()> {
    match value.get("account") {
        Some(Value::Null) if value["requiresOpenaiAuth"] == true => Err("官方认证需要重新登录"),
        Some(account) if account["type"] == "chatgpt" => Ok(()),
        _ => Err("Codex 未返回可确认的订阅账号，已停止检查"),
    }
}
pub fn merge_refresh(bundle: &Bundle, response: &Value, time: u64) -> Result<Bundle> {
    let access = response["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("刷新响应缺少新凭据")?;
    let expires = response["expires_in"]
        .as_u64()
        .filter(|n| *n > 0 && *n < 366 * 86400)
        .ok_or("刷新响应缺少有效期限")?;
    let mut result = bundle.clone();
    let value = result
        .credential
        .as_object_mut()
        .ok_or("认证材料格式无效")?;
    value.insert("accessToken".into(), json!(access));
    value.insert("expiresAt".into(), json!((time + expires) * 1000));
    if let Some(refresh) = response["refresh_token"].as_str().filter(|v| !v.is_empty()) {
        value.insert("refreshToken".into(), json!(refresh));
    }
    if let Some(expires) = response["refresh_token_expires_in"].as_u64() {
        value.insert(
            "refreshTokenExpiresAt".into(),
            json!((time + expires) * 1000),
        );
    }
    if let Some(scope) = response["scope"].as_str() {
        value.insert(
            "scopes".into(),
            json!(scope.split_whitespace().collect::<Vec<_>>()),
        );
    }
    result.metadata()?;
    Ok(result)
}
fn config_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}
pub fn http(
    url: &str,
    token: Option<&str>,
    body: Option<&Value>,
    usage: bool,
) -> Result<(u16, Value)> {
    let mut config = format!(
        "url = {}\nheader = \"Content-Type: application/json\"\nheader = \"Cache-Control: no-cache\"\n",
        config_quote(url)
    );
    if let Some(token) = token {
        if token.contains(['\r', '\n']) {
            return Err("认证材料包含无效字符");
        }
        config.push_str(&format!(
            "header = {}\n",
            config_quote(&format!("Authorization: Bearer {token}"))
        ));
    }
    if usage {
        config.push_str("header = \"anthropic-beta: oauth-2025-04-20\"\n");
    }
    let reset_metadata = url == crate::claude_resets::USAGE_URL && usage;
    if reset_metadata {
        if body.is_some() {
            return Err("额度重置信息只允许只读查询");
        }
        config.push_str("header = \"x-app: cli\"\n");
    }
    if let Some(body) = body {
        config.push_str(&format!("data = {}\n", config_quote(&body.to_string())));
    }
    let (exit, bytes) = crate::network::curl_status(
        &[
            "-q",
            "--silent",
            "--max-time",
            "20",
            "--max-filesize",
            "1048576",
            "--user-agent",
            if reset_metadata {
                crate::claude_resets::USAGE_AGENT
            } else {
                "AuthManager/0.3.3"
            },
            "--write-out",
            "\n%{time_pretransfer} %{http_code}",
            "--config",
            "-",
        ],
        config.as_bytes(),
        Duration::from_secs(23),
    )?;
    let text = String::from_utf8(bytes).map_err(|_| "在线检查响应无效")?;
    let (body, trailer) = text.rsplit_once('\n').ok_or("在线检查响应无效")?;
    let (pretransfer, status) = trailer.split_once(' ').ok_or("在线检查响应无效")?;
    if exit != Some(0) {
        // curl only starts the pre-transfer clock once connection, proxy tunnel and TLS are ready
        // to carry the request; zero proves nothing reached the server.
        return Err(if pretransfer.parse::<f64>() == Ok(0.0) {
            crate::network::NOT_SENT
        } else {
            "网络中断，未收到回应"
        });
    }
    Ok((
        status.parse().map_err(|_| "在线检查响应无效")?,
        serde_json::from_str(body).map_err(|_| "在线检查响应无效")?,
    ))
}
struct CodexRpc {
    child: Child,
    input: ChildStdin,
    output: Receiver<Value>,
    serial: u64,
}
impl CodexRpc {
    fn start(binary: &Path, home: &Path, force_file: bool) -> Result<Self> {
        let mut command = Command::new(binary);
        crate::network::configure(&mut command)?;
        if force_file {
            command.args(["-c", "cli_auth_credentials_store=\"file\""]);
        }
        command
            .arg("app-server")
            .env("CODEX_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| "无法启动 Codex 检查")?;
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, output) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if line.len() > 1024 * 1024 {
                    break;
                }
                if let Ok(value) = serde_json::from_str(&line)
                    && sender.send(value).is_err()
                {
                    break;
                }
            }
        });
        let mut rpc = Self {
            child,
            input,
            output,
            serial: 0,
        };
        rpc.call(
            "initialize",
            json!({"clientInfo":{"name":"authmanager","version":"0.3.3"}}),
        )?;
        writeln!(rpc.input, "{}", json!({"method":"initialized"})).map_err(|_| "Codex 连接中断")?;
        Ok(rpc)
    }
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.serial += 1;
        let id = self.serial;
        writeln!(
            self.input,
            "{}",
            json!({"id":id,"method":method,"params":params})
        )
        .map_err(|_| "Codex 连接中断")?;
        let deadline = std::time::Instant::now() + Duration::from_secs(25);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let value = match self.output.recv_timeout(left) {
                Ok(value) => value,
                Err(mpsc::RecvTimeoutError::Timeout) => return Err("Codex 在线检查超时"),
                // The app-server closed its output, so no reply can arrive.
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("Codex 连接中断"),
            };
            if value["id"] == id {
                if value.get("error").is_some() {
                    let message = value["error"]["message"]
                        .as_str()
                        .unwrap_or("")
                        .to_lowercase();
                    return Err(
                        if message.contains("refresh_token")
                            || message.contains("invalid_grant")
                            || message.contains("sign in")
                        {
                            "官方认证需要重新登录"
                        } else if method == "account/rateLimits/read" {
                            "Codex 额度查询暂不可用；未确认账号失效"
                        } else if method == "account/read" {
                            "Codex 认证检查失败；未确认续期结果"
                        } else {
                            "Codex 本地服务接口暂不可用"
                        },
                    );
                }
                return Ok(value["result"].clone());
            }
        }
    }
}
impl Drop for CodexRpc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refresh_preserves_omitted_refresh_fields() {
        let bundle = Bundle {
            provider: "claude".into(),
            credential: json!({"accessToken":"old","refreshToken":"keep","refreshTokenExpiresAt":999999999,"scopes":["user:inference"]}),
            identity: json!({"accountUuid":"a","organizationUuid":"o","emailAddress":"synthetic@example.invalid"}),
        };
        let next = merge_refresh(
            &bundle,
            &json!({"access_token":"new","expires_in":3600}),
            100,
        )
        .unwrap();
        assert_eq!(next.credential["refreshToken"], "keep");
        assert_eq!(next.credential["refreshTokenExpiresAt"], 999999999);
        assert_eq!(next.credential["expiresAt"], 3700000);
        assert!(merge_refresh(&bundle, &json!({"access_token":"new"}), 100).is_err());
    }

    #[test]
    fn an_app_server_that_exits_ends_the_call_at_once() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "authmanager-rpc-{}",
            crate::platform::random_id().unwrap()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("exiting-codex");
        std::fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let started = std::time::Instant::now();
        let result = CodexRpc::start(&binary, &root, true).map(|_| ());
        let elapsed = started.elapsed();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(result, Err("Codex 连接中断"));
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }
}
