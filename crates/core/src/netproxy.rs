//! Windows' own proxy setting, for what the app sends out.
//!
//! A PC behind a company proxy is set up in Windows' settings (Network &
//! Internet > Proxy), and every program that asks Windows goes through it.
//! The HTTP library under the app reads only the `HTTPS_PROXY` / `HTTP_PROXY`
//! environment variables, so on such a PC every update check, GitHub call
//! and limits reading went straight out and was stopped at the company's
//! edge.
//!
//! What stays direct whatever Windows says: this PC itself, the local
//! network, the tailnet, link-local addresses -- the app's own doors and the
//! machines it reaches over the tailnet are never behind the company's
//! proxy, and sent to it they would not be reached at all -- and whatever
//! Windows' own list of exceptions names (`<local>` there being any name
//! without a dot). A proxy given in the environment wins over Windows'
//! setting, as it always did. A setup script (PAC) is not read.

use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long Windows' setting is taken as read: it is asked for on every
/// request otherwise, and a change made in the settings reaches the app
/// within this long
const READ_FOR: Duration = Duration::from_secs(30);

/// Windows' proxy setting, as the parts that matter here
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Setting {
    /// `ProxyServer`: one `host:port` for everything, or
    /// `http=host:port;https=host:port;...` by scheme
    pub server: String,
    /// `ProxyOverride`: the exceptions, `;` between them
    pub bypass: String,
}

impl Setting {
    /// The proxy for a request of this scheme, as `host:port`
    fn server_for(&self, scheme: &str) -> Option<String> {
        let server = self.server.trim();
        if server.is_empty() {
            return None;
        }
        if !server.contains('=') {
            return Some(strip_scheme(server).to_string());
        }
        let by = |name: &str| {
            server.split(';').find_map(|part| {
                let (k, v) = part.split_once('=')?;
                (k.trim().eq_ignore_ascii_case(name) && !v.trim().is_empty()).then(|| strip_scheme(v.trim()).to_string())
            })
        };
        // Each scheme to the proxy named for it, and nowhere else: a proxy
        // named for http alone leaves https going direct, as Windows' own
        // programs read the setting
        match scheme {
            "https" => by("https"),
            _ => by("http"),
        }
    }

    /// Whether Windows' list of exceptions names this host
    fn bypasses(&self, host: &str) -> bool {
        let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
        self.bypass.split(';').map(str::trim).filter(|p| !p.is_empty()).any(|pattern| {
            if pattern.eq_ignore_ascii_case("<local>") {
                return !host.contains('.') && !host.contains(':');
            }
            // Written with a scheme or a port at times; the host is what is
            // compared
            let pattern = strip_scheme(pattern).to_ascii_lowercase();
            let pattern = match pattern.rsplit_once(':') {
                Some((h, port)) if !h.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => h.to_string(),
                _ => pattern,
            };
            wildcard(&pattern, &host)
        })
    }
}

fn strip_scheme(s: &str) -> &str {
    s.split_once("://").map_or(s, |(_, rest)| rest).trim_end_matches('/')
}

/// `*` standing for any run of characters, the way Windows reads its list
fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&b| b == b'*')
}

/// A destination never sent through a proxy: this PC, the local network,
/// the tailnet (100.64.0.0/10 and its `.ts.net` names), link-local and
/// `.local` names
pub fn always_direct(host: &str) -> bool {
    let host = host.trim_matches(['[', ']']).trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host.ends_with(".ts.net") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let [a, b, ..] = ip.octets();
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() || (a == 100 && (64..128).contains(&b))
        }
        Ok(IpAddr::V6(ip)) => {
            let first = ip.segments()[0];
            ip.is_loopback()
                || ip.is_unspecified()
                || (first & 0xffc0) == 0xfe80
                || (first & 0xfe00) == 0xfc00
                || ip.to_ipv4_mapped().is_some_and(|v4| always_direct(&v4.to_string()))
        }
        Err(_) => false,
    }
}

/// The proxy a request to this address goes through, by this setting:
/// `None` for one that goes direct
pub fn proxy_by(setting: &Setting, url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let host = if authority.starts_with('[') {
        authority.split_once(']').map_or(authority, |(h, _)| &h[1..])
    } else {
        authority.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map_or(authority, |(h, _)| h)
    };
    if host.is_empty() || always_direct(host) || setting.bypasses(host) {
        return None;
    }
    setting.server_for(&scheme)
}

/// Windows' setting as it is now, when a proxy is switched on there
#[cfg(windows)]
fn read_setting() -> Option<Setting> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    let on: u32 = key.get_value("ProxyEnable").ok()?;
    if on == 0 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    let bypass: String = key.get_value("ProxyOverride").unwrap_or_default();
    Some(Setting { server, bypass })
}

#[cfg(not(windows))]
fn read_setting() -> Option<Setting> {
    None
}

/// Windows' setting, read again only every [`READ_FOR`]
fn setting() -> Option<Setting> {
    static READ: Mutex<Option<(Instant, Option<Setting>)>> = Mutex::new(None);
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    match &*read {
        Some((at, s)) if at.elapsed() < READ_FOR => s.clone(),
        _ => {
            let s = read_setting();
            *read = Some((Instant::now(), s.clone()));
            s
        }
    }
}

/// Whether a proxy is given in the environment: then it is the one used,
/// by the library itself, with `NO_PROXY` beside it
fn given_in_environment() -> bool {
    ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()))
}

/// The proxy of Windows' setting a request to this address goes through,
/// for the HTTP library; `None` when it goes direct, or when the
/// environment names a proxy (the library then uses that one itself)
pub fn for_url(url: &str) -> Option<ureq::Proxy> {
    if given_in_environment() {
        return None;
    }
    let server = proxy_by(&setting()?, url)?;
    match ureq::Proxy::new(&format!("http://{server}")) {
        Ok(proxy) => Some(proxy),
        Err(e) => {
            crate::append_hook_log(&format!("proxy: Windows' proxy setting could not be used ({server}): {e}"));
            None
        }
    }
}

/// An agent going out through Windows' proxy where its setting says to
pub trait Outward {
    /// Set to go through Windows' proxy for requests to this address -- the
    /// one the agent is for. Left as it was (the environment's proxy, or
    /// none) when the address goes direct
    fn outward(self, url: &str) -> Self;
}

impl Outward for ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    fn outward(self, url: &str) -> Self {
        match for_url(url) {
            Some(proxy) => self.proxy(Some(proxy)),
            None => self,
        }
    }
}

/// Which way a request to this address goes, as a key for keeping one
/// agent per way: the proxy's address, or empty for direct
pub fn way_of(url: &str) -> String {
    if given_in_environment() {
        return String::new();
    }
    setting().and_then(|s| proxy_by(&s, url)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(server: &str, bypass: &str) -> Setting {
        Setting { server: server.into(), bypass: bypass.into() }
    }

    /// What the app reaches on this PC, the local network and the tailnet
    /// never goes to the company's proxy, whatever Windows says
    #[test]
    fn the_apps_own_doors_and_the_tailnet_stay_direct() {
        let s = set("proxy.example.com:8080", "");
        for url in [
            "http://127.0.0.1:8788/api",
            "http://localhost:3000/",
            "http://[::1]:80/",
            "http://192.168.1.20/",
            "http://10.0.0.5:9000/x",
            "http://172.20.1.1/",
            "http://100.101.102.103:8788/",
            "https://pc.tail1234.ts.net/",
            "http://printer.local/",
            "http://169.254.1.1/",
            "http://[fd7a:115c:a1e0::1]:8788/",
            "http://[fe80::1]/",
        ] {
            assert_eq!(proxy_by(&s, url), None, "{url} went to the proxy");
        }
        for url in ["https://api.github.com/user", "http://example.com/", "https://100.128.0.1/", "https://172.32.0.1/"] {
            assert_eq!(proxy_by(&s, url).as_deref(), Some("proxy.example.com:8080"), "{url} went direct");
        }
    }

    /// The proxy named for a request's scheme, the way Windows writes it
    #[test]
    fn a_proxy_named_by_scheme_is_used_for_that_scheme() {
        let s = set("http=web:80;https=secure:443;ftp=files:21", "");
        assert_eq!(proxy_by(&s, "https://api.github.com/").as_deref(), Some("secure:443"));
        assert_eq!(proxy_by(&s, "http://example.com/").as_deref(), Some("web:80"));
        let only_http = set("http=web:80", "");
        assert_eq!(proxy_by(&only_http, "https://api.github.com/"), None, "a proxy named for http was used for https");
        assert_eq!(proxy_by(&only_http, "http://example.com/").as_deref(), Some("web:80"));
        let only_socks = set("socks=s:1080", "");
        assert_eq!(proxy_by(&only_socks, "https://api.github.com/"), None, "a SOCKS proxy is not one this library speaks here");
        assert_eq!(proxy_by(&set("http://proxy:3128/", ""), "https://a.example/").as_deref(), Some("proxy:3128"));
        assert_eq!(proxy_by(&set("", ""), "https://a.example/"), None);
    }

    /// Windows' own exceptions are kept: names, wildcards, `<local>`
    #[test]
    fn windows_exceptions_go_direct() {
        let s = set("proxy:8080", "*.corp.example;git.internal;<local>;203.0.113.*;https://docs.example:443");
        for url in [
            "https://code.corp.example/api",
            "https://GIT.internal/x",
            "http://intranet/",
            "http://203.0.113.9/",
            "https://docs.example/",
        ] {
            assert_eq!(proxy_by(&s, url), None, "{url} went to the proxy");
        }
        for url in ["https://corp.example.com/", "https://api.github.com/", "http://203.0.114.9/"] {
            assert!(proxy_by(&s, url).is_some(), "{url} went direct");
        }
        assert!(wildcard("*", "anything") && wildcard("a*c", "abbbc") && !wildcard("a*c", "abbb"));
    }

    /// The proxy Windows names is the one the request is sent to
    #[test]
    fn a_request_goes_to_the_proxy_windows_names() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let heard = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            // One request, read whole: what was asked through the proxy
            let read_one = |conn: &mut std::net::TcpStream| {
                let mut asked = Vec::new();
                let mut buf = [0u8; 2048];
                while !asked.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = conn.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    asked.extend_from_slice(&buf[..n]);
                }
                String::from_utf8_lossy(&asked).to_string()
            };
            // Asked to open a way through, the way an HTTP proxy is; then
            // the request itself comes down it
            let opened = read_one(&mut conn);
            conn.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
            let asked = read_one(&mut conn);
            conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
            conn.flush().unwrap();
            let _ = conn.shutdown(std::net::Shutdown::Write);
            let _ = conn.read(&mut [0u8; 64]);
            (opened, asked)
        });
        let s = set(&format!("http=127.0.0.1:{port}"), "");
        let url = "http://far.example.invalid/check";
        let server = proxy_by(&s, url).unwrap();
        let agent = ureq::Agent::config_builder()
            .proxy(Some(ureq::Proxy::new(&format!("http://{server}")).unwrap()))
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .new_agent();
        let body = agent.get(url).call().unwrap().body_mut().read_to_string().unwrap();
        assert_eq!(body, "ok");
        let (opened, asked) = heard.join().unwrap();
        assert!(opened.starts_with("CONNECT far.example.invalid:80 "), "{opened}");
        assert!(asked.starts_with("GET /check "), "{asked}");
    }

    /// Who and port in an address are not taken for its host
    #[test]
    fn the_host_is_read_from_the_whole_address() {
        let s = set("proxy:8080", "");
        assert_eq!(proxy_by(&s, "http://user:pw@127.0.0.1:99/"), None);
        assert!(proxy_by(&s, "https://127.0.0.1.example.com/").is_some());
        assert_eq!(proxy_by(&s, "not an address"), None);
    }
}
