//! Evidence projections. Configured credentials do not prove a running client's identity.
use crate::credentials::{Metadata, claims};
use crate::platform::{Result, read_json};
use serde_json::{Value, json};
use std::path::Path;

pub struct Source {
    pub id: &'static str,
    pub owner: Option<String>,
    pub display: Option<String>,
    pub message: &'static str,
}
impl Source {
    pub fn native(id: &'static str, result: Result<Metadata>) -> Self {
        match result {
            Ok(metadata) => Self {
                id,
                owner: Some(metadata.owner),
                display: Some(metadata.display),
                message: "默认认证存储已读取；运行中的进程可能使用自己的缓存或环境凭据",
            },
            Err(_) => Self {
                id,
                owner: None,
                display: None,
                message: "默认认证未能识别；不据此判断正在运行的客户端已退出登录",
            },
        }
    }
    pub fn pi(path: &Path) -> Self {
        let owner = (|| -> Option<String> {
            let auth = read_json(path).ok()??;
            let data = claims(auth["openai-codex"]["access"].as_str()?).ok()?;
            let identity = &data["https://api.openai.com/auth"];
            let user = identity["chatgpt_user_id"]
                .as_str()
                .or(identity["user_id"].as_str())?;
            let account = identity["chatgpt_account_id"].as_str()?;
            Some(format!("codex:{user}:{account}"))
        })();
        Self {
            id: "pi",
            owner,
            display: None,
            message: "此处仅检查 pi 原生 Codex 凭据；扩展 provider 或运行中的会话可选择不同账号",
        }
    }
}
pub fn login_notice(provider: &str, metadata: Option<&Metadata>, time: u64) -> Option<Value> {
    if provider != "claude" {
        return None;
    }
    let Some(metadata) = metadata else {
        return Some(json!({"state":"unknown","message":"尚未读取登录期限"}));
    };
    let Some(expiry) = metadata.refresh_expires_at else {
        return Some(
            json!({"state":"unknown","message":"官方存储未提供登录续期期限；不推算固定登录周期"}),
        );
    };
    // Match installed Claude's suppression for access material that outlives the recorded deadline.
    if metadata
        .expires_at
        .is_some_and(|access| access > expiry.saturating_add(3 * 86400))
    {
        return Some(
            json!({"state":"unknown","expires_at":expiry,"message":"记录的登录期限与当前凭据不一致，暂不据此提醒重新登录"}),
        );
    }
    let remaining = expiry.saturating_sub(time);
    let state = if expiry <= time {
        "expired"
    } else if remaining <= 3 * 86400 {
        "soon"
    } else {
        "normal"
    };
    let message = match state {
        "expired" => "记录的登录续期期限已过；请检查认证，可能需要重新登录".into(),
        "soon" => format!(
            "登录续期期限剩余约 {} 天，建议提前重新登录",
            remaining.div_ceil(86400)
        ),
        _ => "当前不在重新登录提醒窗口内".into(),
    };
    Some(
        json!({"state":state,"expires_at":expiry,"message":message,"source":"本机登录记录；可能由官方客户端在登录时推算，不是订阅到期日"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_warning_uses_refresh_deadline_not_access_expiry() {
        let mut metadata = Metadata {
            owner: "synthetic".into(),
            display: "s***@example.invalid".into(),
            plan: "pro".into(),
            expires_at: Some(100),
            refresh_expires_at: Some(400000),
        };
        assert_eq!(
            login_notice("claude", Some(&metadata), 1).unwrap()["state"],
            "normal"
        );
        assert_eq!(
            login_notice("claude", Some(&metadata), 400000 - 3 * 86400).unwrap()["state"],
            "soon"
        );
        assert_eq!(
            login_notice("claude", Some(&metadata), 400000).unwrap()["state"],
            "expired"
        );
        metadata.expires_at = Some(700000);
        assert_eq!(
            login_notice("claude", Some(&metadata), 399999).unwrap()["state"],
            "unknown"
        );
        metadata.refresh_expires_at = None;
        assert_eq!(
            login_notice("claude", Some(&metadata), 1).unwrap()["state"],
            "unknown"
        );
        assert!(login_notice("codex", Some(&metadata), 1).is_none());
    }
}
