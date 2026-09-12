//! Where a page is drawn.
//!
//! There are two browsers a runtime can put a page in: the one on this machine
//! (see [`crate::chrome`]) and the one belonging to whoever is connected (see
//! [`crate::faraway`]). Which is better depends on what the page is for, and
//! nobody but the person can know that:
//!
//! - **Here** is the one that works when nobody is watching, the one a phone
//!   can be shown, and the one whose logins are the same session for every
//!   device. It costs this machine the work of painting.
//! - **There** is immediate -- native scrolling, real text, the person's own
//!   devtools -- and costs this machine nothing. It needs somebody present,
//!   it cannot be watched from anywhere else, and its cookies belong to that
//!   one device.
//!
//! Both, then, and a setting. What is *not* a setting is `localhost`: a page
//! drawn on somebody's desk still reaches the network through this machine
//! (see [`crate::tunnel`]), so the port an agent here just opened means the
//! same thing on both sides. Without that, drawing elsewhere would be a
//! different browser looking at a different machine, and there would be
//! nothing to choose between.
//!
//! A page does not move once it is open. Which side has it is written down
//! when it is opened and every later word about that page goes to the same
//! side -- changing the setting changes where the *next* page is drawn.

use shikisha_shared::{BrowserHost, BrowserProfile, Found, Go, Input, OpReport, Sel};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Which browser draws a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Draw {
    /// The browser on the machine the agents are on
    Here,
    /// The browser on the machine the person is at
    There,
}

impl Draw {
    /// What a setting's word means. Anything unrecognised is `Here`, which is
    /// the side that works with nobody connected
    pub fn of(said: &str) -> Self {
        match said.trim() {
            "there" | "device" | "client" => Draw::There,
            _ => Draw::Here,
        }
    }
}

/// Every page the runtime has open, on whichever machine it is drawn.
pub struct Placed {
    here: Rc<crate::chrome::Pages>,
    there: Rc<crate::faraway::Far>,
    /// Where the next page goes
    prefer: Cell<Draw>,
    /// Where each page already is
    owner: RefCell<HashMap<String, Draw>>,
}

impl Default for Placed {
    fn default() -> Self {
        Self::new()
    }
}

impl Placed {
    pub fn new() -> Self {
        Self {
            here: Rc::new(crate::chrome::Pages::new()),
            there: Rc::new(crate::faraway::Far::new()),
            prefer: Cell::new(Draw::Here),
            owner: RefCell::new(HashMap::new()),
        }
    }

    /// The same, with pages of this machine's own kept somewhere particular.
    /// For a test that wants a real browser and nobody's real cookies
    #[cfg(test)]
    pub(crate) fn under(root: std::path::PathBuf, temporary: bool) -> Self {
        Self {
            here: Rc::new(crate::chrome::Pages::under(root, temporary)),
            ..Self::new()
        }
    }

    /// Where the next page should be drawn.
    pub fn prefer(&self, draw: Draw) {
        self.prefer.set(draw);
    }

    /// The end of the line a connected client answers on.
    pub fn far(&self) -> crate::faraway::Line {
        self.there.line()
    }

    /// Everything both sides have reported since this was last asked.
    pub fn drain(&self) -> Vec<shikisha_shared::Ev> {
        let mut said = self.here.drain();
        said.extend(self.there.drain());
        said
    }

    /// Which side has this page. A page nobody opened is treated as this
    /// machine's: the answer will be "no such page", and it should come from
    /// the side that is always there
    fn side(&self, to: Option<&str>) -> Draw {
        to.and_then(|name| self.owner.borrow().get(name).copied())
            .unwrap_or(Draw::Here)
    }

    /// Hand one page's work to whichever browser has it.
    fn on<T>(
        &self,
        to: Option<&str>,
        f: impl FnOnce(&dyn BrowserHost) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        match self.side(to) {
            Draw::Here => f(self.here.as_ref()),
            Draw::There => f(self.there.as_ref()),
        }
    }
}

impl BrowserHost for Placed {
    /// Open a page, on the side the person asked for.
    ///
    /// Falling back to this machine when the other side is not there: a script
    /// that opens a page while nobody is connected should open a page, not
    /// fail. The setting says where a page goes when there is a choice
    fn open_child(
        &self,
        name: &str,
        url: &str,
        rect: (i32, i32, i32, i32),
        profile: BrowserProfile,
    ) -> anyhow::Result<()> {
        let side = match self.prefer.get() == Draw::There && self.there.here() {
            true => Draw::There,
            false => Draw::Here,
        };
        match side {
            Draw::Here => self.here.open_child(name, url, rect, profile)?,
            Draw::There => self.there.open_child(name, url, rect, profile)?,
        }
        // Which side a page landed on is the first thing anybody asks when it
        // does not look the way they expected, and the one thing that cannot
        // be seen from either end afterwards
        crate::append_hook_log(&format!(
            "page {name}: drawn {} ({url})",
            match side {
                Draw::Here => "on this machine",
                Draw::There => "on the connected device",
            }
        ));
        self.owner.borrow_mut().insert(name.to_string(), side);
        Ok(())
    }

    fn close_child(&self, name: &str) -> anyhow::Result<()> {
        let out = self.on(Some(name), |b| b.close_child(name));
        self.owner.borrow_mut().remove(name);
        out
    }

    fn child_bounds(&self, name: &str, rect: (i32, i32, i32, i32)) -> anyhow::Result<()> {
        self.on(Some(name), |b| b.child_bounds(name, rect))
    }

    /// Both sides are told. This is the app saying "pages from here are mine",
    /// and it is true wherever a page of ours is drawn
    fn trust(&self, url: &str) -> anyhow::Result<()> {
        let _ = self.there.trust(url);
        self.here.trust(url)
    }

    /// Likewise: there is one recorder, and silencing it means everywhere
    fn record_all_off(&self) {
        self.here.record_all_off();
        self.there.record_all_off();
    }

    fn go(&self, to: Option<&str>, go: Go) -> anyhow::Result<()> {
        self.on(to, |b| b.go(to, go.clone()))
    }
    fn focus(&self, to: Option<&str>) -> anyhow::Result<()> {
        self.on(to, |b| b.focus(to))
    }
    fn ask_where(&self, to: Option<&str>) -> anyhow::Result<()> {
        self.on(to, |b| b.ask_where(to))
    }
    fn basic_auth(&self, to: Option<&str>, user: &str, pass: &str) -> anyhow::Result<()> {
        self.on(to, |b| b.basic_auth(to, user, pass))
    }
    fn eval_in(&self, to: Option<&str>, js: &str) -> anyhow::Result<u64> {
        self.on(to, |b| b.eval_in(to, js))
    }
    fn inject(&self, to: Option<&str>, input: Input) -> anyhow::Result<()> {
        self.on(to, |b| b.inject(to, input.clone()))
    }
    fn screencast(&self, to: Option<&str>, on: bool) -> anyhow::Result<()> {
        self.on(to, |b| b.screencast(to, on))
    }

    /// The one host that has two answers to this. A page of this machine's own
    /// says nothing (there is nothing to say: it is here), and one on somebody
    /// else's desk names the desk
    fn drawn_on(&self, to: Option<&str>) -> Option<String> {
        match self.side(to) {
            Draw::Here => None,
            Draw::There => Some(self.there.who()),
        }
    }
    fn record(&self, to: Option<&str>, on: bool) -> anyhow::Result<()> {
        self.on(to, |b| b.record(to, on))
    }
    fn find(&self, to: Option<&str>, sel: &Sel, ms: u64) -> anyhow::Result<Found> {
        self.on(to, |b| b.find(to, sel, ms))
    }
    fn click(&self, to: Option<&str>, sel: &Sel, ms: u64) -> anyhow::Result<OpReport> {
        self.on(to, |b| b.click(to, sel, ms))
    }
    fn fill(&self, to: Option<&str>, sel: &Sel, value: &str, ms: u64) -> anyhow::Result<OpReport> {
        self.on(to, |b| b.fill(to, sel, value, ms))
    }
    fn text(&self, to: Option<&str>, sel: &Sel, ms: u64) -> anyhow::Result<Option<String>> {
        self.on(to, |b| b.text(to, sel, ms))
    }
    fn href(&self, to: Option<&str>, ms: u64) -> anyhow::Result<String> {
        self.on(to, |b| b.href(to, ms))
    }
    fn html(&self, to: Option<&str>, ms: u64) -> anyhow::Result<String> {
        self.on(to, |b| b.html(to, ms))
    }
    fn digest(&self, to: Option<&str>, ms: u64) -> anyhow::Result<String> {
        self.on(to, |b| b.digest(to, ms))
    }
    fn snapshot(&self, to: Option<&str>, ms: u64) -> anyhow::Result<Vec<u8>> {
        self.on(to, |b| b.snapshot(to, ms))
    }
    fn cookies_out(&self, to: Option<&str>, ms: u64) -> anyhow::Result<serde_json::Value> {
        self.on(to, |b| b.cookies_out(to, ms))
    }
    fn cookies_in(&self, to: Option<&str>, cookies: &serde_json::Value, ms: u64) -> anyhow::Result<()> {
        self.on(to, |b| b.cookies_in(to, cookies, ms))
    }
    fn storage_out(&self, to: Option<&str>, ms: u64) -> anyhow::Result<serde_json::Value> {
        self.on(to, |b| b.storage_out(to, ms))
    }
    fn storage_in(&self, to: Option<&str>, items: &serde_json::Value, ms: u64) -> anyhow::Result<()> {
        self.on(to, |b| b.storage_in(to, items, ms))
    }
    fn fetch(&self, to: Option<&str>, url: &str, opts: &serde_json::Value, ms: u64) -> anyhow::Result<String> {
        self.on(to, |b| b.fetch(to, url, opts, ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page opened while nobody is connected is opened here rather than not
    /// at all -- and the setting is not forgotten by that
    #[test]
    fn with_nobody_connected_a_page_is_drawn_on_this_machine() {
        let placed = Placed::new();
        placed.prefer(Draw::There);
        assert_eq!(placed.side(Some("ws/p")), Draw::Here, "開く前から向こう扱い");
        // Opening needs a browser, which a test machine may not have -- what
        // is being checked is which side was asked, which the error says
        let refused = placed
            .open_child("ws/p", "https://example.com/", (0, 0, 800, 600), BrowserProfile::shared_default());
        let said = refused.map(|()| String::new()).unwrap_or_else(|e| e.to_string());
        assert!(
            !said.contains(&crate::i18n::t("err.far.nobody")),
            "誰も居ないのに向こうに投げている: {said}"
        );
    }

    /// What the setting's word means, including a word nobody wrote
    #[test]
    fn the_setting_reads_as_one_of_two_places() {
        assert_eq!(Draw::of("there"), Draw::There);
        assert_eq!(Draw::of("device"), Draw::There);
        assert_eq!(Draw::of("here"), Draw::Here);
        assert_eq!(Draw::of(""), Draw::Here);
        assert_eq!(Draw::of("なにか"), Draw::Here, "知らない語は留守番できる側");
    }

    /// A page drawn over there says which device has it, and a page of this
    /// machine's own says nothing.
    ///
    /// This is what the screen is built on. Such a page has no picture anybody
    /// else can be shown (`faraway::Far::screencast` refuses, and rightly), so
    /// the board says where the page is rather than showing a relay that can
    /// never fill in -- which reads as the app having stopped.
    #[test]
    fn a_page_drawn_on_the_connected_device_says_which_device() {
        let placed = Placed::new();
        // Somebody is there, and answers "done" to whatever is put to them
        let line = placed.far();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        line.attach(tx, "台所のノート");
        let back = line.clone();
        std::thread::spawn(move || {
            while let Ok(text) = rx.recv() {
                let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                let said = crate::faraway::Said::Answer {
                    id: v["id"].as_u64().unwrap(),
                    ok: true,
                    value: serde_json::Value::Null,
                };
                back.heard(&serde_json::to_string(&said).unwrap());
            }
        });
        placed.prefer(Draw::There);
        placed
            .open_child(
                "0/probe",
                "http://127.0.0.1:9990/",
                (0, 0, 800, 600),
                BrowserProfile::shared_default(),
            )
            .expect("向こうに開けない");
        assert_eq!(
            placed.drawn_on(Some("0/probe")).as_deref(),
            Some("台所のノート"),
            "向こうで描いているのに、どの端末かを言わない"
        );
        // The next page goes here. Where each one is was written down when it
        // was opened, so this one is unaffected by that
        placed.prefer(Draw::Here);
        assert_eq!(
            placed.drawn_on(Some("0/probe")).as_deref(),
            Some("台所のノート"),
            "開いた後に設定で場所が変わってしまう"
        );
        // And a page nobody opened is this machine's business, like every other
        // question about one
        assert_eq!(placed.drawn_on(Some("0/never")), None, "無いページが向こう扱い");
    }

    /// A page that was never opened is this machine's business, so the answer
    /// comes from the side that is always there
    #[test]
    fn a_page_nobody_opened_is_answered_by_the_side_that_is_always_here() {
        let placed = Placed::new();
        let err = placed
            .href(Some("ws/never"), 1_000)
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains(&crate::i18n::t("err.far.nobody")),
            "無いページを向こうに聞きに行っている: {err}"
        );
    }
}
