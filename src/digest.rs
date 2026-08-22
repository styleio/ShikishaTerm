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
}

/// Hard ceiling on emitted lines. Never truncates silently: when hit, the tail
/// line says how many elements were left out
const MAX_LINES: usize = 1200;
const NAME_MAX: usize = 80;
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
    scroll: (f64, f64),
    children: Vec<Vec<usize>>,
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
        if let Ok(p) = usize::try_from(p) {
            if p < n {
                children[p].push(i);
            }
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
}

pub fn build(ax: &Value, snap: &Value, metrics: &Value) -> Digest {
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
                if role == "link" {
                    if let Some(h) = a.get("href") {
                        extras.push(tidy(h, HREF_MAX));
                    }
                }
                if role == "textbox" || role == "combobox" {
                    if let Some(p) = a.get("placeholder") {
                        extras.push(format!("placeholder=\"{}\"", tidy(p, VALUE_MAX)));
                    }
                }
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
        entries.push(Entry {
            order,
            backend: Some(backend),
            role: role.to_string(),
            name,
            extras,
            off_screen: off,
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
            if mine || contains[i] {
                if let Some(Ok(p)) = d.parent.get(i).map(|&p| usize::try_from(p)) {
                    if p < n {
                        contains[p] = true;
                    }
                }
            }
        }
        let mut accepted: HashSet<usize> = HashSet::new();
        for i in 0..n {
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
            if backend < 0 || included.contains(&backend) || contains[i] {
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
            entries.push(Entry {
                order: (di, i),
                backend: Some(backend),
                role: format!("{}*", d.tag.get(i).map(String::as_str).unwrap_or("?")),
                name,
                extras,
                off_screen: off_screen(di, i),
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
                });
            }
        }
    }

    entries.sort_by_key(|e| e.order);

    // ---- Render
    let mut text = String::from(
        "# operable elements — act by number: browser_click(BR,{ref=N}) / browser_fill(BR,{ref=N},\"text\")\n\
         # roles marked * are JS-clickables without a standard role; §… names the section (heading) an element sits under; take a new digest after the page changes\n",
    );
    let mut refs = Vec::new();
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
        let mut line = format!("{head} {}", e.role);
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
    }
    if total > MAX_LINES {
        text.push_str(&format!(
            "# … {} more elements omitted — narrow down with browser_html if needed\n",
            total - MAX_LINES
        ));
    }
    Digest { text, refs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Compact builder for one snapshot document.
    /// nodes: (parentIndex, nodeType, tag, nodeValue, backendId, [attr pairs])
    fn snap_doc(
        nodes: &[(i64, i64, &str, &str, i64, &[(&str, &str)])],
        layout: &[(usize, [f64; 4], &str)],
        clickable: &[usize],
    ) -> (Value, Vec<String>) {
        let mut strings: Vec<String> = Vec::new();
        let mut idx = |s: &str, strings: &mut Vec<String>| -> i64 {
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
        // <a href> と <input> がAXレーンから、名前・属性つきで番号順に出る
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
        assert_eq!(d.refs, vec![10, 12], "AXの2要素が順に採番される: {}", d.text);
        assert!(d.text.contains("[1] link \"つぎへ\" https://example.com/x"), "{}", d.text);
        assert!(d.text.contains("[2] textbox \"名前\""), "{}", d.text);
        assert!(d.text.contains("placeholder=\"名前\""), "{}", d.text);
    }

    #[test]
    fn clickable_div_without_role_is_supplemented() {
        // AXに現れないJSクリッカブル(<div onclick>)が div* として拾われる
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
        // AXで拾ったリンクを包むクリッカブルdivは重複掲載しない(中身が本体)
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
        assert_eq!(d.refs, vec![31], "包みのdivは載らない: {}", d.text);
    }

    #[test]
    fn nested_pointer_children_collapse_to_the_boundary() {
        // cursor:pointerは継承するので、境界(外側)の1要素だけが載る
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
        assert_eq!(d.refs, vec![40], "境界の外側だけ: {}", d.text);
    }

    #[test]
    fn out_of_viewport_is_flagged_not_dropped() {
        // 画面外の要素は捨てずに off_screen フラグで残す
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
        // ignored=trueのAXノードと、名前もhrefもないリンクは載らない
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
        assert!(d.refs.is_empty(), "何も載らないはず: {}", d.text);
    }

    #[test]
    fn elements_carry_their_section_heading() {
        // 見出しの配下にある要素は §見出し を名乗る (AI要約内の引用と
        // 本物の検索結果を、行単体で見分けられるように)
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
