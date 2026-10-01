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
}
