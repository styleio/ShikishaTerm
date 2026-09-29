//! Elements a person picked on a page, to hand to an AI (🎯).
//!
//! The page describes what was pressed (see `pagejs`, `pickDescribe`); this
//! side keeps what was picked per page, lets a note ride on each, and writes
//! them out as the words an AI's input is given. Kept here rather than in the
//! capabilities so the writing can be tested without a page, and so the board
//! and a script read the very same list.

use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;

/// How many picks a page keeps. The 🎯 panel lays them out in one row to be
/// looked over before sending; twelve still fit a window's width as chips, and
/// a person pointing at more than that at once is describing a page, which a
/// screenshot does better. The oldest makes room for the newest
pub const KEPT: usize = 12;

/// Past this many characters, what is handed over goes into a file and the
/// input is given its path. A draft is something a person reads and adds to
/// before sending: beyond about two screenfuls of a terminal it cannot be
/// read there, and pasting it into a CLI takes seconds of paced typing
/// (`send.rs`) during which the tab cannot be used
pub const INLINE_MAX: usize = 6000;

/// A note is one line of what the person wants done with this element. Long
/// enough for a sentence or two; the place for more is the AI's own input
pub const NOTE_MAX: usize = 400;

/// One element, as the page described it, with the person's note
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    /// Its number while the page keeps it (1, 2, ...). Numbers are not reused
    /// after one is taken out, so "number 3" keeps meaning the same element
    pub n: u32,
    pub item: Value,
    pub note: String,
}

/// What one page holds: whether presses on it pick right now, and what has
/// been picked
#[derive(Clone, Debug, Default)]
pub struct Picking {
    pub armed: bool,
    pub items: VecDeque<Picked>,
    next: u32,
}

impl Picking {
    /// Keep one more. Whatever the page sent is kept only if it is an object
    /// with a tag -- a page's script can post anything, and a pick that
    /// describes nothing is not worth a place in somebody's list
    pub fn add(&mut self, item: Value) -> Option<u32> {
        if !item.is_object() || item.get("tag").and_then(Value::as_str).is_none_or(str::is_empty) {
            return None;
        }
        self.next += 1;
        let n = self.next;
        self.items.push_back(Picked { n, item, note: String::new() });
        while self.items.len() > KEPT {
            self.items.pop_front();
        }
        Some(n)
    }

    pub fn note(&mut self, n: u32, text: &str) -> bool {
        let Some(p) = self.items.iter_mut().find(|p| p.n == n) else { return false };
        p.note = text.chars().take(NOTE_MAX).collect::<String>().replace(['\r', '\n'], " ");
        true
    }

    pub fn drop_one(&mut self, n: u32) -> bool {
        let before = self.items.len();
        self.items.retain(|p| p.n != n);
        self.items.len() != before
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// What the board draws for this page
    pub fn state(&self) -> PickState {
        PickState {
            on: self.armed,
            items: self
                .items
                .iter()
                .map(|p| PickRow { n: p.n, label: label(&p.item), note: p.note.clone() })
                .collect(),
        }
    }
}

/// The 🎯 panel's view of one page
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct PickState {
    /// Presses on the page pick right now
    pub on: bool,
    pub items: Vec<PickRow>,
}

#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct PickRow {
    pub n: u32,
    /// How it is named in a chip: its tag, and the words it shows
    pub label: String,
    pub note: String,
}

fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn num(v: &Value, a: &str, b: &str) -> i64 {
    v.get(a).and_then(|x| x.get(b)).and_then(Value::as_i64).unwrap_or(0)
}

/// `button "Save"`: what a person would call it, short enough for a chip
pub fn label(item: &Value) -> String {
    let tag = text(item, "tag");
    let name: String = text(item, "name").chars().take(24).collect();
    let cut = if text(item, "name").chars().count() > 24 { "…" } else { "" };
    if name.is_empty() { tag.to_string() } else { format!("{tag} \"{name}{cut}\"") }
}

/// The picked elements, written for an AI's input.
///
/// Plain text, not Markdown: a CLI's input box shows what is pasted as it is,
/// and headings made of `#` read as noise there. Grouped by the page they were
/// picked on, because the address is what an AI needs to find the code, and
/// several picks usually share one
pub fn describe(items: &[Picked]) -> String {
    let mut out = String::from("Elements I picked on a web page:\n");
    let mut page = None::<String>;
    for p in items {
        let v = &p.item;
        let url = text(v, "url").to_string();
        if page.as_deref() != Some(url.as_str()) {
            let (w, h) = (num(v, "view", "w"), num(v, "view", "h"));
            out.push_str(&format!("\nPage: {url} (window {w}x{h})\n"));
            page = Some(url);
        }
        let tag = text(v, "tag");
        let name = text(v, "name");
        let role = text(v, "role");
        out.push_str(&format!("\n{}. <{tag}>", p.n));
        if !role.is_empty() {
            out.push_str(&format!(" role={role}"));
        }
        if !name.is_empty() {
            out.push_str(&format!(" \"{name}\""));
        }
        out.push('\n');
        let line = |out: &mut String, k: &str, val: &str| {
            if !val.trim().is_empty() {
                out.push_str(&format!("   {k}: {val}\n"));
            }
        };
        line(&mut out, "Note", &p.note);
        line(&mut out, "Selector", text(v, "sel"));
        let path: Vec<&str> = v
            .get("path")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        line(&mut out, "Inside", &path.join(" > "));
        line(&mut out, "Source", text(v, "source"));
        line(
            &mut out,
            "Box",
            &format!(
                "{}x{} at x {}, y {}",
                num(v, "box", "w"),
                num(v, "box", "h"),
                num(v, "box", "x"),
                num(v, "box", "y")
            ),
        );
        line(&mut out, "Style", text(v, "style"));
        let html = text(v, "html");
        if !html.trim().is_empty() {
            out.push_str("   Markup:\n");
            for l in html.lines() {
                out.push_str("     ");
                out.push_str(l);
                out.push('\n');
            }
        }
    }
    out
}

/// What the input is given when the description went into a file instead
pub fn pointer(path: &str, count: usize) -> String {
    format!("I picked {count} element(s) on a web page; they are described in {path}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(tag: &str, name: &str) -> Value {
        json!({"tag": tag, "name": name, "sel": "#x", "url": "http://a.test/p",
               "view": {"w": 800, "h": 600}, "box": {"x": 1, "y": 2, "w": 3, "h": 4},
               "path": ["main", "section.card"], "style": "display: flex", "html": "<b>hi</b>\n<i>x</i>"})
    }

    #[test]
    fn a_pick_that_describes_nothing_is_not_kept() {
        let mut p = Picking::default();
        assert_eq!(p.add(json!("hello")), None);
        assert_eq!(p.add(json!({"name": "x"})), None);
        assert_eq!(p.add(json!({"tag": ""})), None);
        assert_eq!(p.add(item("button", "Save")), Some(1));
    }

    #[test]
    fn the_oldest_makes_room_and_numbers_are_not_reused() {
        let mut p = Picking::default();
        for i in 0..KEPT + 3 {
            p.add(item("a", &i.to_string()));
        }
        assert_eq!(p.items.len(), KEPT);
        assert_eq!(p.items.front().unwrap().n, 4);
        assert!(p.drop_one(5));
        assert_eq!(p.add(item("b", "")), Some(KEPT as u32 + 4));
    }

    #[test]
    fn a_note_is_one_line_and_bounded() {
        let mut p = Picking::default();
        p.add(item("a", ""));
        assert!(p.note(1, &format!("line\none{}", "x".repeat(NOTE_MAX))));
        let n = &p.items[0].note;
        assert!(!n.contains('\n'));
        assert_eq!(n.chars().count(), NOTE_MAX);
        assert!(!p.note(9, "nobody"));
    }

    #[test]
    fn the_description_names_the_page_once_and_indents_markup() {
        let mut p = Picking::default();
        p.add(item("button", "Save"));
        p.add(item("div", ""));
        p.note(1, "make it blue");
        let items: Vec<Picked> = p.items.iter().cloned().collect();
        let s = describe(&items);
        assert_eq!(s.matches("Page: http://a.test/p (window 800x600)").count(), 1);
        assert!(s.contains("1. <button> \"Save\"\n   Note: make it blue\n   Selector: #x\n"));
        assert!(s.contains("   Inside: main > section.card\n"));
        assert!(s.contains("     <b>hi</b>\n     <i>x</i>\n"));
        assert!(s.contains("\n2. <div>\n"));
        assert!(!s.contains("Note: \n"), "an empty note says nothing");
    }

    #[test]
    fn a_chip_is_short() {
        assert_eq!(label(&item("button", "Save")), "button \"Save\"");
        assert_eq!(label(&item("div", "")), "div");
        assert!(label(&item("p", &"w".repeat(40))).ends_with("…\""));
    }
}
