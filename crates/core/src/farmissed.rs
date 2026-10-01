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

use serde::{Deserialize, Serialize};

/// The file, in the bridge's own folder: it outlives the resident process,
/// so an app that connects to the next one is still told
pub const FILE: &str = "missed.json";
/// How many calls are kept
pub const MOST: usize = 200;
/// How many days a call is kept
pub const DAYS: u64 = 14;

/// One call that did not reach its app
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Missed {
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
}

impl Book {
    /// Write a call down, letting go of what is past the limits as of `now`
    pub fn add(&mut self, call: Missed, now: u64) {
        self.calls.push(call);
        self.trim(now);
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
        for c in gone {
            self.dropped += 1;
            self.dropped_from = if self.dropped_from == 0 { c.at } else { self.dropped_from.min(c.at) };
            self.dropped_to = self.dropped_to.max(c.at);
        }
    }

    /// Strike out the calls the person looked at: each by its time, tab and
    /// command. `None` strikes out every one, and the count of those let go
    pub fn seen(&mut self, which: Option<&[Missed]>) {
        match which {
            None => *self = Book::default(),
            Some(list) => self.calls.retain(|c| !list.iter().any(|s| s.at == c.at && s.tab == c.tab && s.method == c.method)),
        }
    }
}

/// What of a call's line is written down: its command, and the tab its first
/// word names when it is a tab's name. A name is short and plain; a first
/// word that is not (a sentence, a path, a secret) is not kept
pub fn read_call(line: &str) -> (String, String) {
    let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
    let method = v["method"].as_str().unwrap_or_default().chars().take(64).collect::<String>();
    let first = v["params"].get(0).and_then(|p| p.as_str()).unwrap_or_default();
    let named = !first.is_empty() && first.len() <= 40 && first.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    (method, if named { first.to_string() } else { String::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(at: u64) -> Missed {
        Missed { at, tab: "t".into(), method: "ask_tab".into(), to: String::new(), cut: false }
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
        for at in [10, 20, 30] {
            b.add(call(at), 40);
        }
        b.seen(Some(&[call(20)]));
        assert_eq!(b.calls.iter().map(|c| c.at).collect::<Vec<_>>(), vec![10, 30]);
        b.dropped = 3;
        b.seen(None);
        assert_eq!(b, Book::default());
    }

    /// Only a command's name and a tab's name are taken from the line: never
    /// what it said
    #[test]
    fn only_the_command_and_a_tab_name_are_written_down() {
        assert_eq!(read_call(r#"{"id":"1","method":"ask_tab","params":["teal","the secret is 1234"]}"#), ("ask_tab".into(), "teal".into()));
        assert_eq!(read_call(r#"{"id":"1","method":"report","params":["Here is everything I found"]}"#), ("report".into(), String::new()));
        assert_eq!(read_call("not a call"), (String::new(), String::new()));
    }
}
