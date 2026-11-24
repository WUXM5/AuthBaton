//! Per-child proxy settings. Never mutate the service environment or system preferences.
use crate::pac::Route;
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

/// `url` is where the request goes (for an official CLI, its sign-in or main host): a proxy
/// auto-configuration script decides per URL.
fn environment(url: &str) -> Result<Vec<(String, String)>> {
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
        |script| pac(script, url),
    )
}

/// Fetches the system's PAC script directly, as macOS does, and runs it for `url`.
fn pac(script: &str, url: &str) -> Result<Vec<(String, String)>> {
    let script = pipe_command(
        Path::new("/usr/bin/curl"),
        &[
            "-q",
            "--silent",
            "--fail",
            "--max-time",
            "2",
            "--max-filesize",
            "1048576",
            "--proto",
            "=http,https,file",
            "--noproxy",
            "*",
            "--url",
            script,
        ],
        &[],
        Duration::from_secs(3),
    )
    .ok()
    .and_then(|bytes| String::from_utf8(bytes).ok())
    .ok_or("读不到系统自动代理的 PAC 脚本")?;
    Ok(match crate::pac::first_route(&script, url)? {
        Route::Direct => vec![],
        Route::Http(host, port) => {
            let proxy = endpoint("http", &host, u16::try_from(port).ok())?;
            vec![
                ("http_proxy".into(), proxy.clone()),
                ("https_proxy".into(), proxy),
            ]
        }
        Route::Socks(host, port) => vec![(
            "all_proxy".into(),
            endpoint("socks5h", &host, u16::try_from(port).ok())?,
        )],
    })
}

/// Every value is written out, including ones this process already has, so a child with a
/// cleared or filtered environment gets the same proxy as one that inherits.
fn resolve(
    get: impl Fn(&str) -> Option<String>,
    system: impl FnOnce() -> Result<String>,
    pac: impl FnOnce(&str) -> Result<Vec<(String, String)>>,
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
    let mut values = parse_system(&system()?, pac)?;
    if !explicit.is_empty() {
        values.retain(|(key, _)| key != "no_proxy");
    }
    values.extend(explicit);
    Ok(values)
}

fn parse_system(
    text: &str,
    pac: impl FnOnce(&str) -> Result<Vec<(String, String)>>,
) -> Result<Vec<(String, String)>> {
    if !text.trim_start().starts_with("<dictionary> {") {
        return Err("无法识别系统代理设置");
    }
    let scalar = |key: &str| {
        text.lines().find_map(|line| {
            let (name, value) = line.trim().split_once(" : ")?;
            (name == key).then_some(value.trim())
        })
    };
    // macOS runs the script ahead of manual proxies, the bypass list and auto discovery.
    if scalar("ProxyAutoConfigEnable") == Some("1") {
        return pac(scalar("ProxyAutoConfigURLString").ok_or("系统自动代理没有设置 PAC 脚本地址")?);
    }
    if scalar("ProxyAutoDiscoveryEnable") == Some("1") {
        return Err("系统开着自动代理发现，暂不支持；请改用 PAC、手动代理或代理环境变量");
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
        let port = scalar(&format!("{prefix}Port")).and_then(|p| p.parse::<u16>().ok());
        env.push((key.into(), endpoint(scheme, host, port)?));
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

/// A proxy URL from a host and port read from system settings or a PAC script.
fn endpoint(scheme: &str, host: &str, port: Option<u16>) -> Result<String> {
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
    let port = port.filter(|p| *p > 0).ok_or("系统代理端口无效")?;
    Ok(format!("{scheme}://{host}:{port}"))
}

/// `url`: see `environment`.
pub fn configure(command: &mut Command, url: &str) -> Result<()> {
    command.envs(environment(url)?);
    Ok(())
}

/// `url` is where the request goes; the URL itself is passed to curl in `args` or `input`.
pub fn curl(url: &str, args: &[&str], input: &[u8], timeout: Duration) -> Result<Vec<u8>> {
    pipe_command_with_environment(
        Path::new("/usr/bin/curl"),
        args,
        input,
        timeout,
        &environment(url)?,
    )
}

/// A request that provably never left this machine; any other transport failure may have been
/// received and acted on by the server.
pub const NOT_SENT: &str = "网络未连通，请求未发出；稍后自动重试";
/// Proxy settings that no request can use. As with `NOT_SENT`, nothing left this machine.
pub const PROXY_NOT_SENT: &str =
    "系统代理用不了（例如读不到 PAC 脚本），请求未发出；代理恢复后自动重试";
pub fn not_sent(message: &str) -> bool {
    matches!(message, NOT_SENT | PROXY_NOT_SENT)
}

/// Like `curl`, but keeps the exit status so a caller can classify a failed transfer.
pub fn curl_status(
    url: &str,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
) -> Result<(Option<i32>, Vec<u8>)> {
    curl_status_with(
        environment(url),
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

    fn no_pac(_: &str) -> Result<Vec<(String, String)>> {
        panic!("manual or explicit settings never run a PAC script")
    }

    #[test]
    fn gui_without_proxy_environment_uses_current_system_settings() {
        let env = resolve(|_| None, || Ok(SETTINGS.into()), no_pac).unwrap();
        assert!(env.contains(&("https_proxy".into(), "http://127.0.0.1:1082".into())));
        assert!(env.contains(&("all_proxy".into(), "socks5h://127.0.0.1:1082".into())));
        assert!(env.contains(&("no_proxy".into(), "localhost,.local,10.0.0.0/8".into())));
        let changed = resolve(|_| None, || Ok(SETTINGS.replace("1082", "12345")), no_pac).unwrap();
        assert!(changed.contains(&("https_proxy".into(), "http://127.0.0.1:12345".into())));
        assert!(
            resolve(|_| None, || Ok("<dictionary> {\n}".into()), no_pac)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn explicit_proxy_and_bypass_environment_are_preserved_and_written_out() {
        let own = |key: &'static str| move |k: &str| (k == key).then(|| format!("own-{k}"));
        for key in PROXY_KEYS {
            assert_eq!(
                resolve(own(key), || panic!("must not read system settings"), no_pac).unwrap(),
                vec![(key.to_string(), format!("own-{key}"))]
            );
        }
        // An explicit empty value still wins over the system proxy.
        assert_eq!(
            resolve(
                |k| (k == "https_proxy").then(String::new),
                || panic!(),
                no_pac
            )
            .unwrap(),
            vec![("https_proxy".to_string(), String::new())]
        );
        let env = resolve(own("NO_PROXY"), || Ok(SETTINGS.into()), no_pac).unwrap();
        assert!(!env.iter().any(|(key, _)| key == "no_proxy"));
        assert!(env.contains(&("NO_PROXY".into(), "own-NO_PROXY".into())));
        assert!(env.contains(&("https_proxy".into(), "http://127.0.0.1:1082".into())));
    }

    #[test]
    fn a_request_that_never_started_is_classified_as_not_sent() {
        let unusable = parse_system(
            &SETTINGS.replace("HTTPEnable : 1", "ProxyAutoDiscoveryEnable : 1"),
            no_pac,
        );
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
            // A script setting without a script address, and auto discovery.
            SETTINGS.replace("HTTPEnable : 1", "ProxyAutoConfigEnable : 1"),
            SETTINGS.replace("HTTPEnable : 1", "ProxyAutoDiscoveryEnable : 1"),
        ] {
            assert!(parse_system(&value, no_pac).is_err());
        }
        let env = parse_system(&SETTINGS.replace("127.0.0.1", "::1"), no_pac).unwrap();
        assert!(env.contains(&("https_proxy".into(), "http://[::1]:1082".into())));
    }

    #[test]
    fn pac_runs_ahead_of_manual_proxies_bypass_list_and_discovery() {
        let pac = SETTINGS.replace(
            " ExceptionsList",
            " ProxyAutoConfigEnable : 1\n ProxyAutoConfigURLString : http://127.0.0.1:10000/pac\n ProxyAutoDiscoveryEnable : 1\n ExceptionsList",
        );
        let routed = vec![(
            "https_proxy".to_string(),
            "http://127.0.0.1:7890".to_string(),
        )];
        let env = resolve(
            |_| None,
            || Ok(pac.clone()),
            |script| {
                assert_eq!(script, "http://127.0.0.1:10000/pac");
                Ok(routed.clone())
            },
        );
        assert_eq!(env, Ok(routed));
        // DIRECT from the script means no proxy at all, not the manual one.
        assert_eq!(
            resolve(|_| None, || Ok(pac.clone()), |_| Ok(vec![])),
            Ok(vec![])
        );
        // A script that gives nothing usable leaves the request unsent rather than direct.
        let failed = resolve(|_| None, || Ok(pac.clone()), |_| Err(crate::pac::NO_ROUTE));
        assert_eq!(failed, Err(crate::pac::NO_ROUTE));
        let unset = pac.replace(
            " ProxyAutoConfigURLString : http://127.0.0.1:10000/pac\n",
            "",
        );
        assert!(parse_system(&unset, no_pac).is_err());
    }

    #[test]
    fn pac_scripts_are_fetched_and_their_first_route_becomes_the_child_environment() {
        const API: &str = "https://api.anthropic.com/api/oauth/usage";
        let dir = std::env::temp_dir().join(format!("authmanager-pac-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            let text = format!("function FindProxyForURL(url, host) {{ {body} }}");
            std::fs::write(&path, text).unwrap();
            format!("file://{}", path.display())
        };
        let split = script(
            "split.pac",
            "return dnsDomainIs(host, \".example.org\") ? \"PROXY 127.0.0.1:7890; DIRECT\" : \"DIRECT\";",
        );
        assert_eq!(pac(&split, API), Ok(vec![]));
        let proxy = "http://127.0.0.1:7890".to_string();
        assert_eq!(
            pac(&split, "https://www.example.org/"),
            Ok(vec![
                ("http_proxy".into(), proxy.clone()),
                ("https_proxy".into(), proxy)
            ])
        );
        let socks = script("socks.pac", "return \"SOCKS 127.0.0.1:7891\";");
        assert_eq!(
            pac(&socks, API),
            Ok(vec![(
                "all_proxy".into(),
                "socks5h://127.0.0.1:7891".into()
            )])
        );
        let empty = script("empty.pac", "return \"\";");
        assert_eq!(pac(&empty, API), Err(crate::pac::NO_ROUTE));
        let throws = script("throws.pac", "throw new Error(\"x\");");
        assert_eq!(pac(&throws, API), Err(crate::pac::FAILED));
        let ipv6 = script("ipv6.pac", "return \"PROXY [::1]:7890\";");
        assert_ne!(pac(&ipv6, API), Ok(vec![]));
        let missing = format!("file://{}", dir.join("missing.pac").display());
        assert_eq!(pac(&missing, API), Err("读不到系统自动代理的 PAC 脚本"));
        std::fs::remove_dir_all(&dir).unwrap();
        // Served over http, as proxy apps do; fetched directly, never through a proxy.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).unwrap();
            let body = "function FindProxyForURL(url, host) { return \"PROXY 127.0.0.1:7890\"; }";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(format!("{head}{body}").as_bytes())
                .unwrap();
        });
        let served = pac(&format!("http://127.0.0.1:{port}/pac"), API).unwrap();
        server.join().unwrap();
        assert!(served.contains(&("https_proxy".into(), "http://127.0.0.1:7890".into())));
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
                url,
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
