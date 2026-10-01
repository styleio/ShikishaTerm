//! The calls a tab over there made while the app that started it was away
//! (far-keep plan §4.6, the first version).
//!
//! A tab's `shikisha` command whose app is not connected is answered at once
//! that the PC is away, so the AI that made the call knows it did not get
//! through. What it does with that is up to it, and an AI that reads the
//! failure and carries on leaves nothing for the person to find. So the
//! resident process also writes each such call down, and the app, connecting
//! again, shows the person how many there were and where in which tab.
//!
//! **What is written down, and what is not.** The command's name, the tab it
//! was meant for when its first word names one, when, from which tab, and
//! whether it was cut in the middle (the app went while it was being
//! handled, so it may or may not have been carried out). Never what it said:
//! a call's words may carry a secret or a long text, and are not copied onto
//! another machine to be kept.
//!
//! **How much.** At most [`MOST`] calls, none older than [`DAYS`] days. What is
//! let go of past that is counted, with the span it covered, so the person
//! is told that there were more rather than shown a list that quietly stops.
//! A call the person has looked at is struck out when they say so.
//!
//! **Kept, not only written down (the later version of §4.6).** A call that
//! asks nothing back -- a report, a note, a notification, a word said
//! ([`KEPT`]) -- is kept whole instead, what it said included, and handed to
//! the app the next time it gives that tab its key: the AI is answered at
//! once that it is kept. Each is handed over until the app answers it, and
//! carries a number of its own the app runs only once ([`kept_id`]), so a
//! hand-over cut in the middle and done again does nothing twice. The same
//! limits hold, counted the same way.

use serde::{Deserialize, Serialize};

/// The file, in the bridge's own folder: it outlives the resident process,
/// so an app that connects to the next one is still told
pub const FILE: &str = "missed.json";
/// How many calls are kept
pub const MOST: usize = 200;
/// How many days a call is kept
pub const DAYS: u64 = 14;

/// The commands that ask nothing back, kept while the app is away and handed
/// over when it is back: a report on a job, a note, a notification, a word
/// said. Anything else is answered that the PC is away, and written down
pub const KEPT: &[&str] = &["report", "note", "notify", "say"];

/// One call kept to be handed over: its line whole, what it said included
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    /// Its own number in the book
    pub id: u64,
    pub at: u64,
    /// The tab that made it, by the name its key file has
    pub tab: String,
    /// The call as the tab sent it
    pub line: String,
}

/// The id a kept call is handed over under: this book's own, and the
/// call's number in it, so two machines' calls are never taken for one
pub fn kept_id(book: &str, n: u64) -> String {
    format!("kept-{book}-{n}")
}

/// One call that did not reach its app
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Missed {
    /// Its own number in the book, never given twice: what it is struck out
    /// by (two calls of one tab, one command, in one second are two calls)
    #[serde(default)]
    pub id: u64,
    /// When, in seconds since 1970
    pub at: u64,
    /// Which tab made it, by the name its key file has (`farlink::key_name`).
    /// Empty when this resident process never saw the tab's key given
    pub tab: String,
    /// The command (`ask_tab`, `tab_run`, ...)
    pub method: String,
    /// The tab it was meant for, when its first word names one; else empty
    pub to: String,
    /// Cut in the middle: the app went while it was being handled
    #[serde(default)]
    pub cut: bool,
}

/// The whole file
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub calls: Vec<Missed>,
    /// How many were let go of past the limits, and the span they covered
    #[serde(default)]
    pub dropped: u64,
    #[serde(default)]
    pub dropped_from: u64,
    #[serde(default)]
    pub dropped_to: u64,
    /// The number the next call written down is given
    #[serde(default)]
    pub next: u64,
    /// The calls kept to be handed over
    #[serde(default)]
    pub kept: Vec<Kept>,
    /// This book's own name, made with it: what its kept calls are known by
    #[serde(default)]
    pub uid: String,
}

impl Book {
    /// Write a call down, letting go of what is past the limits as of `now`
    pub fn add(&mut self, mut call: Missed, now: u64) {
        self.next += 1;
        call.id = self.next;
        self.calls.push(call);
        self.trim(now);
    }

    /// Keep a call to be handed over: its number, under which it goes
    pub fn keep(&mut self, tab: &str, line: &str, now: u64) -> u64 {
        if self.uid.is_empty() {
            self.uid = crate::random_hex(6);
        }
        self.next += 1;
        let id = self.next;
        self.kept.push(Kept { id, at: now, tab: tab.to_string(), line: line.to_string() });
        self.trim(now);
        id
    }

    /// The kept calls of one tab, oldest first
    pub fn kept_for(&self, tab: &str) -> Vec<Kept> {
        self.kept.iter().filter(|k| k.tab == tab).cloned().collect()
    }

    /// A kept call was handed over and answered: gone
    pub fn handed(&mut self, id: u64) {
        self.kept.retain(|k| k.id != id);
    }

    /// Let go of the calls past the limits, counting them
    pub fn trim(&mut self, now: u64) {
        let oldest = now.saturating_sub(DAYS * 24 * 60 * 60);
        let mut gone: Vec<Missed> = Vec::new();
        self.calls.retain(|c| {
            let keep = c.at >= oldest;
            if !keep {
                gone.push(c.clone());
            }
            keep
        });
        if self.calls.len() > MOST {
            let over = self.calls.len() - MOST;
            gone.extend(self.calls.drain(..over));
        }
        // The kept ones by the same limits: what goes is counted with the
        // calls let go of, since either way it did not reach the app
        let mut gone_at: Vec<u64> = gone.iter().map(|c| c.at).collect();
        self.kept.retain(|k| {
            let keep = k.at >= oldest;
            if !keep {
                gone_at.push(k.at);
            }
            keep
        });
        if self.kept.len() > MOST {
            let over = self.kept.len() - MOST;
            gone_at.extend(self.kept.drain(..over).map(|k| k.at));
        }
        for at in gone_at {
            self.dropped += 1;
            self.dropped_from = if self.dropped_from == 0 { at } else { self.dropped_from.min(at) };
            self.dropped_to = self.dropped_to.max(at);
        }
    }

    /// Strike out the calls the person looked at: each by its time, tab and
    /// command. `None` strikes out every one, and the count of those let go
    pub fn seen(&mut self, which: Option<&[Missed]>) {
        match which {
            None => {
                // The calls written down go; the kept ones stay to be handed over
                self.calls.clear();
                self.dropped = 0;
                self.dropped_from = 0;
                self.dropped_to = 0;
            }
            Some(list) => self.calls.retain(|c| !list.iter().any(|s| s.id == c.id)),
        }
    }
}

/// The commands whose first word is the tab they are addressed to: only for
/// these is it written down. Any other command's first word may be what it
/// says -- a short report, a token -- and is never kept
const ADDRESSED: &[&str] = &[
    "ask_tab",
    "tab_run",
    "browser_do",
    "send_to_tab",
    "draft_to_tab",
    "send_keys",
    "tab_screen",
    "tab_read",
    "tab_output",
    "tab_conversation",
    "show",
    "restart",
    "close_tab",
];

/// What of a call's line is written down: its command, and -- for a command
/// addressed to a tab -- the tab its first word names, when that is a tab's
/// name (short and plain)
pub fn read_call(line: &str) -> (String, String) {
    let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
    let method = v["method"].as_str().unwrap_or_default().chars().take(64).collect::<String>();
    let first = v["params"].get(0).and_then(|p| p.as_str()).unwrap_or_default();
    let named = ADDRESSED.contains(&method.as_str())
        && !first.is_empty()
        && first.len() <= 40
        && first.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    (method, if named { first.to_string() } else { String::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(at: u64) -> Missed {
        Missed { id: 0, at, tab: "t".into(), method: "ask_tab".into(), to: String::new(), cut: false }
    }

    /// Past the count or the age, calls are let go of -- and counted, with
    /// the span they covered, so the person hears there were more
    #[test]
    fn calls_past_the_limits_are_let_go_of_and_counted() {
        let day = 24 * 60 * 60;
        let now = 100 * day;
        let mut b = Book::default();
        b.add(call(now - 20 * day), now);
        assert!(b.calls.is_empty(), "one older than the days kept");
        assert_eq!((b.dropped, b.dropped_from, b.dropped_to), (1, now - 20 * day, now - 20 * day));
        for i in 0..(MOST as u64 + 5) {
            b.add(call(now - 1000 + i), now);
        }
        assert_eq!(b.calls.len(), MOST);
        assert_eq!(b.dropped, 6);
        assert_eq!(b.dropped_from, now - 20 * day);
        assert_eq!(b.calls[0].at, now - 1000 + 5, "the oldest go first");
    }

    /// The person strikes out what they looked at, one by one or all at once
    #[test]
    fn what_was_looked_at_is_struck_out() {
        let mut b = Book::default();
        for at in [10, 20, 20, 30] {
            b.add(call(at), 40);
        }
        // The second of two calls in one second, by its own number
        let second = b.calls[2].clone();
        b.seen(Some(&[second]));
        assert_eq!(b.calls.iter().map(|c| (c.at, c.id)).collect::<Vec<_>>(), vec![(10, 1), (20, 2), (30, 4)]);
        b.dropped = 3;
        b.seen(None);
        assert_eq!((b.calls.len(), b.dropped, b.next), (0, 0, 4), "struck out, and numbers never given twice");
    }

    /// A call kept is the tab's to be handed over, oldest first, gone once
    /// handed; looking at what was written down leaves it kept
    #[test]
    fn kept_calls_wait_for_their_tab_and_go_once_handed() {
        let mut b = Book::default();
        let one = b.keep("t1", r#"{"id":"1","method":"report","params":["done"]}"#, 10);
        let two = b.keep("t2", r#"{"id":"2","method":"note","params":["x"]}"#, 11);
        let three = b.keep("t1", r#"{"id":"3","method":"notify","params":["y"]}"#, 12);
        assert!(!b.uid.is_empty());
        assert_eq!(b.kept_for("t1").iter().map(|k| k.id).collect::<Vec<_>>(), vec![one, three]);
        b.seen(None);
        assert_eq!(b.kept.len(), 3, "looking at the calls written down hands nothing over");
        b.handed(one);
        assert_eq!(b.kept_for("t1").iter().map(|k| k.id).collect::<Vec<_>>(), vec![three]);
        assert_eq!(b.kept_for("t2")[0].id, two);
        assert_ne!(kept_id(&b.uid, one), kept_id("another", one), "two books' calls are two calls");
    }

    /// Only a command's name and a tab's name are taken from the line: never
    /// what it said
    #[test]
    fn only_the_command_and_a_tab_name_are_written_down() {
        assert_eq!(read_call(r#"{"id":"1","method":"ask_tab","params":["teal","the secret is 1234"]}"#), ("ask_tab".into(), "teal".into()));
        assert_eq!(read_call(r#"{"id":"1","method":"report","params":["Here is everything I found"]}"#), ("report".into(), String::new()));
        assert_eq!(read_call(r#"{"id":"1","method":"report","params":["ABC123SECRET"]}"#), ("report".into(), String::new()), "a short word of a command not addressed to a tab");
        assert_eq!(read_call("not a call"), (String::new(), String::new()));
    }
}
