//! What a page said on its console, kept per page for the column's Console
//! panel, an AI, and a script.
//!
//! The browser tells us through the DevTools protocol: what the page's own
//! code logged, what it threw and nobody caught, and what the browser itself
//! had to say about the page (a file that did not load, a request refused).
//! This module turns each of those into one line and keeps the last several
//! hundred; the browser that listens lives with the window (`browser.rs`).
//!
//! Listening starts only when somebody asks -- the panel opened on that page,
//! or a script reading it. Turning on the part of the protocol that reports
//! logging is something some sites watch for, as a sign the browser is being
//! driven, and a sign-in page that refuses a driven browser would stop
//! working for everyone who never looked at a console.

use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;

/// How many lines a page keeps. A person reads back a few screens of a
/// console, not its history; and the lines travel to a phone as they arrive,
/// so what is kept is also what a phone opening the panel is sent at once
pub const KEPT: usize = 400;

/// How long one line may be. A stack trace of a dozen frames fits; a page
/// logging a whole response body does not need to be carried whole to be read
pub const LINE_MAX: usize = 2000;

/// How many lines are handed to an AI at most: about two screenfuls of a
/// terminal, which is what can be looked over before sending. The newest win
pub const HANDED_MAX: usize = 80;

/// How bad a line is, from worst. The names the page uses and the browser's
/// own are folded into these five
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warn,
    Info,
    Log,
    Debug,
}

impl Level {
    fn of(word: &str) -> Level {
        match word {
            "error" | "assert" => Level::Error,
            "warning" | "warn" => Level::Warn,
            "info" => Level::Info,
            "debug" | "verbose" | "trace" => Level::Debug,
            _ => Level::Log,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Log => "log",
            Level::Debug => "debug",
        }
    }
}

/// One line of a page's console
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Line {
    /// Counts up per page and is never reused, so "everything after 41" means
    /// the same lines to the panel, to a phone and to a script
    pub seq: u64,
    pub level: Level,
    /// Who said it: the page's code (`page`), an error nobody caught
    /// (`thrown`), or the browser about the page (`browser`)
    pub from: &'static str,
    pub text: String,
    /// Where it came from, `address:line`, when the browser said
    pub at: String,
    /// When, as milliseconds since 1970
    pub ms: i64,
}

/// What one page has said, and whether anybody is listening
#[derive(Clone, Debug, Default)]
pub struct Book {
    pub listening: bool,
    pub lines: VecDeque<Line>,
    next: u64,
}

impl Book {
    /// Keep one more line, as the browser reported it. Answers its number
    pub fn add(&mut self, entry: &Value) -> Option<u64> {
        let text = entry.get("text").and_then(Value::as_str).unwrap_or_default();
        if text.is_empty() {
            return None;
        }
        self.next += 1;
        let from = match entry.get("from").and_then(Value::as_str) {
            Some("thrown") => "thrown",
            Some("browser") => "browser",
            _ => "page",
        };
        self.lines.push_back(Line {
            seq: self.next,
            level: Level::of(entry.get("level").and_then(Value::as_str).unwrap_or_default()),
            from,
            text: cut(text, LINE_MAX),
            at: entry.get("at").and_then(Value::as_str).unwrap_or_default().to_string(),
            ms: entry.get("ms").and_then(Value::as_i64).unwrap_or(0),
        });
        while self.lines.len() > KEPT {
            self.lines.pop_front();
        }
        Some(self.next)
    }

    /// Lines after `since`, oldest first
    pub fn after(&self, since: u64) -> Vec<Line> {
        self.lines.iter().filter(|l| l.seq > since).cloned().collect()
    }

    /// The number the next line will be one more than
    pub fn last(&self) -> u64 {
        self.next
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// A value the page logged, as the console would print it
fn shown(arg: &Value) -> String {
    match arg.get("value") {
        Some(Value::String(s)) => s.clone(),
        Some(v) if !v.is_null() => v.to_string(),
        _ => arg
            .get("description")
            .and_then(Value::as_str)
            .or_else(|| arg.get("unserializableValue").and_then(Value::as_str))
            .or_else(|| arg.get("type").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string(),
    }
}

fn place(url: Option<&str>, line: Option<i64>) -> String {
    match (url.filter(|u| !u.is_empty()), line) {
        // The protocol counts lines from 0; a person counts from 1
        (Some(u), Some(n)) => format!("{u}:{}", n + 1),
        (Some(u), None) => u.to_string(),
        _ => String::new(),
    }
}

/// One protocol event as one line to keep, or nothing when the event is not
/// one of the three this listens to. `ms` is when it arrived
pub fn entry_of(method: &str, params: &Value, ms: i64) -> Option<Value> {
    let (level, from, text, at) = match method {
        "Runtime.consoleAPICalled" => {
            let args = params.get("args").and_then(Value::as_array)?;
            let text = args.iter().map(shown).collect::<Vec<_>>().join(" ");
            let top = params.pointer("/stackTrace/callFrames/0");
            let at = place(
                top.and_then(|f| f.get("url")).and_then(Value::as_str),
                top.and_then(|f| f.get("lineNumber")).and_then(Value::as_i64),
            );
            let kind = params.get("type").and_then(Value::as_str).unwrap_or("log");
            (kind.to_string(), "page", text, at)
        }
        "Runtime.exceptionThrown" => {
            let d = params.get("exceptionDetails")?;
            let text = d
                .pointer("/exception/description")
                .and_then(Value::as_str)
                .or_else(|| d.get("text").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string();
            let at = place(d.get("url").and_then(Value::as_str), d.get("lineNumber").and_then(Value::as_i64));
            ("error".to_string(), "thrown", text, at)
        }
        "Log.entryAdded" => {
            let e = params.get("entry")?;
            let text = e.get("text").and_then(Value::as_str).unwrap_or_default().to_string();
            let at = place(e.get("url").and_then(Value::as_str), e.get("lineNumber").and_then(Value::as_i64));
            let level = e.get("level").and_then(Value::as_str).unwrap_or("info").to_string();
            (level, "browser", text, at)
        }
        _ => return None,
    };
    Some(serde_json::json!({ "level": level, "from": from, "text": text, "at": at, "ms": ms }))
}

/// The events a browser has to be listening to for `entry_of` to be told
/// anything, and the protocol domains that have to be on for them
pub const EVENTS: [&str; 3] = ["Runtime.consoleAPICalled", "Runtime.exceptionThrown", "Log.entryAdded"];
pub const DOMAINS: [&str; 2] = ["Runtime", "Log"];

/// Lines written out for an AI's input: what page, then one line each, the
/// newest `HANDED_MAX` of them. Plain text, as the picks are
pub fn describe(url: &str, lines: &[Line]) -> String {
    let skip = lines.len().saturating_sub(HANDED_MAX);
    let mut out = format!("Console of the web page {url} ({} line(s)):\n", lines.len() - skip);
    for l in &lines[skip..] {
        out.push_str(&format!("[{}] {}", l.level.word(), l.text));
        if !l.at.is_empty() {
            out.push_str(&format!("  ({})", l.at));
        }
        out.push('\n');
    }
    out
}

/// What the input is given when the lines went into a file instead
pub fn pointer(path: &str, url: &str) -> String {
    format!("The console of the web page {url} is in {path}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_logged_line_reads_as_the_console_prints_it() {
        let e = entry_of(
            "Runtime.consoleAPICalled",
            &json!({"type": "warning", "args": [{"type": "string", "value": "low"}, {"type": "number", "value": 3},
                     {"type": "object", "description": "Object"}],
                    "stackTrace": {"callFrames": [{"url": "http://a/app.js", "lineNumber": 9}]}}),
            5,
        )
        .unwrap();
        assert_eq!(e["text"], "low 3 Object");
        assert_eq!(e["at"], "http://a/app.js:10", "lines are counted from 1");
        let mut b = Book::default();
        b.add(&e);
        assert_eq!(b.lines[0].level, Level::Warn);
        assert_eq!(b.lines[0].from, "page");
    }

    #[test]
    fn an_uncaught_error_and_the_browsers_word_are_both_heard() {
        let thrown = entry_of(
            "Runtime.exceptionThrown",
            &json!({"exceptionDetails": {"text": "Uncaught", "exception": {"description": "TypeError: x is undefined"},
                    "url": "http://a/b.js", "lineNumber": 0}}),
            1,
        )
        .unwrap();
        assert_eq!(thrown["level"], "error");
        assert_eq!(thrown["text"], "TypeError: x is undefined");
        let browser = entry_of(
            "Log.entryAdded",
            &json!({"entry": {"source": "network", "level": "error", "text": "Failed to load resource: 404",
                    "url": "http://a/missing.png"}}),
            1,
        )
        .unwrap();
        assert_eq!(browser["from"], "browser");
        assert_eq!(browser["at"], "http://a/missing.png");
        assert!(entry_of("Page.loadEventFired", &json!({}), 1).is_none());
    }

    #[test]
    fn a_book_keeps_the_newest_and_counts_on() {
        let mut b = Book::default();
        for i in 0..KEPT + 5 {
            b.add(&json!({"text": format!("n{i}"), "level": "log"}));
        }
        assert_eq!(b.lines.len(), KEPT);
        assert_eq!(b.lines.front().unwrap().seq, 6);
        assert_eq!(b.after(KEPT as u64 + 3).len(), 2);
        b.clear();
        assert_eq!(b.add(&json!({"text": "x"})), Some(KEPT as u64 + 6), "numbers go on after a clear");
        assert_eq!(b.add(&json!({"text": ""})), None, "an empty line is not kept");
    }

    #[test]
    fn a_long_line_is_cut() {
        let mut b = Book::default();
        b.add(&json!({"text": "x".repeat(LINE_MAX + 50)}));
        assert_eq!(b.lines[0].text.chars().count(), LINE_MAX + 1);
    }

    #[test]
    fn what_an_ai_is_handed_is_the_newest() {
        let mut b = Book::default();
        for i in 0..HANDED_MAX + 3 {
            b.add(&json!({"text": format!("n{i}"), "level": "error", "at": "u:1"}));
        }
        let lines: Vec<Line> = b.lines.iter().cloned().collect();
        let s = describe("http://a/", &lines);
        assert!(s.starts_with(&format!("Console of the web page http://a/ ({HANDED_MAX} line(s)):\n")));
        assert!(!s.contains("] n2  "), "the oldest are left out");
        assert!(s.contains(&format!("[error] n{}  (u:1)\n", HANDED_MAX + 2)));
    }
}
