//! Endpoint for the Claude Code companion mod (`integrations/claude-code`). The mod's host only
//! speaks HTTP over a Unix socket, so this is one small HTTP/1.1 route. Rate-limit readings come
//! in as hints that can only bring a quota GET forward; the reply says when the default Claude
//! login last changed. No credential or account identifier crosses this socket.
use crate::accounts::Accounts;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::{
    fs::{FileTypeExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Deserialize)]
pub struct Report {
    /// The session's last turn ended on a usage-limit error.
    #[serde(default)]
    pub limited: bool,
    #[serde(default)]
    pub windows: Vec<Window>,
}
#[derive(Deserialize)]
pub struct Window {
    pub kind: String,
    pub used: f64,
}
impl Report {
    /// Highest use of the primary five-hour and seven-day windows, as the response reported it.
    pub fn primary_used(&self) -> f64 {
        self.windows
            .iter()
            .filter(|w| matches!(w.kind.as_str(), "five_hour" | "seven_day"))
            .map(|w| w.used)
            .fold(0.0, f64::max)
    }
}

/// The last change of the default Claude login that running sessions will pick up.
#[derive(Clone, Serialize)]
pub struct Change {
    pub at: u64,
    pub kind: &'static str,
    pub label: String,
}
#[derive(Default)]
pub struct Runtime {
    pub change: Option<Change>,
    pub last_query: u64,
    pub last_report: Option<u64>,
}
impl Runtime {
    pub fn snapshot(&self) -> Value {
        json!({"last_report_at": self.last_report, "change": self.change})
    }
}

pub struct Listener {
    listener: UnixListener,
    path: PathBuf,
}
impl Listener {
    pub fn bind(root: &Path) -> Result<Self, &'static str> {
        let path = root.join("claude-mod.sock");
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_socket() => {
                fs::remove_file(&path).map_err(|_| "无法清理 Claude Code 插件 socket")?;
            }
            Ok(_) => return Err("Claude Code 插件 socket 路径被占用"),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err("无法检查 Claude Code 插件 socket"),
        }
        let listener = UnixListener::bind(&path).map_err(|_| "无法启动 Claude Code 插件接口")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "无法保护 Claude Code 插件 socket")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "无法设置 Claude Code 插件 socket")?;
        Ok(Self { listener, path })
    }
    pub fn poll(&self, accounts: &mut Accounts) {
        if let Ok((mut stream, _)) = self.listener.accept() {
            let _ = serve(&mut stream, |report| accounts.mod_report(report));
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn serve(stream: &mut UnixStream, handle: impl FnOnce(Report) -> Value) -> io::Result<()> {
    // An accepted socket inherits the listener's non-blocking flag on macOS.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_millis(400)))?;
    stream.set_write_timeout(Some(Duration::from_millis(400)))?;
    same_user(stream)?;
    let (status, body) = match read_request(stream) {
        Ok(body) => match serde_json::from_slice::<Report>(&body) {
            Ok(report) => ("200 OK", handle(report)),
            Err(_) => ("400 Bad Request", json!({"error": "invalid report"})),
        },
        Err(status) => (status, json!({"error": status})),
    };
    let body = body.to_string();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Reads `POST /v1/report` with a Content-Length body of at most 4 KiB.
fn read_request(stream: &mut UnixStream) -> Result<Vec<u8>, &'static str> {
    const BAD: &str = "400 Bad Request";
    let mut reader = BufReader::new(stream.take(8192));
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|_| BAD)?;
    let mut parts = line.split_whitespace();
    if parts.next() != Some("POST") {
        return Err("405 Method Not Allowed");
    }
    if parts.next() != Some("/v1/report") {
        return Err("404 Not Found");
    }
    let mut length = None;
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|_| BAD)? == 0 {
            return Err(BAD);
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>().map_err(|_| BAD)?);
        }
    }
    let length = length.ok_or("411 Length Required")?;
    if length > 4096 {
        return Err("413 Content Too Large");
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|_| BAD)?;
    Ok(body)
}

fn same_user(stream: &UnixStream) -> io::Result<()> {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: getpeereid only writes the two out-parameters for this connected socket.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0
        || uid != unsafe { libc::geteuid() }
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn exchange(request: &str) -> (String, Option<Report>) {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client.write_all(request.as_bytes()).unwrap();
        let mut seen = None;
        serve(&mut server, |r| {
            seen = Some(r);
            json!({"ok": true})
        })
        .unwrap();
        drop(server);
        let mut reply = String::new();
        client.read_to_string(&mut reply).unwrap();
        (reply, seen)
    }

    #[test]
    fn report_route_parses_body_and_answers_json() {
        // Byte for byte what Claude Code 2.1.287 (Bun 1.4.3) sent from the mod in an isolated run.
        let request = "POST /v1/report HTTP/1.1\r\nContent-Type: application/json\r\nConnection: keep-alive\r\nUser-Agent: Bun/1.4.3\r\nAccept: */*\r\nHost: authmanager\r\nAccept-Encoding: gzip, deflate, br, zstd\r\nContent-Length: 91\r\n\r\n{\"limited\":true,\"windows\":[{\"kind\":\"five_hour\",\"used\":100},{\"kind\":\"seven_day\",\"used\":40}]}";
        let (reply, seen) = exchange(request);
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        assert!(reply.ends_with(r#"{"ok":true}"#), "{reply}");
        let report = seen.unwrap();
        assert!(report.limited);
        assert_eq!(report.primary_used(), 100.0);
        let gateway =
            r#"{"windows":[{"kind":"seven_day","used":97.5},{"kind":"spend_limit","used":120}]}"#;
        let (_, seen) = exchange(&format!(
            "POST /v1/report HTTP/1.1\r\ncontent-length: {}\r\n\r\n{gateway}",
            gateway.len()
        ));
        assert_eq!(seen.unwrap().primary_used(), 97.5);
    }

    #[test]
    fn other_routes_and_oversized_or_unsized_bodies_are_refused_without_handling() {
        for (request, status) in [
            ("GET /v1/report HTTP/1.1\r\n\r\n", "405"),
            ("POST /other HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}", "404"),
            ("POST /v1/report HTTP/1.1\r\n\r\n{}", "411"),
            (
                "POST /v1/report HTTP/1.1\r\nContent-Length: 5000\r\n\r\n",
                "413",
            ),
            (
                "POST /v1/report HTTP/1.1\r\nContent-Length: 3\r\n\r\n{x}",
                "400",
            ),
        ] {
            let (reply, seen) = exchange(request);
            assert!(reply.starts_with(&format!("HTTP/1.1 {status}")), "{reply}");
            assert!(seen.is_none());
        }
    }
}
