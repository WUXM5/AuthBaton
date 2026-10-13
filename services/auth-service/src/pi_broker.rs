//! Dedicated local socket: a tiny, versioned access-token consumer API.
//! The general UI IPC never returns these responses.
use crate::accounts::Accounts;
use crate::platform::Result;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
    id: String,
    method: String,
    #[serde(default)]
    account_id: Option<String>,
}
pub(crate) fn response(input: &[u8], accounts: &mut Accounts) -> Value {
    let request = match serde_json::from_slice::<Request>(input) {
        Ok(r)
            if r.version == 1
                && !r.id.is_empty()
                && r.id.len() <= 80
                && r.account_id
                    .as_ref()
                    .is_none_or(|id| !id.is_empty() && id.len() <= 80) =>
        {
            r
        }
        _ => return json!({"version":1,"id":null,"error":"pi 认证请求格式无效"}),
    };
    let result = match request.method.as_str() {
        "accounts" if request.account_id.is_none() => Ok(accounts.pi_accounts()),
        "access" => accounts.pi_access(request.account_id.as_deref()),
        _ => Err("不支持的 pi 认证操作"),
    };
    match result {
        Ok(result) => json!({"version":1,"id":request.id,"result":result}),
        Err(message) => json!({"version":1,"id":request.id,"error":message}),
    }
}
pub struct Listener {
    listener: UnixListener,
    path: PathBuf,
}
impl Listener {
    /// Called only after the parent service has acquired its single-owner lock.
    pub fn bind(root: &Path) -> Result<Self> {
        let path = root.join("pi.sock");
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                fs::remove_file(&path).map_err(|_| "无法清理旧 pi socket")?
            }
            Ok(_) => return Err("pi socket 路径已被其他文件占用"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("无法检查 pi socket"),
        }
        let listener = UnixListener::bind(&path).map_err(|_| "无法启动 pi 凭据接口")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "无法保护 pi socket")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "无法设置 pi socket")?;
        Ok(Self { listener, path })
    }
    pub fn poll(&self, accounts: &mut Accounts) {
        if let Ok((mut stream, _)) = self.listener.accept() {
            let _ = handle(&mut stream, accounts);
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
fn handle(stream: &mut UnixStream, accounts: &mut Accounts) -> std::io::Result<()> {
    // An accepted socket inherits the listener's non-blocking flag on macOS.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut input = Vec::new();
    BufReader::new((&mut *stream).take(4097)).read_until(b'\n', &mut input)?;
    if input.len() > 4096 || input.last() != Some(&b'\n') {
        return Ok(());
    }
    serde_json::to_writer(&mut *stream, &response(&input, accounts))?;
    stream.write_all(b"\n")
}
