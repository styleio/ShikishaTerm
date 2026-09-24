//! What the ? knows: every settings screen, and the words on it.
//!
//! The ? beside the gear answers questions about using this program, and the
//! only way an answer about the settings can be right is if what it reads was
//! built from the settings themselves. So nothing here is written by hand.
//! Two machine-readable sources are joined:
//!
//! - **`lang/<code>.json`** holds every word the settings screen shows. It
//!   cannot fall behind, because the screen is drawn from it.
//! - **the settings page's own script** says which screens exist
//!   (`globalSections()` / `deskSections()`) and which function draws each one.
//!
//! Following the calls out of that function gives the words that screen shows.
//! Two levels deep, and past the first step never into a part three or more
//! screens share -- go further and everything travels through `el` and `row`
//! until every word belongs to every screen. Measured on the page as it
//! stands: 663 of the 1,117 `settings.*` words land on a screen, 65 of them on
//! two, 9 on more than two, and no screen comes out empty.
//!
//! The words left over are still in the index; they simply **name no screen**.
//! Saying nothing about where a thing is beats saying something false, and some
//! of it cannot be known from the source at all -- a page that looks up
//! `T["settings.dsec." + id]` has written that key down nowhere.
//!
//! The design memo is `docs/design/guide.md`.

use std::collections::{BTreeMap, HashMap, HashSet};

/// Whose settings a screen belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// The program's own, the same on every desk
    Program,
    /// One desk's, which each desk answers for itself
    Desk,
    /// One project's, which each project answers for itself: one page,
    /// drawn for whichever project is asked about, reached by a folder in it
    Project,
}

/// One settings screen, as the board can ask for it by name.
#[derive(Clone, Debug)]
pub struct Screen {
    /// The deep-link handle: what `openSettings("...")` is given
    pub id: String,
    pub scope: Scope,
    /// The dictionary key of its name in the list, and of its one-line subtitle
    pub label_key: String,
    pub sub_key: String,
    /// Every `settings.*` word this screen shows, in the order the page holds
    /// them, without repeats
    pub words: Vec<String>,
    /// Of those, the ones the page puts at the head of a card
    pub cards: HashSet<String>,
    /// ...and the ones it writes beside a field, as that field's name
    pub labels: HashSet<String>,
}

/// One thing on a screen: what it is called, and the line under it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Item {
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

/// A screen with its words resolved into one language.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Page {
    /// The handle, which is the screen's own name for the program's settings
    /// and `desk:<name>` for a desk's. Two screens are called "Basic" -- the
    /// program's and every desk's -- so the name alone would send half the
    /// answers to the wrong one
    pub id: String,
    /// Where a person is told to go, spelled out: "Settings > Phone connection"
    pub at: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// The cards on this screen, each with the things a person sets on it
    pub cards: Vec<Card>,
    /// What else this screen says: states, warnings, the wording on buttons.
    /// Not settings, but somebody who has read one of them may well ask
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub says: Vec<String>,
}

/// One card on a screen, as the person sees it laid out.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Card {
    /// Empty for whatever stands above the first card's own heading
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub items: Vec<Item>,
}

/// The whole of what the ? is given.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Index {
    pub pages: Vec<Page>,
    /// Words that belong to the settings but name no screen
    pub loose: Vec<Item>,
    /// Every key combination, as it ships
    pub keys: Vec<Item>,
}

impl Screen {
    /// What this screen is called where one screen has to be named: its own
    /// name for the program's settings, `desk:<name>` for a desk's.
    pub fn handle(&self) -> String {
        match self.scope {
            Scope::Program | Scope::Project => self.id.clone(),
            Scope::Desk => format!("{DESK}{}", self.id),
        }
    }
}

/// What a desk's screen is called in front of its own name.
pub const DESK: &str = "desk:";

/// Where a screen is, spelled out for a person: "Settings > Phone connection".
pub fn where_it_is(s: &Screen) -> String {
    crate::i18n::tp(at_key(s.scope), &[("name", &crate::i18n::t(&s.label_key))])
}

/// The dictionary key that spells out where a screen of this scope is.
fn at_key(scope: Scope) -> &'static str {
    match scope {
        Scope::Program => "guide.at.program",
        Scope::Desk => "guide.at.desk",
        Scope::Project => "guide.at.project",
    }
}

/// The dictionary keys the reference heads a scope's screens with: the
/// heading, and the line under it saying whose these settings are
fn doc_keys(scope: Scope) -> (&'static str, &'static str) {
    match scope {
        Scope::Program => ("guide.doc.program", "guide.doc.program.about"),
        Scope::Desk => ("guide.doc.desk", "guide.doc.desk.about"),
        Scope::Project => ("guide.doc.project", "guide.doc.project.about"),
    }
}

/// The screen a handle names, if this program has one.
pub fn screen_of(handle: &str) -> Option<&'static Screen> {
    screens().iter().find(|s| s.handle() == handle)
}

// ── Reading the page ──────────────────────────────────────────────

/// The screens, worked out once from the settings page's own script.
pub fn screens() -> &'static [Screen] {
    static ONCE: std::sync::OnceLock<Vec<Screen>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| read_screens(crate::webui::page()))
}

/// How far out of a screen's own function the words are followed.
const DEPTH: usize = 2;

/// How many screens have to reach a function before it counts as a part they
/// share rather than one screen's own.
const SHARED_AT: usize = 3;

/// A function the page defines, as a span of the script.
struct Fun {
    /// Where its body starts and ends, in characters
    from: usize,
    to: usize,
}

/// Everything the script defines that has a body, by name.
///
/// Both ways the page writes one: `function name(...) {...}` and
/// `const name = (...) => {...}`. A function written inside another is inside
/// its body already, so it is found through whoever holds it.
fn functions(code: &[char]) -> HashMap<String, Fun> {
    let mut out: HashMap<String, Fun> = HashMap::new();
    let mut i = 0;
    while i < code.len() {
        let Some(start) = name_start(code, i) else { break };
        let (name, after) = name_at(code, start);
        i = after;
        // `function name(` -- the word before it says so
        let declared = word_before(code, start);
        let opens = match declared.as_str() {
            "function" => skip_params(code, after),
            "const" | "let" | "var" => arrow_body(code, after),
            _ => None,
        };
        let Some(open) = opens else { continue };
        if let Some(end) = matching_brace(code, open) {
            out.entry(name).or_insert(Fun { from: open + 1, to: end });
        }
    }
    out
}

/// Where the next name begins, at or after `i`.
fn name_start(code: &[char], mut i: usize) -> Option<usize> {
    while i < code.len() {
        let c = code[i];
        if (c.is_ascii_alphabetic() || c == '_' || c == '$') && !is_name(code.get(i.wrapping_sub(1)))
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn is_name(c: Option<&char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
}

/// The name that starts at `i`, and where it ends.
fn name_at(code: &[char], i: usize) -> (String, usize) {
    let mut j = i;
    while j < code.len() && is_name(code.get(j)) {
        j += 1;
    }
    (code[i..j].iter().collect(), j)
}

/// The word standing before the name at `i`, if any.
fn word_before(code: &[char], i: usize) -> String {
    let mut j = i;
    while j > 0 && code[j - 1].is_whitespace() {
        j -= 1;
    }
    let end = j;
    while j > 0 && is_name(code.get(j - 1)) {
        j -= 1;
    }
    code[j..end].iter().collect()
}

/// Past a parameter list, to the `{` of the body.
fn skip_params(code: &[char], i: usize) -> Option<usize> {
    let open = next_code(code, i)?;
    if code[open] != '(' {
        return None;
    }
    let close = matching(code, open, '(', ')')?;
    let brace = next_code(code, close + 1)?;
    (code[brace] == '{').then_some(brace)
}

/// Past `= (...) =>` or `= x =>`, to the `{` of the body. An arrow that
/// answers with an expression rather than a block has no body to walk.
fn arrow_body(code: &[char], i: usize) -> Option<usize> {
    let eq = next_code(code, i)?;
    if code[eq] != '=' || code.get(eq + 1) == Some(&'=') || code.get(eq + 1) == Some(&'>') {
        return None;
    }
    let mut at = next_code(code, eq + 1)?;
    // `async` before the parameters
    if code[at].is_ascii_alphabetic() {
        let (w, after) = name_at(code, at);
        if w == "async" {
            at = next_code(code, after)?;
        }
    }
    let arrow = if code[at] == '(' {
        matching(code, at, '(', ')')? + 1
    } else {
        // One parameter, written without brackets
        let (_, after) = name_at(code, at);
        after
    };
    let a = next_code(code, arrow)?;
    if code[a] != '=' || code.get(a + 1) != Some(&'>') {
        return None;
    }
    let brace = next_code(code, a + 2)?;
    (code[brace] == '{').then_some(brace)
}

/// Where the next character that is not a space is.
fn next_code(code: &[char], i: usize) -> Option<usize> {
    code.get(i..)?.iter().position(|c| !c.is_whitespace()).map(|n| i + n)
}

fn matching_brace(code: &[char], open: usize) -> Option<usize> {
    matching(code, open, '{', '}')
}

/// Where the bracket opened at `open` closes.
fn matching(code: &[char], open: usize, l: char, r: char) -> Option<usize> {
    let mut depth = 0usize;
    for (n, c) in code[open..].iter().enumerate() {
        if *c == l {
            depth += 1;
        } else if *c == r {
            depth -= 1;
            if depth == 0 {
                return Some(open + n);
            }
        }
    }
    None
}

/// Whether the name at `i` is read off something else rather than standing on
/// its own.
///
/// One dot is a property. Three are a spread, and `...gitFields(desk)` is the
/// git screen calling its own fields -- read as a property it goes unseen, and
/// that screen comes out with five words on it.
fn after_a_dot(code: &[char], i: usize) -> bool {
    i > 0 && code[i - 1] == '.' && code.get(i.wrapping_sub(2)) != Some(&'.')
}

/// The names called inside a span.
fn calls(code: &[char], span: (usize, usize)) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = span.0;
    while i < span.1 {
        let Some(start) = name_start(code, i) else { break };
        if start >= span.1 {
            break;
        }
        let (name, after) = name_at(code, start);
        i = after;
        if !after_a_dot(code, start) && next_code(code, after).is_some_and(|n| code[n] == '(') {
            out.push(name);
        }
    }
    out
}

/// The names written inside a span at all, called or not. Used for the one
/// place a screen names its drawing function without calling it (`build:card`).
fn names(code: &[char], span: (usize, usize)) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = span.0;
    while i < span.1 {
        let Some(start) = name_start(code, i) else { break };
        if start >= span.1 {
            break;
        }
        let (name, after) = name_at(code, start);
        i = after;
        if !after_a_dot(code, start) {
            out.push(name);
        }
    }
    out
}

/// Reads the screens, and the words each one shows, out of a settings page.
fn read_screens(html: &str) -> Vec<Screen> {
    let orig: Vec<char> = html.chars().collect();
    let code: Vec<char> =
        crate::pagelint::code_only(&crate::pagelint::scripts_of(html)).chars().collect();
    debug_assert_eq!(orig.len(), code.len(), "blanking the prose moved the script");
    let funs = functions(&code);

    // Each screen, and the functions named where it says what draws it
    let mut roots: Vec<(Screen, Vec<String>)> = Vec::new();
    for e in section_entries(&code, &orig) {
        let named: Vec<String> =
            names(&code, e.draws).into_iter().filter(|n| funs.contains_key(n)).collect();
        roots.push((
            Screen {
                id: e.id,
                scope: e.scope,
                label_key: e.label_key,
                sub_key: e.sub_key,
                words: Vec::new(),
                cards: HashSet::new(),
                labels: HashSet::new(),
            },
            named,
        ));
    }

    // A part three or more screens reach is theirs together, not any one's
    let mut reached: HashMap<&str, usize> = HashMap::new();
    for (_, start) in &roots {
        for name in walk(&code, &funs, start, &HashSet::new()) {
            *reached.entry(funs.get_key_value(&name).expect("walked to a known name").0.as_str())
                .or_default() += 1;
        }
    }
    let shared: HashSet<String> = reached
        .iter()
        .filter(|(_, n)| **n >= SHARED_AT)
        .map(|(f, _)| (*f).to_string())
        // A screen's own drawing function stays its own however many name it
        .filter(|f| !roots.iter().any(|(_, start)| start.contains(f)))
        .collect();

    for (screen, start) in &mut roots {
        let mut seen = HashSet::new();
        let mut spans: Vec<(usize, usize)> = walk(&code, &funs, start, &shared)
            .into_iter()
            .map(|n| {
                let f = &funs[&n];
                (f.from, f.to)
            })
            .collect();
        spans.sort_unstable();
        for span in spans {
            for key in words_in(&orig, span) {
                if seen.insert(key.clone()) {
                    screen.words.push(key);
                }
            }
            for (key, role) in roles_in(&orig, span) {
                match role {
                    Role::Card => screen.cards.insert(key),
                    Role::Label => screen.labels.insert(key),
                };
            }
        }
    }
    roots.into_iter().map(|(s, _)| s).collect()
}

/// Every function reachable from `start`, itself included, within [`DEPTH`].
///
/// What a screen calls itself is that screen's, however many other screens
/// call it too: `gitFields` is the git screen's fields whoever else wants
/// them. Only further out does a name shared by several stop being anyone's.
fn walk(
    code: &[char],
    funs: &HashMap<String, Fun>,
    start: &[String],
    skip: &HashSet<String>,
) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let mut edge: Vec<String> = start.to_vec();
    for step in 0..=DEPTH {
        let mut next = Vec::new();
        for name in edge {
            if (step > 1 && skip.contains(&name)) || !seen.insert(name.clone()) {
                continue;
            }
            let Some(f) = funs.get(&name) else { continue };
            out.push(name);
            for called in calls(code, (f.from, f.to)) {
                if funs.contains_key(&called) && !seen.contains(&called) {
                    next.push(called);
                }
            }
        }
        edge = next;
    }
    out
}

/// The `settings.*` keys a span asks the dictionary for, in the order written.
fn words_in(orig: &[char], span: (usize, usize)) -> Vec<String> {
    const OPEN: &[char] = &['T', '[', '"'];
    let mut out = Vec::new();
    let mut i = span.0;
    while i + OPEN.len() < span.1 {
        if orig[i..i + OPEN.len()] != *OPEN {
            i += 1;
            continue;
        }
        let from = i + OPEN.len();
        let Some(len) = orig[from..span.1.min(orig.len())].iter().position(|c| *c == '"') else {
            break;
        };
        let key: String = orig[from..from + len].iter().collect();
        // A key built as it runs ends where the page stops writing it, and
        // only the page knows the rest
        if key.starts_with("settings.") && !key.ends_with('.') {
            out.push(key);
        }
        i = from + len;
    }
    out
}

/// A screen as the list writes it down, before its words are gathered.
struct Entry {
    id: String,
    scope: Scope,
    label_key: String,
    sub_key: String,
    /// The span of the list that says what draws it
    draws: (usize, usize),
}

/// What the page is doing with a word where it writes it.
#[derive(Clone, Copy)]
enum Role {
    /// At the head of a card
    Card,
    /// Beside a field, as its name
    Label,
}

/// How the page says each of those, written once so that a shape that changes
/// is changed here and nowhere else.
///
/// This is the other half of telling a **setting** from a **line of text**.
/// The dictionary says one thing (a word with a hint under it is a setting);
/// the page says the other (a word written as a row's label is a setting).
/// Either is enough, because each catches what the other misses: a field
/// nobody wrote a hint for, and a hint on something built as the page runs.
const SAYS: &[(&str, Role)] = &[
    ("card(T[\"", Role::Card),
    ("el(\"h2\", {}, T[\"", Role::Card),
    ("el(\"h3\", {}, T[\"", Role::Card),
    ("row(T[\"", Role::Label),
    ("el(\"label\", {}, T[\"", Role::Label),
];

/// The words a span writes as a card's title or a field's name.
fn roles_in(orig: &[char], span: (usize, usize)) -> Vec<(String, Role)> {
    let mut out = Vec::new();
    for (says, role) in SAYS {
        let pat: Vec<char> = says.chars().collect();
        let mut i = span.0;
        while let Some(at) = find(orig, says, i, span.1) {
            let from = at + pat.len();
            let Some(len) = orig[from..span.1.min(orig.len())].iter().position(|c| *c == '"')
            else {
                break;
            };
            let key: String = orig[from..from + len].iter().collect();
            if key.starts_with("settings.") && !key.ends_with('.') {
                out.push((key, *role));
            }
            i = from + len;
        }
    }
    out
}

/// Each screen named in the two lists.
fn section_entries(code: &[char], orig: &[char]) -> Vec<Entry> {
    let mut out = Vec::new();
    // The program's own: `{id:"basic", label:T["..."], sub:T["..."], build:basicCard}`
    if let Some(body) = body_of(code, "function globalSections()") {
        let mut i = body.0;
        while let Some(at) = find(orig, "{id:\"", i, body.1) {
            let from = at + 5;
            let Some(len) = orig[from..body.1].iter().position(|c| *c == '"') else { break };
            let id: String = orig[from..from + len].iter().collect();
            let Some(end) = matching_brace(code, at) else { break };
            let mut keys = words_in(orig, (at, end));
            let label = keys.first().cloned().unwrap_or_default();
            let sub = keys.drain(..).nth(1).unwrap_or_default();
            out.push(Entry {
                id,
                scope: Scope::Program,
                label_key: label,
                sub_key: sub,
                draws: (at, end),
            });
            i = end;
        }
    }
    // A desk's: `s("git", gitCard)`, whose names the helper builds from the id
    if let Some(body) = body_of(code, "function deskSections(desk)") {
        let mut i = body.0;
        while let Some(at) = find(orig, "s(\"", i, body.1) {
            i = at + 3;
            // `...s("git"` and `things("git"` both end in `s("`
            if is_name(orig.get(at.wrapping_sub(1))) {
                continue;
            }
            let Some(len) = orig[i..body.1].iter().position(|c| *c == '"') else { break };
            let id: String = orig[i..i + len].iter().collect();
            let Some(end) = matching(code, at + 1, '(', ')') else { break };
            out.push(Entry {
                label_key: format!("settings.dsec.{id}"),
                sub_key: format!("settings.dsec.{id}.sub"),
                id,
                scope: Scope::Desk,
                draws: (at, end),
            });
            i = end;
        }
    }
    // A project's: one page, `projectPane`, drawn for whichever project is
    // asked about. It is not in a list the page holds, so it is named here;
    // the span is the function's own name, so that what draws it is that
    // function and nothing it happens to mention
    const PROJECT_PAGE: &str = "function projectPane(desk, p)";
    if let Some(at) = find(code, PROJECT_PAGE, 0, code.len()) {
        out.push(Entry {
            id: "project".into(),
            scope: Scope::Project,
            label_key: "settings.project.screen".into(),
            sub_key: "settings.project.screen.sub".into(),
            draws: (at, at + PROJECT_PAGE.chars().count()),
        });
    }
    out
}

/// The body of the block that `opens` names.
fn body_of(code: &[char], opens: &str) -> Option<(usize, usize)> {
    let at = find(code, opens, 0, code.len())?;
    let brace = code[at..].iter().position(|c| *c == '{').map(|n| at + n)?;
    let end = matching_brace(code, brace)?;
    Some((brace + 1, end))
}

fn find(hay: &[char], needle: &str, from: usize, to: usize) -> Option<usize> {
    let pat: Vec<char> = needle.chars().collect();
    let to = to.min(hay.len());
    (from..to.saturating_sub(pat.len() - 1)).find(|&i| hay[i..i + pat.len()] == pat[..])
}

// ── Turning it into an answer ─────────────────────────────────────

/// The suffixes that belong to the word above them rather than standing alone.
///
/// This is also what tells a **setting** from a **line of text**: the settings
/// were written with a label and a line under it, so a word something hangs
/// under is a thing a person sets, and a word with nothing under it is
/// something the screen says (a state, a warning, the wording on a button).
/// The rule comes off how the words were written, not off a guess at what they
/// look like.
const UNDER: &[&str] = &[".hint", ".label", ".sub", ".note", ".placeholder", ".ph"];

/// The index in one language. `word` looks a key up, and answers with the key
/// itself when there is nothing under it.
pub fn index(word: &dyn Fn(&str) -> String) -> Index {
    let mut placed: HashSet<&str> = HashSet::new();
    let mut pages = Vec::new();
    for s in screens() {
        let title = word(&s.label_key);
        let at = crate::i18n::tp(at_key(s.scope), &[("name", &title)]);
        let mut cards: Vec<Card> = vec![Card { title: String::new(), items: Vec::new() }];
        let mut says = Vec::new();
        for key in &s.words {
            placed.insert(key.as_str());
            // Its own name in the list is not a thing on the screen, and
            // neither is a line that hangs under something else
            if *key == s.label_key || *key == s.sub_key || UNDER.iter().any(|u| key.ends_with(u)) {
                continue;
            }
            let text = word(key);
            if text == *key {
                continue;
            }
            // A card's own heading opens a card, unless the page also writes
            // it beside a field -- then it is a field, where it was written
            if s.cards.contains(key) && !s.labels.contains(key) {
                // A screen with one card names it after itself, and a heading
                // that repeats the heading above it is a line to scroll past
                let head = if text == title { String::new() } else { text };
                cards.push(Card { title: head, items: Vec::new() });
                continue;
            }
            match entry(key, s.labels.contains(key), word) {
                Some(it) => cards.last_mut().expect("a card is always open").items.push(it),
                None => says.push(text),
            }
        }
        cards.retain(|c| !c.items.is_empty());
        let about = word(&s.sub_key);
        pages.push(Page {
            id: s.handle(),
            at,
            title,
            about: if about == s.sub_key { String::new() } else { about },
            cards,
            says,
        });
    }
    // Everything else the settings say, with no screen to name
    let mut loose = Vec::new();
    for key in all_settings_words() {
        if placed.contains(key.as_str()) || UNDER.iter().any(|u| key.ends_with(u)) {
            continue;
        }
        let text = word(key);
        if text != *key {
            loose.push(entry(key, true, word).expect("named, so an entry"));
        }
    }
    let keys = crate::keys::shipped_in(word)
        .into_iter()
        .map(|(press, does)| Item { label: press, hint: does })
        .collect();
    Index { pages, loose, keys }
}

/// One thing a person sets: what it is called, and the line written under it.
///
/// `None` when neither source calls it one -- the page does not write it
/// beside a field, and the dictionary hangs nothing under it -- which makes it
/// a line the screen says rather than a thing to set.
fn entry(key: &str, a_label: bool, word: &dyn Fn(&str) -> String) -> Option<Item> {
    let hint = UNDER
        .iter()
        .map(|u| format!("{key}{u}"))
        .map(|k| (word(&k), k))
        .find(|(text, k)| text != k)
        .map(|(text, _)| text);
    (a_label || hint.is_some())
        .then(|| Item { label: word(key), hint: hint.unwrap_or_default() })
}

/// Every `settings.*` key the program ships with, in dictionary order.
pub fn all_settings_words() -> &'static [String] {
    static ONCE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let en: BTreeMap<String, serde_json::Value> =
            serde_json::from_str(crate::i18n::english()).unwrap_or_default();
        en.into_iter()
            .filter(|(k, v)| k.starts_with("settings.") && v.is_string())
            .map(|(k, _)| k)
            .collect()
    })
    .as_slice()
}

// ── The reference people read ─────────────────────────────────────

/// The settings written out as a page of markdown, in one language.
///
/// The same index the ? answers from, so the two cannot disagree. Written to
/// `docs/SETTINGS<.code>.md`, from where the site picks it up and the download
/// carries it.
pub fn reference(word: &dyn Fn(&str) -> String) -> String {
    let idx = index(word);
    let mut out = String::new();
    let line = |out: &mut String, s: &str| {
        out.push_str(s);
        out.push('\n');
    };
    line(&mut out, &format!("# {}", word("guide.doc.title")));
    out.push('\n');
    line(&mut out, &word("guide.doc.intro"));
    out.push('\n');
    line(&mut out, &format!("<!-- {} -->", word("guide.doc.generated")));

    let mut scope = None;
    for (page, s) in idx.pages.iter().zip(screens()) {
        if scope != Some(s.scope) {
            scope = Some(s.scope);
            let (head, about) = doc_keys(s.scope);
            out.push('\n');
            line(&mut out, &format!("## {}", word(head)));
            out.push('\n');
            line(&mut out, &word(about));
        }
        out.push('\n');
        line(&mut out, &format!("### {}", page.title));
        out.push('\n');
        if !page.about.is_empty() {
            line(&mut out, &page.about);
            out.push('\n');
        }
        if page.cards.is_empty() {
            line(&mut out, &word("guide.doc.nothing"));
            continue;
        }
        for card in &page.cards {
            if !card.title.is_empty() {
                line(&mut out, &format!("**{}**", card.title));
                out.push('\n');
            }
            for it in &card.items {
                match it.hint.is_empty() {
                    true => line(&mut out, &format!("- **{}**", it.label)),
                    false => line(&mut out, &format!("- **{}** — {}", it.label, it.hint)),
                }
            }
            out.push('\n');
        }
        // The blank line before the next heading is written by whoever opens it
        while out.ends_with("\n\n") {
            out.pop();
        }
    }

    out.push('\n');
    line(&mut out, &format!("## {}", word("guide.doc.keys")));
    out.push('\n');
    line(&mut out, &word("guide.doc.keys.about"));
    out.push('\n');
    for k in &idx.keys {
        line(&mut out, &format!("- `{}` — {}", k.label, k.hint));
    }
    out
}

/// The lists in the manual that the program fills in for itself.
///
/// The manual is prose, and prose is written by a person. But a *list* of what
/// ships -- every key, every settings screen -- is not prose, and a list
/// written by hand falls behind: this manual went 63 commits without being
/// touched while the screen it describes changed 16 times. So the prose stays
/// hand-written and the lists are written from here, between the marks
/// `<!-- guide: keys -->` and `<!-- /guide -->`.
pub fn fill_manual(text: &str, word: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(OPENS) {
        let from = at + OPENS.len();
        let Some(len) = rest[from..].find(SHUTS) else { break };
        let Some((name, _)) = rest[from..from + len].split_once(CLOSES) else { break };
        let name = name.trim();
        out.push_str(&rest[..from]);
        out.push_str(name);
        out.push_str(CLOSES);
        out.push_str("\n\n");
        out.push_str(&filling(name, word));
        out.push('\n');
        rest = &rest[from + len..];
    }
    out.push_str(rest);
    out
}

const OPENS: &str = "<!-- guide: ";
const CLOSES: &str = " -->";
const SHUTS: &str = "<!-- /guide -->";

/// What goes between one pair of marks.
fn filling(name: &str, word: &dyn Fn(&str) -> String) -> String {
    let mut out = String::new();
    match name {
        // Every key as it ships, in the table the manual reads in
        "keys" => {
            out.push_str(&format!(
                "| {} | {} |\n|---|---|\n",
                word("guide.doc.key"),
                word("guide.doc.does")
            ));
            for (press, does) in crate::keys::shipped_in(word) {
                let press =
                    press.split(" / ").map(|k| format!("`{k}`")).collect::<Vec<_>>().join(" / ");
                out.push_str(&format!("| {press} | {does} |\n"));
            }
        }
        // Every settings screen, so the manual says where things are while the
        // reference says what each one holds
        "screens" => {
            let idx = index(word);
            let mut scope = None;
            for (page, s) in idx.pages.iter().zip(screens()) {
                if scope != Some(s.scope) {
                    scope = Some(s.scope);
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&format!("**{}**\n\n", word(doc_keys(s.scope).0)));
                }
                match page.about.is_empty() {
                    true => out.push_str(&format!("- **{}**\n", page.title)),
                    false => out.push_str(&format!("- **{}** — {}\n", page.title, page.about)),
                }
            }
        }
        _ => out.push_str(name),
    }
    out
}

/// Where a language's manual is kept.
pub fn manual_path(root: &std::path::Path, code: &str) -> std::path::PathBuf {
    root.join("docs").join(match code {
        "en" => "MANUAL.md".to_string(),
        other => format!("MANUAL.{other}.md"),
    })
}

/// What a piece of prose sends somebody to, of the paths it writes into the
/// settings, that the settings no longer call that.
///
/// "Settings > Basic > Deleting a worktree" is a promise about a screen, and a
/// screen can be renamed by somebody who never opens the manual. Reading the
/// promises back out is what lets a test say so -- which is why the manual
/// writes a path this one way and no other.
///
/// A name is matched against what the settings offer rather than read to the
/// next full stop, because a name has spaces in it ("Default command") and a
/// sentence carries on afterwards ("...says what runs"). The longest name that
/// fits is the one meant: `Git` must not answer for `Git accounts`.
pub fn paths_wrong(text: &str, word: &dyn Fn(&str) -> String, offered: &HashSet<String>) -> Vec<String> {
    /// What can never be part of a name, so an unknown one stops there
    const ENDS: &str = "、。（）()「」『』,|\n*`";
    /// What stands between one name and the next
    const STEP: &str = " > ";
    let head = word("guide.doc.path.head");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(&head) {
        rest = &rest[at + head.len()..];
        while let Some(after) = rest.strip_prefix(STEP) {
            match offered
                .iter()
                .filter(|n| after.starts_with(n.as_str()))
                .max_by_key(|n| n.len())
            {
                Some(name) => rest = &after[name.len()..],
                None => {
                    let end = after
                        .find(|c: char| ENDS.contains(c))
                        .into_iter()
                        .chain(after.find(STEP))
                        .min()
                        .unwrap_or(after.len());
                    let bad = after[..end].trim();
                    rest = &after[end..];
                    if bad.is_empty() {
                        break;
                    }
                    out.push(bad.to_string());
                }
            }
        }
    }
    out
}

/// How many paths into the settings a piece of prose writes at all, so a test
/// can tell "nothing is wrong" from "nothing was read".
pub fn paths_counted(text: &str, word: &dyn Fn(&str) -> String) -> usize {
    text.matches(&format!("{} > ", word("guide.doc.path.head"))).count()
}

/// Every name the settings offer, for checking what prose says against it.
pub fn names_offered(word: &dyn Fn(&str) -> String) -> HashSet<String> {
    let idx = index(word);
    let mut out = HashSet::new();
    for page in &idx.pages {
        out.insert(page.title.clone());
        for card in &page.cards {
            out.insert(card.title.clone());
            for it in &card.items {
                out.insert(it.label.clone());
            }
        }
    }
    for it in &idx.loose {
        out.insert(it.label.clone());
    }
    // The word the list of a desk's own settings stands under
    out.insert(word("settings.nav.desk"));
    out.remove("");
    out
}

/// Where a language's reference is kept.
pub fn reference_path(root: &std::path::Path, code: &str) -> std::path::PathBuf {
    root.join("docs").join(match code {
        "en" => "SETTINGS.md".to_string(),
        other => format!("SETTINGS.{other}.md"),
    })
}

// ── The panel ────────────────────────────────────────────────────

/// The ? panel, as it is written before a language is laid over it.
///
/// Served by the settings' own loopback server, because that is where the
/// token, the dictionary and the phone's way in already are. In the window it
/// is a page the app places over the board; on a phone the board lays it over
/// itself in a frame, which is why nothing here touches the window.
pub fn page() -> &'static str {
    PANEL
}

const PANEL: &str = r##"<!doctype html>
<html lang="{{__lang__}}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{guide.title}}</title>
<style>
:root{ {{THEME}}
  --s1:4px; --s2:6px; --s3:10px; --s4:14px; --s5:18px;
  --ui: -apple-system, "Segoe UI", "Hiragino Kaku Gothic ProN", "Noto Sans JP", sans-serif;
  --mono: "Cascadia Mono", Consolas, "Noto Sans Mono", monospace; }
*{box-sizing:border-box}
html,body{height:100%;margin:0}
/* The page is the panel: what is placed in the window has no frame of its
   own, so the edge and the corners are drawn here (style guide 5.2) */
body{background:var(--panel);color:var(--text);font:13px/1.6 var(--ui);
     display:flex;flex-direction:column;overflow:hidden;
     border:1px solid var(--line);border-radius:10px}
/* The head is the only part that is picked up */
#head{height:40px;flex:none;display:flex;align-items:center;gap:var(--s2);
      padding:0 var(--s2) 0 var(--s4);border-bottom:1px solid var(--line);
      cursor:grab;user-select:none}
#head.holding{cursor:grabbing}
#head.still{cursor:default}
#title{font-size:13.5px;font-weight:600;flex:1;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
button{font:12.5px var(--ui);color:var(--text);background:var(--raise);
       border:1px solid var(--edge);border-radius:6px;padding:var(--s1) var(--s3);cursor:pointer}
button:hover{border-color:var(--edge-hi)}
button.icon{background:none;border:none;padding:var(--s1) var(--s2);color:var(--dim);font-size:14px}
button.icon:hover{color:var(--text)}
button.go{border-color:var(--brand)}
#talk{flex:1;overflow-y:auto;padding:var(--s4);display:flex;flex-direction:column;gap:var(--s3)}
.turn{display:flex;flex-direction:column;gap:var(--s2)}
.mine{align-self:flex-end;max-width:85%;background:var(--raise);border-radius:10px;
      padding:var(--s2) var(--s3);white-space:pre-wrap;overflow-wrap:anywhere}
.theirs{max-width:95%;white-space:pre-wrap;overflow-wrap:anywhere}
.acts{display:flex;flex-wrap:wrap;gap:var(--s2)}
.bad{color:var(--stop)}
#first{color:var(--faint);font-size:11.5px}
#first p{margin:0 0 var(--s3)}
#first p:last-child{margin:var(--s3) 0 0}
#first a{color:var(--brand)}
#first li{margin:0 0 var(--s1)}
#first ul{margin:0;padding-left:var(--s5)}
#pick{flex:none;display:none;align-items:center;gap:var(--s2);margin:0 var(--s4) var(--s2);
      padding:var(--s2) var(--s3);border:1px solid var(--pick);border-radius:6px;
      background:color-mix(in srgb, var(--pick) 9%, transparent);font-size:11.5px}
#pick.on{display:flex}
#pickname{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
#foot{flex:none;display:flex;gap:var(--s2);padding:var(--s3) var(--s4);border-top:1px solid var(--line)}
#q{flex:1;min-width:0;font:13px var(--ui);color:var(--text);background:var(--bg);
   border:1px solid var(--edge);border-radius:6px;padding:var(--s2) var(--s3);resize:none;height:34px;max-height:96px}
#q:focus{outline:none;border-color:var(--brand);box-shadow:0 0 0 3px color-mix(in srgb, var(--brand) 22%, transparent)}
#q::placeholder{color:var(--faint)}
.waiting{color:var(--dim);font-size:11.5px}
</style></head><body>
<div id="head"><span id="title">{{guide.title}}</span>
  <button class="icon" id="shut" title="{{common.close}}">✕</button></div>
<div id="talk"><div id="first"></div></div>
<div id="pick"><span id="pickname"></span><button class="icon" id="unpick" title="{{guide.pick.drop}}">✕</button></div>
<div id="foot">
  <textarea id="q" rows="1" placeholder="{{guide.ask.placeholder}}" autocomplete="off"></textarea>
  <button class="go" id="send">{{guide.ask.send}}</button>
</div>
<script>
const TOKEN = "__TOKEN__";
const T = __DICT__;
// On a phone this page is laid inside the board, which owns where it sits
const INSIDE = window.parent !== window;
const el = (tag, attrs, ...kids) => {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === null || v === undefined) continue;
    if (k.startsWith("on")) n[k] = v; else n.setAttribute(k, v);
  }
  for (const c of kids) if (c !== null && c !== undefined) n.append(c);
  return n;
};
const post = (path, body) => fetch("/api/guide" + path, {method:"POST",
  headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
  body: JSON.stringify(body || {})}).then(r => r.json());
const get = path => fetch("/api/guide" + path, {headers:{"X-Token":TOKEN}}).then(r => r.json());

// ── What has been said ────────────────────────────────────
// Kept here and handed back with every question: the AI is asked once per
// question and remembers nothing of its own
let said = [];
let asking = false;
const talk = document.getElementById("talk");

function draw() {
  talk.textContent = "";
  if (!said.length) {
    const first = el("div", {id:"first"});
    first.append(el("p", {}, T["guide.first"] || ""));
    const ul = el("ul", {});
    for (const k of ["guide.first.a", "guide.first.b", "guide.first.c"]) {
      if (T[k]) ul.append(el("li", {}, T[k]));
    }
    first.append(ul);
    // What the ? used to be. Still one press away, from inside what replaced it.
    // Framed on a phone, it is the browser reading this that opens it: the app
    // would open it on the PC's screen, where nobody is standing
    first.append(el("p", {}, INSIDE
      ? el("a", {href: T["tui.help.url"] || "", target:"_blank", rel:"noopener"},
          T["guide.manual"] || "")
      : el("a", {href:"#", onclick:e => { e.preventDefault();
          fetch("/api/open?dest=manual", {headers:{"X-Token":TOKEN}}); }},
          T["guide.manual"] || "")));
    talk.append(first);
  }
  for (const turn of said) {
    const box = el("div", {class:"turn"});
    box.append(el("div", {class:"mine"}, turn.asked));
    box.append(el("div", {class:"theirs" + (turn.bad ? " bad" : "")}, turn.said));
    const acts = el("div", {class:"acts"});
    if (turn.open) {
      acts.append(el("button", {class:"go", onclick:() => go(turn)}, turn.at || T["guide.open"]));
    }
    if (turn.fill) {
      acts.append(el("button", {class:"go", onclick:() => write(turn)}, T["guide.fill"]));
    }
    if (acts.childElementCount) box.append(acts);
    talk.append(box);
  }
  if (asking) talk.append(el("div", {class:"waiting"}, T["guide.thinking"] || ""));
  talk.scrollTop = talk.scrollHeight;
}

async function ask() {
  const box = document.getElementById("q");
  const question = box.value.trim();
  if (!question || asking) return;
  box.value = "";
  asking = true;
  draw();
  let r = {};
  try {
    r = await post("/ask", {question, so_far: said.map(t => ({asked:t.asked, said:t.said}))});
  } catch (e) {
    r = {error: String(e)};
  }
  asking = false;
  said.push(r.error
    ? {asked: question, said: r.error, bad: true}
    : {asked: question, said: r.say || "", open: r.open || "", at: r.at || "", fill: r.fill || ""});
  draw();
}

// Nothing is opened or written until this is pressed: an answer is words
// until a person acts on it
async function go(turn) {
  // Framed on a phone, the board holding this frame is what opens a screen
  // there. The window's way -- leaving it for the loop that draws the window
  // -- would open it on the PC, and the phone would sit there having watched
  // a button do nothing
  if (INSIDE) {
    window.parent.postMessage({guide: "open", screen: turn.open}, location.origin);
    turn.open = "";
    draw();
    return;
  }
  const r = await post("/open", {screen: turn.open});
  if (r && r.error) { turn.said += "\n" + r.error; turn.bad = true; }
  turn.open = "";
  draw();
}
async function write(turn) {
  const r = await post("/fill", {text: turn.fill});
  if (r && r.error) { turn.said += "\n" + r.error; turn.bad = true; }
  turn.fill = "";
  draw();
}

// ── The box somebody picked ───────────────────────────────
// Read from the settings screen, which knows what it is drawing. Only what it
// is called and what goes in it ever arrives here; never what is in it
let pickedNow = null;
async function readPicked() {
  let p = null;
  try { p = await get("/picked"); } catch (e) { return; }
  const had = pickedNow && pickedNow.label;
  pickedNow = p && p.label ? p : null;
  const bar = document.getElementById("pick");
  bar.classList.toggle("on", !!pickedNow);
  if (pickedNow) {
    document.getElementById("pickname").textContent =
      (T["guide.pick.at"] || "{label}").replace("{label}", pickedNow.label);
  }
  if (pickedNow && pickedNow.label !== had) document.getElementById("q").focus();
}
// Saying it is here, on the same beat. Before the settings screen lets a box
// be picked it asks the app whether the ? is up, and only the window can
// answer for a panel the window placed -- a frame the board stood over the
// settings on a phone has nobody to say it but itself
const beat = () => { if (INSIDE) post("/here"); readPicked(); };
setInterval(beat, 1200);
beat();

// ── Being moved ───────────────────────────────────────────
// The app holds the rectangle, so the page says how far it was dragged and the
// app puts it there. The panel follows the pointer exactly, so the pointer
// stays on the head it is holding
const head = document.getElementById("head");
if (INSIDE) {
  head.classList.add("still");
} else {
  let from = null;
  head.addEventListener("pointerdown", e => {
    if (e.target.closest("button")) return;
    from = {x: e.screenX, y: e.screenY};
    head.setPointerCapture(e.pointerId);
    head.classList.add("holding");
  });
  head.addEventListener("pointermove", e => {
    if (!from) return;
    const by = {x: e.screenX - from.x, y: e.screenY - from.y};
    if (!by.x && !by.y) return;
    from = {x: e.screenX, y: e.screenY};
    post("/move", by);
  });
  const done = e => {
    if (!from) return;
    from = null;
    head.classList.remove("holding");
    try { head.releasePointerCapture(e.pointerId); } catch (err) {}
  };
  head.addEventListener("pointerup", done);
  head.addEventListener("pointercancel", done);
}

document.getElementById("send").onclick = ask;
document.getElementById("shut").onclick = () => {
  if (INSIDE) window.parent.postMessage({guide: "shut"}, location.origin);
  else post("/shut");
};
document.getElementById("unpick").onclick = () => { post("/picked", {}); readPicked(); };
document.getElementById("q").addEventListener("keydown", e => {
  if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); ask(); }
});
document.getElementById("q").addEventListener("input", e => {
  e.target.style.height = "34px";
  e.target.style.height = Math.min(96, e.target.scrollHeight) + "px";
});
draw();
document.getElementById("q").focus();
</script></body></html>
"##;

// ── What the two screens say to each other ───────────────────────
//
// The panel and the settings are two pages, and the field somebody picked is
// on one of them while the answer that fills it comes back to the other. What
// passes between them is held here, in the one process both are served by --
// the same honesty as `keys::IN_FORCE`: there is exactly one ? per process, so
// a place per process is what it is.
//
// Never a value. What crosses is what a box is called and what goes in it, and
// in the other direction what to put there. A box holding a secret is not
// picked at all, which the settings page decides before anything is sent.

static PICKED: std::sync::Mutex<Option<Picked>> = std::sync::Mutex::new(None);
static TO_FILL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static WANTS: std::sync::Mutex<Wants> = std::sync::Mutex::new(Wants::none());
static UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PHONE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// How long a panel on a phone is taken to be there after it last said so.
/// Longer than its beat, short enough that a page that has gone is not
/// believed for long
const PHONE_GONE_AFTER: std::time::Duration = std::time::Duration::from_secs(4);

/// Whether the panel is on screen, on either surface.
///
/// The settings screen asks, because what it does when a box is pressed
/// depends on it. In the window only the window knows, the panel being a page
/// the window places rather than something the page it is over can see; on a
/// phone it is a frame the board stands over the settings, and there the page
/// itself is the only thing that knows, so it says so on a beat.
pub fn is_up() -> bool {
    UP.load(std::sync::atomic::Ordering::Relaxed) || phone_up()
}

fn phone_up() -> bool {
    PHONE.lock().ok().and_then(|p| *p).is_some_and(|at| at.elapsed() < PHONE_GONE_AFTER)
}

/// Said by the window, which is the only thing that puts it up or takes it
/// away. Taking it away lets go of the box it was writing in
pub fn set_up(up: bool) {
    UP.store(up, std::sync::atomic::Ordering::Relaxed);
    if !up {
        pick(None);
    }
}

/// Said by the panel framed on a phone: still here, or gone. Going lets go of
/// the box it was writing in, the same as the window taking its panel away
pub fn phone_here(up: bool) {
    if let Ok(mut p) = PHONE.lock() {
        *p = up.then(std::time::Instant::now);
    }
    if !up {
        pick(None);
    }
}

/// What the panel has asked the app for, waiting to be acted on.
///
/// The panel is a page on the settings' own server, and the app is the loop
/// that draws the window; they are the same process but not the same thread,
/// so what one asks the other sits here until the loop comes round. The same
/// shape as [`crate::mailbox`], for the same reason, and drained the same way.
#[derive(Default, Debug, Clone)]
pub struct Wants {
    /// A settings screen to open, by handle
    pub open: Option<String>,
    /// How far the panel was dragged since last time
    pub moved: (i32, i32),
    /// The ✕ was pressed
    pub shut: bool,
}

impl Wants {
    const fn none() -> Self {
        Wants { open: None, moved: (0, 0), shut: false }
    }
    pub fn anything(&self) -> bool {
        self.open.is_some() || self.moved != (0, 0) || self.shut
    }
}

/// Everything the panel has asked for, leaving the box empty.
pub fn take_wants() -> Wants {
    WANTS.lock().map(|mut w| std::mem::replace(&mut *w, Wants::none())).unwrap_or_default()
}

/// Open a settings screen, if this program has one by that name.
pub fn want_open(handle: &str) -> bool {
    let known = screen_of(handle).is_some();
    if known && let Ok(mut w) = WANTS.lock() {
        w.open = Some(handle.to_string());
    }
    known
}

/// Move the panel. Adds up, because a drag arrives as many small steps and the
/// loop comes round once for a handful of them
pub fn want_move(by: (i32, i32)) {
    if let Ok(mut w) = WANTS.lock() {
        w.moved = (w.moved.0 + by.0, w.moved.1 + by.1);
    }
}

/// Put the panel away.
pub fn want_shut() {
    if let Ok(mut w) = WANTS.lock() {
        w.shut = true;
    }
}

/// The box the person has picked, if any.
pub fn picked() -> Option<Picked> {
    PICKED.lock().ok().and_then(|p| p.clone())
}

/// Pick one, or, with `None`, let the last one go.
pub fn pick(what: Option<Picked>) {
    if let Ok(mut p) = PICKED.lock() {
        *p = what;
    }
    // What was waiting to be written belonged to the box that was picked then
    if let Ok(mut f) = TO_FILL.lock() {
        *f = None;
    }
}

/// Leave a value for the settings page to put in the picked box.
pub fn leave_to_fill(text: &str) {
    if let Ok(mut f) = TO_FILL.lock() {
        *f = Some(text.to_string());
    }
}

/// Take it, if there is one. Taking it empties it: a value is written once,
/// and a page that asks again has nothing more to write
pub fn take_to_fill() -> Option<String> {
    TO_FILL.lock().ok().and_then(|mut f| f.take())
}

// ── Answering ────────────────────────────────────────────────────

/// What the ? came back with.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Answer {
    /// The reply, in the language the question was asked in
    #[serde(default)]
    pub say: String,
    /// A settings screen worth opening, by the handle the board opens one
    /// with. Empty when the answer does not lead to one
    #[serde(default)]
    pub open: String,
    /// What to put in the field the person picked, if they picked one
    #[serde(default)]
    pub fill: String,
}

/// One turn of a conversation with the ?.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Said {
    pub asked: String,
    pub said: String,
}

/// The field the person picked on the settings screen, as the screen itself
/// reads it: never its value, only what it is called and what goes in it.
///
/// Read off the page being drawn rather than out of the index, because the DOM
/// in front of somebody is the thing they are looking at and cannot disagree
/// with itself.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Picked {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub hint: String,
    /// text / number / tick / choice, and for a choice what it offers
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub options: Vec<String>,
    /// Which screen it is on, so the answer is about the right one
    #[serde(default)]
    pub screen: String,
}

/// How long the ? waits. A question somebody typed is worth more patience than
/// a name for a folder, and less than a picture: measured at one to seven
/// seconds for a short answer, so a minute is a network or a first start
const ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// How many screens go into the question in full. The map of every screen goes
/// in whatever happens, so this is how much detail rides along -- enough for
/// the answer to quote the right box, small enough that the cheapest model is
/// not reading a hundred screens to answer "how do I use my phone"
const IN_FULL: usize = 4;

/// The shape the answer has to come back in.
pub fn answer_shape() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "say": {"type": "string"},
            "open": {"type": "string"},
            "fill": {"type": "string"},
        },
        "required": ["say"],
        "additionalProperties": false,
    })
}

/// Ask the assistant AI about using this program, and read its answer.
///
/// Everything it is told comes from [`index`], so an answer about the settings
/// is an answer about the settings as they are. It is given no tools and
/// nothing to reach: what it has is the question and the index.
pub fn ask(question: &str, so_far: &[Said], picked: Option<&Picked>) -> anyhow::Result<Answer> {
    let idx = index(&crate::i18n::t);
    let prompt = question_for(&idx, question, so_far, picked);
    let said = crate::webui::ask_local_ai_shaped(
        &prompt,
        &system_for(&idx, picked.is_some()),
        &answer_shape().to_string(),
        None,
        ASK_TIMEOUT,
    )?;
    Ok(read_answer(&said, &idx))
}

/// What the AI is told it is doing.
///
/// Written for the job, not for a question: the rules here are the ones that
/// hold for every question the ? will ever be asked. Anything that only
/// answers one of them belongs in the index, where it can be read.
fn system_for(idx: &Index, picking: bool) -> String {
    let mut out = String::new();
    out.push_str(&crate::asking::HELP_WHO);
    out.push('\n');
    out.push_str(&crate::asking::HELP_HOW);
    out.push('\n');
    out.push_str(&crate::i18n::fill(
        crate::asking::HELP_OPEN,
        &[("ids", &idx.pages.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", "))],
    ));
    if picking {
        out.push('\n');
        out.push_str(&crate::asking::HELP_FILL);
    }
    // The answer is read by whoever asked, so which language to write it
    // in is said rather than guessed at from the question. Last, where an
    // instruction is hardest to lose sight of
    out.push('\n');
    out.push_str(&crate::asking::answer_in(&crate::i18n::language_name()));
    out
}

/// The question as the AI is handed it: what was asked, what was said before,
/// what the person is pointing at, and the part of the index worth reading.
fn question_for(idx: &Index, question: &str, so_far: &[Said], picked: Option<&Picked>) -> String {
    let mut out = String::new();
    for turn in so_far.iter().rev().take(6).rev() {
        out.push_str(&format!("Q: {}\nA: {}\n\n", turn.asked.trim(), turn.said.trim()));
    }
    out.push_str(&format!("Q: {}\n\n", question.trim()));
    if let Some(p) = picked {
        out.push_str(&crate::i18n::fill(
            crate::asking::HELP_PICKED,
            &[("label", &p.label), ("hint", &p.hint), ("kind", &kind_of(p))],
        ));
        out.push_str("\n\n");
    }
    out.push_str("--- \n\n");
    // The map of every screen, so an answer can send somebody anywhere.
    //
    // The handle stands after the name and behind a word that says what it is.
    // Written in front of it -- "[basic] Settings > Basic" -- it was copied
    // into the reply as though it were part of the name, and a person read
    // "turn off X in [basic] Settings > Basic"
    for page in &idx.pages {
        out.push_str(&format!("{} — {} (handle: {})\n", page.at, page.about, page.id));
    }
    out.push('\n');
    // ...and the few screens this question is actually about, in full
    for page in best(idx, &together(question, so_far, picked), IN_FULL) {
        out.push_str(&format!("## {} (handle: {})\n", page.at, page.id));
        for card in &page.cards {
            if !card.title.is_empty() {
                out.push_str(&format!("### {}\n", card.title));
            }
            for it in &card.items {
                out.push_str(&format!("- {}: {}\n", it.label, it.hint));
            }
        }
        for line in &page.says {
            out.push_str(&format!("> {line}\n"));
        }
        out.push('\n');
    }
    out
}

/// A picked field described in words, for the AI that has to fill it.
fn kind_of(p: &Picked) -> String {
    match p.options.is_empty() {
        true => p.kind.clone(),
        false => format!("{} ({})", p.kind, p.options.join(" / ")),
    }
}

/// Everything the person has said in this conversation, which is what the
/// search runs over: "and the port?" on its own finds nothing.
fn together(question: &str, so_far: &[Said], picked: Option<&Picked>) -> String {
    let mut out = question.to_string();
    for turn in so_far.iter().rev().take(2) {
        out.push(' ');
        out.push_str(&turn.asked);
    }
    if let Some(p) = picked {
        out.push(' ');
        out.push_str(&p.label);
        out.push(' ');
        out.push_str(&p.hint);
    }
    out
}

/// The screens a piece of text is most about.
///
/// Two things are looked for, and the first is worth far more than the second:
///
/// 1. **A name from the screen, written in the question.** Somebody asking
///    about a port has written the word "port", which is what that box is
///    called. Longer names count for more, so "Automatic names" beats "Name".
/// 2. **Pairs of neighbouring characters in common**, as a tie-break. Pairs
///    rather than words, because a question can be in any language this
///    program is translated into and only some of them put spaces between
///    words, and a pair is enough to tell "ポート" from "ポータル".
///
/// The second on its own is not enough: "ポート番号を変えたい" shares 変・え・
/// た・い with every screen carrying a sentence of Japanese, and the answer
/// went to whichever screen had the most writing on it.
fn best<'a>(idx: &'a Index, text: &str, how_many: usize) -> Vec<&'a Page> {
    let asked = text.to_lowercase();
    let wanted = pairs(text);
    if wanted.is_empty() {
        return Vec::new();
    }
    let said: Vec<(HashSet<(char, char)>, &Page)> =
        idx.pages.iter().map(|p| (pairs(&flat(p)), p)).collect();
    let mut scored: Vec<(usize, &Page)> = said
        .iter()
        .map(|(mine, p)| {
            let named: usize = names_on(p)
                .iter()
                .filter(|n| n.chars().count() > 1 && asked.contains(n.as_str()))
                .map(|n| 100 * n.chars().count())
                .sum();
            let shared = wanted.iter().filter(|w| mine.contains(*w)).count();
            (named + shared, *p)
        })
        .filter(|(n, _)| *n > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
    scored.into_iter().take(how_many).map(|(_, p)| p).collect()
}

/// Everything on one screen that has a name, in lower case.
fn names_on(p: &Page) -> Vec<String> {
    let mut out = vec![p.title.to_lowercase()];
    for card in &p.cards {
        out.push(card.title.to_lowercase());
        out.extend(card.items.iter().map(|it| it.label.to_lowercase()));
    }
    out.retain(|n| !n.is_empty());
    out
}

/// Everything one screen says, as one string to search.
fn flat(p: &Page) -> String {
    let mut out = format!("{} {} ", p.title, p.about);
    for card in &p.cards {
        out.push_str(&card.title);
        out.push(' ');
        for it in &card.items {
            out.push_str(&it.label);
            out.push(' ');
            out.push_str(&it.hint);
            out.push(' ');
        }
    }
    for line in &p.says {
        out.push_str(line);
        out.push(' ');
    }
    out
}

/// The pairs of neighbouring characters in a piece of text, with the spaces
/// and the punctuation taken out.
fn pairs(text: &str) -> HashSet<(char, char)> {
    let letters: Vec<char> = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    letters.windows(2).map(|w| (w[0], w[1])).collect()
}

/// The answer, out of whatever the AI printed.
///
/// It is asked for a shape and two of the three can be held to one, so the
/// usual case is a line of JSON. The third is asked in words and answers in
/// words, and any of them can wrap it in a fence or say something first -- so
/// the JSON is looked for, and what is left over is read as the answer itself.
/// A screen it names that does not exist is dropped rather than offered.
fn read_answer(said: &str, idx: &Index) -> Answer {
    let mut out = match shaped(said) {
        Some(a) => a,
        None => Answer { say: said.trim().to_string(), ..Default::default() },
    };
    if !idx.pages.iter().any(|p| p.id == out.open) {
        out.open.clear();
    }
    out
}

/// The JSON in what was printed, if any of it is the shape asked for.
fn shaped(said: &str) -> Option<Answer> {
    let text = said.trim();
    if let Ok(a) = serde_json::from_str::<Answer>(text)
        && !a.say.trim().is_empty()
    {
        return Some(a);
    }
    // A fence, a line of chatter before it, or one line of events per line
    let mut from = 0;
    while let Some(at) = text[from..].find('{') {
        let start = from + at;
        from = start + 1;
        for end in (start..text.len()).rev() {
            if !text.is_char_boundary(end + 1) || text.as_bytes()[end] != b'}' {
                continue;
            }
            if let Ok(a) = serde_json::from_str::<Answer>(&text[start..=end])
                && !a.say.trim().is_empty()
            {
                return Some(a);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every screen the settings offer is found, and none of them comes out
    /// with nothing on it. A card renamed or emptied shows up here rather than
    /// as an answer that sends somebody to a screen that says nothing.
    #[test]
    fn every_screen_is_found_and_has_something_on_it() {
        let all = screens();
        assert!(all.len() > 25, "only {} screens were found", all.len());
        assert!(all.iter().any(|s| s.id == "remote" && s.scope == Scope::Program));
        assert!(all.iter().any(|s| s.id == "permissions" && s.scope == Scope::Desk));
        // The project's page, with what git does on it: the words a person
        // asking "where do I change the commit message" has to be sent to
        let project = all.iter().find(|s| s.id == "project" && s.scope == Scope::Project).expect("the project's page is a screen");
        assert!(project.words.iter().any(|w| w == "settings.git.hint.about"), "the project's page does not show the commit prompt: {:?}", project.words);
        assert!(project.words.iter().any(|w| w == "settings.protect.hint"), "the project's page does not show the protected branches: {:?}", project.words);
        assert!(project.cards.contains("settings.project.protect.title"), "the protected branches are not a card of their own");
        for s in all {
            assert!(
                !s.words.is_empty(),
                "the {:?} screen \"{}\" shows no words at all -- what draws it has been renamed or emptied",
                s.scope,
                s.id
            );
        }
    }

    /// The board sends people to these by name, so the two lists are the same
    /// list. The settings' own test checks the other direction.
    #[test]
    fn the_board_only_names_screens_that_are_here() {
        let board = crate::shell::page();
        let known: HashSet<&str> = screens().iter().map(|s| s.id.as_str()).collect();
        let mut asked = 0;
        for pat in ["openSettings(\"", "settings: \"", "edit:\""] {
            let mut at = 0;
            while let Some(i) = board[at..].find(pat) {
                let from = at + i + pat.len();
                let Some(len) = board[from..].find('"') else { break };
                let id = &board[from..from + len];
                at = from + len;
                if id.is_empty() {
                    continue;
                }
                asked += 1;
                // One card of the project's page ("project-gitacct", the
                // prompts) is the project screen, which the settings' own
                // test checks the card is on; a browser tab's models
                // ("words", and "words-slow" opened to say why it is slow)
                // are reached by the tab's key and are no screen
                if id.starts_with("project-") || id == "words" || id == "words-slow" {
                    continue;
                }
                assert!(
                    known.contains(id) || crate::webui::desk_link(id).is_some(),
                    "the board sends people to \"{id}\", which is no screen the guide knows"
                );
            }
        }
        assert!(asked > 5, "the board's links are no longer being found");
    }

    /// Following the calls has to stop somewhere useful: if the words spread
    /// to every screen, an answer can no longer say where anything is.
    #[test]
    fn a_word_belongs_to_few_screens() {
        let mut where_: HashMap<&str, usize> = HashMap::new();
        for s in screens() {
            for w in &s.words {
                *where_.entry(w.as_str()).or_default() += 1;
            }
        }
        let spread = where_.values().filter(|n| **n > 2).count();
        assert!(
            spread < where_.len() / 10,
            "{spread} of {} words are on more than two screens -- the walk is bleeding through a shared part",
            where_.len()
        );
    }

    /// Enough of the settings' words find a screen to be worth having.
    #[test]
    fn most_words_find_their_screen() {
        let placed: HashSet<&str> =
            screens().iter().flat_map(|s| s.words.iter().map(String::as_str)).collect();
        let all = all_settings_words();
        let hit = all.iter().filter(|k| placed.contains(k.as_str())).count();
        assert!(
            hit * 2 > all.len(),
            "only {hit} of {} settings words found a screen",
            all.len()
        );
    }

    /// Framed on a phone, every button on the panel is answered by something
    /// the phone can see. Both of these used to be asked of the app instead:
    /// the manual opened a browser on the PC and the walk to a settings screen
    /// was left for the loop that draws the window -- so from the phone, the
    /// two buttons simply did nothing at all.
    #[test]
    fn a_panel_in_a_frame_does_not_ask_the_pc_to_open_things() {
        let page = super::page();
        assert!(page.contains("const INSIDE"), "the panel cannot tell whether it is framed");
        assert!(
            page.contains("T[\"tui.help.url\"]"),
            "the manual is only opened by the app, which on a phone opens it where nobody is"
        );
        assert!(
            page.contains("window.parent.postMessage({guide: \"open\", screen: turn.open}"),
            "the walk to a settings screen is only left for the window"
        );
        assert!(
            page.contains("if (INSIDE) post(\"/here\")"),
            "a framed panel never says it is there, so the settings beside it stay unpickable"
        );
    }

    /// The settings screen asks whether the ? is up before it lets a box be
    /// picked, and a panel framed on a phone is the only thing that can say so
    /// for itself. Going lets the picked box go, as the window's does.
    #[test]
    fn a_phone_saying_it_is_there_counts_as_up() {
        assert!(!is_up(), "something else left the ? up");
        phone_here(true);
        assert!(is_up(), "the phone said it was there and the settings were told otherwise");
        pick(Some(Picked { label: "Port".into(), ..Default::default() }));
        phone_here(false);
        assert!(!is_up());
        assert!(picked().is_none(), "the ? has gone and a box is still picked for it");
    }

    /// The index resolves into text, and says where each screen is.
    #[test]
    fn the_index_reads_as_words() {
        let idx = index(&crate::i18n::t);
        let remote = idx.pages.iter().find(|p| p.id == "remote").expect("the phone's screen");
        assert!(!remote.title.is_empty() && !remote.title.starts_with("settings."));
        assert!(remote.at.contains(&remote.title), "\"{}\" does not say where it is", remote.at);
        assert!(
            remote.cards.iter().any(|c| !c.items.is_empty()),
            "the phone's screen lists nothing to set"
        );
        assert!(!idx.keys.is_empty(), "no key combinations are listed");
    }
}


#[cfg(test)]
mod answer_tests {
    use super::*;

    fn en() -> impl Fn(&str) -> String {
        let dict = crate::i18n::dictionary("en", &crate::repo_root().join("lang"));
        move |k: &str| dict.get(k).cloned().unwrap_or_else(|| k.to_string())
    }

    /// A question finds the screen it is about.
    #[test]
    fn a_question_finds_its_screen() {
        let idx = index(&en());
        for (question, want) in [
            ("How do I use this from my phone?", "remote"),
            ("I want to change the prefix key", "keys"),
            ("where do I put my github token", "desk:secrets"),
            ("stop it asking before it deletes a worktree", "basic"),
        ] {
            let found: Vec<&str> =
                best(&idx, question, IN_FULL).into_iter().map(|p| p.id.as_str()).collect();
            assert!(
                found.contains(&want),
                "\"{question}\" looked at {found:?}, and not at \"{want}\""
            );
        }
    }

    /// ...and a question in Japanese finds it too, which is the whole reason
    /// the search runs on pairs of characters rather than on words.
    #[test]
    fn a_question_with_no_spaces_in_it_finds_its_screen() {
        let dict = crate::i18n::dictionary("ja", &crate::repo_root().join("lang"));
        let word = |k: &str| dict.get(k).cloned().unwrap_or_else(|| k.to_string());
        let idx = index(&word);
        for (question, want) in [
            ("スマホから使いたい", "remote"),
            ("ポート番号を変えたい", "remote"),
            ("秘密情報はどこに置きますか", "desk:secrets"),
        ] {
            let found: Vec<&str> =
                best(&idx, question, IN_FULL).into_iter().map(|p| p.id.as_str()).collect();
            assert!(
                found.contains(&want),
                "\"{question}\" looked at {found:?}, and not at \"{want}\""
            );
        }
    }

    /// Where the phone's screen says it is, however that is worded
    fn remote_at(idx: &Index) -> String {
        idx.pages.iter().find(|p| p.id == "remote").expect("the phone's screen").at.clone()
    }

    /// What is handed over carries the map of every screen, the screens the
    /// question is about, and nothing the person did not ask about.
    #[test]
    fn the_question_carries_the_map_and_the_detail() {
        let idx = index(&en());
        let asked = question_for(&idx, "How do I use this from my phone?", &[], None);
        for page in &idx.pages {
            assert!(
            asked.contains(&format!("(handle: {})", page.id)),
            "{} is not on the map",
            page.id
        );
        }
        assert!(
            asked.contains(&format!("## {} (handle: remote)", remote_at(&idx))),
            "the phone's screen is not written out in full"
        );
        assert_eq!(
            asked.matches("\n## ").count(),
            IN_FULL,
            "a different number of screens went in full than were asked for"
        );
    }

    /// An answer is read whether it came back as the shape asked for, wrapped
    /// in a fence, or with the AI saying something first.
    #[test]
    fn an_answer_is_read_however_it_arrives() {
        let idx = index(&en());
        let plain = r#"{"say":"Turn it on under the phone screen.","open":"remote"}"#;
        assert_eq!(read_answer(plain, &idx).open, "remote");
        let fenced = format!("Here you go:\n```json\n{plain}\n```\n");
        assert_eq!(read_answer(&fenced, &idx).open, "remote");
        assert_eq!(read_answer(&fenced, &idx).say, "Turn it on under the phone screen.");
        // Nothing shaped in it at all: what it said is the answer
        let words = "Turn it on under Settings > Phone connection.";
        assert_eq!(read_answer(words, &idx).say, words);
        assert!(read_answer(words, &idx).open.is_empty());
    }

    /// A screen it names that this program does not have is not offered.
    ///
    /// The button under an answer opens what the answer names, and an AI that
    /// invents a name would leave a button that goes nowhere.
    #[test]
    fn a_screen_that_does_not_exist_is_not_offered() {
        let idx = index(&en());
        let made_up = r#"{"say":"Look there.","open":"phone-settings-page"}"#;
        assert!(read_answer(made_up, &idx).open.is_empty(), "a made-up screen was offered");
        assert_eq!(read_answer(made_up, &idx).say, "Look there.");
    }

    /// What the AI is told holds for any question, and names every screen it
    /// is allowed to send somebody to.
    #[test]
    fn the_instructions_name_every_screen() {
        let idx = index(&en());
        let system = system_for(&idx, false);
        for page in &idx.pages {
            assert!(system.contains(&page.id), "{} is not one it may name", page.id);
        }
        assert!(!system.contains("fill"), "it is told about filling a box nobody picked");
        assert!(system_for(&idx, true).len() > system.len(), "picking a box tells it nothing");
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;

    /// Which languages the reference is written in. English is embedded;
    /// the others are the files beside it, so a translation that arrives is
    /// carried along without this list being touched.
    fn languages(root: &std::path::Path) -> Vec<String> {
        let mut out = vec!["en".to_string()];
        let Ok(rd) = std::fs::read_dir(root.join("lang")) else { return out };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json")
                && let Some(code) = p.file_stem().map(|s| s.to_string_lossy().to_string())
                && code != "en"
            {
                out.push(code);
            }
        }
        out.sort();
        out
    }

    /// The reference in the tree is the reference this program would write.
    ///
    /// It has to be a file in the repository, because the site's build reads
    /// `docs/` and nothing else -- and a file is a copy, and a copy falls
    /// behind. This is what stops it: change a word on the settings screen and
    /// this fails until the reference is written again.
    ///
    ///     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core reference
    #[test]
    fn the_reference_says_what_the_settings_say() {
        let root = crate::repo_root();
        let writing = std::env::var("SHIKISHA_WRITE_DOCS").is_ok();
        for code in languages(&root) {
            let dict = crate::i18n::dictionary(&code, &root.join("lang"));
            let word = |k: &str| dict.get(k).cloned().unwrap_or_else(|| k.to_string());
            let text = reference(&word);
            let path = reference_path(&root, &code);
            if writing {
                std::fs::write(&path, &text).expect("the reference could be written");
                continue;
            }
            let have = std::fs::read_to_string(&path).unwrap_or_default();
            assert_eq!(
                have.replace("\r\n", "\n"),
                text,
                "{} is not what the settings now say. Write it again:\n    \
                 SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core reference",
                path.display()
            );
        }
    }

    /// The manual's own lists are the lists this program ships with.
    ///
    /// The prose around them is a person's, and stays a person's. These are
    /// the parts that were going stale on their own.
    ///
    ///     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core reference
    #[test]
    fn the_manual_lists_what_ships() {
        let root = crate::repo_root();
        let writing = std::env::var("SHIKISHA_WRITE_DOCS").is_ok();
        for code in languages(&root) {
            let dict = crate::i18n::dictionary(&code, &root.join("lang"));
            let word = |k: &str| dict.get(k).cloned().unwrap_or_else(|| k.to_string());
            let path = manual_path(&root, &code);
            let Ok(have) = std::fs::read_to_string(&path) else { continue };
            let have = have.replace("\r\n", "\n");
            assert!(have.contains(OPENS), "{} fills nothing in for itself", path.display());
            let want = fill_manual(&have, &word);
            if writing {
                std::fs::write(&path, &want).expect("the manual could be written");
                continue;
            }
            assert_eq!(
                have,
                want,
                "{} no longer lists what ships. Write it again:\n    \
                 SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core reference",
                path.display()
            );
        }
    }

    /// Everything the manual sends somebody to by name is still called that.
    ///
    /// A sentence saying "Settings > Basic > Deleting a worktree" is a
    /// promise about a screen, and a screen can be renamed by someone who
    /// never opens the manual. This is the only part of the prose a machine
    /// can hold to the program, which is why the manual writes a path this one
    /// way and no other.
    #[test]
    fn the_manual_sends_people_where_things_are() {
        let root = crate::repo_root();
        for code in languages(&root) {
            let dict = crate::i18n::dictionary(&code, &root.join("lang"));
            let word = |k: &str| dict.get(k).cloned().unwrap_or_else(|| k.to_string());
            let Ok(text) = std::fs::read_to_string(manual_path(&root, &code)) else { continue };
            let offered = names_offered(&word);
            assert!(
                paths_counted(&text, &word) > 3,
                "{code}: the manual names no settings at all"
            );
            let wrong = paths_wrong(&text, &word, &offered);
            assert!(
                wrong.is_empty(),
                "{code}: the manual sends people to {wrong:?}, which the settings no longer call that"
            );
        }
    }

    /// A screen's entries read as entries: a name somebody would recognise,
    /// and a line saying what it does.
    #[test]
    fn an_entry_carries_both_halves() {
        let dict = crate::i18n::dictionary("en", &crate::repo_root().join("lang"));
        let word = |k: &str| dict.get(k).cloned().unwrap_or_else(|| k.to_string());
        let idx = index(&word);
        let entries: Vec<&Item> =
            idx.pages.iter().flat_map(|p| p.cards.iter().flat_map(|c| c.items.iter())).collect();
        // Well under what the page holds (107 as this was written): a floor
        // to catch the reading collapsing, not a count to keep up to date
        assert!(entries.len() > 80, "only {} entries were found", entries.len());
        for it in entries {
            assert!(!it.label.is_empty(), "{it:?} has no name");
            assert!(!it.label.starts_with("settings."), "{} was never translated", it.label);
        }
    }
}
