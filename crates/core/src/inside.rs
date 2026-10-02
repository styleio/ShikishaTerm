//! Rules for places inside what a new worktree inherits.
//!
//! A line of an ignore file decides how everything it matches comes along --
//! but git hands back a wholly ignored folder as one thing, so a line can only
//! ever speak for the folder as a whole. `.claude/` is copied with whatever is
//! in it, `.claude/worktrees/` and every other agent's build folder included.
//!
//! A rule here names a place at any depth, written the way a line of a
//! `.gitignore` is, and says how that place comes along: the same four
//! answers a line has. The rules are the project's own and live in its
//! settings beside the lines' rules; nothing here reads or writes an ignore
//! file. Which rule decides a place:
//!
//! 1. the one naming the deepest place -- a rule for a folder covers what is
//!    in it until a rule for something deeper says otherwise;
//! 2. at the same place, a rule here beats the rule of the ignore line, since
//!    somebody named that place on purpose;
//! 3. between two rules here naming the same place, the later one, as in an
//!    ignore file.
//!
//! What is linked is the original's own folder, seen through a second name:
//! nothing inside it can be decided differently, and the dialog says so
//! rather than leaving a rule there that does nothing.
//!
//! [`walk`] is the one account of what a carry does inside a folder. The copy,
//! its count of files and the dialog's sizes all go through it, so what is
//! said before the button is pressed is what the button does.

use std::path::Path;

/// A rule for a place inside that the app itself brings, for every project.
///
/// These are the places AI tools keep for themselves inside a checkout --
/// their helpers' own worktrees, their undo history, their messages -- which
/// sit inside a folder somebody may well copy whole (`.claude/`), and which
/// are never the project's: a copied helper worktree is a whole second
/// checkout with its build folders, and on 2026-10-01 copies of them filled a
/// drive. The list is the app's, not the person's file's: a project's
/// settings keep only what the person changed about one of these (another
/// answer, or "without it"), so a rule added to this list in a later version
/// reaches every project, and one taken off it leaves every project that never
/// touched it. See [`effective`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Shipped {
    /// Never changes once shipped: what a project's change is filed under
    pub id: &'static str,
    /// Written the way a rule here is
    pub path: &'static str,
    pub how: &'static str,
    /// Which edition of this list it first appeared in. A project's page says
    /// "added in this version" of the ones newer than what it has shown it
    pub since: u32,
}

/// The app's own rules for places inside. Add at the end, with the next
/// `since`; never reuse an id. Taking one out is safe: a project that changed
/// it keeps the change as its own rule
pub const SHIPPED: &[Shipped] = &[
    // Claude Code's helper agents each work in a worktree of their own here
    Shipped { id: "claude-helper-worktrees", path: "**/.claude/worktrees/", how: "skip", since: 1 },
    // Its record of edits, for going back to an earlier point
    Shipped { id: "claude-checkpoints", path: "**/.claude/checkpoints/", how: "skip", since: 1 },
    // What its agents leave each other
    Shipped { id: "claude-mailbox", path: "**/.claude/mailbox/", how: "skip", since: 1 },
    // Its record of this PC's state at this place: routines it ran, agents it
    // started, its background process, its scheduled tasks. Small, but about
    // this checkout on this PC -- in another worktree they would describe
    // something that is not there
    Shipped { id: "claude-routine-state", path: "**/.claude/routines/.state/", how: "skip", since: 2 },
    Shipped { id: "claude-agent-registry", path: "**/.claude/agent-registry.json", how: "skip", since: 2 },
    Shipped { id: "claude-assistant-state", path: "**/.claude/assistant-daemon-state.json", how: "skip", since: 2 },
    Shipped { id: "claude-schedule-lock", path: "**/.claude/scheduled_tasks.lock", how: "skip", since: 2 },
    Shipped { id: "claude-schedule", path: "**/.claude/scheduled_tasks.json", how: "skip", since: 2 },
];

/// The newest edition of [`SHIPPED`]: what a project has been shown once its
/// rules have been looked at
pub fn shipped_edition() -> u32 {
    SHIPPED.iter().map(|s| s.since).max().unwrap_or(0)
}

/// The app's rule written exactly this way, if it is one
pub fn shipped_written(path: &str) -> Option<&'static Shipped> {
    let path = path.trim();
    SHIPPED.iter().find(|s| s.path == path)
}

/// A project's rules for what comes along, with the app's own rules for
/// places inside put in, the way they stand today.
///
/// The app's rules come first, so a rule the person wrote for the same place
/// is the later one and decides (see the module's order). Each takes the
/// answer the person gave it, or is left out when they did without it. A
/// change filed under an id the app no longer has is what the person decided
/// about that place, and stays as their own rule; "do without" for one gone
/// says nothing any more. Everything else is the person's, in their order
pub fn effective(bring: &[crate::config::BringRule]) -> Vec<crate::config::BringRule> {
    merged(SHIPPED, bring)
}

/// [`effective`] against a given list of the app's rules: what a version
/// with that list makes of the same settings
fn merged(list: &[Shipped], bring: &[crate::config::BringRule]) -> Vec<crate::config::BringRule> {
    let change_of = |id: &str| bring.iter().filter(|r| r.shipped.as_deref() == Some(id)).last();
    let mut out: Vec<crate::config::BringRule> = list
        .iter()
        .filter_map(|s| match change_of(s.id) {
            Some(c) if c.dropped => None,
            Some(c) if crate::worktree::HOWS.contains(&c.how.as_str()) => Some(crate::config::BringRule {
                path: Some(s.path.to_string()),
                how: c.how.clone(),
                replace: c.replace.clone(),
                ..Default::default()
            }),
            _ => Some(crate::config::BringRule { path: Some(s.path.to_string()), how: s.how.to_string(), ..Default::default() }),
        })
        .collect();
    for r in bring {
        match r.shipped.as_deref() {
            None => out.push(r.clone()),
            Some(id) if list.iter().any(|s| s.id == id) => {}
            Some(_) if r.dropped || r.path.is_none() => {}
            Some(_) => out.push(crate::config::BringRule { shipped: None, ..r.clone() }),
        }
    }
    out
}

/// Which edition of [`SHIPPED`] each project has been shown, by the project's
/// uid (`config::ProjectSpec::uid`): a name is a project's only on its desk
/// and only until it is renamed, and neither is a reason to have been shown
/// the rules or not.
///
/// Kept with the app's state, not in the settings: having looked at a page is
/// not something the person chose, and writing the settings because a page was
/// opened would be a change nobody made. Lost, it costs only a label shown
/// once more
fn shown_file() -> std::path::PathBuf {
    crate::config::state_path("inside-rules-shown.json")
}

fn shown_all() -> std::collections::BTreeMap<String, u32> {
    std::fs::read_to_string(shown_file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The edition of the app's rules the project `project` (its uid) has been
/// shown; 0 for one never shown any
pub fn shown(project: &str) -> u32 {
    shown_all().get(project).copied().unwrap_or(0)
}

/// Every project's, for a page that lists several
pub fn shown_json() -> String {
    serde_json::to_string(&shown_all()).unwrap_or_else(|_| "{}".into())
}

/// The project `project` (its uid) has now been shown the app's rules as
/// they stand. A project that has none -- worked out from a checkout and not
/// written down yet -- has nothing to keep it under, and is shown them again
pub fn mark_shown(project: &str) {
    if !crate::config::is_tab_uid(project) {
        return;
    }
    let mut all = shown_all();
    let now = shipped_edition();
    if all.get(project) == Some(&now) {
        return;
    }
    all.insert(project.to_string(), now);
    if let Ok(text) = serde_json::to_string_pretty(&all) {
        let _ = crate::crypto::write_atomic(&shown_file(), &text);
    }
}

/// The app's rules newer than what the project `project` (its uid) has been
/// shown, as written
pub fn new_to(project: &str) -> Vec<String> {
    newer_than(SHIPPED, shown(project))
}

/// What came after the edition `seen`. Nothing for a project that has never
/// been shown the list: every rule it holds is simply the app's, and saying
/// "added" of rules that were there before the person ever looked reads as
/// this update having changed what a worktree is given
fn newer_than(list: &[Shipped], seen: u32) -> Vec<String> {
    if seen == 0 {
        return Vec::new();
    }
    list.iter().filter(|s| s.since > seen).map(|s| s.path.to_string()).collect()
}

/// One rule, read from the settings
#[derive(Debug, Clone)]
struct Rule {
    /// As written
    text: String,
    /// The whole path of a place, `/` between its parts, relative to the
    /// worktree; see [`compile`]
    re: regex::Regex,
    /// Written with a trailing `/`: a folder and never a file
    folders_only: bool,
    how: String,
    replace: Vec<crate::config::Replace>,
    /// What every place it can name starts with, read off the pattern up to
    /// its first wildcard; None when it can name a place at any depth
    prefix: Option<String>,
}

/// The project's rules for places inside what comes along, read once and
/// shared by everything one making carries
#[derive(Debug, Clone, Default)]
pub struct Inside {
    rules: std::sync::Arc<Vec<Rule>>,
}

impl PartialEq for Inside {
    fn eq(&self, other: &Self) -> bool {
        self.rules.len() == other.rules.len()
            && self
                .rules
                .iter()
                .zip(other.rules.iter())
                .all(|(a, b)| a.text == b.text && a.how == b.how && a.replace == b.replace)
    }
}
impl Eq for Inside {}

/// The rule that decided a place, as the dialog says it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decided<'a> {
    pub how: &'a str,
    pub replace: &'a [crate::config::Replace],
    /// The rule as written
    pub by: &'a str,
}

impl Inside {
    /// The rules among a project's that name a place, in the order written.
    /// One that cannot be read as a place (a `!` line: every rule here says
    /// how, so there is nothing to take back) is left out, and the settings
    /// page says so where it is written
    pub fn of(rules: &[crate::config::BringRule]) -> Inside {
        let read = rules
            .iter()
            .filter(|r| crate::worktree::HOWS.contains(&r.how.as_str()))
            .filter_map(|r| {
                let text = r.path.as_deref()?;
                let (re, folders_only, prefix) = compile(text)?;
                Some(Rule { text: text.trim().to_string(), re, folders_only, how: r.how.clone(), replace: r.replace.clone(), prefix })
            })
            .collect();
        Inside { rules: std::sync::Arc::new(read) }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The rules as written, one string: what a count made under them is
    /// filed under, so a count made before a rule changed is not reused
    pub fn key(&self) -> String {
        self.rules.iter().map(|r| format!("{}\u{1f}{}", r.text, r.how)).collect::<Vec<_>>().join("\u{1e}")
    }

    /// The rule for this place itself, when one names it: the last of those
    /// that do. `at` is relative to the worktree, `/` between its parts
    pub fn at(&self, at: &str, folder: bool) -> Option<Decided<'_>> {
        let at = at.trim_matches('/');
        self.rules
            .iter()
            .rev()
            .find(|r| (folder || !r.folders_only) && r.re.is_match(at))
            .map(|r| Decided { how: &r.how, replace: &r.replace, by: &r.text })
    }

    /// Whether some rule could name a place inside `folder` and have it come
    /// along: what decides whether a folder left out still has to be looked
    /// into. Answered from the pattern alone, so it may say yes for a folder
    /// where in the end nothing matches -- never no where something would
    pub fn brings_inside(&self, folder: &str) -> bool {
        self.reaches_inside(folder, |r| r.how != "skip")
    }

    /// Whether any rule could name a place inside `folder`, whatever it says:
    /// the reason a folder that is linked deserves a word
    pub fn says_inside(&self, folder: &str) -> bool {
        self.reaches_inside(folder, |_| true)
    }

    fn reaches_inside(&self, folder: &str, wanted: impl Fn(&Rule) -> bool) -> bool {
        let under = format!("{}/", folder.trim_matches('/'));
        self.rules.iter().filter(|r| wanted(r)).any(|r| match &r.prefix {
            None => true,
            Some(p) => p.starts_with(&under) || under.starts_with(p.as_str()),
        })
    }
}

/// A line written the way an ignore file's is, as a match for a place's whole
/// path. Answers the match, whether it is for folders only, and the part of a
/// path every match starts with when there is one.
///
/// The ignore file's own reading: a `/` anywhere but at the end ties the line
/// to the top of the worktree, and without one it names a place at any depth;
/// `*` and `?` stop at a `/`; `**` is any number of folders; a trailing `/` is
/// a folder. A `\` is read as a `/` -- this is written on Windows by people
/// who copy paths out of Explorer, not by people escaping a `*` -- and a line
/// starting with `!` is not a rule here
pub fn compile(line: &str) -> Option<(regex::Regex, bool, Option<String>)> {
    let line = line.trim().replace('\\', "/");
    if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
        return None;
    }
    let folders_only = line.ends_with('/');
    let body = line.trim_end_matches('/');
    let tied = body.contains('/');
    let body = body.trim_start_matches('/');
    let parts: Vec<&str> = body.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let mut re = String::from(if tied { "^" } else { "^(?:.*/)?" });
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        if *part == "**" {
            re.push_str(if last { ".*" } else { "(?:.*/)?" });
            continue;
        }
        re.push_str(&glob(part));
        if !last {
            re.push('/');
        }
    }
    re.push('$');
    let prefix = tied.then(|| {
        let cut = body.find(['*', '?', '[']).unwrap_or(body.len());
        body[..cut].to_string()
    });
    regex::Regex::new(&re).ok().map(|re| (re, folders_only, prefix))
}

/// One part of a path, its wildcards made a match that stays inside the part
fn glob(part: &str) -> String {
    let mut out = String::new();
    let mut chars = part.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '[' => {
                // A class runs to the next `]`; one never closed is a `[`
                let rest: String = chars.clone().collect();
                match rest.find(']') {
                    Some(end) if end > 0 => {
                        let inner = &rest[..end];
                        let inner = match inner.strip_prefix('!') {
                            Some(not) => format!("^{}", not.replace('\\', "\\\\")),
                            None => inner.replace('\\', "\\\\"),
                        };
                        out.push('[');
                        out.push_str(&inner);
                        out.push(']');
                        for _ in 0..=end {
                            chars.next();
                        }
                    }
                    _ => out.push_str("\\["),
                }
            }
            c => out.push_str(&regex::escape(&c.to_string())),
        }
    }
    out
}

/// One thing a carry does, met on the way down
#[derive(Debug, PartialEq, Eq)]
pub enum Step<'a> {
    /// A folder to make, so one left empty arrives as well
    Folder,
    /// A file to copy; `replace` is what to write differently in it, when
    /// it is copied that way
    File { replace: &'a [crate::config::Replace] },
    /// A second name for the original's own folder or file
    Link { folder: bool },
    /// A place a rule leaves out of something that comes along: not copied,
    /// and walked into only for what a deeper rule brings back (said as steps
    /// of their own after this one). `by` is the rule as written. Said so that
    /// what is left out can be counted and shown, not only what comes
    Left { by: &'a str },
}

/// What a carry does with `from` -- known in the worktree as `at` -- and with
/// everything under it, given how `from` itself comes along. `visit` is told
/// each step, top down, with the place's name in the worktree and its path on
/// disk; whatever it answers with an error stops the walk with that error.
///
/// A second name met on the way is not followed: following one out of the
/// folder could copy something nobody meant to bring (it is left out, as a
/// copy always has)
pub fn walk(
    from: &Path,
    at: &str,
    how: &str,
    replace: &[crate::config::Replace],
    inside: &Inside,
    visit: &mut dyn FnMut(&str, &Path, Step) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let Ok(meta) = std::fs::symlink_metadata(from) else { return Ok(()) };
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let folder = meta.is_dir();
    match how {
        "link" => return visit(at, from, Step::Link { folder }),
        "skip" if !(folder && inside.brings_inside(at)) => return Ok(()),
        "skip" => {}
        _ if folder => visit(at, from, Step::Folder)?,
        _ => {
            let replace = if how == "replace" { replace } else { &[] };
            return visit(at, from, Step::File { replace });
        }
    }
    let Ok(entries) = std::fs::read_dir(from) else { return Ok(()) };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let there = format!("{}/{name}", at.trim_end_matches('/'));
        let is_folder = std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_dir());
        match inside.at(&there, is_folder) {
            // Left out of what comes: said, then walked only for what a
            // deeper rule may bring back out of it
            Some(d) if d.how == "skip" && how != "skip" => {
                visit(&there, &e.path(), Step::Left { by: d.by })?;
                walk(&e.path(), &there, "skip", &[], inside, visit)?
            }
            Some(d) => walk(&e.path(), &there, d.how, d.replace, inside, visit)?,
            None => walk(&e.path(), &there, how, replace, inside, visit)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(path: &str, how: &str) -> crate::config::BringRule {
        crate::config::BringRule { path: Some(path.into()), how: how.into(), ..Default::default() }
    }

    /// A line means here what it means in an ignore file
    #[test]
    fn a_rule_reads_the_way_an_ignore_line_does() {
        let names = |line: &str, at: &str, folder: bool| Inside::of(&[rule(line, "skip")]).at(at, folder).is_some();
        // No `/` but at the end: a name at any depth
        assert!(names("target/", "target", true));
        assert!(names("target/", "web/app/target", true));
        assert!(!names("target/", "target", false), "a folder rule named a file");
        assert!(names("*.log", "a/b/c.log", false));
        assert!(!names("*.log", "a/b/c.logs", false));
        // A `/` inside ties it to the top
        assert!(names(".claude/worktrees/", ".claude/worktrees", true));
        assert!(!names(".claude/worktrees/", "x/.claude/worktrees", true), "a tied rule matched below the top");
        assert!(names("/build", "build", true));
        assert!(!names("/build", "web/build", true));
        // `*` and `?` stop at a `/`; `**` does not
        assert!(!names("a/*/c", "a/b/x/c", true));
        assert!(names("a/*/c", "a/b/c", true));
        assert!(names("a/**/c", "a/c", true));
        assert!(names("a/**/c", "a/b/x/c", true));
        assert!(names("**/cache/", "deep/down/cache", true));
        assert!(names("a/**", "a/b/c", false));
        assert!(!names("a/**", "a", true), "a/** named the folder itself rather than what is in it");
        assert!(names("data[0-9].bin", "data7.bin", false));
        assert!(names("data[!0-9].bin", "datax.bin", false));
        assert!(!names("data[!0-9].bin", "data7.bin", false));
        // A Windows path is the same place
        assert!(names(".claude\\worktrees\\", ".claude/worktrees", true));
        // Dots and other letters are themselves
        assert!(!names(".env", "xenv", false));
        // Not rules here
        assert!(Inside::of(&[rule("!keep/", "copy"), rule("# note", "skip"), rule("  ", "skip")]).is_empty());
    }

    /// The later of two rules naming the same place is the one that counts
    #[test]
    fn the_later_rule_for_a_place_decides() {
        let inside = Inside::of(&[rule("*.json", "skip"), rule("keep.json", "copy")]);
        assert_eq!(inside.at("keep.json", false).map(|d| d.how), Some("copy"));
        assert_eq!(inside.at("other.json", false).map(|d| (d.how, d.by)), Some(("skip", "*.json")));
    }

    /// Whether a folder has to be looked into for something a rule brings,
    /// answered from the pattern: no for a rule tied somewhere else
    #[test]
    fn a_folder_is_looked_into_only_where_a_rule_could_reach() {
        let inside = Inside::of(&[rule("node_modules/.bin/", "copy"), rule("/tmp/", "skip")]);
        assert!(inside.brings_inside("node_modules"));
        assert!(!inside.brings_inside("vendor"));
        assert!(!inside.brings_inside("tmp"), "a rule that leaves something out made a folder worth walking");
        assert!(inside.says_inside("tmp"), "a rule that leaves something out was not said to be there");
        assert!(!inside.says_inside("vendor"));
        assert!(Inside::of(&[rule("*.env", "copy")]).brings_inside("anything"), "a rule for any depth reaches every folder");
    }

    fn tree(name: &str) -> std::path::PathBuf {
        let root = crate::test_temp(name);
        let _ = std::fs::remove_dir_all(&root);
        for (f, body) in [
            ("a/keep.txt", "1"),
            ("a/worktrees/one/target/big.bin", "22"),
            ("a/worktrees/one/src/x.rs", "333"),
            ("a/cache/c.bin", "4444"),
            ("a/cache/keep/k.txt", "5"),
        ] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        root
    }

    fn steps(root: &Path, how: &str, inside: &Inside) -> Vec<String> {
        let mut said = Vec::new();
        walk(&root.join("a"), "a", how, &[], inside, &mut |at, _, step| {
            said.push(match step {
                Step::Folder => format!("{at}/"),
                Step::File { .. } => at.to_string(),
                Step::Link { folder } => format!("{at} -> link{}", if folder { "/" } else { "" }),
                // What is left out is not part of what a carry does
                Step::Left { .. } => return Ok(()),
            });
            Ok(())
        })
        .unwrap();
        said.sort();
        said
    }

    /// The deepest rule decides: a folder left out of a copied one, a folder
    /// linked inside it, and one thing brought back out of what is left out
    #[test]
    fn the_deepest_rule_decides_what_a_carry_does() {
        let root = tree("inside-walk");
        let inside = Inside::of(&[rule("a/worktrees/", "skip"), rule("cache/", "link")]);
        assert_eq!(steps(&root, "copy", &inside), ["a/", "a/cache -> link/", "a/keep.txt"]);

        let inside = Inside::of(&[rule("a/worktrees/", "skip"), rule("a/worktrees/*/src/", "copy")]);
        assert_eq!(
            steps(&root, "copy", &inside),
            ["a/", "a/cache/", "a/cache/c.bin", "a/cache/keep/", "a/cache/keep/k.txt", "a/keep.txt", "a/worktrees/one/src/", "a/worktrees/one/src/x.rs"],
        );

        // Left out as a whole, with one place in it brought
        let inside = Inside::of(&[rule("a/cache/keep/", "copy")]);
        assert_eq!(steps(&root, "skip", &inside), ["a/cache/keep/", "a/cache/keep/k.txt"]);
        // Left out with nothing in it brought: not even looked into
        assert!(steps(&root, "skip", &Inside::default()).is_empty());
        // Linked: one second name, whatever the rules say about inside it
        assert_eq!(steps(&root, "link", &inside), ["a -> link/"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── The app's own rules, and what a project changed about them ──

    const V1: &[Shipped] = &[
        Shipped { id: "one", path: "**/.tool/helpers/", how: "skip", since: 1 },
        Shipped { id: "two", path: "**/.tool/undo/", how: "skip", since: 1 },
    ];
    // A later version: "two" taken off the list, "three" added
    const V2: &[Shipped] = &[
        Shipped { id: "one", path: "**/.tool/helpers/", how: "skip", since: 1 },
        Shipped { id: "three", path: "**/.other/cache/", how: "skip", since: 2 },
    ];

    fn change(id: &str, path: &str, how: &str) -> crate::config::BringRule {
        crate::config::BringRule { shipped: Some(id.into()), path: Some(path.into()), how: how.into(), ..Default::default() }
    }
    fn without(id: &str, path: &str) -> crate::config::BringRule {
        crate::config::BringRule { shipped: Some(id.into()), path: Some(path.into()), dropped: true, ..Default::default() }
    }
    fn said(rules: &[crate::config::BringRule]) -> Vec<(String, String)> {
        rules.iter().map(|r| (r.path.clone().unwrap_or_else(|| format!("line {}", r.pattern.clone().unwrap_or_default())), r.how.clone())).collect()
    }
    fn pair(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }

    /// Nobody touched anything: every version's list is what the project has
    #[test]
    fn a_project_that_changed_nothing_has_each_versions_list() {
        assert_eq!(said(&merged(V1, &[])), [pair("**/.tool/helpers/", "skip"), pair("**/.tool/undo/", "skip")]);
        assert_eq!(said(&merged(V2, &[])), [pair("**/.tool/helpers/", "skip"), pair("**/.other/cache/", "skip")]);
    }

    /// A rule of the app's changed by the person keeps their answer, and a
    /// rule added later still arrives beside it
    #[test]
    fn a_changed_rule_keeps_its_answer_and_new_ones_still_arrive() {
        let bring = [change("one", "**/.tool/helpers/", "copy")];
        assert_eq!(said(&merged(V1, &bring)), [pair("**/.tool/helpers/", "copy"), pair("**/.tool/undo/", "skip")]);
        assert_eq!(said(&merged(V2, &bring)), [pair("**/.tool/helpers/", "copy"), pair("**/.other/cache/", "skip")]);
    }

    /// Done without: gone while the app has it, and not brought back by a
    /// version that adds something else
    #[test]
    fn a_rule_done_without_stays_away() {
        let bring = [without("one", "**/.tool/helpers/")];
        assert_eq!(said(&merged(V1, &bring)), [pair("**/.tool/undo/", "skip")]);
        assert_eq!(said(&merged(V2, &bring)), [pair("**/.other/cache/", "skip")]);
    }

    /// A rule the app takes off its list: gone where nobody touched it, kept
    /// as the person's own where they had decided something about it, and a
    /// "without it" for it says nothing any more
    #[test]
    fn a_rule_taken_off_the_list_leaves_what_the_person_decided() {
        assert!(!said(&merged(V2, &[])).iter().any(|(p, _)| p == "**/.tool/undo/"));
        let changed = [change("two", "**/.tool/undo/", "link")];
        assert_eq!(said(&merged(V2, &changed)).last(), Some(&pair("**/.tool/undo/", "link")));
        let kept = merged(V2, &changed);
        assert!(kept.iter().all(|r| r.shipped.is_none()), "a kept change still claimed to be the app's");
        let dropped = [without("two", "**/.tool/undo/")];
        assert_eq!(said(&merged(V2, &dropped)), said(&merged(V2, &[])));
    }

    /// The person's own rules come after the app's in their own order, so one
    /// written for the same place decides; lines of ignore files pass through
    #[test]
    fn the_persons_own_rules_come_after_and_decide() {
        let bring = [
            crate::config::BringRule { pattern: Some(".claude/".into()), how: "copy".into(), ..Default::default() },
            rule("**/.tool/helpers/", "copy"),
            rule("data/", "skip"),
        ];
        let all = merged(V1, &bring);
        assert_eq!(
            said(&all),
            [
                pair("**/.tool/helpers/", "skip"),
                pair("**/.tool/undo/", "skip"),
                pair("line .claude/", "copy"),
                pair("**/.tool/helpers/", "copy"),
                pair("data/", "skip"),
            ]
        );
        assert_eq!(Inside::of(&all).at("x/.tool/helpers", true).map(|d| d.how), Some("copy"));
        assert_eq!(Inside::of(&all).at("x/.tool/undo", true).map(|d| d.how), Some("skip"));
    }

    /// A change is written the way the settings page writes it, and only the
    /// change: an untouched project's settings hold nothing of the app's list
    #[test]
    fn a_change_reads_and_writes_as_the_settings_have_it() {
        let read: Vec<crate::config::BringRule> = serde_json::from_str(
            r#"[{"default":"one","path":"**/.tool/helpers/","how":"copy"},{"default":"two","path":"**/.tool/undo/","how":"","dropped":true}]"#,
        )
        .unwrap();
        assert_eq!(said(&merged(V1, &read)), [pair("**/.tool/helpers/", "copy")]);
        let written = serde_json::to_value(&read[1]).unwrap();
        assert_eq!(written["default"], "two");
        assert_eq!(written["dropped"], true);
        let plain = serde_json::to_value(rule("x/", "skip")).unwrap();
        assert!(plain.get("default").is_none() && plain.get("dropped").is_none(), "{plain}");
    }

    /// What is new to a project is what came after the edition it was shown
    #[test]
    fn only_rules_after_what_was_shown_are_new() {
        assert!(newer_than(V2, 0).is_empty(), "a project that never looked was told of rules as added");
        assert_eq!(newer_than(V2, 1), ["**/.other/cache/"]);
        assert!(newer_than(V2, 2).is_empty());
    }

    /// The app's own list is well formed: ids are never shared, every path
    /// reads as a rule here, and every answer is one a rule can give
    #[test]
    fn the_apps_list_is_well_formed() {
        let mut ids: Vec<&str> = SHIPPED.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SHIPPED.len(), "two of the app's rules share an id");
        for s in SHIPPED {
            assert!(compile(s.path).is_some(), "{} does not read as a rule", s.path);
            assert!(crate::worktree::HOWS.contains(&s.how), "{} says {}", s.path, s.how);
            assert!(s.since >= 1);
        }
        // A helper's worktree is left out of a copied .claude at any depth
        let inside = Inside::of(&effective(&[]));
        assert_eq!(inside.at(".claude/worktrees", true).map(|d| d.how), Some("skip"));
        assert_eq!(inside.at("web/.claude/worktrees", true).map(|d| d.how), Some("skip"));
        assert!(inside.at(".claude/settings.json", false).is_none());
        // The state files are files: a rule written without a trailing /
        // names a file as well, at any depth
        for f in ["agent-registry.json", "assistant-daemon-state.json", "scheduled_tasks.lock", "scheduled_tasks.json"] {
            assert_eq!(inside.at(&format!(".claude/{f}"), false).map(|d| d.how), Some("skip"), "{f}");
            assert_eq!(inside.at(&format!("web/.claude/{f}"), false).map(|d| d.how), Some("skip"), "{f} deeper");
        }
        assert_eq!(inside.at(".claude/routines/.state", true).map(|d| d.how), Some("skip"));
        assert!(inside.at(".claude/routines/my.md", false).is_none(), "a routine the person wrote was left out");
        assert!(inside.at(".claude/agent-memory-local", true).is_none());
    }


    /// The editions as shipped: the second batch is new to a project shown
    /// only the first three, and nothing is new to one shown both
    #[test]
    fn the_second_batch_is_new_to_whoever_saw_only_the_first() {
        let first: Vec<&str> = SHIPPED.iter().filter(|s| s.since == 1).map(|s| s.path).collect();
        assert_eq!(first, ["**/.claude/worktrees/", "**/.claude/checkpoints/", "**/.claude/mailbox/"]);
        assert_eq!(
            newer_than(SHIPPED, 1),
            [
                "**/.claude/routines/.state/",
                "**/.claude/agent-registry.json",
                "**/.claude/assistant-daemon-state.json",
                "**/.claude/scheduled_tasks.lock",
                "**/.claude/scheduled_tasks.json",
            ]
        );
        assert!(newer_than(SHIPPED, 0).is_empty());
        assert!(newer_than(SHIPPED, shipped_edition()).is_empty());
        assert_eq!(shipped_edition(), 2);
    }

    /// A copy of a folder walks past what a rule leaves out without going in,
    /// and says what it left
    #[test]
    fn a_place_left_out_is_said_and_not_walked() {
        let root = tree("inside-left");
        let inside = Inside::of(&[rule("a/worktrees/", "skip")]);
        let mut left = Vec::new();
        let mut files = Vec::new();
        walk(&root.join("a"), "a", "copy", &[], &inside, &mut |at, _, step| {
            match step {
                Step::Left { by } => left.push((at.to_string(), by.to_string())),
                Step::File { .. } => files.push(at.to_string()),
                _ => {}
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(left, [("a/worktrees".to_string(), "a/worktrees/".to_string())]);
        assert!(files.iter().all(|f| !f.starts_with("a/worktrees")), "{files:?}");
        // Nothing is said of what is left out of a folder already left out
        let mut said_any = false;
        let keep = Inside::of(&[rule("a/worktrees/", "skip"), rule("a/cache/keep/", "copy")]);
        walk(&root.join("a"), "a", "skip", &[], &keep, &mut |_, _, step| {
            said_any |= matches!(step, Step::Left { .. });
            Ok(())
        })
        .unwrap();
        assert!(!said_any);

        // Left out with a part brought back out of it: said as left, and the
        // part brought still comes -- even when the rule that brings it is
        // one that could reach any folder
        let back = Inside::of(&[rule("a/worktrees/", "skip"), rule("**/src/", "copy")]);
        let mut left = Vec::new();
        let mut files = Vec::new();
        walk(&root.join("a"), "a", "copy", &[], &back, &mut |at, _, step| {
            match step {
                Step::Left { .. } => left.push(at.to_string()),
                Step::File { .. } => files.push(at.to_string()),
                _ => {}
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(left, ["a/worktrees"]);
        assert!(files.contains(&"a/worktrees/one/src/x.rs".to_string()), "{files:?}");
        assert!(!files.iter().any(|f| f.contains("target")), "{files:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
