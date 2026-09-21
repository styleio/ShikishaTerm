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

/// The suffixes that belong to the entry above them rather than standing alone.
const UNDER: &[&str] = &[".hint", ".label", ".sub", ".note", ".placeholder", ".ph"];

/// The index in one language. `word` looks a key up.
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
        let mut items = Vec::new();
        for key in &s.words {
            placed.insert(key.as_str());
            // Its own name in the list is not a thing on the screen
            if *key == s.label_key || *key == s.sub_key || UNDER.iter().any(|u| key.ends_with(u)) {
                continue;
            }
            items.push(item(key, word));
        }
        let about = word(&s.sub_key);
        pages.push(Page {
            id: s.id.clone(),
            at,
            title,
            about: if about == s.sub_key { String::new() } else { about },
            items,
        });
    }
    // Everything else the settings say, with no screen to name
    let mut loose = Vec::new();
    for key in all_settings_words() {
        if placed.contains(key.as_str()) || UNDER.iter().any(|u| key.ends_with(u)) {
            continue;
        }
        loose.push(item(key, word));
    }
    let keys = crate::keys::shipped()
        .into_iter()
        .map(|(press, does)| Item { label: press, hint: does })
        .collect();
    Index { pages, loose, keys }
}

/// One entry: its own words, and the line written under it.
fn item(key: &str, word: &dyn Fn(&str) -> String) -> Item {
    let label = word(key);
    let hint = [".hint", ".note"]
        .iter()
        .map(|u| format!("{key}{u}"))
        .map(|k| (word(&k), k))
        .find(|(text, k)| text != k)
        .map(|(text, _)| text)
        .unwrap_or_default();
    Item { label: if label == key { String::new() } else { label }, hint }
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
        assert!(remote.items.iter().any(|i| !i.label.is_empty()), "the screen lists nothing");
        assert!(!idx.keys.is_empty(), "no key combinations are listed");
    }
}

