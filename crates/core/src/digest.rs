//! Page digest: distill a page into the list of elements an agent can act on.
//!
//! Two CDP sources are merged, and both are needed:
//! - `Accessibility.getFullAXTree` supplies role and accessible name, computed
//!   by the browser itself (label/aria/alt resolution exactly as a screen
//!   reader would see it). Recomputing accessible names by hand stays
//!   permanently "almost right", so the browser's own answer is used.
//! - `DOMSnapshot.captureSnapshot` supplies layout (position, visibility),
//!   attributes, and the two "this reacts to clicks" signals the AX tree
//!   misses when a site skips semantics: Chromium's own `isClickable` mark
//!   (nodes with click handlers) and a computed `cursor: pointer` boundary.
//!
//! The merged list is numbered. The numbers (`ref`) are what the operate
//! primitives accept as `{ref=N}`; the caller keeps `refs` to resolve a number
//! back to the CDP backendNodeId it was minted from.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub struct Digest {
    /// One element per line: `[N] role "name" extras…`, ready to show an AI
    pub text: String,
    /// `refs[N-1]` = the backendNodeId behind `[N]`
    pub refs: Vec<i64>,
    /// The same elements as data, for a caller that has to build a question
    /// out of them rather than show them to somebody. Same numbers, same
    /// order, same source -- written here rather than parsed back out of the
    /// text, because a second reader of our own format is a second format
    pub elements: Vec<serde_json::Value>,
}

/// Hard ceiling on emitted lines. Never truncates silently: when hit, the tail
/// line says how many elements were left out
const MAX_LINES: usize = 1200;
const NAME_MAX: usize = 80;
/// How far in a line may be indented before the indent costs more than it says
const MAX_DEPTH: usize = 6;
const HREF_MAX: usize = 160;
const VALUE_MAX: usize = 60;

/// One parsed DOMSnapshot document (the parallel arrays, re-shaped)
struct SnapDoc {
    parent: Vec<i64>,
    node_type: Vec<i64>,
    /// lowercased element name ("div"), or "#text" etc.
    tag: Vec<String>,
    /// nodeValue (text nodes carry their text here)
    value: Vec<String>,
    backend: Vec<i64>,
    /// lowercased attribute name -> value
    attrs: Vec<HashMap<String, String>>,
    /// Chromium's "responds to mouse clicks" mark (anchors + JS listeners)
    clickable: HashSet<usize>,
    input_value: HashMap<usize, String>,
    input_checked: HashSet<usize>,
    /// iframe node -> index of its document in `documents` (same-process only)
    content_doc: HashSet<usize>,
    /// computed style `cursor: pointer` (from the layout tree)
    cursor_pointer: HashSet<usize>,
    /// document-coordinate [x, y, w, h] for laid-out nodes
    bounds: HashMap<usize, [f64; 4]>,
    /// How tall a node's box is on screen, and how tall its content really is.
    /// The two differ exactly where something scrolls inside the page
    boxes: HashMap<usize, (f64, f64)>,
    scroll: (f64, f64),
    children: Vec<Vec<usize>>,
}

impl SnapDoc {
    /// How many screenfuls hide inside this node, when it is a scrolling box
    /// of its own. `None` when everything it holds is already in view
    fn scrolls(&self, at: usize) -> Option<f64> {
        let (client, content) = self.boxes.get(&at).copied()?;
        // A few pixels of slack: sub-pixel layout makes almost every box
        // "scrollable" by a hair, and saying so about all of them says nothing
        if client <= 0.0 || content <= client + 8.0 {
            return None;
        }
        Some(content / client)
    }
}

fn f64_of(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(0.0)
}

fn i64s(v: Option<&Value>) -> Vec<i64> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(|x| x.as_i64().unwrap_or(-1)).collect())
        .unwrap_or_default()
}

/// Resolve a string-table index (-1 and out-of-range become "")
fn s<'a>(strings: &'a [&'a str], idx: i64) -> &'a str {
    usize::try_from(idx).ok().and_then(|i| strings.get(i).copied()).unwrap_or("")
}

/// A RareBooleanData ({"index":[…]}) as a set
fn rare_bool(v: Option<&Value>) -> HashSet<usize> {
    i64s(v.and_then(|r| r.get("index")))
        .into_iter()
        .filter_map(|i| usize::try_from(i).ok())
        .collect()
}

/// A RareStringData ({"index":[…],"value":[…]}) as index -> resolved string
fn rare_string(v: Option<&Value>, strings: &[&str]) -> HashMap<usize, String> {
    let idx = i64s(v.and_then(|r| r.get("index")));
    let val = i64s(v.and_then(|r| r.get("value")));
    idx.into_iter()
        .zip(val)
        .filter_map(|(i, sv)| usize::try_from(i).ok().map(|i| (i, s(strings, sv).to_string())))
        .collect()
}

fn parse_doc(doc: &Value, strings: &[&str]) -> SnapDoc {
    let nodes = doc.get("nodes");
    let parent = i64s(nodes.and_then(|n| n.get("parentIndex")));
    let node_type = i64s(nodes.and_then(|n| n.get("nodeType")));
    let tag: Vec<String> = i64s(nodes.and_then(|n| n.get("nodeName")))
        .into_iter()
        .map(|i| s(strings, i).to_lowercase())
        .collect();
    let value: Vec<String> = i64s(nodes.and_then(|n| n.get("nodeValue")))
        .into_iter()
        .map(|i| s(strings, i).to_string())
        .collect();
    let backend = i64s(nodes.and_then(|n| n.get("backendNodeId")));
    let attrs: Vec<HashMap<String, String>> = nodes
        .and_then(|n| n.get("attributes"))
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let flat = i64s(Some(row));
                    flat.chunks(2)
                        .filter(|c| c.len() == 2)
                        .map(|c| (s(strings, c[0]).to_lowercase(), s(strings, c[1]).to_string()))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();

    let n = parent.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, &p) in parent.iter().enumerate() {
        if let Ok(p) = usize::try_from(p)
            && p < n {
                children[p].push(i);
            }
    }

    // Layout: which nodes have a box, where it is, and their computed cursor
    // (we request exactly ["cursor"], so styles[i][0] is it)
    let layout = doc.get("layout");
    let l_nodes = i64s(layout.and_then(|l| l.get("nodeIndex")));
    let mut bounds = HashMap::new();
    let mut cursor_pointer = HashSet::new();
    if let Some(bs) = layout.and_then(|l| l.get("bounds")).and_then(Value::as_array) {
        for (li, b) in bs.iter().enumerate() {
            let (Some(&ni), Some(b)) = (l_nodes.get(li), b.as_array()) else { continue };
            let Ok(ni) = usize::try_from(ni) else { continue };
            if b.len() == 4 {
                bounds.insert(
                    ni,
                    [f64_of(b.first()), f64_of(b.get(1)), f64_of(b.get(2)), f64_of(b.get(3))],
                );
            }
        }
    }
    // Drawn height against content height. `clientRects`/`scrollRects` arrive
    // only when the snapshot was asked for DOM rects; without them nothing is
    // reported as scrollable, which is the honest answer rather than a guess
    let mut boxes = HashMap::new();
    if let (Some(cr), Some(sr)) = (
        layout.and_then(|l| l.get("clientRects")).and_then(Value::as_array),
        layout.and_then(|l| l.get("scrollRects")).and_then(Value::as_array),
    ) {
        for (li, (c, s)) in cr.iter().zip(sr).enumerate() {
            let (Some(&ni), Some(c), Some(s)) = (l_nodes.get(li), c.as_array(), s.as_array()) else {
                continue;
            };
            let Ok(ni) = usize::try_from(ni) else { continue };
            if c.len() == 4 && s.len() == 4 {
                boxes.insert(ni, (f64_of(c.get(3)), f64_of(s.get(3))));
            }
        }
    }
    if let Some(styles) = layout.and_then(|l| l.get("styles")).and_then(Value::as_array) {
        for (li, row) in styles.iter().enumerate() {
            let (Some(&ni), Some(row)) = (l_nodes.get(li), row.as_array()) else { continue };
            let Ok(ni) = usize::try_from(ni) else { continue };
            if s(strings, row.first().and_then(Value::as_i64).unwrap_or(-1)) == "pointer" {
                cursor_pointer.insert(ni);
            }
        }
    }

    SnapDoc {
        parent,
        node_type,
        tag,
        value,
        backend,
        attrs: {
            let mut a = attrs;
            a.resize(n, HashMap::new());
            a
        },
        clickable: rare_bool(nodes.and_then(|x| x.get("isClickable"))),
        input_value: rare_string(nodes.and_then(|x| x.get("inputValue")), strings),
        input_checked: rare_bool(nodes.and_then(|x| x.get("inputChecked"))),
        content_doc: rare_bool(nodes.and_then(|x| x.get("contentDocumentIndex"))),
        cursor_pointer,
        bounds,
        boxes,
        scroll: (
            f64_of(doc.get("scrollOffsetX")),
            f64_of(doc.get("scrollOffsetY")),
        ),
        children,
    }
}

/// Map a CDP AX role to the label shown in the digest. `None` = not an element
/// the digest cares about. CDP reports Blink's internal role names
/// ("textField", "checkBox", …); ARIA-style spellings are accepted too
fn role_label(role: &str) -> Option<&'static str> {
    Some(match role {
        "button" | "popupbutton" | "togglebutton" => "button",
        "link" => "link",
        "textfield" | "textbox" | "textfieldwithcombobox" | "searchbox" | "date" | "datetime"
        | "inputtime" | "time" | "colorwell" => "textbox",
        "combobox" | "comboboxmenubutton" | "comboboxgrouping" | "comboboxselect" | "listbox" => {
            "combobox"
        }
        "checkbox" => "checkbox",
        "radiobutton" | "radio" => "radio",
        "switch" => "switch",
        "slider" => "slider",
        "spinbutton" => "spinbutton",
        "menuitem" | "menuitemcheckbox" | "menuitemradio" => "menuitem",
        "tab" => "tab",
        "option" | "listboxoption" | "menulistoption" => "option",
        // Not operable, but headings anchor the reader ("which section am I in")
        "heading" => "heading",
        _ => return None,
    })
}

/// Roles that stay listed even without a name (an unlabeled field is still
/// operable; an unlabeled link is only noise)
fn keep_nameless(role: &str) -> bool {
    matches!(
        role,
        "textbox" | "combobox" | "checkbox" | "radio" | "switch" | "slider" | "spinbutton"
    )
}

/// Collapse whitespace runs and cap length (by characters, so multibyte
/// text is never split)
fn tidy(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut last_ws = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            // A quote would break the `role "name"` shape when read back
            out.push(if ch == '"' { '”' } else { ch });
            last_ws = false;
        }
    }
    let trimmed = out.trim_end();
    let mut clipped: String = trimmed.chars().take(max).collect();
    if trimmed.chars().count() > max {
        clipped.push('…');
    }
    clipped
}

/// Gather the text a human sees inside a node: descendant text nodes, plus
/// image alt / aria-label / title attributes. Used for JS-clickables, which
/// have no accessible name to borrow
fn harvest_text(doc: &SnapDoc, at: usize) -> String {
    let mut out = String::new();
    let mut stack = vec![at];
    while let Some(i) = stack.pop() {
        if out.chars().count() > NAME_MAX * 2 {
            break;
        }
        if doc.node_type.get(i) == Some(&3) {
            out.push(' ');
            out.push_str(doc.value.get(i).map(String::as_str).unwrap_or(""));
        }
        if let Some(a) = doc.attrs.get(i) {
            for key in ["aria-label", "alt", "title"] {
                if let Some(v) = a.get(key) {
                    out.push(' ');
                    out.push_str(v);
                }
            }
        }
        if let Some(kids) = doc.children.get(i) {
            // push in reverse so text comes out in document order
            for &k in kids.iter().rev() {
                stack.push(k);
            }
        }
    }
    tidy(&out, NAME_MAX)
}

/// One future line of the digest, still carrying its sort position
struct Entry {
    /// (document, node) position, so AX finds and supplements interleave in
    /// document order
    order: (usize, usize),
    backend: Option<i64>,
    role: String,
    name: String,
    extras: Vec<String>,
    off_screen: bool,
    /// Which verbs apply here: any of "click", "fill", "select", "scroll".
    /// Offering an element for an operation it cannot perform is how a
    /// decision becomes a wasted move
    ops: Vec<&'static str>,
}

/// Which verbs apply to an element of this role.
///
/// The one place that decides it, because two places would eventually offer
/// a button for typing into. `listed` says the element is a real `<select>`,
/// which is the difference between choosing a value and typing one
fn verbs_for(role: &str, listed: bool, scrolls: bool) -> Vec<&'static str> {
    let mut ops: Vec<&'static str> = match role {
        "heading" | "iframe" => Vec::new(),
        // `listed` on an option means it belongs to a real <select>
        "option" if listed => Vec::new(),
        "combobox" if listed => vec!["select", "click"],
        "textbox" | "combobox" | "spinbutton" => vec!["fill", "click"],
        "scrollbox" => Vec::new(),
        _ => vec!["click"],
    };
    if scrolls {
        ops.push("scroll");
    }
    ops
}

/// The select's own choices, written onto its line.
///
/// A dropdown whose options are only discoverable by opening it costs a whole
/// move to look inside; the browser already knows them, so they are simply
/// said. Capped, because a country list would otherwise be the whole digest
fn select_options(d: &SnapDoc, at: usize) -> Option<String> {
    const SHOWN: usize = 6;
    let mut labels: Vec<String> = Vec::new();
    // Pushed back to front, so popping walks the options the way they are read
    let mut stack: Vec<usize> =
        d.children.get(at).map(|k| k.iter().rev().copied().collect()).unwrap_or_default();
    let mut total = 0usize;
    while let Some(i) = stack.pop() {
        match d.tag.get(i).map(String::as_str) {
            // optgroup holds its options one level further down
            Some("optgroup") => {
                if d.attrs.get(i).is_some_and(|a| a.contains_key("disabled")) {
                    continue;
                }
                if let Some(kids) = d.children.get(i) {
                    stack.extend(kids.iter().rev().copied());
                }
            }
            Some("option") => {
                // Only what can actually be chosen. Listing a choice the page
                // refuses is how a decision becomes a wasted move, and the
                // one refusing it is the same code that wrote this line
                if d.attrs.get(i).is_some_and(|a| a.contains_key("disabled")) {
                    continue;
                }
                total += 1;
                if labels.len() < SHOWN {
                    let text = harvest_text(d, i);
                    if !text.is_empty() {
                        labels.push(text);
                    }
                }
            }
            _ => {}
        }
    }
    if total == 0 {
        return None;
    }
    let mut out = format!("choices={total}");
    if !labels.is_empty() {
        out.push_str(&format!(" [{}", labels.join(" | ")));
        out.push_str(if total > labels.len() { " | …]" } else { "]" });
    }
    Some(out)
}

/// Everything the reader is told, in one place, so that adding a mark means
/// adding its explanation (a legend that drifts from the lines is worse than
/// no legend: it teaches the reader something untrue)
const LEGEND: &str = "# operable elements — act by number: browser_click(BR,{ref=N}) / browser_fill(BR,{ref=N},\"text\")\n\
     # indenting shows what sits inside what; + marks what appeared since the last look; §… names the section (heading) an element sits under\n\
     # roles marked * are JS-clickables without a standard role; scrolls=N means N screenfuls hide inside it — browser_scroll(BR, \"down\", {ref=N})\n\
     # take a new digest after the page changes\n";

pub fn build(ax: &Value, snap: &Value, metrics: &Value) -> Digest {
    build_against(ax, snap, metrics, &[])
}

/// As [`build`], marking what was not in `prev` (the refs of the digest taken
/// just before this one) with a leading `+`.
///
/// Knowing which three lines are new is the difference between reading a
/// menu that just opened and reading the whole page again. There is no
/// page identity to compare against -- the browser hands out fresh node ids
/// for a fresh document -- so a wholesale change is taken for what it almost
/// always is, a different page, and nothing is marked at all. Marking every
/// line says no more than marking none, and costs a screenful
pub fn build_against(ax: &Value, snap: &Value, metrics: &Value, prev: &[i64]) -> Digest {
    let strings: Vec<&str> = snap
        .get("strings")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| v.as_str().unwrap_or("")).collect())
        .unwrap_or_default();
    let docs: Vec<SnapDoc> = snap
        .get("documents")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|d| parse_doc(d, &strings)).collect())
        .unwrap_or_default();

    let mut where_of: HashMap<i64, (usize, usize)> = HashMap::new();
    for (di, d) in docs.iter().enumerate() {
        for (ni, &b) in d.backend.iter().enumerate() {
            if b >= 0 {
                where_of.entry(b).or_insert((di, ni));
            }
        }
    }

    // Viewport (CSS px). Zero/absent viewport disables the off_screen flag
    // rather than flagging everything
    let vp = metrics.get("cssVisualViewport");
    let (vw, vh) = (
        f64_of(vp.and_then(|v| v.get("clientWidth"))),
        f64_of(vp.and_then(|v| v.get("clientHeight"))),
    );
    let off_screen = |di: usize, ni: usize| -> bool {
        if vw <= 0.0 || vh <= 0.0 {
            return false;
        }
        let Some(d) = docs.get(di) else { return false };
        let Some(b) = d.bounds.get(&ni) else {
            // In the AX tree but never laid out: treat as off-screen, not
            // invisible — the AX tree already dropped display:none nodes
            return true;
        };
        let (sx, sy) = d.scroll;
        !(b[0] + b[2] > sx && b[0] < sx + vw && b[1] + b[3] > sy && b[1] < sy + vh)
    };

    let mut included: HashSet<i64> = HashSet::new();
    let mut entries: Vec<Entry> = Vec::new();

    // ---- Lane 1: the accessibility tree (roles and names, browser-computed)
    for (ai, n) in ax
        .get("nodes")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        if n.get("ignored").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let role_raw = n
            .get("role")
            .and_then(|r| r.get("value"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let Some(role) = role_label(&role_raw) else { continue };
        let Some(backend) = n.get("backendDOMNodeId").and_then(Value::as_i64) else { continue };
        if !included.insert(backend) {
            continue;
        }

        let mut name = tidy(
            n.get("name")
                .and_then(|x| x.get("value"))
                .and_then(Value::as_str)
                .unwrap_or(""),
            NAME_MAX,
        );
        let ax_value = n
            .get("value")
            .and_then(|x| x.get("value"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let mut disabled = false;
        let mut ax_checked = false;
        if let Some(props) = n.get("properties").and_then(Value::as_array) {
            for p in props {
                let pname = p.get("name").and_then(Value::as_str).unwrap_or("");
                let pval = p.get("value").and_then(|v| v.get("value"));
                match pname {
                    "disabled" => disabled = pval.and_then(Value::as_bool).unwrap_or(false),
                    "checked" => {
                        ax_checked = pval
                            .and_then(Value::as_str)
                            .map(|v| v == "true" || v == "mixed")
                            .unwrap_or(false)
                    }
                    _ => {}
                }
            }
        }

        // Enrich from the snapshot: attributes, live input value, position
        let pos = where_of.get(&backend).copied();
        let mut extras = Vec::new();
        let mut value = ax_value.to_string();
        let mut checked = ax_checked;
        if let Some((di, ni)) = pos {
            let d = &docs[di];
            if name.is_empty() {
                name = harvest_text(d, ni);
            }
            if let Some(v) = d.input_value.get(&ni) {
                value = v.clone();
            }
            checked = checked || d.input_checked.contains(&ni);
            if let Some(a) = d.attrs.get(ni) {
                if role == "link"
                    && let Some(h) = a.get("href") {
                        extras.push(tidy(h, HREF_MAX));
                    }
                if (role == "textbox" || role == "combobox")
                    && let Some(p) = a.get("placeholder") {
                        extras.push(format!("placeholder=\"{}\"", tidy(p, VALUE_MAX)));
                    }
            }
            // A real <select>: say what it offers, so choosing costs no move
            if d.tag.get(ni).map(String::as_str) == Some("select")
                && let Some(o) = select_options(d, ni) {
                    extras.push(o);
                }
            if let Some(times) = d.scrolls(ni) {
                extras.push(format!("scrolls={times:.1}"));
            }
        }
        if (role == "textbox" || role == "combobox") && !value.is_empty() {
            extras.push(format!("value=\"{}\"", tidy(&value, VALUE_MAX)));
        }
        if matches!(role, "checkbox" | "radio" | "switch") && checked {
            extras.push("checked".into());
        }
        if disabled {
            extras.push("disabled".into());
        }

        if name.is_empty() && extras.is_empty() && !keep_nameless(role) {
            included.remove(&backend);
            continue;
        }
        let (order, off) = match pos {
            Some((di, ni)) => ((di, ni), off_screen(di, ni)),
            // In the AX tree but absent from the snapshot (rare): keep it,
            // ordered after everything else
            None => ((usize::MAX, ai), false),
        };
        // Whether this really is a native list, or one of its choices. Both
        // are worked through the list itself, and neither is clicked: a click
        // on a native dropdown opens something the page cannot see into
        let native_list = pos
            .and_then(|(di, ni)| docs.get(di).and_then(|d| d.tag.get(ni)))
            .is_some_and(|t| t == "select" || t == "option");
        let ops = verbs_for(
            role,
            native_list,
            extras.iter().any(|x| x.starts_with("scrolls=")),
        );
        entries.push(Entry {
            order,
            backend: Some(backend),
            role: role.to_string(),
            name,
            extras,
            off_screen: off,
            ops,
        });
    }

    // ---- Lane 2: JS-clickables the AX tree has no role for.
    // Signals: Chromium's isClickable mark, or a cursor:pointer *boundary*
    // (cursor inherits, so only the outermost pointer element is the widget).
    // Skipped when the element already surfaced via lane 1, sits inside a
    // lane-1 element, or wraps one (that inner element is the real control)
    for (di, d) in docs.iter().enumerate() {
        let n = d.parent.len();
        // does this subtree contain a lane-1 element?
        let mut contains = vec![false; n];
        for i in (0..n).rev() {
            let mine = d.backend.get(i).map(|b| included.contains(b)).unwrap_or(false);
            if (mine || contains[i])
                && let Some(Ok(p)) = d.parent.get(i).map(|&p| usize::try_from(p))
                    && p < n {
                        contains[p] = true;
                    }
        }
        let mut accepted: HashSet<usize> = HashSet::new();
        for (i, &holds_one) in contains.iter().enumerate() {
            if d.node_type.get(i) != Some(&1) || !d.bounds.contains_key(&i) {
                continue;
            }
            let pointer_boundary = d.cursor_pointer.contains(&i)
                && !usize::try_from(*d.parent.get(i).unwrap_or(&-1))
                    .map(|p| d.cursor_pointer.contains(&p))
                    .unwrap_or(false);
            if !d.clickable.contains(&i) && !pointer_boundary {
                continue;
            }
            let backend = *d.backend.get(i).unwrap_or(&-1);
            if backend < 0 || included.contains(&backend) || holds_one {
                continue;
            }
            // inside something already listed?
            let mut anc = *d.parent.get(i).unwrap_or(&-1);
            let mut covered = false;
            while let Ok(p) = usize::try_from(anc) {
                if p >= n {
                    break;
                }
                if accepted.contains(&p)
                    || d.backend.get(p).map(|b| included.contains(b)).unwrap_or(false)
                {
                    covered = true;
                    break;
                }
                anc = *d.parent.get(p).unwrap_or(&-1);
            }
            if covered {
                continue;
            }
            let name = harvest_text(d, i);
            let href = d.attrs.get(i).and_then(|a| a.get("href").cloned());
            if name.is_empty() && href.is_none() {
                continue;
            }
            included.insert(backend);
            accepted.insert(i);
            let mut extras = Vec::new();
            if let Some(h) = href {
                extras.push(tidy(&h, HREF_MAX));
            }
            if let Some(times) = d.scrolls(i) {
                extras.push(format!("scrolls={times:.1}"));
            }
            let ops = verbs_for("", false, d.scrolls(i).is_some());
            entries.push(Entry {
                order: (di, i),
                backend: Some(backend),
                role: format!("{}*", d.tag.get(i).map(String::as_str).unwrap_or("?")),
                name,
                extras,
                off_screen: off_screen(di, i),
                ops,
            });
        }

        // ---- Lane 3: boxes that scroll on their own.
        // A results list inside a panel holds the rest of the answer, and
        // nothing above would have listed it: it takes no clicks and has no
        // role. Without a number for it, the only scroll anyone can ask for
        // is the window's, which moves the wrong thing
        for i in 0..n {
            let backend = *d.backend.get(i).unwrap_or(&-1);
            if backend < 0 || included.contains(&backend) || d.node_type.get(i) != Some(&1) {
                continue;
            }
            let Some(times) = d.scrolls(i) else { continue };
            // The document's own scrolling box is the page, which is said once
            // in the header rather than as an element
            if matches!(d.tag.get(i).map(String::as_str), Some("html" | "body")) {
                continue;
            }
            included.insert(backend);
            entries.push(Entry {
                order: (di, i),
                backend: Some(backend),
                role: "scrollbox".into(),
                name: harvest_text(d, i),
                extras: vec![format!("scrolls={times:.1}")],
                off_screen: off_screen(di, i),
                ops: vec!["scroll"],
            });
        }

        // Out-of-process iframes never appear in the snapshot's documents;
        // say so instead of letting the gap read as "nothing there"
        for i in 0..n {
            if d.tag.get(i).map(String::as_str) == Some("iframe")
                && d.bounds.contains_key(&i)
                && !d.content_doc.contains(&i)
            {
                let src = d
                    .attrs
                    .get(i)
                    .and_then(|a| a.get("src").cloned())
                    .unwrap_or_default();
                entries.push(Entry {
                    order: (di, i),
                    backend: None,
                    role: "iframe".into(),
                    name: tidy(&src, HREF_MAX),
                    extras: vec!["content out of reach".into()],
                    off_screen: off_screen(di, i),
                    ops: Vec::new(),
                });
            }
        }
    }

    entries.sort_by_key(|e| e.order);

    // ---- Nesting. An element's depth is how many *listed* elements it sits
    // inside, not how deep the HTML happens to be: a form five wrappers down
    // reads as one step in, which is what a person would say about it
    let mut listed: Vec<HashSet<usize>> = vec![HashSet::new(); docs.len()];
    for e in &entries {
        let (di, ni) = e.order;
        if let Some(set) = listed.get_mut(di) {
            set.insert(ni);
        }
    }
    let depth_of = |di: usize, ni: usize| -> usize {
        let (Some(d), Some(set)) = (docs.get(di), listed.get(di)) else { return 0 };
        let mut depth = 0usize;
        let mut anc = *d.parent.get(ni).unwrap_or(&-1);
        while let Ok(p) = usize::try_from(anc) {
            if p >= d.parent.len() {
                break;
            }
            if set.contains(&p) {
                depth += 1;
                // Past a certain point the indent stops carrying meaning and
                // starts eating the line
                if depth >= MAX_DEPTH {
                    break;
                }
            }
            anc = *d.parent.get(p).unwrap_or(&-1);
        }
        depth
    };

    // ---- What is new since the last look (see `build_against`)
    let seen: HashSet<i64> = prev.iter().copied().collect();
    let fresh = |b: Option<i64>| b.map(|b| !seen.contains(&b)).unwrap_or(false);
    let newcomers = entries.iter().filter(|e| fresh(e.backend)).count();
    let mark_new =
        !seen.is_empty() && !entries.is_empty() && newcomers * 10 < entries.len() * 7;

    // ---- Render
    let mut text = String::from(LEGEND);
    // How much of the page is out of sight. Said once, at the top, because it
    // is the page's own fact and not any element's
    let content_h = f64_of(metrics.get("cssContentSize").and_then(|c| c.get("height")));
    if vh > 0.0 && content_h > vh + 8.0 {
        let page_y = f64_of(vp.and_then(|v| v.get("pageY")));
        text.push_str(&format!(
            "# the page is {:.1} screens tall and you are {:.1} screens down — browser_scroll(BR, \"down\")\n",
            content_h / vh,
            page_y / vh
        ));
    }
    let mut refs = Vec::new();
    let mut elements: Vec<serde_json::Value> = Vec::new();
    let total = entries.len();
    // Each element carries the section it sits under (the nearest preceding
    // heading), so "the search-results link" and "the same link quoted in an
    // AI summary" are tellable apart on their own lines, not only by reading
    // the whole listing in order
    let mut section = String::new();
    for e in entries.into_iter().take(MAX_LINES) {
        if e.role == "heading" {
            section = e.name.chars().take(16).collect();
        }
        let head = match e.backend {
            Some(b) => {
                refs.push(b);
                format!("[{}]", refs.len())
            }
            None => "[-]".to_string(),
        };
        let indent = "  ".repeat(if e.order.0 < docs.len() { depth_of(e.order.0, e.order.1) } else { 0 });
        let new = if mark_new && fresh(e.backend) { "+" } else { "" };
        let mut line = format!("{indent}{new}{head} {}", e.role);
        if !e.name.is_empty() {
            line.push_str(&format!(" \"{}\"", e.name));
        }
        for x in &e.extras {
            line.push(' ');
            line.push_str(x);
        }
        if e.role != "heading" && !section.is_empty() {
            line.push_str(&format!(" §{section}"));
        }
        if e.off_screen {
            line.push_str("  off_screen");
        }
        line.push('\n');
        text.push_str(&line);
        // The same element as data. Built here, from the same entry that made
        // the line, so the two can never come to disagree about what [7] is
        if e.backend.is_some() {
            elements.push(serde_json::json!({
                "ref": refs.len(),
                "role": e.role,
                "name": e.name,
                "value": e.extras.iter().find_map(|x| x.strip_prefix("value=\"")).map(|v| v.trim_end_matches('\"')),
                "choices": e.extras.iter().find_map(|x| x.strip_prefix("choices=")),
                "section": section,
                "off_screen": e.off_screen,
                "new": mark_new && fresh(e.backend),
                "can": e.ops,
            }));
        }
    }
    if total > MAX_LINES {
        text.push_str(&format!(
            "# … {} more elements omitted — narrow down with browser_html if needed\n",
            total - MAX_LINES
        ));
    }
    Digest { text, refs, elements }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One node as the browser describes it: whose child it is, what kind of
    /// node, its tag, its text, the id the browser knows it by, and its
    /// attributes in pairs
    type SnapNode<'a> = (i64, i64, &'a str, &'a str, i64, &'a [(&'a str, &'a str)]);

    /// Compact builder for one snapshot document.
    fn snap_doc(
        nodes: &[SnapNode<'_>],
        layout: &[(usize, [f64; 4], &str)],
        clickable: &[usize],
    ) -> (Value, Vec<String>) {
        let mut strings: Vec<String> = Vec::new();
        let idx = |s: &str, strings: &mut Vec<String>| -> i64 {
            if let Some(i) = strings.iter().position(|x| x == s) {
                i as i64
            } else {
                strings.push(s.to_string());
                (strings.len() - 1) as i64
            }
        };
        let mut parent = vec![];
        let mut ntype = vec![];
        let mut name = vec![];
        let mut value = vec![];
        let mut backend = vec![];
        let mut attrs: Vec<Vec<i64>> = vec![];
        for (p, t, tag, val, b, at) in nodes {
            parent.push(*p);
            ntype.push(*t);
            name.push(idx(&tag.to_uppercase(), &mut strings));
            value.push(idx(val, &mut strings));
            backend.push(*b);
            let mut row = vec![];
            for (k, v) in *at {
                row.push(idx(k, &mut strings));
                row.push(idx(v, &mut strings));
            }
            attrs.push(row);
        }
        let mut l_nodes = vec![];
        let mut bounds = vec![];
        let mut styles = vec![];
        for (ni, b, cursor) in layout {
            l_nodes.push(*ni as i64);
            bounds.push(json!([b[0], b[1], b[2], b[3]]));
            styles.push(json!([idx(cursor, &mut strings)]));
        }
        let doc = json!({
            "scrollOffsetX": 0.0, "scrollOffsetY": 0.0,
            "nodes": {
                "parentIndex": parent, "nodeType": ntype, "nodeName": name,
                "nodeValue": value, "backendNodeId": backend, "attributes": attrs,
                "isClickable": { "index": clickable.iter().map(|&i| i as i64).collect::<Vec<_>>() },
            },
            "layout": { "nodeIndex": l_nodes, "bounds": bounds, "styles": styles },
        });
        (doc, strings)
    }

    fn metrics() -> Value {
        json!({"cssVisualViewport": {"clientWidth": 1000.0, "clientHeight": 800.0}})
    }

    fn ax_node(role: &str, name: &str, backend: i64) -> Value {
        json!({"ignored": false, "role": {"value": role}, "name": {"value": name},
               "backendDOMNodeId": backend})
    }

    #[test]
    fn ax_elements_get_named_refs_in_document_order() {
        // <a href> and <input> come from the AX lane, with names and attributes, in number order
        let (doc, strings) = snap_doc(
            &[
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "a", "", 10, &[("href", "https://example.com/x")]),
                (1, 3, "#text", "つぎへ", 11, &[]),
                (0, 1, "input", "", 12, &[("placeholder", "名前")]),
            ],
            &[(1, [0.0, 0.0, 100.0, 20.0], "pointer"), (3, [0.0, 30.0, 100.0, 20.0], "auto")],
            &[1],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [
            ax_node("RootWebArea", "", 1),
            ax_node("link", "つぎへ", 10),
            ax_node("textField", "名前", 12),
        ]});
        let d = build(&ax, &snap, &metrics());
        assert_eq!(d.refs, vec![10, 12], "the two AX elements are numbered in order: {}", d.text);
        assert!(d.text.contains("[1] link \"つぎへ\" https://example.com/x"), "{}", d.text);
        assert!(d.text.contains("[2] textbox \"名前\""), "{}", d.text);
        assert!(d.text.contains("placeholder=\"名前\""), "{}", d.text);
    }

    #[test]
    fn clickable_div_without_role_is_supplemented() {
        // a JS clickable that never appears in AX (<div onclick>) is picked up as div*
        let (doc, strings) = snap_doc(
            &[
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "div", "", 20, &[]),
                (1, 3, "#text", "もっと見る", 21, &[]),
            ],
            &[(1, [0.0, 0.0, 100.0, 20.0], "pointer")],
            &[1],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [ax_node("RootWebArea", "", 1)]});
        let d = build(&ax, &snap, &metrics());
        assert_eq!(d.refs, vec![20], "{}", d.text);
        assert!(d.text.contains("[1] div* \"もっと見る\""), "{}", d.text);
    }

    #[test]
    fn wrapper_around_ax_element_is_not_duplicated() {
        // a clickable div wrapping a link already found in AX is not listed twice (the contents are what counts)
        let (doc, strings) = snap_doc(
            &[
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "div", "", 30, &[]),
                (1, 1, "a", "", 31, &[("href", "https://example.com/")]),
                (2, 3, "#text", "リンク", 32, &[]),
            ],
            &[
                (1, [0.0, 0.0, 200.0, 40.0], "pointer"),
                (2, [0.0, 0.0, 100.0, 20.0], "pointer"),
            ],
            &[1, 2],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [ax_node("RootWebArea", "", 1), ax_node("link", "リンク", 31)]});
        let d = build(&ax, &snap, &metrics());
        assert_eq!(d.refs, vec![31], "the wrapping div is not listed: {}", d.text);
    }

    #[test]
    fn nested_pointer_children_collapse_to_the_boundary() {
        // cursor:pointer is inherited, so only the one element at the boundary (the outermost) is listed
        let (doc, strings) = snap_doc(
            &[
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "div", "", 40, &[]),
                (1, 1, "span", "", 41, &[]),
                (2, 3, "#text", "押す", 42, &[]),
            ],
            &[
                (1, [0.0, 0.0, 100.0, 30.0], "pointer"),
                (2, [0.0, 0.0, 80.0, 20.0], "pointer"),
            ],
            &[],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [ax_node("RootWebArea", "", 1)]});
        let d = build(&ax, &snap, &metrics());
        assert_eq!(d.refs, vec![40], "only the outer edge of the boundary: {}", d.text);
    }

    #[test]
    fn out_of_viewport_is_flagged_not_dropped() {
        // elements off screen are kept with an off_screen flag, not thrown away
        let (doc, strings) = snap_doc(
            &[
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "a", "", 50, &[("href", "https://example.com/below")]),
                (1, 3, "#text", "下のリンク", 51, &[]),
            ],
            &[(1, [0.0, 5000.0, 100.0, 20.0], "pointer")],
            &[1],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [ax_node("RootWebArea", "", 1), ax_node("link", "下のリンク", 50)]});
        let d = build(&ax, &snap, &metrics());
        assert!(d.text.contains("off_screen"), "{}", d.text);
        assert_eq!(d.refs, vec![50]);
    }

    #[test]
    fn ignored_and_unnamed_noise_is_skipped() {
        // AX nodes with ignored=true, and links with neither a name nor an href, are not listed
        let (doc, strings) = snap_doc(
            &[(-1, 9, "#document", "", 1, &[]), (0, 1, "a", "", 60, &[])],
            &[(1, [0.0, 0.0, 10.0, 10.0], "auto")],
            &[],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [
            ax_node("RootWebArea", "", 1),
            {"ignored": true, "role": {"value": "link"}, "name": {"value": "見えない"}, "backendDOMNodeId": 99},
            ax_node("link", "", 60),
        ]});
        let d = build(&ax, &snap, &metrics());
        assert!(d.refs.is_empty(), "nothing should be listed: {}", d.text);
    }

    /// A snapshot document that also carries how tall each box is drawn and
    /// how tall its content really is -- the pair that says "this scrolls"
    fn snap_doc_scrolling(
        nodes: &[SnapNode<'_>],
        layout: &[(usize, [f64; 4], &str)],
        clickable: &[usize],
        heights: &[(usize, f64, f64)],
    ) -> (Value, Vec<String>) {
        let (mut doc, strings) = snap_doc(nodes, layout, clickable);
        let order: Vec<usize> = layout.iter().map(|(ni, _, _)| *ni).collect();
        let mut client = Vec::new();
        let mut scroll = Vec::new();
        for ni in &order {
            let found = heights.iter().find(|(at, _, _)| at == ni);
            let (c, sc) = found.map(|(_, c, s)| (*c, *s)).unwrap_or((0.0, 0.0));
            client.push(json!([0.0, 0.0, 0.0, c]));
            scroll.push(json!([0.0, 0.0, 0.0, sc]));
        }
        doc["layout"]["clientRects"] = json!(client);
        doc["layout"]["scrollRects"] = json!(scroll);
        (doc, strings)
    }

    #[test]
    fn a_dropdowns_own_choices_are_on_its_line() {
        // Reading what a list offers costs a move if it is not simply said, and
        // the browser already knows: the page has to be opened, looked into,
        // and taken in again. So the choices ride on the line, capped
        let (doc, strings) = snap_doc(
            &[
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "select", "", 30, &[("name", "class")]),
                (1, 1, "option", "", 31, &[]),
                (2, 3, "#text", "Economy", 32, &[]),
                (1, 1, "option", "", 33, &[]),
                (4, 3, "#text", "Business", 34, &[]),
            ],
            &[(1, [0.0, 0.0, 200.0, 24.0], "auto")],
            &[],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [
            ax_node("RootWebArea", "", 1),
            ax_node("comboBox", "Class", 30),
        ]});
        let d = build(&ax, &snap, &metrics());
        assert!(d.text.contains("choices=2"), "{}", d.text);
        assert!(d.text.contains("Economy | Business"), "{}", d.text);
        // ...and the element says that choosing, not typing, is what it takes
        let can = d.elements[0]["can"].as_array().expect("verbs").iter()
            .map(|v| v.as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
        assert!(can.contains(&"select".to_string()), "{:?}", can);
        assert!(!can.contains(&"fill".to_string()), "a list is chosen from, not typed into: {can:?}");
    }

    #[test]
    fn a_box_that_scrolls_gets_a_number_of_its_own() {
        // The rest of a result list is inside a panel that takes no clicks and
        // has no role. Without a number for it, the only scroll anyone can ask
        // for is the window's, which moves the wrong thing
        let (doc, strings) = snap_doc_scrolling(
            &[
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "div", "", 40, &[]),
                (1, 3, "#text", "Results", 41, &[]),
            ],
            &[(1, [0.0, 0.0, 300.0, 200.0], "auto")],
            &[],
            &[(1, 200.0, 900.0)],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [ax_node("RootWebArea", "", 1)]});
        let d = build(&ax, &snap, &metrics());
        assert!(d.text.contains("scrollbox"), "{}", d.text);
        assert!(d.text.contains("scrolls=4.5"), "{}", d.text);
        assert_eq!(d.refs, vec![40], "it has to be addressable to be scrolled: {}", d.text);
    }

    #[test]
    fn what_is_new_is_marked_and_a_new_page_is_not() {
        // Knowing which two lines are new is the difference between reading a
        // menu that just opened and reading the page again. A page that
        // changed wholesale gets no marks at all -- marking every line says no
        // more than marking none, and costs a screenful
        let page = |extra: bool| {
            let mut nodes: Vec<SnapNode<'_>> = vec![
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "a", "", 50, &[("href", "https://example.com/one")]),
                (1, 3, "#text", "One", 51, &[]),
                (0, 1, "a", "", 52, &[("href", "https://example.com/two")]),
                (3, 3, "#text", "Two", 53, &[]),
            ];
            if extra {
                nodes.push((0, 1, "a", "", 54, &[("href", "https://example.com/three")]));
                nodes.push((5, 3, "#text", "Three", 55, &[]));
            }
            let layout: Vec<(usize, [f64; 4], &str)> = vec![
                (1, [0.0, 0.0, 100.0, 20.0], "pointer"),
                (3, [0.0, 30.0, 100.0, 20.0], "pointer"),
                (5, [0.0, 60.0, 100.0, 20.0], "pointer"),
            ];
            let n = if extra { 3 } else { 2 };
            snap_doc(&nodes, &layout[..n], &[])
        };
        let ax = |extra: bool| {
            let mut v = vec![
                ax_node("RootWebArea", "", 1),
                ax_node("link", "One", 50),
                ax_node("link", "Two", 52),
            ];
            if extra {
                v.push(ax_node("link", "Three", 54));
            }
            json!({ "nodes": v })
        };
        let (doc, strings) = page(false);
        let first = build(&ax(false), &json!({"documents": [doc], "strings": strings}), &metrics());
        assert!(!first.text.contains("+["), "nothing is new on a first look: {}", first.text);

        let (doc, strings) = page(true);
        let then = build_against(
            &ax(true),
            &json!({"documents": [doc], "strings": strings}),
            &metrics(),
            &first.refs,
        );
        assert!(then.text.contains("+[3] link \"Three\""), "{}", then.text);
        assert_eq!(then.text.matches("+[").count(), 1, "only the new one: {}", then.text);

        // A different page: every number is different, and none is marked
        let (doc, strings) = page(true);
        let elsewhere = build_against(
            &ax(true),
            &json!({"documents": [doc], "strings": strings}),
            &metrics(),
            &[900, 901],
        );
        assert!(!elsewhere.text.contains("+["), "a new page is not news, line by line: {}", elsewhere.text);
    }

    #[test]
    fn what_sits_inside_what_is_shown_by_the_indent() {
        // A flat list of forty lines says nothing about which button belongs to
        // which row. The depth counts listed elements, not tags, so five
        // wrappers deep still reads as one step in
        let (doc, strings) = snap_doc(
            &[
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "div", "", 60, &[("role", "listbox")]),
                (1, 1, "div", "", 61, &[("role", "option")]),
                (2, 3, "#text", "One", 63, &[]),
                (1, 1, "div", "", 62, &[("role", "option")]),
                (4, 3, "#text", "Two", 64, &[]),
            ],
            &[
                (1, [0.0, 0.0, 300.0, 100.0], "auto"),
                (2, [0.0, 10.0, 280.0, 20.0], "pointer"),
                (4, [0.0, 40.0, 280.0, 20.0], "pointer"),
            ],
            &[],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [
            ax_node("RootWebArea", "", 1),
            ax_node("listBox", "Pick one", 60),
            ax_node("option", "One", 61),
            ax_node("option", "Two", 62),
        ]});
        let d = build(&ax, &snap, &metrics());
        assert!(d.text.contains("[1] combobox"), "{}", d.text);
        assert!(d.text.contains("\n  [2] option \"One\""), "an option sits inside its list: {}", d.text);
        assert!(d.text.contains("\n  [3] option \"Two\""), "{}", d.text);
        assert_eq!(d.elements[1]["ref"], 2, "{}", d.text);
    }

    #[test]
    fn how_far_down_the_page_goes_is_said_once() {
        // A fact about the page, not about any element on it, and the thing a
        // reader needs before deciding whether what they want is simply below
        let (doc, strings) = snap_doc(
            &[(-1, 9, "#document", "", 1, &[]), (0, 1, "a", "", 70, &[("href", "https://example.com")])],
            &[(1, [0.0, 0.0, 100.0, 20.0], "pointer")],
            &[1],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [ax_node("RootWebArea", "", 1), ax_node("link", "Down", 70)]});
        let tall = json!({
            "cssVisualViewport": {"clientWidth": 1000.0, "clientHeight": 800.0, "pageY": 800.0},
            "cssContentSize": {"height": 3200.0},
        });
        let d = build(&ax, &snap, &tall);
        assert!(d.text.contains("4.0 screens tall"), "{}", d.text);
        assert!(d.text.contains("1.0 screens down"), "{}", d.text);
        // A page that fits says nothing at all about scrolling
        let short = build(&ax, &snap, &metrics());
        assert!(!short.text.contains("screens tall"), "{}", short.text);
    }

    #[test]
    fn elements_carry_their_section_heading() {
        // an element under a heading calls itself §heading (so a quote inside an AI summary
        // can be told from a real search result by its line alone)
        let (doc, strings) = snap_doc(
            &[
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "h2", "", 80, &[]),
                (1, 3, "#text", "検索結果", 81, &[]),
                (0, 1, "a", "", 82, &[("href", "https://example.com/a")]),
                (3, 3, "#text", "結果リンク", 83, &[]),
            ],
            &[
                (1, [0.0, 0.0, 100.0, 20.0], "auto"),
                (3, [0.0, 30.0, 100.0, 20.0], "pointer"),
            ],
            &[3],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [
            ax_node("RootWebArea", "", 1),
            ax_node("heading", "検索結果", 80),
            ax_node("link", "結果リンク", 82),
        ]});
        let d = build(&ax, &snap, &metrics());
        assert!(d.text.contains("heading \"検索結果\"\n"), "{}", d.text);
        assert!(
            d.text.contains("link \"結果リンク\" https://example.com/a §検索結果"),
            "{}",
            d.text
        );
    }

    #[test]
    fn multibyte_names_clip_without_splitting() {
        let long = "あ".repeat(200);
        let (doc, strings) = snap_doc(
            &[
                (-1, 9, "#document", "", 1, &[]),
                (0, 1, "a", "", 70, &[("href", "https://example.com/")]),
            ],
            &[(1, [0.0, 0.0, 10.0, 10.0], "auto")],
            &[],
        );
        let snap = json!({"documents": [doc], "strings": strings});
        let ax = json!({"nodes": [ax_node("link", &long, 70)]});
        let d = build(&ax, &snap, &metrics());
        assert!(d.text.contains(&"あ".repeat(NAME_MAX)), "{}", d.text);
        assert!(!d.text.contains(&"あ".repeat(NAME_MAX + 1)), "{}", d.text);
        assert!(d.text.contains('…'), "{}", d.text);
    }
}
