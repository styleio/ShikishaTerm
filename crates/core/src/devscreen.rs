//! What the two doors a DevTools screen connects through agree on.
//!
//! The browser's own DevTools screen (`devtools://devtools/...`) talks the
//! DevTools protocol over a WebSocket it is given the address of. This
//! program gives it a door of its own on the loopback, never the browser's
//! port: one in the window (`src/devtools.rs`, for a page the window draws)
//! and one beside the server's browser (`chrome.rs`). Both let in only that
//! screen, and both hold to the same limits, which are written once here

/// The one origin a connection is answered from
pub const ORIGIN: &str = "devtools://devtools";

/// The most the screen may say in one message. What it sends are commands --
/// a node to inspect, an expression to run -- and a command larger than
/// this is a script pasted into the console, which still fits
pub const ONE_COMMAND: usize = 8 * 1024 * 1024;

/// How far behind a screen may fall before it is let go. A page that says a
/// great deal to a screen that has stopped reading -- a tab asleep, a line
/// gone slow -- would otherwise be kept in memory without end
pub const BEHIND: usize = 64 * 1024 * 1024;

/// How long a screen may take nothing of what is written to it before it is
/// taken for gone
pub const STALLED: std::time::Duration = std::time::Duration::from_secs(30);

/// What becomes of something a screen asks
#[derive(Debug, PartialEq, Eq)]
pub enum Asked {
    /// Passed on to the page
    Pass,
    /// Answered here with nothing, as if done: the screen asks it as a
    /// matter of course and goes on without what it would have brought
    Nothing,
    /// Refused: it reaches past the page the screen was opened for
    Refused,
}

/// Whether a screen may ask `method`, with these parameters, of the page
/// it was opened for. `own` says whether a session it names is one of the
/// screen's own (a frame's, a worker's), and `target` is the page's own id
/// when the door knows it.
///
/// A screen is for one page. The protocol it speaks is not: from a page's
/// session it can list every page in the browser, attach to any of them,
/// open new ones, close them, stop the browser, read every site's cookies
/// and change what any site is allowed. What does that is refused here, by
/// name. Two domains are taken the other way round, since the whole of
/// each is about more than the page: of `Target`, only what a screen needs
/// to follow its own page's frames and workers is let through; of
/// `Browser`, only being asked its version.
///
/// What this does not do, and cannot: keep the screen from what its own
/// page can reach. It may send its page anywhere and read what the page
/// holds there, as the person at the page could -- that is what inspecting
/// a page is
pub fn asked(method: &str, params: &serde_json::Value, target: Option<&str>, own: &dyn Fn(&str) -> bool) -> Asked {
    let named = |k: &str| params.get(k).and_then(|v| v.as_str());
    match method {
        // The page's own frames and workers, attached as they appear
        "Target.setAutoAttach" | "Target.autoAttachRelated" => Asked::Pass,
        // About itself; about another page, never
        "Target.getTargetInfo" => match named("targetId") {
            None => Asked::Pass,
            Some(t) if target == Some(t) => Asked::Pass,
            Some(_) => Asked::Refused,
        },
        "Target.detachFromTarget" => match named("sessionId") {
            Some(s) if own(s) => Asked::Pass,
            _ => Asked::Refused,
        },
        // Being told of every page there is: asked by every screen as it
        // opens, and answered as done with nothing told
        "Target.setDiscoverTargets" => Asked::Nothing,
        m if m.starts_with("Target.") => Asked::Refused,
        // The browser as a whole is not the page's: closing it, where its
        // downloads go, what every site is allowed
        "Browser.getVersion" => Asked::Pass,
        m if m.starts_with("Browser.") => Asked::Refused,
        // Every site's cookies, read or written at a stroke. The page's own
        // are asked for another way (`Network.getCookies`, see `trim`)
        "Network.getAllCookies" | "Network.clearBrowserCookies" | "Storage.getCookies" | "Storage.setCookies" | "Storage.clearCookies" => Asked::Refused,
        _ => Asked::Pass,
    }
}

/// One place a message would change what the browser keeps for
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Site {
    /// A cookie's: a host name. A page at that host, or at a host under it,
    /// holds such a cookie (one for `example.com` is held by the page at
    /// `shop.example.com`)
    Cookie(String),
    /// A stored area's (local storage, a database): a scheme, a host and a
    /// port together. Kept by exactly that, and never by a host above or
    /// beside it
    Origin(String),
}

/// The places a message would change what the browser keeps for -- a cookie
/// set or deleted, a site's storage cleared. `None` for a message that
/// changes nothing of the kind. A place that cannot be told from the
/// message is the empty one, which is no page's.
///
/// The protocol takes any address here: a screen on one page could set a
/// cookie for a site the page has nothing to do with (a session of
/// somebody else's choosing, waiting for the person to sign in to it), or
/// delete one. A door that knows where its page is lets these through for
/// the page's own places only ([`within`])
pub fn sites_written(method: &str, params: &serde_json::Value) -> Option<Vec<Site>> {
    let text = |v: &serde_json::Value, k: &str| v.get(k).and_then(|s| s.as_str()).unwrap_or_default().to_string();
    // A cookie may name an address, a domain, or both -- and the browser
    // goes by the domain when it is given one. Every one named is a place
    // it is for; naming none is naming no place
    let cookie = |c: &serde_json::Value| -> Vec<Site> {
        let mut places: Vec<Site> = Vec::new();
        if c.get("url").is_some() {
            places.push(Site::Cookie(host_of(&text(c, "url"))));
        }
        if c.get("domain").is_some() {
            places.push(Site::Cookie(text(c, "domain").trim_start_matches('.').to_ascii_lowercase()));
        }
        match places.is_empty() {
            true => vec![Site::Cookie(String::new())],
            false => places,
        }
    };
    match method {
        "Network.setCookie" | "Network.deleteCookies" => Some(cookie(params)),
        "Network.setCookies" => Some(params.get("cookies").and_then(|c| c.as_array()).map(|c| c.iter().flat_map(cookie).collect()).unwrap_or_default()),
        "Storage.clearDataForOrigin" => Some(vec![Site::Origin(origin_of(&text(params, "origin")))]),
        // A key is an origin, and after a `^` what it is kept apart by
        "Storage.clearDataForStorageKey" => Some(vec![Site::Origin(origin_of(text(params, "storageKey").split('^').next().unwrap_or_default()))]),
        _ => None,
    }
}

/// The scheme, host and port an address names, written one way (small
/// letters, the scheme's own port left out); empty when it names none
pub fn origin_of(address: &str) -> String {
    let Some((scheme, rest)) = address.split_once("://") else { return String::new() };
    let scheme = scheme.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let at = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let port = match at.strip_prefix('[') {
        Some(v6) => v6.split_once(']').and_then(|(_, after)| after.strip_prefix(':')),
        None => at.split_once(':').map(|(_, port)| port),
    };
    let host = host_of(address);
    if host.is_empty() {
        return String::new();
    }
    let host = if host.contains(':') { format!("[{host}]") } else { host };
    let usual = match scheme.as_str() {
        "http" | "ws" => "80",
        "https" | "wss" => "443",
        _ => "",
    };
    match port.filter(|p| !p.is_empty() && *p != usual) {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }
}

/// The host an address names, in small letters; empty when it names none
/// -- or names one in any way but the plain one.
///
/// Read here to decide what a browser is then asked to do with the same
/// address, and a browser reads more ways than one: a backslash is a slash
/// to it, a name and password may stand before the host, letters may be
/// written as percent signs, and spaces and line ends inside an address are
/// taken out. An address using any of these is one this reading and the
/// browser's could disagree on -- so it names no host here, and what was
/// asked for it is refused. What a DevTools screen sends is always plain
pub fn host_of(address: &str) -> String {
    let Some((scheme, rest)) = address.split_once("://") else { return String::new() };
    let plain = |c: char| c.is_ascii_graphic() && c != '\\';
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) || !address.chars().all(plain) {
        return String::new();
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let (host, port) = match authority.strip_prefix('[') {
        // An IPv6 address, in its brackets
        Some(v6) => match v6.split_once(']') {
            Some((host, after)) if host.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.')) => (host, after),
            _ => return String::new(),
        },
        None => match authority.split_once(':') {
            Some((host, port)) => (host, &authority[authority.len() - port.len() - 1..]),
            None => (authority, ""),
        },
    };
    let named = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'));
    let numbered = port.is_empty() || port.strip_prefix(':').is_some_and(|p| p.chars().all(|c| c.is_ascii_digit()));
    match named && numbered {
        true => host.trim_end_matches('.').to_ascii_lowercase(),
        false => String::new(),
    }
}

/// Whether every one of `sites` is a place of a page whose frames are at
/// the addresses `frames` (see [`Site`] for what each kind is held by)
pub fn within(sites: &[Site], frames: &[String]) -> bool {
    let held = |site: &Site| match site {
        Site::Cookie(host) => !host.is_empty() && frames.iter().map(|f| host_of(f)).any(|f| f == *host || f.ends_with(&format!(".{host}"))),
        Site::Origin(origin) => !origin.is_empty() && frames.iter().any(|f| origin_of(f) == *origin),
    };
    !sites.is_empty() && sites.iter().all(held)
}

/// A message from a screen that writes into the browser's keeping, for a
/// door that learns where the page is only after the message is read: the
/// places it writes for, and what the screen is answered when the page is
/// not at them. `None` for a message that writes nothing of the kind
pub fn written_by(text: &str) -> Option<(Vec<Site>, String)> {
    let v = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let sites = sites_written(v.get("method")?.as_str()?, v.get("params").unwrap_or(&serde_json::Value::Null))?;
    let mut answer = refusal(v.get("id").unwrap_or(&serde_json::Value::Null));
    if let (Some(session), Some(o)) = (v.get("sessionId"), answer.as_object_mut()) {
        o.insert("sessionId".into(), session.clone());
    }
    Some((sites, answer.to_string()))
}

/// Cut what a screen asks down to its own page, where the same ask can be
/// made of more: true when something was taken out.
///
/// `Network.getCookies` answers for the page and its frames when it names
/// no address, and for any address it is given -- the cookies of a site the
/// page has nothing to do with, the ones kept from scripts included. The
/// addresses are taken out, so it answers for the page
pub fn trim(method: &str, params: &mut serde_json::Value) -> bool {
    match (method, params.as_object_mut()) {
        ("Network.getCookies", Some(p)) => p.remove("urls").is_some(),
        _ => false,
    }
}

/// What a door that passes a screen's words on as they are does with one
/// message
#[derive(Debug, PartialEq, Eq)]
pub enum Said {
    /// Passed on
    On,
    /// Passed on as this, cut down to the page (see [`trim`])
    Trimmed(String),
    /// Not passed on: answered with this instead
    Instead(String),
    /// Not a message at all
    Dropped,
}

/// One message from a screen, judged by [`asked`], for a door that passes
/// messages on whole. The answer given instead carries the session the
/// message named, so the screen files it where it asked from
pub fn said(text: &str, target: Option<&str>, own: &dyn Fn(&str) -> bool) -> Said {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(text) else { return Said::Dropped };
    let Some(method) = v.get("method").and_then(|m| m.as_str()).map(str::to_string) else { return Said::Dropped };
    let id = v.get("id").cloned().unwrap_or_default();
    let mut answer = match asked(&method, v.get("params").unwrap_or(&serde_json::Value::Null), target, own) {
        Asked::Pass => {
            return match v.get_mut("params").is_some_and(|p| trim(&method, p)) {
                true => Said::Trimmed(v.to_string()),
                false => Said::On,
            };
        }
        Asked::Nothing => serde_json::json!({ "id": id, "result": {} }),
        Asked::Refused => refusal(&id),
    };
    if let Some(through) = v.get("sessionId") {
        answer["sessionId"] = through.clone();
    }
    Said::Instead(answer.to_string())
}

/// How many connections a door serves at once. A person has a screen or two
/// open; whoever opens more than this is not opening screens
pub const AT_ONCE: usize = 32;

/// The connections a door is serving, counted: anybody on the machine can
/// knock, and past [`AT_ONCE`] the rest are not answered
#[derive(Clone, Default)]
pub struct Serving(std::sync::Arc<std::sync::atomic::AtomicUsize>);

/// One connection being served; counted out again when let go of
pub struct Served(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Serving {
    /// Count one more in, or `None` when there is no room for it
    pub fn enter(&self) -> Option<Served> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.0.fetch_add(1, SeqCst) >= AT_ONCE {
            self.0.fetch_sub(1, SeqCst);
            return None;
        }
        Some(Served(self.0.clone()))
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The answer a refused ask is given, under the number it was asked by
pub fn refusal(id: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "id": id, "error": { "code": -32000, "message": "Not allowed from a DevTools screen opened for one page." } })
}

/// How much a screen may say before its page has a session for it. It says
/// a handful of commands as it opens; more than this is not a screen opening
pub const EARLY: usize = 16 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    /// What a screen writes into the browser's keeping is for its page's
    /// own sites
    #[test]
    fn what_a_screen_writes_is_for_its_pages_own_sites() {
        let frames = ["https://shop.example.com/cart?x=1".to_string(), "https://pay.example.net:8443/frame".to_string()];
        let written = |method: &str, params: serde_json::Value| sites_written(method, &params).map(|sites| within(&sites, &frames));
        // Not a write at all
        assert_eq!(written("Network.getCookies", serde_json::json!({})), None);
        assert_eq!(written("Runtime.evaluate", serde_json::json!({"expression": "document.cookie = 'a=b'"})), None);
        // The page's own, and a frame's
        assert_eq!(written("Network.setCookie", serde_json::json!({"name": "a", "value": "b", "url": "https://shop.example.com/cart"})), Some(true));
        assert_eq!(written("Network.setCookie", serde_json::json!({"name": "a", "value": "b", "domain": ".example.com"})), Some(true));
        assert_eq!(written("Network.deleteCookies", serde_json::json!({"name": "a", "domain": "pay.example.net"})), Some(true));
        assert_eq!(written("Storage.clearDataForOrigin", serde_json::json!({"origin": "https://SHOP.example.com:443", "storageTypes": "all"})), Some(true));
        assert_eq!(written("Storage.clearDataForOrigin", serde_json::json!({"origin": "https://pay.example.net:8443"})), Some(true));
        assert_eq!(written("Storage.clearDataForStorageKey", serde_json::json!({"storageKey": "https://shop.example.com/^0https://top.example"})), Some(true));
        // A cookie that names an address of the page's and a domain of
        // somebody else's is for somebody else: the browser goes by the domain
        assert_eq!(
            written("Network.setCookie", serde_json::json!({"name": "sid", "value": "x", "url": "https://shop.example.com/", "domain": "bank.example.org"})),
            Some(false)
        );
        assert_eq!(
            written("Network.deleteCookies", serde_json::json!({"name": "sid", "url": "https://shop.example.com/", "domain": ".bank.example.org"})),
            Some(false)
        );
        assert_eq!(written("Network.setCookie", serde_json::json!({"name": "a", "value": "b", "url": "https://shop.example.com/", "domain": ".example.com"})), Some(true));
        // What is stored is kept by scheme, host and port: not by the
        // domain above, another port, or the other scheme
        for origin in ["https://example.com", "https://shop.example.com:8443", "http://shop.example.com", "https://pay.example.net", "https://other.shop.example.com"] {
            assert_eq!(written("Storage.clearDataForOrigin", serde_json::json!({"origin": origin})), Some(false), "{origin}");
        }
        assert_eq!(origin_of("HTTPS://Shop.Example.com:443/x"), "https://shop.example.com");
        assert_eq!(origin_of("http://[::1]:8080/x"), "http://[::1]:8080");
        assert_eq!(origin_of("about:blank"), "");
        // A message that writes is told apart, with its answer ready
        let (sites, answer) = written_by(r#"{"id":7,"sessionId":"S","method":"Network.setCookie","params":{"name":"a","value":"b","domain":"bank.example.org"}}"#).unwrap();
        assert_eq!(sites, [Site::Cookie("bank.example.org".into())]);
        let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
        assert_eq!((answer["id"].as_u64(), answer["sessionId"].as_str(), answer.get("error").is_some()), (Some(7), Some("S"), true));
        assert_eq!(written_by(r#"{"id":8,"method":"Runtime.evaluate","params":{}}"#), None);
        // Another site's, by any of the roads
        assert_eq!(written("Network.setCookie", serde_json::json!({"name": "sid", "value": "x", "url": "https://bank.example.org/"})), Some(false));
        assert_eq!(written("Network.setCookie", serde_json::json!({"name": "sid", "value": "x", "domain": "bank.example.org"})), Some(false));
        assert_eq!(written("Network.deleteCookies", serde_json::json!({"name": "sid", "url": "https://bank.example.org/"})), Some(false));
        assert_eq!(written("Storage.clearDataForOrigin", serde_json::json!({"origin": "https://bank.example.org", "storageTypes": "cookies"})), Some(false));
        assert_eq!(written("Storage.clearDataForStorageKey", serde_json::json!({"storageKey": "https://bank.example.org/"})), Some(false));
        // One of several is enough to refuse them all; a site that cannot
        // be told is nobody's; a name that only ends like the page's is not it
        assert_eq!(
            written("Network.setCookies", serde_json::json!({"cookies": [{"name": "a", "value": "b", "url": "https://shop.example.com/"}, {"name": "sid", "value": "x", "domain": "bank.example.org"}]})),
            Some(false)
        );
        assert_eq!(written("Network.setCookies", serde_json::json!({"cookies": []})), Some(false));
        assert_eq!(written("Network.setCookie", serde_json::json!({"name": "a", "value": "b"})), Some(false));
        assert_eq!(written("Network.setCookie", serde_json::json!({"name": "a", "value": "b", "domain": "notshop.example.com.evil.test"})), Some(false));
        assert_eq!(written("Network.setCookie", serde_json::json!({"name": "a", "value": "b", "domain": "ample.com"})), Some(false));
        assert_eq!(host_of("https://[::1]:8080/x"), "::1");
        assert_eq!(host_of("HTTPS://Shop.Example.com./x?y#z"), "shop.example.com");
        assert_eq!(host_of("about:blank"), "");
        // An address a browser may read another way names no host here,
        // and what is asked for it is refused: a backslash it takes for a
        // slash, a name before the host, percent signs, spaces
        for odd in [
            "https://bank.example.org\\@shop.example.com/",
            "https://user:pw@shop.example.com/",
            "https://shop.example.com@bank.example.org/",
            "https://shop.example%2ecom/",
            "https://shop.example.com /",
            "https://shop.example.com\t/",
            "https://shop.example.com:80a/",
            "https://[::1/",
            "https:///shop.example.com/",
            "://shop.example.com/",
        ] {
            assert_eq!(host_of(odd), "", "{odd:?}");
            assert_eq!(origin_of(odd), "", "{odd:?}");
            assert_eq!(written("Network.setCookie", serde_json::json!({"name": "sid", "value": "x", "url": odd})), Some(false), "{odd:?}");
        }
    }

    /// A screen follows its own page and its frames, and reaches no other
    #[test]
    fn a_screen_is_refused_what_reaches_past_its_page() {
        let own = |s: &str| s == "CHILD";
        let ask = |method: &str, params: serde_json::Value| asked(method, &params, Some("PAGE"), &own);
        let none = serde_json::json!({});
        // The everyday talk of a screen
        for m in ["Runtime.evaluate", "DOM.getDocument", "Page.enable", "Network.enable", "Browser.getVersion", "Emulation.setDeviceMetricsOverride"] {
            assert_eq!(ask(m, none.clone()), Asked::Pass, "{m}");
        }
        assert_eq!(ask("Target.setAutoAttach", serde_json::json!({"autoAttach": true, "flatten": true})), Asked::Pass);
        assert_eq!(ask("Target.getTargetInfo", none.clone()), Asked::Pass);
        assert_eq!(ask("Target.getTargetInfo", serde_json::json!({"targetId": "PAGE"})), Asked::Pass);
        assert_eq!(ask("Target.detachFromTarget", serde_json::json!({"sessionId": "CHILD"})), Asked::Pass);
        assert_eq!(ask("Target.setDiscoverTargets", serde_json::json!({"discover": true})), Asked::Nothing);
        // Another page, by any road
        assert_eq!(ask("Target.attachToTarget", serde_json::json!({"targetId": "OTHER", "flatten": true})), Asked::Refused);
        assert_eq!(ask("Target.attachToTarget", serde_json::json!({"targetId": "PAGE"})), Asked::Refused, "a second session on its own page is asked for by nothing a screen does");
        assert_eq!(ask("Target.getTargetInfo", serde_json::json!({"targetId": "OTHER"})), Asked::Refused);
        assert_eq!(ask("Target.detachFromTarget", serde_json::json!({"sessionId": "SOMEBODY"})), Asked::Refused);
        for m in ["Target.getTargets", "Target.createTarget", "Target.closeTarget", "Target.attachToBrowserTarget", "Target.exposeDevToolsProtocol", "Target.sendMessageToTarget", "Target.createBrowserContext", "Target.somethingNew"] {
            assert_eq!(ask(m, none.clone()), Asked::Refused, "{m}");
        }
        // With the page's id not known, no page may be named at all
        assert_eq!(asked("Target.getTargetInfo", &serde_json::json!({"targetId": "PAGE"}), None, &own), Asked::Refused);
        // The browser itself, and what belongs to every site at once
        for m in ["Browser.close", "Browser.crash", "Browser.setDownloadBehavior", "Browser.setPermission", "Browser.grantPermissions", "Browser.somethingNew"] {
            assert_eq!(ask(m, none.clone()), Asked::Refused, "{m}");
        }
        for m in ["Network.getAllCookies", "Network.clearBrowserCookies", "Storage.getCookies", "Storage.setCookies", "Storage.clearCookies"] {
            assert_eq!(ask(m, none.clone()), Asked::Refused, "{m}");
        }
        // The page's own cookies are asked for, and only the page's
        assert_eq!(ask("Network.getCookies", serde_json::json!({"urls": ["https://bank.example/"]})), Asked::Pass);
        let mut params = serde_json::json!({"urls": ["https://bank.example/"]});
        assert!(trim("Network.getCookies", &mut params));
        assert_eq!(params, serde_json::json!({}), "another site's cookies were still asked for");
        let mut params = serde_json::json!({});
        assert!(!trim("Network.getCookies", &mut params));
        let mut params = serde_json::json!({"urls": ["x"]});
        assert!(!trim("Runtime.evaluate", &mut params));
    }

    /// A door that passes messages on whole answers the ones it does not,
    /// under their own number and session
    #[test]
    fn a_message_not_passed_on_is_answered_where_it_was_asked_from() {
        let own = |_: &str| true;
        assert_eq!(said(r#"{"id":1,"method":"Runtime.evaluate","params":{"expression":"1"}}"#, None, &own), Said::On);
        let Said::Instead(answer) = said(r#"{"id":7,"sessionId":"W1","method":"Target.attachToTarget","params":{"targetId":"X"}}"#, None, &own) else {
            panic!("reaching another page was passed on");
        };
        let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["sessionId"], "W1");
        assert!(answer.get("error").is_some(), "{answer}");
        let Said::Instead(answer) = said(r#"{"id":8,"method":"Target.setDiscoverTargets","params":{"discover":true}}"#, None, &own) else {
            panic!("not answered");
        };
        assert_eq!(serde_json::from_str::<serde_json::Value>(&answer).unwrap(), serde_json::json!({"id": 8, "result": {}}));
        // Asked for another site's cookies: passed on asking for the page's
        let Said::Trimmed(passed) = said(r#"{"id":9,"method":"Network.getCookies","params":{"urls":["https://bank.example/"]}}"#, None, &own) else {
            panic!("another site's cookies were asked for as they were");
        };
        assert_eq!(serde_json::from_str::<serde_json::Value>(&passed).unwrap(), serde_json::json!({"id": 9, "method": "Network.getCookies", "params": {}}));
        assert_eq!(said(r#"{"id":10,"method":"Network.getCookies","params":{}}"#, None, &own), Said::On);
        assert_eq!(said("not json", None, &own), Said::Dropped);
        assert_eq!(said(r#"{"id":9}"#, None, &own), Said::Dropped);
    }

    /// A door serves so many at once, and has room again when one goes
    #[test]
    fn a_door_serves_only_so_many_at_once() {
        let door = Serving::default();
        let held: Vec<Served> = (0..AT_ONCE).map(|_| door.enter().expect("room")).collect();
        assert!(door.enter().is_none(), "one more than the most was let in");
        drop(held);
        assert!(door.enter().is_some(), "no room after they left");
    }
}
