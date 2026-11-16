//! A page's `alert`, `confirm` and `prompt`, held until somebody answers.
//!
//! A page that opens one of these stops until it is answered, and so does
//! everything asked of that page meanwhile. Nothing here answers for anybody:
//! the box is the page's question to whoever is driving it. A person is asked
//! on the board (and on a phone), in this program's own frame with the site's
//! name in front; a script or an AI is told, in the error of whatever it
//! asked of the page, that a box is open and what it says, and answers it
//! with `browser_dialog`. Whoever answers first has answered it.
//!
//! The one exception is "leave this page?" (`beforeunload`), which the
//! browsers answer with yes where they hear it: a page is never able to keep
//! somebody on it.
//!
//! What is kept here is what the browser said was opened -- by the host that
//! draws the page, never read off a message a page could send -- so the
//! words can be shown and quoted, and so whatever is waited for on that page
//! can stop waiting the moment the box appears instead of running out its
//! time. That last is decided where the waiting is done, for the page the
//! wait is on (`wait`, `stops`), and so holds for everything that waits on
//! a page whoever asked: a script's click, a recording being started, a
//! phone's tap being placed, another machine's request.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// The longest a box's words are shown to a person, in characters: a page
/// decides them, and the buttons have to stay on the screen
pub const SHOWN_MAX: usize = 2000;
/// The longest they are quoted in an error, in characters
pub const QUOTED_MAX: usize = 400;
/// The most of what a page put in a prompt's field that is kept, in
/// characters: what the page gets back when the box is accepted as it
/// stands. A page may put megabytes there; past this it is cut
pub const PRESET_KEPT: usize = 100_000;
/// ...and the most of it that is put in the field on the board
pub const PRESET_SHOWN: usize = 2_000;
/// How long a browser is given to answer something asked of a page whose
/// box is open. Some things it answers regardless (the answer to the box
/// itself, a navigation, the picture of the page); what waits on the page's
/// own script does not come back until the box is gone
pub const GRACE: std::time::Duration = std::time::Duration::from_millis(300);
/// The one request that is always waited for in full: the box's answer
pub const ANSWER: &str = "Page.handleJavaScriptDialog";

/// One box that is open
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Dialog {
    /// Which opening this is: an answer names it, so one meant for a box
    /// that is gone is not given to the next
    pub id: u64,
    /// The page's in-window name
    pub page: String,
    /// `alert`, `confirm` or `prompt`
    pub kind: String,
    /// The page's words, cut to what is shown
    pub message: String,
    /// What a prompt's field starts with, as the page gets it back when the
    /// box is accepted without other words
    #[serde(skip)]
    pub preset: String,
    /// ...as much of it as is put in the field on the board, and whether
    /// that is not all of it
    pub shown: String,
    pub cut: bool,
    /// Who is asking, as a person reads it: the host of the frame that
    /// opened it, so a page inside a page cannot speak as the site around it
    pub host: String,
}

static OPEN: Mutex<Vec<Dialog>> = Mutex::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(1);
/// Other names the pages with a box open are known by where they are
/// waited on: a browser on this machine speaks to a page by its session
static ALSO: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn open() -> std::sync::MutexGuard<'static, Vec<Dialog>> {
    OPEN.lock().unwrap_or_else(|e| e.into_inner())
}

/// The kind as it is kept: anything a browser calls something else is an
/// alert, which has one button and takes no words
fn kind_of(said: &str) -> &'static str {
    match said {
        "confirm" => "confirm",
        "prompt" => "prompt",
        _ => "alert",
    }
}

/// A browser said a page opened a box. One box a page: a second while one is
/// up (another frame's) replaces nothing and is not kept -- the host that
/// heard it turns it away
pub fn opened(page: &str, kind: &str, message: &str, preset: &str, url: &str) -> Option<u64> {
    let mut all = open();
    if all.iter().any(|d| d.page == page) {
        return None;
    }
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    let kind = kind_of(kind);
    // No more of it is gone through than could be shown: a page decides how
    // long its words are
    let message: String = message.chars().take(SHOWN_MAX * 2).collect();
    // Line breaks are the page's layout of its own words and are kept; the
    // rest is cleaned the way a page's notification is
    let message = message
        .lines()
        .map(|l| crate::pagenotice::clean(l, SHOWN_MAX))
        .collect::<Vec<_>>()
        .join("\n");
    let message: String = match message.chars().count() > SHOWN_MAX {
        true => message.chars().take(SHOWN_MAX - 1).chain(std::iter::once('…')).collect(),
        false => message,
    };
    all.push(Dialog {
        id,
        page: page.to_string(),
        kind: kind.to_string(),
        message,
        preset: if kind == "prompt" { cut_to(preset, PRESET_KEPT) } else { String::new() },
        shown: if kind == "prompt" { cut_to(preset, PRESET_SHOWN) } else { String::new() },
        cut: kind == "prompt" && preset.chars().nth(PRESET_SHOWN).is_some(),
        host: crate::pagenotice::site(url).map(|s| crate::pagenotice::shown(&s).to_string()).unwrap_or_default(),
    });
    Some(id)
}

fn cut_to(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// What a host's report says, kept: a box opened, or the one on a page gone.
/// Called where the report is first heard on this machine -- the thread that
/// listens to the browser -- so a wait on that page sees it at once
pub fn heard(ev: &shikisha_shared::Ev) {
    match ev {
        shikisha_shared::Ev::PageDialog { from: Some(page), kind, message, preset, url } => {
            // Which page and what kind, for whoever reads the log later; never the
            // words, which are the page's and may be anybody's business
            if opened(page, kind, message, preset, url).is_some() {
                crate::append_hook_log(&format!("[browser] page '{page}' opened a JavaScript {} and waits for its answer", kind_of(kind)));
            }
        }
        shikisha_shared::Ev::PageDialogGone { from: Some(page) } => gone(page),
        _ => {}
    }
}

/// The box open on a page, if one is
pub fn open_on(page: &str) -> Option<Dialog> {
    open().iter().find(|d| d.page == page).cloned()
}

/// Every box open, oldest first
pub fn all() -> Vec<Dialog> {
    open().clone()
}

/// The box on a page is gone: answered, or the page left it behind (it went
/// somewhere else, it closed)
pub fn gone(page: &str) {
    open().retain(|d| d.page != page);
}

/// Take the box on a page to answer it. `id` when the answer names one: a
/// box that is not that one stays, and nothing is handed back
pub fn taken(page: &str, id: Option<u64>) -> Option<Dialog> {
    let mut all = open();
    let at = all.iter().position(|d| d.page == page && id.is_none_or(|i| d.id == i))?;
    Some(all.remove(at))
}

/// Every box, of the pages a browser that went away was drawing: nobody
/// can answer them any more, and nothing will say they closed
pub fn gone_among(pages: &[String]) {
    open().retain(|d| !pages.contains(&d.page));
}

/// A page with a box open is also known as `name` where it is waited on
pub fn also(name: &str) {
    let mut also = ALSO.lock().unwrap_or_else(|e| e.into_inner());
    if !also.iter().any(|n| n == name) {
        also.push(name.to_string());
    }
}

/// ...and no longer: its box is gone, or the page is
pub fn no_longer(name: &str) {
    ALSO.lock().unwrap_or_else(|e| e.into_inner()).retain(|n| n != name);
}

/// Whether what is waited for on this page is stopped behind a box: the
/// page has one open. `on` is the page's in-window name, or another name
/// it was given here (`also`)
pub fn stops(on: &str) -> bool {
    open().iter().any(|d| d.page == on) || ALSO.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|n| n == on)
}

/// Which page a request waits on, for `wait`: the page it was sent to --
/// or none for the answer to the box itself, which is waited for in full
pub fn watched<'a>(to: Option<&'a str>, method: &str) -> Option<&'a str> {
    to.filter(|_| method != ANSWER)
}

/// Wait for an answer to what was asked of the page `on`, no longer than
/// `timeout` -- and, while a box is open on that page, no longer than the
/// browser needs to answer what it answers regardless (`GRACE`). Giving up
/// behind a box is reported as the time having run out: the answer is not
/// coming while the box is up
pub fn wait<T>(
    rx: &std::sync::mpsc::Receiver<T>,
    timeout: std::time::Duration,
    on: Option<&str>,
) -> Result<T, std::sync::mpsc::RecvTimeoutError> {
    const LOOK: std::time::Duration = std::time::Duration::from_millis(100);
    let started = std::time::Instant::now();
    let until = started + timeout;
    loop {
        let left = until.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left.min(LOOK)) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if !left.is_zero() && !gives_up(on, started) => continue,
            other => return other,
        }
    }
}

/// Whether a wait on `on` that began at `started` ends here, behind a box
pub fn gives_up(on: Option<&str>, started: std::time::Instant) -> bool {
    started.elapsed() >= GRACE && on.is_some_and(stops)
}

/// What a browser's "a dialog is opening" says, as this program's report of
/// it -- or `None` for the one kind that is not held: "leave this page?",
/// which the host answers with yes there and then
pub fn report_of(page: &str, opening: &serde_json::Value) -> Option<shikisha_shared::Ev> {
    let word = |k: &str| opening.get(k).and_then(|v| v.as_str()).unwrap_or_default();
    if word("type") == "beforeunload" {
        return None;
    }
    // Cut here, where it is first heard: the report travels (to the loop, to
    // another machine), and a page decides how long these are
    Some(shikisha_shared::Ev::PageDialog {
        from: Some(page.to_string()),
        kind: word("type").to_string(),
        message: cut_to(word("message"), SHOWN_MAX * 2),
        preset: cut_to(word("defaultPrompt"), PRESET_KEPT),
        url: cut_to(word("url"), 2_048),
    })
}

impl Dialog {
    /// The page's words as an error quotes them: in quotes, with what a
    /// page could use to end the quote early written out, and no longer
    /// than a line or two
    fn quoted(&self) -> String {
        let short: String = match self.message.chars().count() > QUOTED_MAX {
            true => self.message.chars().take(QUOTED_MAX).chain(std::iter::once('…')).collect(),
            false => self.message.clone(),
        };
        serde_json::Value::String(short).to_string()
    }

    /// How it is answered, for the page a script knows as `name`
    fn how(&self, name: &str) -> String {
        let key = if self.kind == "prompt" { "err.browser.dialog_how_prompt" } else { "err.browser.dialog_how" };
        crate::i18n::tp(key, &[("name", name)])
    }

    /// Something was asked of a page whose box is still open
    pub fn refusal(&self, name: &str) -> String {
        crate::i18n::tp(
            "err.browser.dialog_open",
            &[("kind", &self.kind), ("message", &self.quoted()), ("how", &self.how(name))],
        )
    }

    /// The box opened while something asked of the page was being waited for
    pub fn cut_in(&self, name: &str) -> String {
        crate::i18n::tp(
            "err.browser.dialog_opened",
            &[("kind", &self.kind), ("message", &self.quoted()), ("how", &self.how(name))],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests share the one list: each uses a page of its own name
    fn page(name: &str) -> String {
        format!("test-dialog/{name}")
    }

    #[test]
    fn a_page_has_one_box_and_an_answer_names_it() {
        let p = page("one");
        let id = opened(&p, "confirm", "Delete it?", "ignored", "https://shop.example.com/cart").unwrap();
        let d = open_on(&p).unwrap();
        assert_eq!((d.kind.as_str(), d.message.as_str(), d.preset.as_str(), d.host.as_str()), ("confirm", "Delete it?", "", "shop.example.com"));
        assert_eq!((d.shown.as_str(), d.cut), ("", false));
        // Another frame's box while this one is up is not kept
        assert_eq!(opened(&p, "alert", "second", "", "https://ads.example.net/"), None);
        // An answer meant for a box that is gone leaves this one
        assert_eq!(taken(&p, Some(id + 1000)), None);
        assert!(open_on(&p).is_some());
        assert_eq!(taken(&p, Some(id)).map(|d| d.id), Some(id));
        assert_eq!(open_on(&p), None);
        // ...and one that names none takes whichever is up
        opened(&p, "prompt", "Name?", "untitled", "about:blank").unwrap();
        let d = taken(&p, None).unwrap();
        assert_eq!((d.kind.as_str(), d.preset.as_str(), d.host.as_str()), ("prompt", "untitled", ""));
        assert_eq!((d.shown.as_str(), d.cut), ("untitled", false));
        // What a page put in the field is kept up to a size, and less of it is shown
        let huge = "x".repeat(PRESET_KEPT + 500);
        opened(&p, "prompt", "Big?", &huge, "");
        let d = taken(&p, None).unwrap();
        assert_eq!((d.preset.chars().count(), d.shown.chars().count(), d.cut), (PRESET_KEPT, PRESET_SHOWN, true));
        let said = report_of(&p, &serde_json::json!({"type": "prompt", "message": "m".repeat(SHOWN_MAX * 5), "defaultPrompt": huge}));
        assert!(
            matches!(said, Some(shikisha_shared::Ev::PageDialog { ref message, ref preset, .. })
                if message.chars().count() == SHOWN_MAX * 2 && preset.chars().count() == PRESET_KEPT),
            "what a page says travels at whatever length the page chose"
        );
        assert!(report_of(&p, &serde_json::json!({"type": "beforeunload", "message": ""})).is_none());
    }

    #[test]
    fn a_wait_on_the_page_stops_when_its_box_opens() {
        use std::time::{Duration, Instant};
        let (p, other) = (page("waited"), page("other"));
        let (_tx, rx) = std::sync::mpsc::channel::<()>();
        let took = |on: Option<&str>| {
            let started = Instant::now();
            assert!(wait(&rx, Duration::from_millis(1500), on).is_err());
            started.elapsed()
        };
        // A box on another page is not in this one's way: the wait runs its time
        opened(&other, "alert", "elsewhere", "", "");
        assert!(!stops(&p));
        assert!(took(Some(&p)) >= Duration::from_millis(1400));
        heard(&shikisha_shared::Ev::PageDialog {
            from: Some(p.clone()),
            kind: "alert".into(),
            message: "Saved".into(),
            preset: String::new(),
            url: "https://a.example.com/".into(),
        });
        assert!(stops(&p));
        // Behind its own box a wait gives up, after the moment the browser is given
        let behind = took(Some(&p));
        assert!(behind >= GRACE && behind < Duration::from_millis(1200), "{behind:?}");
        // ...but not one on no page, nor the answer to the box itself
        assert!(took(None) >= Duration::from_millis(1400));
        assert_eq!(watched(Some(&p), ANSWER), None);
        assert_eq!(watched(Some(&p), "Runtime.evaluate"), Some(p.as_str()));
        // An answer that comes in time is the answer, box or no box
        let (tx, rx2) = std::sync::mpsc::channel::<u8>();
        tx.send(7).unwrap();
        assert_eq!(wait(&rx2, Duration::from_secs(5), Some(&p)), Ok(7));
        heard(&shikisha_shared::Ev::PageDialogGone { from: Some(p.clone()) });
        assert!(!stops(&p));
        // A page is known by another name where it is waited on
        also("session-waited");
        assert!(stops("session-waited"));
        no_longer("session-waited");
        assert!(!stops("session-waited"));
        // A browser that went takes its pages' boxes with it
        opened(&p, "alert", "later", "", "");
        gone_among(&[p.clone(), other.clone()]);
        assert!(!stops(&p) && !stops(&other));
    }

    #[test]
    fn a_pages_words_are_cleaned_cut_and_quoted() {
        let p = page("words");
        let long = "x".repeat(SHOWN_MAX + 50);
        opened(&p, "weird-kind", &format!("line one\u{202e}\nline \"two\"\n{long}"), "", "https://a.example.com:8443/x");
        let d = taken(&p, None).unwrap();
        assert_eq!(d.kind, "alert");
        assert_eq!(d.host, "a.example.com:8443");
        assert!(d.message.starts_with("line one\nline \"two\"\n"), "{:?}", &d.message[..40]);
        assert_eq!(d.message.chars().count(), SHOWN_MAX);
        let said = d.refusal("web");
        // The quote cannot be ended early by the page's own quotes, and is short
        assert!(said.contains("\\\"two\\\""), "{said}");
        assert!(said.chars().count() < QUOTED_MAX + 400, "{}", said.chars().count());
        assert!(said.contains("browser_dialog(\"web\""), "{said}");
    }
}
