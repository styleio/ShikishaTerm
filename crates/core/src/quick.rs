//! Quick commands: buttons laid out on pages of a grid, each handing the tab in
//! view one line -- a command for a shell, or a prompt for an AI. A button can
//! also be a folder, which opens onto a grid of its own.
//!
//! Three things live here, because every screen that touches a quick command
//! has to agree on them and would drift if each worked them out alone:
//!
//!   - **where each button sits** (`arrange`). The settings file says it, a
//!     person may have written it by hand, and a grid made smaller has to put
//!     somewhere the buttons that no longer fit. The settings page asks this
//!     file rather than doing it again in script, and the board is handed what
//!     it produced;
//!   - **what a button looks like** (`view`, `icon`), with its drawing already
//!     looked up, so neither the window nor a phone has to fetch the whole
//!     icon set just to show eight buttons;
//!   - **what a button sends** (`expand`). A body may name a secret of the
//!     desk on screen as `{{secrets.NAME}}`. The value is put in here, at the
//!     moment of sending, by the same door a script's secrets go through --
//!     it is never written into the settings and never reaches a page.
//!
//! Where a button sits is kept as a place (page, row, column) rather than as a
//! position in a list. A position in a list only means something for one
//! number of columns: add a column and every button after the first row moves.
//! A place stays put when the grid grows, and only the buttons that fall
//! outside a smaller grid have to move -- into the first free place, in
//! reading order, from the page they were on.
//!
//! A folder's grid is the same size as the one it is in, and the first place
//! of each of its pages is the way back out. That place is never given to a
//! button: a way out that moved, or that a button could cover, is a folder a
//! person can be stuck in.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// The grid a new set of quick commands starts with. Eight across and four
/// down fits a laptop's window with room around it, and is a size people
/// already know from the button boxes this is modelled on
pub const COLS_DEFAULT: u16 = 8;
pub const ROWS_DEFAULT: u16 = 4;
/// The largest grid offered. Past this the buttons on a laptop's window are
/// too small to read a name under, which is the whole face of a button
pub const COLS_MAX: u16 = 12;
pub const ROWS_MAX: u16 = 8;
/// Pages a hand-written file may put a button on. A page number past this is
/// taken for a typo and the button is moved to a free place, rather than a
/// pager with ten thousand numbers being drawn. Buttons that genuinely need
/// more pages -- a grid shrunk to two places -- still get them
pub const PAGES_MAX: u16 = 50;
/// How long a name, and a body, may be. A name is read under a picture 88
/// pixels wide; a body is a command or a prompt, and a prompt can be long
pub const LABEL_MAX: usize = 80;
pub const BODY_MAX: usize = 8000;

/// A secret named inside a body: `{{secrets.NAME}}`, spaces allowed inside the
/// braces. The same pattern is handed to the settings page (which says, under
/// the body, that a secret will be put in), so the two cannot disagree about
/// what counts as one. The name part is exactly what
/// `config::valid_secret_name` allows.
///
/// The shape is the one CI systems use for the same purpose, so it is already
/// familiar; and nothing that a shell or an AI reads uses it, unlike `${...}`
/// or `@name`, which a real command is full of
pub const SECRET_REF: &str = r"\{\{\s*secrets\.([A-Za-z0-9_-]+)\s*\}\}";

/// What a button is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A command, typed at a shell
    #[default]
    Terminal,
    /// A prompt, given to an AI
    Ai,
    /// A grid of more buttons
    Folder,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Terminal => "terminal",
            Kind::Ai => "ai",
            Kind::Folder => "folder",
        }
    }

    /// Whether a tab is one this kind of command is for. A prompt typed at a
    /// shell is a command nobody meant, and a command handed to an AI is a
    /// request it will act on in its own way -- neither is sent. A folder
    /// sends nothing to anyone
    pub fn fits(self, tab_is_ai: bool) -> bool {
        match self {
            Kind::Terminal => !tab_is_ai,
            Kind::Ai => tab_is_ai,
            Kind::Folder => false,
        }
    }
}

fn yes() -> bool {
    true
}

/// One button, as the settings file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QuickItem {
    /// What the button is known by when it is pressed. Made by the settings
    /// page when the button is; a hand-written one without it is given one
    /// from its place (`arrange`). One of a kind across every folder
    #[serde(default)]
    pub id: String,
    /// Its place in the grid it is in, counted from 0
    #[serde(default)]
    pub page: u16,
    #[serde(default)]
    pub row: u16,
    #[serde(default)]
    pub col: u16,
    /// A drawing's name from the icon set, or empty for none
    #[serde(default)]
    pub icon: String,
    /// The name under the drawing
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub kind: Kind,
    /// The command or the prompt. Nothing, for a folder
    #[serde(default)]
    pub body: String,
    /// Whether Enter is pressed after it. Off leaves the text at the prompt
    /// for the person to finish
    #[serde(default = "yes")]
    pub enter: bool,
    /// For a prompt: which AI it is for, by the command that starts it
    /// (`claude`, `codex`...). Empty is whichever AI the folder already has,
    /// or the first one this PC can start
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ai: String,
    /// A folder's own pages, and the buttons in it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<QuickItem>,
}

/// The quick commands, as the settings file holds them (`quick_commands`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QuickSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u16>,
    #[serde(default)]
    pub items: Vec<QuickItem>,
}

impl QuickSpec {
    pub fn cols(&self) -> u16 {
        self.cols.unwrap_or(COLS_DEFAULT).clamp(1, COLS_MAX)
    }
    /// At least two places in all, so a folder has room for its way out and
    /// for something besides
    pub fn rows(&self) -> u16 {
        let rows = self.rows.unwrap_or(ROWS_DEFAULT).clamp(1, ROWS_MAX);
        if self.cols() == 1 { rows.max(2) } else { rows }
    }
    /// A button anywhere, in any folder, by its id
    pub fn find(&self, id: &str) -> Option<&QuickItem> {
        fn look<'a>(items: &'a [QuickItem], id: &str) -> Option<&'a QuickItem> {
            items.iter().find_map(|i| if i.id == id { Some(i) } else { look(&i.items, id) })
        }
        look(&self.items, id)
    }
}

/// Whether a place is the way back out of a folder
pub fn is_way_out(in_folder: bool, row: u16, col: u16) -> bool {
    in_folder && row == 0 && col == 0
}

/// Every button in a place of its own, inside the grid, in every folder.
///
/// What is already in a free place inside the grid stays exactly there. The
/// rest -- outside the grid, on a page past `PAGES_MAX`, on a place another
/// button already holds, or on a folder's way out -- moves to the first free
/// place in reading order, starting from the page it was on, adding pages
/// when there is no room. They move in their own reading order, so buttons
/// that were side by side are still in the same order afterwards.
///
/// Nothing is dropped: a smaller grid is a choice about the layout, not about
/// which commands to keep. Names and bodies past their limits are cut, and a
/// button with no id -- or one another button already has -- is given one
/// from where it is.
pub fn arrange(spec: &QuickSpec) -> QuickSpec {
    let cols = spec.cols();
    let rows = spec.rows();
    let (mut items, pages) = arrange_level(&spec.items, spec.pages, cols, rows, false);
    name_every_button(&mut items);
    QuickSpec { cols: Some(cols), rows: Some(rows), pages: Some(pages), items }
}

fn arrange_level(
    items: &[QuickItem],
    pages: Option<u16>,
    cols: u16,
    rows: u16,
    in_folder: bool,
) -> (Vec<QuickItem>, u16) {
    let mut taken: HashSet<(u16, u16, u16)> = HashSet::new();
    let mut placed: Vec<QuickItem> = Vec::new();
    let mut homeless: Vec<QuickItem> = Vec::new();
    for item in items {
        let mut item = item.clone();
        item.label = cut(&item.label, LABEL_MAX);
        if item.kind != Kind::Ai {
            item.ai = String::new();
        }
        if item.kind == Kind::Folder {
            item.body = String::new();
            let (inner, inner_pages) = arrange_level(&item.items, item.pages, cols, rows, true);
            item.items = inner;
            item.pages = Some(inner_pages);
        } else {
            item.body = cut(&item.body, BODY_MAX);
            item.items = Vec::new();
            item.pages = None;
        }
        let inside = item.row < rows
            && item.col < cols
            && item.page < PAGES_MAX
            && !is_way_out(in_folder, item.row, item.col);
        if inside && taken.insert((item.page, item.row, item.col)) {
            placed.push(item);
        } else {
            homeless.push(item);
        }
    }
    homeless.sort_by_key(|i| (i.page, i.row, i.col));
    for mut item in homeless {
        // From its own page when it has a believable one; from the first
        // page when the number it carries is not one
        let mut page = if item.page < PAGES_MAX { item.page } else { 0 };
        'find: loop {
            for row in 0..rows {
                for col in 0..cols {
                    if !is_way_out(in_folder, row, col) && taken.insert((page, row, col)) {
                        (item.page, item.row, item.col) = (page, row, col);
                        break 'find;
                    }
                }
            }
            page += 1;
        }
        placed.push(item);
    }
    placed.sort_by_key(|i| (i.page, i.row, i.col));
    let used = placed.iter().map(|i| i.page + 1).max().unwrap_or(1);
    let asked = pages.unwrap_or(1).clamp(1, PAGES_MAX);
    (placed, asked.max(used))
}

/// Ids, across every folder: kept when they are usable and the first of
/// their spelling, made from where the button is otherwise. Walked in the
/// same order both times, so the same file gives the same ids every time
fn name_every_button(items: &mut [QuickItem]) {
    fn usable(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 64
            && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    }
    fn keep(items: &[QuickItem], seen: &mut HashSet<String>, out: &mut Vec<bool>) {
        for i in items {
            out.push(usable(&i.id) && seen.insert(i.id.clone()));
            keep(&i.items, seen, out);
        }
    }
    fn give(items: &mut [QuickItem], at: &str, keeps: &mut std::slice::Iter<bool>, seen: &mut HashSet<String>) {
        for i in items.iter_mut() {
            let here = format!("{at}{}-{}-{}", i.page, i.row, i.col);
            if !keeps.next().copied().unwrap_or(false) {
                let mut id = here.clone();
                let mut n = 2;
                while seen.contains(&id) {
                    id = format!("{here}-{n}");
                    n += 1;
                }
                seen.insert(id.clone());
                i.id = id;
            }
            give(&mut i.items, &format!("{here}-in-"), keeps, seen);
        }
    }
    let mut seen = HashSet::new();
    let mut keeps = Vec::new();
    keep(items, &mut seen, &mut keeps);
    give(items, "at-", &mut keeps.iter(), &mut seen);
}

/// At most `max` characters, cut on a character rather than a byte
fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => text[..at].to_string(),
        None => text.to_string(),
    }
}

/// One button as the board draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuickTile {
    pub id: String,
    pub page: u16,
    pub row: u16,
    pub col: u16,
    pub label: String,
    pub kind: &'static str,
    pub enter: bool,
    /// For a prompt, the AI it is for (empty: any)
    pub ai: String,
    /// The inside of the drawing's `<svg>`, from the icon set carried in the
    /// program. Empty when it has none, or names one the set does not have
    pub svg: &'static str,
    /// What it sends, as written -- a secret by its name, never its value.
    /// Shown when the pointer rests on the button: what runs is shown
    pub body: String,
    /// A folder's pages and buttons
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<u16>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<QuickTile>,
}

/// The quick commands as the board draws them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct QuickView {
    pub cols: u16,
    pub rows: u16,
    pub pages: u16,
    pub items: Vec<QuickTile>,
    /// Every destination a button here can have, as `dest_key` spells it, so
    /// the state can say where each one would go right now without being
    /// worked out for every button on every frame
    pub dests: Vec<String>,
}

pub fn view(spec: &QuickSpec) -> QuickView {
    fn tiles(items: Vec<QuickItem>, dests: &mut Vec<String>) -> Vec<QuickTile> {
        items
            .into_iter()
            .map(|i| {
                if i.kind != Kind::Folder {
                    let key = dest_key(i.kind, &i.ai);
                    if !dests.contains(&key) {
                        dests.push(key);
                    }
                }
                QuickTile {
                    svg: icon(&i.icon).unwrap_or(""),
                    kind: i.kind.word(),
                    id: i.id,
                    page: i.page,
                    row: i.row,
                    col: i.col,
                    label: i.label,
                    enter: i.enter,
                    ai: i.ai,
                    body: i.body,
                    pages: i.pages,
                    items: tiles(i.items, dests),
                }
            })
            .collect()
    }
    let spec = arrange(spec);
    let mut dests = Vec::new();
    let items = tiles(spec.items.clone(), &mut dests);
    dests.sort();
    QuickView { cols: spec.cols(), rows: spec.rows(), pages: spec.pages.unwrap_or(1), items, dests }
}

/// How a destination is named in the state: `terminal`, or `ai:` and the AI
/// (`ai:` alone for any). The page reads a button's with the same rule
pub fn dest_key(kind: Kind, ai: &str) -> String {
    match kind {
        Kind::Ai => format!("ai:{ai}"),
        _ => "terminal".to_string(),
    }
}

/// Where a button of one kind would go if it were pressed now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuickDest {
    /// `open` (a new tab) or `none`
    pub how: &'static str,
    /// Where the new tab would open and with what, or the sentence saying why
    /// it cannot go
    pub name: String,
}

/// The drawings of the icons the buttons use, by name, for a page that holds
/// buttons by their icon's name (the settings' editor) and has not fetched
/// the whole set
pub fn drawings(spec: &QuickSpec) -> std::collections::BTreeMap<String, &'static str> {
    fn walk(items: &[QuickItem], out: &mut std::collections::BTreeMap<String, &'static str>) {
        for i in items {
            if let Some(svg) = icon(&i.icon) {
                out.insert(i.icon.clone(), svg);
            }
            walk(&i.items, out);
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(&spec.items, &mut out);
    out
}

/// The names of the secrets a body asks for, each once, in the order written
pub fn secret_refs(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in secret_re().captures_iter(body) {
        let name = c[1].to_string();
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The body with every secret it names put in.
///
/// `value` is asked once per name, and is where the refusing happens: a name
/// this desk does not have, or one not open to whoever is on the receiving
/// end. The first refusal is the answer and nothing is sent -- a command with
/// half its secrets in is not a smaller version of the same command.
pub fn expand(
    body: &str,
    mut value: impl FnMut(&str) -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    let mut got: HashMap<String, String> = HashMap::new();
    for name in secret_refs(body) {
        let v = value(&name)?;
        got.insert(name, v);
    }
    Ok(secret_re()
        .replace_all(body, |c: &regex::Captures| got.get(&c[1]).cloned().unwrap_or_default())
        .into_owned())
}

fn secret_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(SECRET_REF).expect("the secret pattern compiles"))
}

/// What a button looks like, shared by the launcher (`shell.rs`) and the
/// settings' editor (`webui.rs`), so a button drawn in one is the same button
/// in the other. The frame around it -- floating over the board, or sitting in
/// a grid being edited -- is each page's own.
///
/// Free of `__` and `{{`, like the toast: both pages are checked for leftover
/// placeholders after they are put together. The two drawings a folder and the
/// way out wear without a picture of their own are poured in from the icon set
/// by `render`, not copied here.
pub const CSS: &str = r#"
 .qface { position:relative; width:100%; height:100%; box-sizing:border-box; overflow:hidden;
   display:flex; flex-direction:column; align-items:center; justify-content:center;
   gap:var(--s1); padding:var(--s2); color:var(--text); text-align:center; }
 .qface svg { flex:none; width:42%; height:42%; max-width:40px; max-height:40px; fill:none;
   stroke:currentColor; stroke-width:2; stroke-linecap:round; stroke-linejoin:round; }
 .qface .ql { max-width:100%; font-size:11px; line-height:1.3; overflow:hidden;
   text-overflow:ellipsis; white-space:nowrap; }
 /* A button with no picture: its name is the face, so it may take the room */
 .qface.bare .ql { font-size:12px; white-space:normal; overflow-wrap:anywhere;
   display:-webkit-box; -webkit-line-clamp:3; -webkit-box-orient:vertical; }
 /* Who it is for, where that is not a shell: a quiet word in the corner */
 .qface .qk { position:absolute; top:var(--s1); left:var(--s1); font-size:9px; line-height:1;
   letter-spacing:.02em; padding:2px 3px; border-radius:var(--r-chip); color:var(--dim);
   border:1px solid var(--line); }
 /* The way out of a folder: only the drawing */
 .qface.back { color:var(--dim); }
 @media (max-width:520px) { .qface .ql { font-size:10px; } }
"#;

pub const JS: &str = r#"
// One button's face: its drawing and its name, a folder's, or the way out.
// `t` is a button as the settings or the state hold it ({kind, label, svg or
// icon}); `svgOf` turns an icon name into its drawing where only the name is
// known. Built with the DOM rather than the page's own helper, which is not
// the same function on the two pages this is shared by
function quickFace(t, svgOf) {
  const face = document.createElement("div");
  const back = t.kind === "back";
  face.className = "qface" + (back ? " back" : "");
  let svg = back ? QUICK_BACK_SVG : (t.svg || (svgOf && t.icon ? svgOf(t.icon) : "") || "");
  if (!svg && t.kind === "folder") svg = QUICK_FOLDER_SVG;
  if (svg) {
    const ns = "http://www.w3.org/2000/svg";
    const s = document.createElementNS(ns, "svg");
    s.setAttribute("viewBox", "0 0 24 24");
    s.setAttribute("aria-hidden", "true");
    // The drawing comes from the icon set carried in the program, never from
    // anything a person typed
    s.innerHTML = svg;
    face.append(s);
  } else if (!back) {
    face.classList.add("bare");
  }
  if (!back) {
    const l = document.createElement("span");
    l.className = "ql";
    l.textContent = t.label || "";
    face.append(l);
  }
  if (t.kind === "ai") {
    const k = document.createElement("span");
    k.className = "qk";
    k.textContent = "AI";
    face.append(k);
  }
  return face;
}
"#;

/// Drops the shared button face into a page carrying `{{QUICK_CSS}}` and
/// `{{QUICK_JS}}`
pub fn render(html: String) -> String {
    let svg = |name: &str| serde_json::to_string(icon(name).unwrap_or("")).unwrap_or_else(|_| "\"\"".into());
    html.replace("{{QUICK_CSS}}", CSS).replace(
        "{{QUICK_JS}}",
        &format!(
            "const QUICK_FOLDER_SVG = {};\nconst QUICK_BACK_SVG = {};\n{JS}",
            svg("folder"),
            svg("corner-up-left")
        ),
    )
}

/// The icon set, carried inside the program (Lucide, ISC; its licence is in
/// THIRD-PARTY-NOTICES.txt). One line per drawing: name, search words, and
/// the inside of its `<svg>`, tab-separated. Built by tools/make-quick-icons.mjs
const ICONS: &str = include_str!("../../../vendor/lucide/icons.tsv");

/// Where the whole set is served, for the settings page's picker. The board
/// never asks: it is handed the drawings of the buttons it has
pub const ICONS_PATH: &str = "/vendor/lucide/icons.tsv";

/// The set's bytes, if the path names it. The name is compared whole, so there
/// is nothing here to walk out of
pub fn asset(path: &str) -> Option<&'static [u8]> {
    (path == ICONS_PATH).then_some(ICONS.as_bytes())
}

/// The inside of one drawing's `<svg>`, by name
pub fn icon(name: &str) -> Option<&'static str> {
    static INDEX: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    if name.is_empty() {
        return None;
    }
    INDEX
        .get_or_init(|| {
            ICONS
                .lines()
                .filter(|l| !l.starts_with('#'))
                .filter_map(|l| {
                    let l = l.trim_end_matches('\r');
                    let mut parts = l.splitn(3, '\t');
                    let name = parts.next()?;
                    let _words = parts.next()?;
                    Some((name, parts.next()?))
                })
                .collect()
        })
        .get(name)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, page: u16, row: u16, col: u16) -> QuickItem {
        QuickItem { id: id.into(), page, row, col, label: id.into(), enter: true, ..Default::default() }
    }

    fn folder(id: &str, page: u16, row: u16, col: u16, items: Vec<QuickItem>) -> QuickItem {
        QuickItem { kind: Kind::Folder, items, ..item(id, page, row, col) }
    }

    fn places(items: &[QuickItem]) -> Vec<(String, u16, u16, u16)> {
        items.iter().map(|i| (i.id.clone(), i.page, i.row, i.col)).collect()
    }

    /// A grid made bigger moves nothing: every place still exists
    #[test]
    fn a_bigger_grid_leaves_every_button_where_it_was() {
        let spec = QuickSpec {
            cols: Some(10),
            rows: Some(5),
            pages: Some(2),
            items: vec![item("a", 0, 0, 7), item("b", 0, 3, 0), item("c", 1, 2, 5)],
        };
        let out = arrange(&spec);
        assert_eq!(
            places(&out.items),
            vec![("a".into(), 0, 0, 7), ("b".into(), 0, 3, 0), ("c".into(), 1, 2, 5)]
        );
        assert_eq!(out.pages, Some(2));
    }

    /// A grid made smaller moves only what fell outside it, into the first
    /// free places of its own page, keeping their order
    #[test]
    fn a_smaller_grid_moves_only_what_no_longer_fits() {
        let spec = QuickSpec {
            cols: Some(4),
            rows: Some(2),
            pages: Some(1),
            items: vec![
                item("stay", 0, 0, 0),
                item("far", 0, 1, 7),
                item("farther", 0, 3, 2),
                item("also", 0, 0, 1),
            ],
        };
        let out = arrange(&spec);
        assert_eq!(
            places(&out.items),
            vec![
                ("stay".into(), 0, 0, 0),
                ("also".into(), 0, 0, 1),
                ("far".into(), 0, 0, 2),
                ("farther".into(), 0, 0, 3),
            ]
        );
    }

    /// When a page has no room left, what moves spills onto the next one --
    /// adding a page rather than losing a command
    #[test]
    fn no_command_is_lost_when_the_grid_cannot_hold_them() {
        let items = (0..5).map(|n| item(&format!("n{n}"), 0, 0, n)).collect();
        let spec = QuickSpec { cols: Some(2), rows: Some(1), pages: Some(1), items };
        let out = arrange(&spec);
        assert_eq!(out.items.len(), 5);
        assert_eq!(out.pages, Some(3));
        let mut seen = HashSet::new();
        assert!(out.items.iter().all(|i| i.col < 2 && i.row < 1 && seen.insert((i.page, i.row, i.col))));
    }

    /// Two buttons written into one place: the first keeps it
    #[test]
    fn a_place_holds_one_button() {
        let spec = QuickSpec { items: vec![item("first", 0, 1, 1), item("second", 0, 1, 1)], ..Default::default() };
        let out = arrange(&spec);
        assert_eq!(out.find("first").map(|i| (i.row, i.col)), Some((1, 1)));
        assert_ne!(out.find("second").map(|i| (i.row, i.col)), Some((1, 1)));
    }

    /// A hand-written page number from nowhere is not a pager with that many
    /// pages
    #[test]
    fn a_page_number_from_nowhere_is_brought_back() {
        let spec = QuickSpec { items: vec![item("lost", 60000, 0, 0)], ..Default::default() };
        let out = arrange(&spec);
        assert_eq!(out.pages, Some(1));
        assert_eq!(out.items[0].page, 0);
    }

    /// Every button can be pressed by its id, so ids are one of a kind in
    /// every folder together, and a missing or unusable one is made from the
    /// place
    #[test]
    fn every_button_has_an_id_of_its_own() {
        let spec = QuickSpec {
            items: vec![
                item("", 0, 0, 0),
                item("same", 0, 0, 1),
                item("same", 0, 0, 2),
                item("a b", 0, 0, 3),
                folder("box", 0, 1, 0, vec![item("same", 0, 0, 1), item("", 0, 0, 2)]),
            ],
            ..Default::default()
        };
        let out = arrange(&spec);
        let ids: Vec<&str> = out.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids[0], "at-0-0-0");
        assert_eq!(ids[1], "same");
        assert_ne!(ids[2], "same");
        assert_eq!(ids[3], "at-0-0-3");
        let inside: Vec<&str> = out.items[4].items.iter().map(|i| i.id.as_str()).collect();
        assert_ne!(inside[0], "same", "an id inside a folder repeats one outside");
        assert_eq!(inside[1], "at-0-1-0-in-0-0-2");
        let mut all = HashSet::new();
        fn collect<'a>(items: &'a [QuickItem], all: &mut HashSet<&'a str>) -> bool {
            items.iter().all(|i| all.insert(i.id.as_str()) && collect(&i.items, all))
        }
        assert!(collect(&out.items, &mut all));
        // And the same file gives the same ids every time it is read
        assert_eq!(arrange(&spec), out);
        assert_eq!(arrange(&out), out, "arranging twice changes something");
    }

    /// A file that never mentioned the grid gets the default, and one that
    /// asked for the impossible gets the nearest possible
    #[test]
    fn the_grid_is_always_one_that_can_be_drawn() {
        let out = arrange(&QuickSpec::default());
        assert_eq!((out.cols, out.rows, out.pages), (Some(COLS_DEFAULT), Some(ROWS_DEFAULT), Some(1)));
        let out = arrange(&QuickSpec { cols: Some(0), rows: Some(99), pages: Some(0), items: vec![] });
        assert_eq!((out.cols, out.rows, out.pages), (Some(1), Some(ROWS_MAX), Some(1)));
        // One place in all would leave a folder nothing but its way out
        let out = arrange(&QuickSpec { cols: Some(1), rows: Some(1), pages: None, items: vec![] });
        assert_eq!((out.cols, out.rows), (Some(1), Some(2)));
    }

    /// A folder's first place is its way out, on every page: a button written
    /// there moves, and nothing is ever moved onto it
    #[test]
    fn a_folder_keeps_its_way_out_clear() {
        let inner = vec![item("on-exit", 0, 0, 0), item("page2", 1, 0, 0), item("fine", 0, 0, 1)];
        let spec = QuickSpec {
            cols: Some(3),
            rows: Some(1),
            pages: None,
            items: vec![folder("f", 0, 0, 0, inner)],
        };
        let out = arrange(&spec);
        // At the top level the first place is an ordinary place
        assert_eq!((out.items[0].row, out.items[0].col), (0, 0));
        let f = &out.items[0];
        assert!(f.items.iter().all(|i| !is_way_out(true, i.row, i.col)), "{:?}", places(&f.items));
        assert_eq!(f.items.len(), 3);
        assert_eq!(out.find("fine").map(|i| (i.page, i.row, i.col)), Some((0, 0, 1)));
    }

    /// A folder holds buttons and sends nothing; a command holds no buttons
    #[test]
    fn only_a_folder_holds_buttons() {
        let mut cmd = item("cmd", 0, 0, 0);
        cmd.items = vec![item("stray", 0, 0, 1)];
        let mut f = folder("f", 0, 0, 1, vec![item("in", 0, 0, 1)]);
        f.body = "rm -rf".into();
        let out = arrange(&QuickSpec { items: vec![cmd, f], ..Default::default() });
        assert!(out.find("cmd").unwrap().items.is_empty());
        assert_eq!(out.find("f").unwrap().body, "");
        assert!(out.find("in").is_some(), "a button inside a folder is not found by its id");
        assert!(!Kind::Folder.fits(true) && !Kind::Folder.fits(false));
    }

    /// Enter is pressed unless somebody said not to; the kind is a shell's
    /// unless somebody said otherwise
    #[test]
    fn a_hand_written_button_needs_only_what_it_sends() {
        let spec: QuickSpec = serde_json::from_str(r#"{"items":[{"label":"ls","body":"ls -la"}]}"#).unwrap();
        let i = &spec.items[0];
        assert!(i.enter);
        assert_eq!(i.kind, Kind::Terminal);
        let spec: QuickSpec =
            serde_json::from_str(r#"{"items":[{"kind":"ai","enter":false},{"kind":"folder","items":[{"label":"x"}]}]}"#)
                .unwrap();
        assert_eq!(spec.items[0].kind, Kind::Ai);
        assert!(!spec.items[0].enter);
        assert_eq!(spec.items[1].items.len(), 1);
    }

    #[test]
    fn a_kind_goes_to_its_own_kind_of_tab_only() {
        assert!(Kind::Terminal.fits(false));
        assert!(!Kind::Terminal.fits(true));
        assert!(Kind::Ai.fits(true));
        assert!(!Kind::Ai.fits(false));
    }

    /// A secret is put in where it is named, each name asked for once, and
    /// the text around it is left exactly as written -- braces that are not a
    /// secret included
    #[test]
    fn secrets_are_put_in_where_they_are_named() {
        let mut asked = Vec::new();
        let out = expand("curl -H 'X: {{ secrets.tok }}' {{secrets.tok}} {{.Other}} ${HOME}", |n| {
            asked.push(n.to_string());
            Ok("VALUE".into())
        })
        .unwrap();
        assert_eq!(out, "curl -H 'X: VALUE' VALUE {{.Other}} ${HOME}");
        assert_eq!(asked, vec!["tok".to_string()]);
    }

    /// One refusal and nothing is sent
    #[test]
    fn a_secret_that_is_refused_stops_the_whole_command() {
        let out = expand("a {{secrets.ok}} b {{secrets.no}}", |n| {
            if n == "no" { anyhow::bail!("refused") } else { Ok("x".into()) }
        });
        assert!(out.is_err());
    }

    #[test]
    fn a_body_without_secrets_is_not_touched() {
        let body = "echo {{secret.x}} {{secrets.}} {{secrets.a.b}}";
        assert!(secret_refs(body).is_empty());
        assert_eq!(expand(body, |_| anyhow::bail!("asked")).unwrap(), body);
    }

    /// The pattern allows exactly the names a secret may have
    #[test]
    fn the_names_the_pattern_reads_are_names_a_secret_can_have() {
        for name in secret_refs("{{secrets.a_b-9}} {{secrets.Z}}") {
            assert!(crate::config::valid_secret_name(&name), "{name}");
        }
    }

    /// The icon set is carried, and a name finds its drawing
    #[test]
    fn the_icons_are_carried_and_found_by_name() {
        let t = icon("terminal").expect("terminal is in the set");
        assert!(t.starts_with('<') && t.contains("path"), "{t}");
        assert!(icon("").is_none());
        assert!(icon("no-such-icon").is_none());
        assert!(icon("folder").is_some() && icon("corner-up-left").is_some());
        assert!(asset(ICONS_PATH).is_some_and(|b| b.len() > 100_000));
        assert!(asset("/vendor/lucide/../../config.json").is_none());
        // Every line is a name, words and a drawing, and the drawing is only
        // shapes: it is put into the page as markup
        let handler = regex::Regex::new(r"(?i)\son[a-z]+\s*=|<script|<foreign|href").unwrap();
        for l in ICONS.lines().filter(|l| !l.starts_with('#')) {
            let parts: Vec<&str> = l.trim_end_matches('\r').split('\t').collect();
            assert_eq!(parts.len(), 3, "{l}");
            assert!(!handler.is_match(parts[2]), "{l}");
        }
    }

    /// The shared face is poured into a page whole, with the two drawings it
    /// wears by default, and looks like nobody's placeholder
    #[test]
    fn the_shared_face_is_poured_in_whole() {
        for part in [CSS, JS] {
            assert!(!part.contains("__") && !part.contains("{{"), "{part}");
        }
        let page = render("<style>{{QUICK_CSS}}</style><script>{{QUICK_JS}}</script>".into());
        assert!(!page.contains("{{"));
        assert!(page.contains("function quickFace("));
        assert!(!page.contains("const QUICK_FOLDER_SVG = \"\""), "the folder has no drawing");
        assert!(!page.contains("const QUICK_BACK_SVG = \"\""), "the way out has no drawing");
    }

    /// The board is handed drawings, not names it would have to look up --
    /// inside folders too
    #[test]
    fn the_board_is_handed_the_drawing() {
        let spec = QuickSpec {
            items: vec![
                QuickItem { icon: "folder".into(), label: "F".into(), ..item("f", 0, 0, 0) },
                folder("box", 0, 0, 1, vec![QuickItem { icon: "terminal".into(), ..item("t", 0, 0, 1) }]),
            ],
            ..Default::default()
        };
        let v = view(&spec);
        assert!(v.items[0].svg.contains("path"));
        assert_eq!(v.items[0].kind, "terminal");
        assert_eq!(v.items[1].kind, "folder");
        assert!(v.items[1].items[0].svg.contains("path"));
    }
}
