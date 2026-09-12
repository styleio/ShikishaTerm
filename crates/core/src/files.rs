//! Finding files in the working folder, for the panel that lists them.
//!
//! Listing one folder is `runtime::local_rows`, which the file panel and the
//! transfer panel both call: one folder's contents is one question, and it had
//! an answer already. What is here is the other half — finding something in a
//! folder too big to scroll.
//!
//! **What counts as "the files here" is git's answer when there is one.**
//! `git ls-files --cached --others --exclude-standard` is one call that names
//! every tracked and every untracked-but-not-ignored file, which is exactly
//! the set a person means. The alternative — walking the folder and deciding
//! for ourselves what to skip — means inventing a list of names
//! (`node_modules`, `target`, `.venv`, …) that is wrong for the next project
//! along. A folder that is not a repository gets that walk, because then there
//! is nobody to ask.
//!
//! Every search is bounded, and says so when it stopped early. A search that
//! quietly returns half an answer is worse than one that admits it: the person
//! is deciding something from what they see.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How many files a search will look at before giving up. A repository with
/// more than this is one where a narrower search is the answer
const SCAN_CAP: usize = 6000;
/// How much of one file is read looking for text. Generated files are enormous
/// and what a person is looking for is not in the tail of one
const READ_CAP: usize = 512 * 1024;
/// How long a search runs before it stops with what it has
const TIME_CAP: Duration = Duration::from_millis(2500);
/// How deep a plain walk goes when git is not there to ask
const WALK_DEPTH: usize = 12;
/// The most an editor will open in one piece. Past this it is a log or a
/// bundle, not something being read here, and the honest answer is to say so
pub const READ_LIMIT: u64 = 4 * 1024 * 1024;

/// What the disk says about a file without reading it: when it was last
/// written, and how long it is.
///
/// Asked every pass while an editor is open, so it must cost nothing -- which
/// rules out reading the file to hash it. Two writes inside the same second
/// that keep the length are the one thing this misses, and the save still
/// catches that (it compares the contents' own mark).
pub fn stamp_of(path: &Path) -> String {
    let Ok(m) = std::fs::metadata(path) else { return String::new() };
    let when = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{when}-{}", m.len())
}

/// What a file was when it was read, short enough to travel in the state.
///
/// Not a security hash and not trying to be: it answers one question -- "is
/// this still the same bytes I handed out?" -- so that a save can refuse to
/// overwrite somebody else's work. FNV-1a over the contents, with the length,
/// because two files that differ only in length are not the same file.
pub fn mark_of(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{h:x}-{}", bytes.len())
}

/// One thing a search found.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Hit {
    /// Relative to the folder searched, written with forward slashes so the
    /// same string reads the same on both sides of the window
    pub path: String,
    /// Which line it was on, when the search was for text
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The line itself, trimmed. Absent for a search by name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// What a search came back with.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Found {
    pub hits: Vec<Hit>,
    /// Whether it stopped before the end -- of the files, or of the clock
    pub capped: bool,
}

/// Files with this name in them, newest question first.
pub fn by_name(root: &Path, query: &str, limit: usize) -> Found {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Found { hits: Vec::new(), capped: false };
    }
    let (files, mut capped) = candidates(root);
    let mut hits = Vec::new();
    for rel in files {
        if hits.len() >= limit {
            capped = true;
            break;
        }
        if rel.to_lowercase().contains(&needle) {
            hits.push(Hit { path: rel, line: None, text: None });
        }
    }
    Found { hits, capped }
}

/// Files with this text inside them, and the line it is on.
///
/// One hit per file: the panel is a list of files, and a file that says the
/// word thirty times is still one file to open. The first line is the one
/// shown, because a person recognises the place from it.
pub fn by_text(root: &Path, query: &str, limit: usize) -> Found {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Found { hits: Vec::new(), capped: false };
    }
    let (files, mut capped) = candidates(root);
    let started = Instant::now();
    let mut hits = Vec::new();
    for rel in files {
        if hits.len() >= limit || started.elapsed() > TIME_CAP {
            capped = true;
            break;
        }
        let Some(text) = read_head(&root.join(&rel)) else { continue };
        // Binary files have no lines to show, and a match inside one is a
        // coincidence of bytes rather than something a person wrote
        if text.contains('\0') {
            continue;
        }
        for (n, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(&needle) {
                let shown = line.trim();
                hits.push(Hit {
                    path: rel.clone(),
                    line: Some(n as u32 + 1),
                    // A minified bundle is one line the width of a book. What
                    // is shown is the neighbourhood of the match, not the line
                    text: Some(around(shown, &needle)),
                });
                break;
            }
        }
    }
    Found { hits, capped }
}

/// Enough of a long line to recognise the place, centred on the match.
fn around(line: &str, needle: &str) -> String {
    const SHOWN: usize = 160;
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= SHOWN {
        return line.to_string();
    }
    let low: String = line.to_lowercase();
    // Character position, not byte position: the cut is made in characters
    let at = low.find(needle).map(|b| low[..b].chars().count()).unwrap_or(0);
    let start = at.saturating_sub(SHOWN / 3);
    let end = (start + SHOWN).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// The files under `root` worth searching, and whether the list was cut short.
///
/// git first, because the repository already knows what it ignores.
fn candidates(root: &Path) -> (Vec<String>, bool) {
    if let Some(list) = asked_git(root) {
        let capped = list.len() > SCAN_CAP;
        let mut list = list;
        list.truncate(SCAN_CAP);
        return (list, capped);
    }
    walked(root)
}

/// What git says is here: tracked, plus untracked that it does not ignore.
///
/// `-z` rather than lines, because a name with a space or a character outside
/// ASCII comes back quoted and escaped otherwise -- and every Japanese file
/// name is that.
fn asked_git(root: &Path) -> Option<Vec<String>> {
    let out = crate::git::run(
        root,
        &["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
    )
    .ok()?;
    let mut list: Vec<String> = out
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.replace('\\', "/"))
        .collect();
    list.sort();
    list.dedup();
    Some(list)
}

/// No repository to ask: walk it, and skip the one folder that is never
/// content. Nothing else is guessed at -- a folder nobody ignores is a folder
/// somebody wanted.
fn walked(root: &Path) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut capped = false;
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= SCAN_CAP {
            capped = true;
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name == ".git" {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                if depth < WALK_DEPTH {
                    stack.push((e.path(), depth + 1));
                } else {
                    capped = true;
                }
                continue;
            }
            if let Some(rel) = relative(root, &e.path()) {
                out.push(rel);
            }
        }
    }
    out.sort();
    (out, capped)
}

/// A path under the root, written the one way.
fn relative(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root).ok().map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// The front of a file, as text. A file that is not text comes back as
/// whatever its bytes say, and the caller throws it out.
fn read_head(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; READ_CAP];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of this test's own. The process id is in the path because two
    /// test runs on one machine (another window, a watch loop) otherwise share
    /// it and delete each other's files mid-assertion.
    fn scratch(name: &str) -> PathBuf {
        let at = std::env::temp_dir()
            .join(format!("shikisha-files-test-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&at);
        std::fs::create_dir_all(at.join("sub")).unwrap();
        at
    }

    #[test]
    fn a_name_is_found_wherever_it_sits() {
        let at = scratch("byname");
        std::fs::write(at.join("readme.md"), "hello").unwrap();
        std::fs::write(at.join("sub").join("Payments.rs"), "x").unwrap();
        let found = by_name(&at, "payment", 50);
        assert_eq!(
            found.hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["sub/Payments.rs"],
            "名前の一部・大文字小文字の違いでも見つかる"
        );
        assert!(!found.capped);
    }

    #[test]
    fn text_comes_back_with_the_line_it_is_on() {
        let at = scratch("bytext");
        std::fs::write(at.join("a.txt"), "one\ntwo NEEDLE two\nthree\n").unwrap();
        std::fs::write(at.join("b.txt"), "nothing here").unwrap();
        let found = by_text(&at, "needle", 50);
        assert_eq!(found.hits.len(), 1, "中身で見つかるのは1件だけ");
        let hit = &found.hits[0];
        assert_eq!(hit.path, "a.txt");
        assert_eq!(hit.line, Some(2), "行番号は1から数える");
        assert_eq!(hit.text.as_deref(), Some("two NEEDLE two"));
    }

    #[test]
    fn one_file_answers_once() {
        let at = scratch("once");
        std::fs::write(at.join("a.txt"), "hit\nhit\nhit\n").unwrap();
        assert_eq!(by_text(&at, "hit", 50).hits.len(), 1, "同じファイルは1行だけ");
    }

    #[test]
    fn nothing_is_searched_for_an_empty_box() {
        let at = scratch("empty");
        std::fs::write(at.join("a.txt"), "anything").unwrap();
        assert!(by_name(&at, "   ", 50).hits.is_empty(), "空の検索は全件ではない");
        assert!(by_text(&at, "", 50).hits.is_empty());
    }

    #[test]
    fn a_long_line_is_cut_around_the_match() {
        let line = format!("{}NEEDLE{}", "x".repeat(400), "y".repeat(400));
        let shown = around(&line, "needle");
        assert!(shown.chars().count() < 200, "長い行はそのまま出さない");
        assert!(shown.contains("NEEDLE"), "切っても当たりは残る");
        assert!(shown.starts_with('…') && shown.ends_with('…'), "切った側に印が付く");
    }

    #[test]
    fn the_mark_answers_one_question() {
        assert_eq!(mark_of(b"hello"), mark_of(b"hello"), "同じ中身は同じ印");
        assert_ne!(mark_of(b"hello"), mark_of(b"hellp"), "1文字違えば別");
        assert_ne!(mark_of(b"hello"), mark_of(b"hello "), "長さが違えば別");
        assert_ne!(mark_of(b""), mark_of(b"x"));
    }

    #[test]
    fn a_stamp_moves_when_the_file_does() {
        let at = scratch("stamp").join("a.txt");
        std::fs::write(&at, "one").unwrap();
        let first = stamp_of(&at);
        assert!(!first.is_empty(), "そこにあるファイルには印が付く");
        std::fs::write(&at, "one and a half").unwrap();
        assert_ne!(stamp_of(&at), first, "長さが変われば印も変わる");
        assert!(stamp_of(&at.with_file_name("nothing")).is_empty(), "無いものには印が無い");
    }

    #[test]
    fn a_binary_file_is_not_read_as_lines() {
        let at = scratch("binary");
        std::fs::write(at.join("a.bin"), b"pre\0NEEDLE\0post").unwrap();
        assert!(by_text(&at, "needle", 50).hits.is_empty(), "バイナリは中身検索の対象外");
    }
}
