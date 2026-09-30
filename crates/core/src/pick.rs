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
    /// How many values in what the page said were hidden before it was kept
    /// (`scrub`). Said on the panel, because the draft is the person's to check
    pub hidden: usize,
}

/// The longest markup the page sends for one element, in characters: the
/// page cuts its markup to this and adds one "…" (`PICK_HTML` in `pagejs`, held
/// equal by a test). Anything longer did not come from that script
pub const PAGE_HTML_MAX: usize = 3000;

/// The longest any other single string of a pick may be, in characters (the
/// markup and the joined styles excepted). The page's longest others are the
/// words (160), a selector and a path over five ancestors, and the address
/// without its query; a thousand leaves room for a long address and still
/// stops a page that pads a field to fill memory
pub const FIELD_MAX: usize = 1000;

/// The most one pick may weigh as JSON. The markup at its longest is 3000
/// characters -- up to 12 KB if every one takes four bytes -- and the rest of
/// a real pick measured under 3 KB; 32 KB takes both with room, and caps what
/// twelve kept picks can hold at under half a megabyte
pub const ITEM_MAX: usize = 32 * 1024;

/// How deep a pick's JSON may nest. The page's deepest is an object of
/// objects (the styles, the bounds): four levels is already twice that
const DEPTH_MAX: usize = 4;

/// The shortest time between two picks a page is believed for. A person's
/// press -- down, up, and the page drawing its outline -- takes longer than
/// this; a page sending faster is a script, not a person
const PICK_GAP: std::time::Duration = std::time::Duration::from_millis(120);

/// Why a page's report was not kept
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// It weighed more, or nested deeper, or had a longer string, than the
    /// page's own script ever sends
    TooBig,
    /// It came sooner after the last one than a person presses
    TooSoon,
    /// It described nothing (no tag)
    Empty,
}

/// Whether what a page said about a pick is the shape and size the page's own
/// script sends, looked at before anything else is done with it -- before its
/// secrets are looked for, before it is kept, before it is written out. A
/// page's script can post whatever it likes while picking is armed
pub fn admit(item: &Value) -> Result<(), Refused> {
    fn fits(v: &Value, depth: usize) -> bool {
        match v {
            Value::String(s) => s.chars().count() <= PAGE_HTML_MAX + 1,
            Value::Array(a) => depth < DEPTH_MAX && a.iter().all(|x| fits(x, depth + 1)),
            Value::Object(o) => depth < DEPTH_MAX && o.iter().all(|(k, x)| k.len() <= 64 && fits(x, depth + 1)),
            _ => true,
        }
    }
    fn long_elsewhere(v: &Value, key: &str) -> bool {
        match v {
            // The markup, and the styles joined into one line (up to 26 of
            // them, a font list or a grid among them), may run to the
            // markup's length; everything else is a name, a path or an address
            Value::String(s) => key != "html" && key != "style" && s.chars().count() > FIELD_MAX,
            Value::Array(a) => a.iter().any(|x| long_elsewhere(x, key)),
            Value::Object(o) => o.iter().any(|(k, x)| long_elsewhere(x, k)),
            _ => false,
        }
    }
    if item.is_null() {
        return Ok(());
    }
    if !item.is_object() || item.get("tag").and_then(Value::as_str).is_none_or(str::is_empty) {
        return Err(Refused::Empty);
    }
    let weight = serde_json::to_vec(item).map(|b| b.len()).unwrap_or(usize::MAX);
    if weight > ITEM_MAX || !fits(item, 0) || long_elsewhere(item, "") {
        return Err(Refused::TooBig);
    }
    Ok(())
}

/// What one page holds: whether presses on it pick right now, and what has
/// been picked
#[derive(Clone, Debug, Default)]
pub struct Picking {
    pub armed: bool,
    pub items: VecDeque<Picked>,
    next: u32,
    /// When the page's last pick was kept, to tell a person from a script
    last: Option<std::time::Instant>,
}

impl Picking {
    /// Whether a pick arriving `now` comes soon enough after the last to be a
    /// script rather than a person. Taking the time counts it as the last
    pub fn too_soon(&mut self, now: std::time::Instant) -> bool {
        if self.last.is_some_and(|last| now.saturating_duration_since(last) < PICK_GAP) {
            return true;
        }
        self.last = Some(now);
        false
    }

    /// Keep one more. Whatever the page sent is kept only if it is an object
    /// with a tag -- a page's script can post anything, and a pick that
    /// describes nothing is not worth a place in somebody's list
    pub fn add(&mut self, item: Value, hidden: usize) -> Option<u32> {
        if !item.is_object() || item.get("tag").and_then(Value::as_str).is_none_or(str::is_empty) {
            return None;
        }
        self.next += 1;
        let n = self.next;
        self.items.push_back(Picked { n, item, note: String::new(), hidden });
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
            hidden: self.items.iter().map(|p| p.hidden).sum(),
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
    /// How many values were hidden across everything listed
    pub hidden: usize,
}

#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct PickRow {
    pub n: u32,
    /// How it is named in a chip: its tag, and the words it shows
    pub label: String,
    pub note: String,
}

/// Everything a page said about an element, with the values that look like
/// keys -- or are keys this program holds -- hidden, and how many were. Every
/// string that leaves is looked at: the markup, the words, the selector, where
/// it sits, its styles, its source and its address
pub fn scrub<'a>(item: Value, known: impl Fn() -> Vec<&'a str>) -> (Value, usize) {
    fn walk<'a>(v: Value, known: &dyn Fn() -> Vec<&'a str>, hidden: &mut usize) -> Value {
        match v {
            Value::String(s) => {
                let (s, a) = crate::secretscan::hide(&s);
                let (s, b) = crate::secretscan::hide_known(&s, known());
                *hidden += a + b;
                Value::String(s)
            }
            Value::Array(a) => Value::Array(a.into_iter().map(|x| walk(x, known, hidden)).collect()),
            Value::Object(o) => Value::Object(o.into_iter().map(|(k, x)| (k, walk(x, known, hidden))).collect()),
            other => other,
        }
    }
    let mut hidden = 0;
    let out = walk(item, &known, &mut hidden);
    (out, hidden)
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

    impl Picking {
        fn add_plain(&mut self, item: Value) -> Option<u32> {
            self.add(item, 0)
        }
    }

    #[test]
    fn everything_a_page_said_is_scrubbed_and_counted() {
        let key = "ghp_aB3cD9eF1gH7iJ5kL0mN2oP4qR6sT8uVwXy";
        let item = json!({"tag": "div", "name": format!("token {key}"),
            "html": format!("<pre>{key}</pre><span>mine-secret-9</span>"), "path": ["main", "section"],
            "box": {"x": 1}});
        let (out, n) = scrub(item, || vec!["mine-secret-9"]);
        let s = out.to_string();
        assert!(!s.contains(key) && !s.contains("mine-secret-9"), "{s}");
        assert_eq!(n, 3);
        assert_eq!(out["box"]["x"], 1, "numbers are left as they are");
        let mut p = Picking::default();
        p.add(out, n);
        assert_eq!(p.state().hidden, 3);
    }

    fn item(tag: &str, name: &str) -> Value {
        json!({"tag": tag, "name": name, "sel": "#x", "url": "http://a.test/p",
               "view": {"w": 800, "h": 600}, "box": {"x": 1, "y": 2, "w": 3, "h": 4},
               "path": ["main", "section.card"], "style": "display: flex", "html": "<b>hi</b>\n<i>x</i>"})
    }

    /// A page's report is looked at for its size before anything else: a
    /// real pick passes, and one a page's own script could not have made --
    /// padded markup, a padded name, a mountain of fields, deep nesting --
    /// is refused before it is scrubbed or kept. Nothing bounded a report
    /// before, so an armed page could fill twelve picks as large as it liked
    #[test]
    fn a_report_bigger_than_the_page_sends_is_refused() {
        let real = item("button", "Save");
        assert_eq!(admit(&real), Ok(()));
        assert_eq!(admit(&Value::Null), Ok(()), "the Escape is not a pick to measure");
        let longest = json!({"tag": "div", "html": format!("{}…", "x".repeat(PAGE_HTML_MAX)),
            "style": "font-family: ".to_string() + &"a, ".repeat(900)});
        assert_eq!(admit(&longest), Ok(()), "the page's own longest markup passes");

        let mut padded = real.clone();
        padded["html"] = json!("x".repeat(PAGE_HTML_MAX + 2));
        assert_eq!(admit(&padded), Err(Refused::TooBig));
        let mut named = real.clone();
        named["name"] = json!("x".repeat(FIELD_MAX + 1));
        assert_eq!(admit(&named), Err(Refused::TooBig));
        let mut heavy = real.clone();
        for i in 0..400 {
            heavy[format!("f{i}")] = json!("y".repeat(200));
        }
        assert_eq!(admit(&heavy), Err(Refused::TooBig), "many small fields weigh too much together");
        let deep = json!({"tag": "div", "a": {"b": {"c": {"d": {"e": 1}}}}});
        assert_eq!(admit(&deep), Err(Refused::TooBig));
        assert_eq!(admit(&json!({"name": "x"})), Err(Refused::Empty));
    }

    /// Picks come at the pace of a person's presses; a page sending them
    /// faster is refused for the ones in between
    #[test]
    fn picks_faster_than_a_person_presses_are_refused() {
        let mut p = Picking::default();
        let t0 = std::time::Instant::now();
        assert!(!p.too_soon(t0));
        assert!(p.too_soon(t0 + std::time::Duration::from_millis(10)));
        assert!(!p.too_soon(t0 + PICK_GAP), "a person's next press is taken");
    }

    /// What the page cuts its markup to is what this side expects it to be
    #[test]
    fn the_page_and_this_side_agree_on_the_markup_limit() {
        assert!(crate::pagejs::AUTOMATION.contains(&format!("PICK_HTML = {PAGE_HTML_MAX}")));
    }

    #[test]
    fn a_pick_that_describes_nothing_is_not_kept() {
        let mut p = Picking::default();
        assert_eq!(p.add_plain(json!("hello")), None);
        assert_eq!(p.add_plain(json!({"name": "x"})), None);
        assert_eq!(p.add_plain(json!({"tag": ""})), None);
        assert_eq!(p.add_plain(item("button", "Save")), Some(1));
    }

    #[test]
    fn the_oldest_makes_room_and_numbers_are_not_reused() {
        let mut p = Picking::default();
        for i in 0..KEPT + 3 {
            p.add_plain(item("a", &i.to_string()));
        }
        assert_eq!(p.items.len(), KEPT);
        assert_eq!(p.items.front().unwrap().n, 4);
        assert!(p.drop_one(5));
        assert_eq!(p.add_plain(item("b", "")), Some(KEPT as u32 + 4));
    }

    #[test]
    fn a_note_is_one_line_and_bounded() {
        let mut p = Picking::default();
        p.add_plain(item("a", ""));
        assert!(p.note(1, &format!("line\none{}", "x".repeat(NOTE_MAX))));
        let n = &p.items[0].note;
        assert!(!n.contains('\n'));
        assert_eq!(n.chars().count(), NOTE_MAX);
        assert!(!p.note(9, "nobody"));
    }

    #[test]
    fn the_description_names_the_page_once_and_indents_markup() {
        let mut p = Picking::default();
        p.add_plain(item("button", "Save"));
        p.add_plain(item("div", ""));
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
