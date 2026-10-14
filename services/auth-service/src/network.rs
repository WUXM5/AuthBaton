//! Per-child proxy settings. Never mutate the service environment or system preferences.
use crate::platform::{Result, pipe_command, pipe_command_with_environment};
use std::net::IpAddr;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const PROXY_KEYS: [&str; 6] = [
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
];

fn environment() -> Result<Vec<(String, String)>> {
    resolve(
        |key| std::env::var_os(key).map(|v| v.to_string_lossy().into_owned()),
        || {
            let bytes = pipe_command(
                Path::new("/usr/sbin/scutil"),
                &["--proxy"],
                &[],
                Duration::from_secs(2),
            )?;
            String::from_utf8(bytes).map_err(|_| "无法读取系统代理设置")
        },
    )
}

/// Every value is written out, including ones this process already has, so a child with a
/// cleared or filtered environment gets the same proxy as one that inherits.
fn resolve(
    get: impl Fn(&str) -> Option<String>,
    system: impl FnOnce() -> Result<String>,
) -> Result<Vec<(String, String)>> {
    let explicit: Vec<(String, String)> = PROXY_KEYS
        .iter()
        .chain(&["no_proxy", "NO_PROXY"])
        .filter_map(|key| Some((key.to_string(), get(key)?)))
        .collect();
    // Explicit process configuration retains precedence, including an explicit empty value.
    if explicit
        .iter()
        .any(|(key, _)| PROXY_KEYS.contains(&key.as_str()))
    {
        return Ok(explicit);
    }
    let mut values = parse_system(&system()?)?;
    if !explicit.is_empty() {
        values.retain(|(key, _)| key != "no_proxy");
    }
    values.extend(explicit);
    Ok(values)
}

fn parse_system(text: &str) -> Result<Vec<(String, String)>> {
    if !text.trim_start().starts_with("<dictionary> {") {
        return Err("无法识别系统代理设置");
    }
    let scalar = |key: &str| {
        text.lines().find_map(|line| {
            let (name, value) = line.trim().split_once(" : ")?;
            (name == key).then_some(value.trim())
        })
    };
    if scalar("ProxyAutoConfigEnable") == Some("1")
        || scalar("ProxyAutoDiscoveryEnable") == Some("1")
    {
        return Err("当前系统使用自动代理；请配置显式代理环境或手动系统代理");
    }
    let mut env = vec![];
    for (prefix, key, scheme) in [
        ("HTTP", "http_proxy", "http"),
        ("HTTPS", "https_proxy", "http"),
        ("SOCKS", "all_proxy", "socks5h"),
    ] {
        if scalar(&format!("{prefix}Enable")) != Some("1") {
            continue;
        }
        let host = scalar(&format!("{prefix}Proxy")).ok_or("系统代理缺少地址")?;
        let host = match host.parse::<IpAddr>() {
            Ok(IpAddr::V6(ip)) => format!("[{ip}]"),
            Ok(ip) => ip.to_string(),
            Err(_)
                if !host.is_empty()
                    && host
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b)) =>
            {
                host.into()
            }
            _ => return Err("系统代理地址无效"),
        };
        let port = scalar(&format!("{prefix}Port"))
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|p| *p > 0)
            .ok_or("系统代理端口无效")?;
        env.push((key.into(), format!("{scheme}://{host}:{port}")));
    }
    // Our requests use public, fully qualified provider hosts. <local> has no effect on them.
    let mut in_exceptions = false;
    let mut exceptions = vec![];
    for line in text.lines().map(str::trim) {
        if line == "ExceptionsList : <array> {" {
            in_exceptions = true;
        } else if in_exceptions && line == "}" {
            in_exceptions = false;
        } else if in_exceptions && let Some((_, value)) = line.split_once(" : ") {
            if value == "<local>" {
                continue;
            }
            let value = value.strip_prefix('*').unwrap_or(value);
            if value.is_empty()
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-:/".contains(&b))
            {
                return Err("系统代理排除规则暂不支持");
            }
            exceptions.push(value);
        }
    }
    if !env.is_empty() && !exceptions.is_empty() {
        env.push(("no_proxy".into(), exceptions.join(",")));
    }
    Ok(env)
}

pub fn configure(command: &mut Command) -> Result<()> {
    command.envs(environment()?);
    Ok(())
}

pub fn curl(args: &[&str], input: &[u8], timeout: Duration) -> Result<Vec<u8>> {
    pipe_command_with_environment(
        Path::new("/usr/bin/curl"),
        args,
        input,
        timeout,
        &environment()?,
    )
}

/// A request that provably never left this machine; any other transport failure may have been
/// received and acted on by the server.
pub const NOT_SENT: &str = "网络未连通，请求未发出；稍后自动重试";
/// Proxy settings that no request can use. As with `NOT_SENT`, nothing left this machine.
pub const PROXY_NOT_SENT: &str =
    "系统代理设置无法使用（如自动代理），请求未发出；改用手动代理或代理环境变量后自动重试";
pub fn not_sent(message: &str) -> bool {
    matches!(message, NOT_SENT | PROXY_NOT_SENT)
}

/// Like `curl`, but keeps the exit status so a caller can classify a failed transfer.
pub fn curl_status(
    args: &[&str],
    input: &[u8],
    timeout: Duration,
) -> Result<(Option<i32>, Vec<u8>)> {
    curl_status_with(
        environment(),
        Path::new("/usr/bin/curl"),
        args,
        input,
        timeout,
    )
}
fn curl_status_with(
    environment: Result<Vec<(String, String)>>,
    curl: &Path,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
) -> Result<(Option<i32>, Vec<u8>)> {
    // Nothing reaches the network before curl runs, so unusable proxy settings or a curl that
    // cannot launch leave the request unsent.
    let environment = environment.map_err(|_| PROXY_NOT_SENT)?;
    crate::platform::pipe_status_with_cancel(curl, args, input, timeout, None, &environment)
        .map_err(|message| {
            if message == crate::platform::SPAWN_FAILED {
                NOT_SENT
            } else {
                message
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    const SETTINGS: &str = "<dictionary> {\n HTTPEnable : 1\n HTTPProxy : 127.0.0.1\n HTTPPort : 1082\n HTTPSEnable : 1\n HTTPSProxy : 127.0.0.1\n HTTPSPort : 1082\n SOCKSEnable : 1\n SOCKSProxy : 127.0.0.1\n SOCKSPort : 1082\n ExceptionsList : <array> {\n 0 : localhost\n 1 : *.local\n 2 : 10.0.0.0/8\n 3 : <local>\n }\n}";

    #[test]
    fn gui_without_proxy_environment_uses_current_system_settings() {
        let env = resolve(|_| None, || Ok(SETTINGS.into())).unwrap();
        assert!(env.contains(&("https_proxy".into(), "http://127.0.0.1:1082".into())));
        assert!(env.contains(&("all_proxy".into(), "socks5h://127.0.0.1:1082".into())));
        assert!(env.contains(&("no_proxy".into(), "localhost,.local,10.0.0.0/8".into())));
        let changed = resolve(|_| None, || Ok(SETTINGS.replace("1082", "12345"))).unwrap();
        assert!(changed.contains(&("https_proxy".into(), "http://127.0.0.1:12345".into())));
        assert!(
            resolve(|_| None, || Ok("<dictionary> {\n}".into()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn explicit_proxy_and_bypass_environment_are_preserved_and_written_out() {
        let own = |key: &'static str| move |k: &str| (k == key).then(|| format!("own-{k}"));
        for key in PROXY_KEYS {
            assert_eq!(
                resolve(own(key), || panic!("must not read system settings")).unwrap(),
                vec![(key.to_string(), format!("own-{key}"))]
            );
        }
        // An explicit empty value still wins over the system proxy.
        assert_eq!(
            resolve(|k| (k == "https_proxy").then(String::new), || panic!()).unwrap(),
            vec![("https_proxy".to_string(), String::new())]
        );
        let env = resolve(own("NO_PROXY"), || Ok(SETTINGS.into())).unwrap();
        assert!(!env.iter().any(|(key, _)| key == "no_proxy"));
        assert!(env.contains(&("NO_PROXY".into(), "own-NO_PROXY".into())));
        assert!(env.contains(&("https_proxy".into(), "http://127.0.0.1:1082".into())));
    }

    #[test]
    fn a_request_that_never_started_is_classified_as_not_sent() {
        let unusable =
            parse_system(&SETTINGS.replace("HTTPEnable : 1", "ProxyAutoConfigEnable : 1"));
        let args = ["--version"];
        let curl = Path::new("/usr/bin/curl");
        assert_eq!(
            curl_status_with(unusable, curl, &args, b"", Duration::from_secs(5)),
            Err(PROXY_NOT_SENT)
        );
        let missing = Path::new("/nonexistent/authmanager-curl");
        assert_eq!(
            curl_status_with(Ok(vec![]), missing, &args, b"", Duration::from_secs(5)),
            Err(NOT_SENT)
        );
        assert!(
            not_sent(PROXY_NOT_SENT) && not_sent(NOT_SENT) && !not_sent("网络中断，未收到回应")
        );
    }

    #[test]
    fn invalid_or_automatic_proxy_is_not_silently_used_as_direct() {
        for value in [
            "unparseable".into(),
            SETTINGS.replace("1082", "0"),
            SETTINGS.replace("127.0.0.1", "bad\\host"),
            SETTINGS.replace("HTTPEnable : 1", "ProxyAutoConfigEnable : 1"),
        ] {
            assert!(parse_system(&value).is_err());
        }
        let env = parse_system(&SETTINGS.replace("127.0.0.1", "::1")).unwrap();
        assert!(env.contains(&("https_proxy".into(), "http://[::1]:1082".into())));
    }

    #[test]
    #[ignore = "unauthenticated public GET probes using current macOS system proxy"]
    fn system_proxy_reaches_provider_without_process_proxy_environment() {
        assert!(PROXY_KEYS.iter().all(|key| std::env::var_os(key).is_none()));
        for url in [
            "https://chatgpt.com/backend-api/wham/usage",
            "https://api.anthropic.com/api/oauth/profile",
        ] {
            let started = std::time::Instant::now();
            let output = curl(
                &[
                    "-q",
                    "--silent",
                    "--max-time",
                    "8",
                    "--output",
                    "/dev/null",
                    "--write-out",
                    "%{http_code}",
                    url,
                ],
                &[],
                Duration::from_secs(10),
            )
            .unwrap();
            assert_eq!(output, b"401");
            eprintln!(
                "unauthenticated provider responded 401 in {} ms",
                started.elapsed().as_millis()
            );
        }
    }

    #[test]
    #[ignore = "real curl against local sockets; waits out the 20 s transfer limit"]
    fn failed_transfers_are_classified_by_whether_the_request_could_leave() {
        use std::io::Read;
        let body = serde_json::json!({});
        let refused = crate::maintenance::http("http://localhost:9/", None, Some(&body), false);
        assert_eq!(refused, Err(NOT_SENT));
        // Receives the whole request and never answers, like a reply lost on the way back.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut received = [0; 1024];
            let size = stream.read(&mut received).unwrap();
            std::thread::sleep(Duration::from_secs(22));
            size
        });
        let unanswered = crate::maintenance::http(
            &format!("http://localhost:{port}/"),
            None,
            Some(&body),
            false,
        );
        assert!(server.join().unwrap() > 0);
        assert!(unanswered.is_err());
        assert_ne!(unanswered, Err(NOT_SENT));
    }
}
