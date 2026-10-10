//! A web page's notifications, shown as this program's own.
//!
//! A page in a browser tab that calls `new Notification(...)` reaches the
//! script the window puts in its place (`SHIM`, before the page's own): the
//! notification comes here, and is shown the way this program shows the tab
//! that is waiting -- under its name, in the same Notification Centre, and a
//! press on it brings that tab forward. Neither engine's own notification is
//! used: Chromium's would come from a helper app with another name, and
//! WebView2's from a shell of its own; both are refused where the page asks
//! the engine (src/browser/cef_engine.rs, src/browser/webview2.rs).
//!
//! A site may show them once the person has said yes, asked the first time
//! the site asks (`Notification.requestPermission`) and remembered by site
//! from then on. The answers are listed in Settings, where one can be taken
//! back. One list for the whole program, not one per browser profile: the
//! question is whether this person wants to hear from that site, which does
//! not change with who is signed in to it.
//!
//! What a page can say is cut short and cleaned here, and the site's name is
//! put in front by this program: a page decides the words, never whose they
//! look like.
//!
//! What the script cannot stand in for: a service worker's notification
//! (`registration.showNotification`), which runs where no page's script is.
//! The engines refuse those, so a site that only notifies from its service
//! worker is silent here.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// The longest a notification's title and words may be, in characters. A
/// banner shows about one line of title and two of words before the system
/// cuts it; anything past that is a page filling the Notification Centre
pub const TITLE_MAX: usize = 100;
pub const BODY_MAX: usize = 300;

/// The site an address belongs to, as it is filed: `https://host` or
/// `http://host:port`, the port only when it is not the scheme's own. `None`
/// for an address that is not a web page (the program's own pages are
/// filtered out by the window before they get here)
pub fn site(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let usual = match scheme.as_str() {
        "https" => 443,
        "http" => 80,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    // Whoever is signed in by the address is not part of the site
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, after) = v6.split_once(']')?;
        (format!("[{}]", h.to_ascii_lowercase()), after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_ascii_lowercase(), Some(p)),
            None => (authority.to_ascii_lowercase(), None),
        }
    };
    if host.is_empty() || host == "[]" {
        return None;
    }
    let port: u16 = match port {
        Some(p) if !p.is_empty() => p.parse().ok()?,
        _ => usual,
    };
    Some(if port == usual { format!("{scheme}://{host}") } else { format!("{scheme}://{host}:{port}") })
}

/// The site as a person reads it: its host, with the port when there is one
pub fn shown(site: &str) -> &str {
    site.split_once("://").map_or(site, |(_, h)| h)
}

/// A page's words made fit for a banner: control characters gone, and the
/// characters that turn the direction text is shown in (which can make words
/// read in another order than they are written), runs of space made one, and
/// no longer than `max` characters
pub fn clean(text: &str, max: usize) -> String {
    let turns = |c: char| matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    let flat: String = text.chars().map(|c| if c.is_control() || turns(c) { ' ' } else { c }).collect();
    let words: Vec<&str> = flat.split_whitespace().collect();
    let joined = words.join(" ");
    if joined.chars().count() <= max {
        return joined;
    }
    let mut cut: String = joined.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The answers given so far: site to yes or no
fn path() -> std::path::PathBuf {
    crate::config::browser_data_dir().join("notices.json")
}

/// Read, changed and written under one lock: the loop answers questions while
/// the settings page takes answers back
static FILE: Mutex<()> = Mutex::new(());

fn read() -> BTreeMap<String, bool> {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write(answers: &BTreeMap<String, bool>) -> anyhow::Result<()> {
    let _ = std::fs::create_dir_all(crate::config::browser_data_dir());
    crate::crypto::write_atomic(&path(), &serde_json::to_string_pretty(answers)?)
}

/// What the person said about a site's notifications, if they have
pub fn answer(site: &str) -> Option<bool> {
    let _held = FILE.lock().unwrap_or_else(|e| e.into_inner());
    read().get(site).copied()
}

/// Keep the person's answer about a site
pub fn remember(site: &str, allow: bool) -> anyhow::Result<()> {
    let _held = FILE.lock().unwrap_or_else(|e| e.into_inner());
    let mut answers = read();
    answers.insert(site.to_string(), allow);
    write(&answers)
}

/// Take an answer back: the site is asked again the next time it asks
pub fn forget(site: &str) -> anyhow::Result<()> {
    let _held = FILE.lock().unwrap_or_else(|e| e.into_inner());
    let mut answers = read();
    if answers.remove(site).is_some() {
        write(&answers)?;
    }
    Ok(())
}

/// Every answer, by site
pub fn answers() -> Vec<(String, bool)> {
    let _held = FILE.lock().unwrap_or_else(|e| e.into_inner());
    read().into_iter().collect()
}

/// A question about a site's notifications, waiting for the person: the
/// pages of that site waiting for the answer, by name
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Asking {
    /// The site, as it is filed
    pub site: String,
    /// The site as a person reads it
    pub host: String,
    #[serde(skip)]
    pub pages: Vec<String>,
}

static ASKING: Mutex<Vec<Asking>> = Mutex::new(Vec::new());

/// The questions waiting, oldest first
pub fn asking() -> Vec<Asking> {
    ASKING.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A page of `site` asked: put the question up, or add the page to the one
/// already up for that site
pub fn ask(site: &str, page: &str) {
    let mut asking = ASKING.lock().unwrap_or_else(|e| e.into_inner());
    match asking.iter_mut().find(|a| a.site == site) {
        Some(a) => {
            if !a.pages.iter().any(|p| p == page) {
                a.pages.push(page.to_string());
            }
        }
        None => asking.push(Asking { site: site.to_string(), host: shown(site).to_string(), pages: vec![page.to_string()] }),
    }
}

/// The question about `site` answered (`Some`) or put away (`None`): it goes,
/// and the pages that were waiting for it are handed back to be told
pub fn answered(site: &str, allow: Option<bool>) -> anyhow::Result<Vec<String>> {
    if let Some(allow) = allow {
        remember(site, allow)?;
    }
    let mut asking = ASKING.lock().unwrap_or_else(|e| e.into_inner());
    let pages = asking.iter().find(|a| a.site == site).map(|a| a.pages.clone()).unwrap_or_default();
    asking.retain(|a| a.site != site);
    Ok(pages)
}

/// A page went away: it waits for nothing any more, and a question nobody is
/// waiting for is put away
pub fn page_gone(page: &str) {
    let mut asking = ASKING.lock().unwrap_or_else(|e| e.into_inner());
    for a in asking.iter_mut() {
        a.pages.retain(|p| p != page);
    }
    asking.retain(|a| !a.pages.is_empty());
}

/// What a page is told about its site: the state its `Notification.permission`
/// reads from now on (`granted`, `denied`, or `default` while unanswered)
pub fn tell_js(allow: Option<bool>) -> String {
    let state = match allow {
        Some(true) => "granted",
        Some(false) => "denied",
        None => "default",
    };
    format!("window.__shikisha_notice_state && window.__shikisha_notice_state({state:?});")
}

/// The script put in every page before its own, in the window: the page's
/// `Notification` (and the notifications entry of `navigator.permissions`)
/// becomes this program's. It knows nothing until it is told
/// (`__shikisha_notice_state`), so it asks as the document starts; and it
/// shows nothing itself -- what to show and whether is decided by the program
pub const SHIM: &str = r##"
(function () {
  if (window.__shikisha_notice_state || !/^https?:$/.test(location.protocol)) return;
  const post = (o) => window.__shikisha_post(JSON.stringify(o));
  let state = "default";
  const waiting = [];
  const watchers = [];
  window.__shikisha_notice_state = function (s) {
    if (s !== "granted" && s !== "denied" && s !== "default") return;
    const was = state;
    state = s;
    for (const done of waiting.splice(0)) done(s);
    if (s !== was) for (const w of watchers) w();
  };
  class Notification extends EventTarget {
    constructor(title, options) {
      super();
      if (arguments.length === 0) {
        throw new TypeError("Failed to construct 'Notification': 1 argument required, but only 0 present.");
      }
      const o = options || {};
      this.title = String(title);
      this.body = o.body == null ? "" : String(o.body);
      this.tag = o.tag == null ? "" : String(o.tag);
      this.data = o.data === undefined ? null : o.data;
      this.icon = o.icon == null ? "" : String(o.icon);
      this.silent = !!o.silent;
      this.requireInteraction = !!o.requireInteraction;
      this.dir = o.dir || "auto";
      this.lang = o.lang || "";
      this.timestamp = Date.now();
      this.onclick = this.onshow = this.onerror = this.onclose = null;
      const fire = (kind) => {
        const e = new Event(kind);
        this.dispatchEvent(e);
        const on = this["on" + kind];
        if (typeof on === "function") on.call(this, e);
      };
      // After the constructor returns, as a browser does, so a handler set on
      // the next line still hears it
      setTimeout(() => {
        if (state !== "granted") { fire("error"); return; }
        post({ kind: "notice", title: this.title, body: this.body });
        fire("show");
      }, 0);
    }
    close() {}
    static get permission() { return state; }
    static get maxActions() { return 0; }
    static requestPermission(done) {
      const asked = state !== "default"
        ? Promise.resolve(state)
        : new Promise((resolve) => { waiting.push(resolve); post({ kind: "notice-ask", ask: true }); });
      if (typeof done === "function") asked.then(done);
      return asked;
    }
  }
  Object.defineProperty(window, "Notification", { value: Notification, writable: true, configurable: true });
  // The other way a page asks: the same answer, in the words that one uses
  if (navigator.permissions && navigator.permissions.query) {
    const query = navigator.permissions.query.bind(navigator.permissions);
    navigator.permissions.query = function (what) {
      if (!what || what.name !== "notifications") return query(what);
      const status = new EventTarget();
      const word = () => (state === "default" ? "prompt" : state);
      Object.defineProperty(status, "state", { get: word });
      Object.defineProperty(status, "name", { value: "notifications" });
      status.onchange = null;
      watchers.push(() => {
        const e = new Event("change");
        status.dispatchEvent(e);
        if (typeof status.onchange === "function") status.onchange.call(status, e);
      });
      return Promise.resolve(status);
    };
  }
  post({ kind: "notice-ask", ask: false });
})();
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// A site is its scheme, host and port, filed one way however the address
    /// was written, and nothing that is not a web page is one
    #[test]
    fn an_address_is_filed_by_its_site() {
        assert_eq!(site("https://Mail.Example.com/inbox?x=1#top").as_deref(), Some("https://mail.example.com"));
        assert_eq!(site("https://example.com:443/").as_deref(), Some("https://example.com"));
        assert_eq!(site("http://example.com:8080").as_deref(), Some("http://example.com:8080"));
        assert_eq!(site("https://user:pw@example.com/").as_deref(), Some("https://example.com"));
        assert_eq!(site("http://[::1]:3000/x").as_deref(), Some("http://[::1]:3000"));
        assert_eq!(site("file:///tmp/a.html"), None);
        assert_eq!(site("about:blank"), None);
        assert_eq!(site("https://"), None);
        assert_eq!(site("https://example.com:notaport/"), None);
        assert_eq!(shown("https://example.com"), "example.com");
        assert_eq!(shown("http://example.com:8080"), "example.com:8080");
    }

    /// What a page says reaches the banner flat and short: no line breaks or
    /// control characters to push the site's name out of view
    #[test]
    fn a_pages_words_are_made_fit_for_a_banner() {
        assert_eq!(clean("  Hello\n\tthere\u{7}  ", 50), "Hello there");
        assert_eq!(clean("abcdef", 4), "abc…");
        assert_eq!(clean("abcd", 4), "abcd");
        assert_eq!(clean("evil\u{202e}moc.elpmaxe", 50), "evil moc.elpmaxe", "the words can still be turned around");
    }

    /// Two pages of one site waiting on one question get one answer; the
    /// question goes once nobody is waiting for it
    #[test]
    fn one_question_per_site() {
        let s = "https://one-question.example";
        ask(s, "a");
        ask(s, "b");
        ask(s, "a");
        assert_eq!(asking().iter().filter(|a| a.site == s).count(), 1);
        assert_eq!(asking().iter().find(|a| a.site == s).map(|a| a.pages.clone()), Some(vec!["a".to_string(), "b".to_string()]));
        page_gone("a");
        page_gone("b");
        assert!(!asking().iter().any(|a| a.site == s), "a question nobody waits for stayed up");
        ask(s, "c");
        assert_eq!(answered(s, None).unwrap(), vec!["c".to_string()]);
        assert!(!asking().iter().any(|a| a.site == s));
    }

    /// What a page is told is one of the three states its Notification reads
    #[test]
    fn a_page_is_told_one_of_three_states() {
        assert!(tell_js(Some(true)).contains("\"granted\""));
        assert!(tell_js(Some(false)).contains("\"denied\""));
        assert!(tell_js(None).contains("\"default\""));
    }
}
