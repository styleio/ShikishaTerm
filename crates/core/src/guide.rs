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
        let at = crate::i18n::tp(
            match s.scope {
                Scope::Program => "guide.at.program",
                Scope::Desk => "guide.at.desk",
            },
            &[("name", &title)],
        );
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
            id: s.id.clone(),
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
            out.push('\n');
            line(
                &mut out,
                &format!(
                    "## {}",
                    word(match s.scope {
                        Scope::Program => "guide.doc.program",
                        Scope::Desk => "guide.doc.desk",
                    })
                ),
            );
            out.push('\n');
            line(
                &mut out,
                &word(match s.scope {
                    Scope::Program => "guide.doc.program.about",
                    Scope::Desk => "guide.doc.desk.about",
                }),
            );
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
                    out.push_str(&format!(
                        "**{}**\n\n",
                        word(match s.scope {
                            Scope::Program => "guide.doc.program",
                            Scope::Desk => "guide.doc.desk",
                        })
                    ));
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
        assert!(all.iter().any(|s| s.id == "git" && s.scope == Scope::Desk));
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
                // The project's own page is not one of the sections
                if id == "project" || id == "project-gitacct" || id.is_empty() {
                    continue;
                }
                asked += 1;
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
