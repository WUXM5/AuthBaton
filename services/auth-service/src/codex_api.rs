//! Fixed Codex quota endpoints. No token refresh, credential writer, or redirect support.
use crate::credentials::Bundle;
use crate::platform::Result;
use serde_json::{Value, json};

pub struct Access {
    token: String,
    account: String,
    expires_at: Option<u64>,
}
impl Access {
    pub fn from_bundle(bundle: &Bundle) -> Result<Self> {
        if bundle.provider != "codex" {
            return Err("此入口仅支持 Codex");
        }
        let metadata = bundle.metadata()?;
        let token = bundle.credential["tokens"]["access_token"]
            .as_str()
            .unwrap();
        let account = bundle.credential["tokens"]["account_id"].as_str().unwrap();
        if token.chars().chain(account.chars()).any(char::is_control) {
            return Err("额度查询凭据包含无效字符");
        }
        Ok(Self {
            token: token.into(),
            account: account.into(),
            expires_at: metadata.expires_at,
        })
    }
    pub fn expired(&self, time: u64) -> bool {
        self.expires_at.is_some_and(|expiry| expiry <= time)
    }
    #[cfg(test)]
    pub fn synthetic(expires_at: u64) -> Self {
        Self {
            token: "synthetic".into(),
            account: "synthetic".into(),
            expires_at: Some(expires_at),
        }
    }
}
pub enum Request {
    Usage,
    ResetCredits,
    Consume {
        request_id: String,
        credit_id: String,
    },
}
pub struct Response {
    pub status: u16,
    pub body: Value,
    pub retry_after: Option<u64>,
}
pub type Transport = fn(&Access, &Request) -> Result<Response>;

fn request_config(access: &Access, request: &Request) -> String {
    let quote = |value: &str| format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""));
    let path = match request {
        Request::Usage => "usage",
        Request::ResetCredits => "rate-limit-reset-credits",
        Request::Consume { .. } => "rate-limit-reset-credits/consume",
    };
    let mut config = format!(
        "url = \"https://chatgpt.com/backend-api/wham/{path}\"\nheader = {}\nheader = {}\nheader = \"Accept: application/json\"\n",
        quote(&format!("Authorization: Bearer {}", access.token)),
        quote(&format!("ChatGPT-Account-Id: {}", access.account)),
    );
    if let Request::Consume {
        request_id,
        credit_id,
    } = request
    {
        let body = json!({"redeem_request_id":request_id,"credit_id":credit_id});
        config.push_str(&format!(
            "request = \"POST\"\nheader = \"Content-Type: application/json\"\ndata = {}\n",
            quote(&body.to_string())
        ));
    }
    config
}
#[cfg(not(test))]
pub fn http(access: &Access, request: &Request) -> Result<Response> {
    let bytes = crate::network::curl(
        &[
            "-q",
            "--silent",
            "--max-time",
            "15",
            "--max-filesize",
            "1048576",
            "--user-agent",
            "codex-cli",
            "--write-out",
            "\n%{http_code}\n%header{retry-after}",
            "--config",
            "-",
        ],
        request_config(access, request).as_bytes(),
        std::time::Duration::from_secs(18),
    )?;
    parse_response(bytes)
}
fn parse_response(bytes: Vec<u8>) -> Result<Response> {
    let text = String::from_utf8(bytes).map_err(|_| "额度响应无效")?;
    let (text, retry_after) = text.rsplit_once('\n').ok_or("额度响应无效")?;
    let (body, status) = text.rsplit_once('\n').ok_or("额度响应无效")?;
    let status = status.parse().map_err(|_| "额度响应无效")?;
    Ok(Response {
        status,
        body: if status == 200 {
            serde_json::from_str(body).unwrap_or(Value::Null)
        } else {
            Value::Null
        },
        retry_after: retry_after.trim().parse().ok(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_endpoints_keep_secrets_in_stdin_and_consume_uses_the_exact_pair() {
        let access = Access::synthetic(100);
        let read = request_config(&access, &Request::Usage);
        assert!(read.contains("https://chatgpt.com/backend-api/wham/usage"));
        assert!(!read.contains("POST"));
        assert!(!read.contains("data ="));
        let consume = request_config(
            &access,
            &Request::Consume {
                request_id: "request-1".into(),
                credit_id: "credit-1".into(),
            },
        );
        assert!(consume.contains("rate-limit-reset-credits/consume"));
        assert!(consume.contains("redeem_request_id"));
        assert!(consume.contains("request-1"));
        assert!(consume.contains("credit-1"));
        assert!(!consume.contains("refresh_token"));
        let response = parse_response(b"<html>private</html>\n429\n1800".to_vec()).unwrap();
        assert!(response.body.is_null());
        assert_eq!(response.retry_after, Some(1800));
    }
}
