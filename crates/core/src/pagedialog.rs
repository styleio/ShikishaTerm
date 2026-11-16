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
//! time.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// The longest a box's words are shown to a person, in characters: a page
/// decides them, and the buttons have to stay on the screen
pub const SHOWN_MAX: usize = 2000;
/// The longest they are quoted in an error, in characters
pub const QUOTED_MAX: usize = 400;

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
    /// What a prompt's field starts with
    pub preset: String,
    /// Who is asking, as a person reads it: the host of the frame that
    /// opened it, so a page inside a page cannot speak as the site around it
    pub host: String,
}

static OPEN: Mutex<Vec<Dialog>> = Mutex::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(1);
thread_local! {
    /// The page this thread is asking something of right now, by its
    /// in-window name. By thread: the one that asks is the one that waits
    static ACTING: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

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
        preset: if kind == "prompt" { preset.to_string() } else { String::new() },
        host: crate::pagenotice::site(url).map(|s| crate::pagenotice::shown(&s).to_string()).unwrap_or_default(),
    });
    Some(id)
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

/// Held while something is asked of a page and waited for
pub struct Acting(());

/// Say which page is being asked something, until what is handed back is let go of
pub fn acting(page: &str) -> Acting {
    ACTING.with(|a| *a.borrow_mut() = Some(page.to_string()));
    Acting(())
}

impl Drop for Acting {
    fn drop(&mut self) {
        ACTING.with(|a| *a.borrow_mut() = None);
    }
}

/// Whether the page being asked something has a box open: what was asked
/// will not come back until the box is answered, so whoever waits for it
/// stops waiting
pub fn in_the_way() -> bool {
    let page = ACTING.with(|a| a.borrow().clone());
    page.is_some_and(|p| open().iter().any(|d| d.page == p))
}

/// Wait for an answer to what was asked of a page, no longer than `timeout`
/// -- and no longer than until a box opens on that page, which is reported
/// as the time having run out: the answer is not coming while the box is up
pub fn wait<T>(rx: &std::sync::mpsc::Receiver<T>, timeout: std::time::Duration) -> Result<T, std::sync::mpsc::RecvTimeoutError> {
    const LOOK: std::time::Duration = std::time::Duration::from_millis(100);
    let until = std::time::Instant::now() + timeout;
    loop {
        let left = until.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left.min(LOOK)) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if !left.is_zero() && !in_the_way() => continue,
            other => return other,
        }
    }
}

/// What a browser's "a dialog is opening" says, as this program's report of
/// it -- or `None` for the one kind that is not held: "leave this page?",
/// which the host answers with yes there and then
pub fn report_of(page: &str, opening: &serde_json::Value) -> Option<shikisha_shared::Ev> {
    let word = |k: &str| opening.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    if word("type") == "beforeunload" {
        return None;
    }
    Some(shikisha_shared::Ev::PageDialog {
        from: Some(page.to_string()),
        kind: word("type"),
        message: word("message"),
        preset: word("defaultPrompt"),
        url: word("url"),
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
    }

    #[test]
    fn a_wait_on_the_page_stops_when_its_box_opens() {
        let (p, other) = (page("waited"), page("other"));
        assert!(!in_the_way());
        {
            let _held = acting(&p);
            assert!(!in_the_way());
            // A box on another page is not in this one's way
            opened(&other, "alert", "elsewhere", "", "");
            assert!(!in_the_way());
            heard(&shikisha_shared::Ev::PageDialog {
                from: Some(p.clone()),
                kind: "alert".into(),
                message: "Saved".into(),
                preset: String::new(),
                url: "https://a.example.com/".into(),
            });
            assert!(in_the_way());
            heard(&shikisha_shared::Ev::PageDialogGone { from: Some(p.clone()) });
            assert!(!in_the_way());
        }
        opened(&p, "alert", "later", "", "");
        // Nothing is being asked of it any more
        assert!(!in_the_way());
        gone(&p);
        gone(&other);
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
