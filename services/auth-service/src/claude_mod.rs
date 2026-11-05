//! Endpoint for the Claude Code companion mod (`integrations/claude-code`). The mod's host only
//! speaks HTTP over a Unix socket, so this is one small HTTP/1.1 route. Rate-limit readings come
//! in as hints that can only bring a quota GET forward; the reply says when the default Claude
//! login last changed. No credential or account identifier crosses this socket.
//!
//! `POST /v1/network` (v0.22) is the mod's word on each model request of a session: `begin`
//! before it is sent, `alive` while it streams, `end` after. While a network switch is pending
//! the reply to `begin` is held back, up to `HOLD` at a time, so Claude Code sends nothing new
//! until the switch is over; the hook waits on the request, which costs it none of its budget.
use crate::accounts::Accounts;
use crate::network_switch::{ModEvent, ModReply};
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
use std::time::{Duration, Instant};

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

/// The longest a held `begin` waits for its answer; the mod asks again after it.
const HOLD: Duration = Duration::from_secs(20);
const HELD_MAX: usize = 64;

struct Held {
    stream: UnixStream,
    event: ModEvent,
    until: Instant,
}

pub struct Listener {
    listener: UnixListener,
    path: PathBuf,
    held: Vec<Held>,
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
        Ok(Self {
            listener,
            path,
            held: vec![],
        })
    }
    pub fn poll(&mut self, accounts: &mut Accounts) {
        self.release(|event| accounts.network_mod(event));
        let Ok((mut stream, _)) = self.listener.accept() else {
            return;
        };
        let Ok(Some(event)) = serve(&mut stream, accounts) else {
            return;
        };
        if self.held.len() >= HELD_MAX {
            let _ = reply(&mut stream, "200 OK", &json!({"hold": true}));
            return;
        }
        self.held.push(Held {
            stream,
            event,
            until: Instant::now() + HOLD,
        });
    }
    /// Answers each held `begin` whose session may go on, or whose wait ran out.
    fn release(&mut self, mut decide: impl FnMut(&ModEvent) -> ModReply) {
        let now = Instant::now();
        let mut kept = vec![];
        for mut held in self.held.drain(..) {
            let decision = decide(&held.event);
            if decision == ModReply::Go || now >= held.until {
                let hold = decision == ModReply::Hold;
                let _ = reply(&mut held.stream, "200 OK", &json!({ "hold": hold }));
            } else {
                kept.push(held);
            }
        }
        self.held = kept;
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// What answers the mod: the account service, or a test's stand-in.
pub trait Handler {
    fn report(&mut self, report: Report) -> Value;
    fn network(&mut self, event: &ModEvent) -> ModReply;
}
impl Handler for Accounts {
    fn report(&mut self, report: Report) -> Value {
        self.mod_report(report)
    }
    fn network(&mut self, event: &ModEvent) -> ModReply {
        self.network_mod(event)
    }
}

/// Reads one request and answers it, except a `begin` the network gate holds: that event comes
/// back for the caller to answer later on the same stream.
fn serve(stream: &mut UnixStream, handler: &mut impl Handler) -> io::Result<Option<ModEvent>> {
    // An accepted socket inherits the listener's non-blocking flag on macOS.
    stream.set_nonblocking(false)?;
    stream.set_write_timeout(Some(Duration::from_millis(400)))?;
    same_user(stream)?;
    let request = Deadline {
        stream,
        until: Instant::now() + Duration::from_secs(1),
    };
    let (status, body) = match read_request(request) {
        Ok((Route::Report, body)) => match serde_json::from_slice::<Report>(&body) {
            Ok(r) => ("200 OK", handler.report(r)),
            Err(_) => ("400 Bad Request", json!({"error": "invalid report"})),
        },
        Ok((Route::Network, body)) => match serde_json::from_slice::<ModEvent>(&body) {
            Ok(event) if !event.session.is_empty() && event.session.len() <= 128 => {
                match handler.network(&event) {
                    ModReply::Hold if event.event == "begin" => return Ok(Some(event)),
                    ModReply::Hold => ("200 OK", json!({"hold": true})),
                    ModReply::Go => ("200 OK", json!({"hold": false})),
                }
            }
            _ => ("400 Bad Request", json!({"error": "invalid event"})),
        },
        Err(status) => (status, json!({"error": status})),
    };
    reply(stream, status, &body).map(|_| None)
}

fn reply(stream: &mut UnixStream, status: &str, body: &Value) -> io::Result<()> {
    let body = body.to_string();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

enum Route {
    Report,
    Network,
}

/// One deadline for the whole request. The service loop waits on it, and a read timeout alone
/// would let a client that sends a byte at a time hold that loop for as long as it likes.
struct Deadline<'a> {
    stream: &'a UnixStream,
    until: Instant,
}
impl Read for Deadline<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        let mut stream = self.stream;
        stream.read(buffer)
    }
}

/// Reads `POST /v1/report` or `POST /v1/network` with a Content-Length body of at most 4 KiB.
fn read_request(stream: impl Read) -> Result<(Route, Vec<u8>), &'static str> {
    const BAD: &str = "400 Bad Request";
    let mut reader = BufReader::new(stream.take(8192));
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|_| BAD)?;
    let mut parts = line.split_whitespace();
    if parts.next() != Some("POST") {
        return Err("405 Method Not Allowed");
    }
    let route = match parts.next() {
        Some("/v1/report") => Route::Report,
        Some("/v1/network") => Route::Network,
        _ => return Err("404 Not Found"),
    };
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
    Ok((route, body))
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

    struct Stub {
        seen: Option<Report>,
        gate: ModReply,
    }
    impl Handler for Stub {
        fn report(&mut self, report: Report) -> Value {
            self.seen = Some(report);
            json!({"ok": true})
        }
        fn network(&mut self, _: &ModEvent) -> ModReply {
            match self.gate {
                ModReply::Go => ModReply::Go,
                ModReply::Hold => ModReply::Hold,
            }
        }
    }
    fn stub(gate: ModReply) -> Stub {
        Stub { seen: None, gate }
    }

    fn exchange(request: &str) -> (String, Option<Report>) {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client.write_all(request.as_bytes()).unwrap();
        let mut handler = stub(ModReply::Go);
        serve(&mut server, &mut handler).unwrap();
        drop(server);
        let mut reply = String::new();
        client.read_to_string(&mut reply).unwrap();
        (reply, handler.seen)
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

    fn post(body: &str) -> String {
        format!(
            "POST /v1/network HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn a_held_begin_is_answered_once_the_gate_opens_and_others_answer_at_once() {
        let begin = r#"{"session":"s1","project":"notes","event":"begin","request":"t1:main:0"}"#;
        // Gate open: answered on the spot.
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client.write_all(post(begin).as_bytes()).unwrap();
        let held = serve(&mut server, &mut stub(ModReply::Go)).unwrap();
        assert!(held.is_none());
        drop(server);
        let mut reply = String::new();
        client.read_to_string(&mut reply).unwrap();
        assert!(reply.ends_with(r#"{"hold":false}"#), "{reply}");
        // Gate closed: the begin comes back to be held; an alive is answered at once.
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client.write_all(post(begin).as_bytes()).unwrap();
        let event = serve(&mut server, &mut stub(ModReply::Hold))
            .unwrap()
            .unwrap();
        assert_eq!(event.request, "t1:main:0");
        let listener_path = std::env::temp_dir().join(format!("mod-{}.sock", std::process::id()));
        let mut listener = Listener {
            listener: UnixListener::bind(&listener_path).unwrap(),
            path: listener_path,
            held: vec![Held {
                stream: server,
                event,
                until: Instant::now() + HOLD,
            }],
        };
        listener.release(|_| ModReply::Hold);
        assert_eq!(listener.held.len(), 1);
        listener.release(|_| ModReply::Go);
        assert!(listener.held.is_empty());
        let mut reply = String::new();
        client.read_to_string(&mut reply).unwrap();
        assert!(reply.ends_with(r#"{"hold":false}"#), "{reply}");
        // A wait that runs out says to ask again.
        let (mut client, server) = UnixStream::pair().unwrap();
        listener.held.push(Held {
            stream: server,
            event: serde_json::from_str(begin).unwrap(),
            until: Instant::now(),
        });
        listener.release(|_| ModReply::Hold);
        let mut reply = String::new();
        client.read_to_string(&mut reply).unwrap();
        assert!(reply.ends_with(r#"{"hold":true}"#), "{reply}");
        let alive = r#"{"session":"s1","event":"alive","request":"t1:main:0"}"#;
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client.write_all(post(alive).as_bytes()).unwrap();
        assert!(
            serve(&mut server, &mut stub(ModReply::Hold))
                .unwrap()
                .is_none()
        );
        drop(server);
        let mut reply = String::new();
        client.read_to_string(&mut reply).unwrap();
        assert!(reply.ends_with(r#"{"hold":true}"#), "{reply}");
    }

    #[test]
    fn a_client_sending_a_byte_at_a_time_cannot_hold_the_service_past_one_second() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            for byte in b"POST /v1/report HTTP/1.1\r\n".iter().take(20) {
                std::thread::sleep(Duration::from_millis(300));
                if client.write_all(&[*byte]).is_err() {
                    break;
                }
            }
        });
        let started = Instant::now();
        let mut handler = stub(ModReply::Go);
        let _ = serve(&mut server, &mut handler);
        assert!(started.elapsed() < Duration::from_millis(1500));
        assert!(handler.seen.is_none());
        drop(server);
        writer.join().unwrap();
    }
}
