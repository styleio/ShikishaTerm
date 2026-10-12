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

/// The path of `name` in the folder `rel` is in: what a rename or a copy beside
/// it is written to. None for a name that is not one name -- empty, a dot or
/// two, or with a slash that would put it in another folder
pub fn beside(rel: &str, name: &str) -> Option<String> {
    let name = name.trim();
    let one = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']) && !name.chars().any(char::is_control);
    if !one {
        return None;
    }
    let rel = rel.replace('\\', "/");
    Some(match rel.trim_matches('/').rsplit_once('/') {
        Some((folder, _)) => format!("{folder}/{name}"),
        None => name.to_string(),
    })
}

/// The path of `name` inside the folder `at` ("" is the root): where a new
/// folder is made. None for a name that is not one name, as [`beside`]
pub fn inside(at: &str, name: &str) -> Option<String> {
    let at = at.replace('\\', "/");
    let at = at.trim_matches('/');
    beside(&if at.is_empty() { "x".to_string() } else { format!("{at}/x") }, name)
}

/// A copy of a file, or of a folder and everything in it, at `to`, which must
/// not be there yet. A folder is never copied into itself
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if !meta.is_dir() {
        return std::fs::copy(from, to).map(|_| ());
    }
    if to.starts_with(from) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "a folder cannot be copied into itself"));
    }
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        copy_tree(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

/// Into the recycle bin, where it can be taken back from. A drive with no
/// recycle bin (a network share) has Windows itself ask before it deletes for
/// good, since the question the page asked promised the bin. A machine with no
/// recycle bin at all has it deleted
#[cfg(windows)]
pub fn to_bin(path: &Path) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    use std::os::windows::ffi::OsStrExt;
    // A list of paths, each ended by a zero and the list by another
    let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
    let mut op = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT | FOF_WANTNUKEWARNING) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    if std::fs::symlink_metadata(path).is_err() {
        return Err(crate::i18n::t("err.files.gone"));
    }
    // SAFETY: `from` is a list ended as the call wants, and outlives it
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted != 0 {
        return Err(crate::i18n::tp("err.files.not_binned", &[("code", &format!("0x{code:x}"))]));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn to_bin(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| crate::i18n::t("err.files.gone"))?;
    if meta.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }.map_err(|e| e.to_string())
}

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
    stamp_from(when, m.len())
}

/// The same mark, from what another machine said about its file. One way of
/// writing it, so a file there and a file here are followed by the same page
pub fn stamp_from(modified: u64, len: u64) -> String {
    format!("{modified}-{len}")
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
    let (files, capped) = candidates(root);
    named(files, capped, &needle, limit)
}

/// The files of a list whose path has the lowercased `needle` in it.
fn named(files: Vec<String>, mut capped: bool, needle: &str, limit: usize) -> Found {
    let mut hits = Vec::new();
    for rel in files {
        if hits.len() >= limit {
            capped = true;
            break;
        }
        if rel.to_lowercase().contains(needle) {
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
    walked_up_to(root, SCAN_CAP)
}

fn walked_up_to(root: &Path, cap: usize) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut capped = false;
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= cap {
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

// -- Searching inside every file, every match --------------------------------
//
// The column's "Inside" search: every place the words are, in every file the
// folder is made of, grouped by file. Off the board's thread, and given up
// when a newer search for the same panel starts.

/// What a search inside the files asks for
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct Ask {
    #[serde(default)]
    pub q: String,
    /// Upper and lower case told apart
    #[serde(default)]
    pub case: bool,
    /// Only where the words stand as a whole word
    #[serde(default)]
    pub word: bool,
    /// The words are a regular expression
    #[serde(default)]
    pub regex: bool,
    /// Which files to look in and which to leave out, as patterns separated
    /// by commas (`*.rs, src/**`). Empty is every file
    #[serde(default)]
    pub include: String,
    #[serde(default)]
    pub exclude: String,
}

/// One place the words are: the line, where on it (counted in characters,
/// from 1) and how long, and the line as shown with where the match starts in
/// what is shown
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Spot {
    pub line: u32,
    pub col: u32,
    pub len: u32,
    /// The same place counted the way the editor counts a line (in UTF-16
    /// units, from 1): a character outside the first plane is two there
    pub col16: u32,
    pub text: String,
    /// Where the match starts in `text`, in characters
    pub at: u32,
}

/// Every place in one file
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileSpots {
    pub path: String,
    pub spots: Vec<Spot>,
}

/// What a search inside the files came back with
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Spots {
    pub files: Vec<FileSpots>,
    /// Places found, across every file
    pub total: usize,
    /// Whether it stopped before the end: of the files, of the clock or of
    /// what a list is worth reading
    pub capped: bool,
    /// Files not looked in because they are bigger than the editor opens
    #[serde(skip_serializing_if = "is_zero")]
    pub too_big: usize,
}

/// How many places a search lists before it stops. More than anybody reads
/// down; past it the answer is narrower words or a pattern of files
pub const SPOTS_CAP: usize = 1000;
/// How many files a search inside them looks at: off the board's thread, so
/// a big repository is looked through rather than given up on
const SEARCH_FILES_CAP: usize = 60_000;
/// How long a search inside the files runs before it stops with what it has
const SEARCH_TIME_CAP: Duration = Duration::from_secs(12);
/// How much of a line is shown around a match. A minified file is one line
/// the width of a book
const SHOWN: usize = 160;

/// The words as the search reads them, or why they cannot be read
pub fn matcher(ask: &Ask) -> Result<regex::Regex, String> {
    let q = ask.q.trim();
    if q.is_empty() {
        return Err(String::new());
    }
    let body = if ask.regex { q.to_string() } else { regex::escape(q) };
    let body = if ask.word { format!(r"\b(?:{body})\b") } else { body };
    regex::RegexBuilder::new(&body)
        .case_insensitive(!ask.case)
        .size_limit(1 << 22)
        .build()
        .map_err(|e| crate::i18n::tp("err.files.bad_pattern", &[("why", &e.to_string())]))
}

/// File patterns, separated by commas or new lines, each read the way git
/// and ripgrep read one: `*` is any part of a name, `**` any number of
/// folders, `?` one character. A pattern with no slash in it names a file
/// or a folder anywhere (`node_modules`, `*.rs`); one with a slash is a
/// path from the folder's top (`src/**/*.ts`)
pub fn patterns(list: &str) -> Vec<(bool, regex::Regex)> {
    globs(list)
        .into_iter()
        .filter_map(|(anchored, body)| regex::Regex::new(&format!("^{}$", body(Dialect::Rust))).ok().map(|r| (anchored, r)))
        .collect()
}

/// Which regular expressions a pattern is written out for: this program's,
/// or the extended ones of `grep -E` on another machine
#[derive(Clone, Copy, PartialEq)]
enum Dialect {
    Rust,
    Posix,
}

/// Each pattern of a list: whether it is a path from the top, and its body
/// as a regular expression of either dialect (without its anchors)
fn globs(list: &str) -> Vec<(bool, impl Fn(Dialect) -> String)> {
    list.split([',', '\n'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let p = p.replace('\\', "/");
            let anchored = p.trim_end_matches('/').contains('/');
            let p = p.trim_start_matches("./").trim_start_matches('/').trim_end_matches('/').to_string();
            (anchored, move |dialect: Dialect| {
                let mut re = String::new();
                let mut chars = p.chars().peekable();
                while let Some(c) = chars.next() {
                    match c {
                        '*' if chars.peek() == Some(&'*') => {
                            chars.next();
                            if chars.peek() == Some(&'/') {
                                chars.next();
                                re.push_str(if dialect == Dialect::Rust { "(?:.*/)?" } else { "(.*/)?" });
                            } else {
                                re.push_str(".*");
                            }
                        }
                        '*' => re.push_str("[^/]*"),
                        '?' => re.push_str("[^/]"),
                        c if dialect == Dialect::Rust => re.push_str(&regex::escape(&c.to_string())),
                        // Only what POSIX gives a meaning to is escaped: a
                        // backslash before anything else is undefined there
                        c if ".[]\\()*+?{}|^$".contains(c) => {
                            re.push('\\');
                            re.push(c);
                        }
                        c => re.push(c),
                    }
                }
                re
            })
        })
        .collect()
}

/// The patterns of a list as one extended regular expression for `grep -E`
/// over the paths a machine lists (`./` before them where `find` made the
/// list), meaning what [`kept`] means here: a path from the top names that
/// path or a folder it is under, any other pattern any one name along the
/// path. `None` for an empty list
pub fn posix_paths(list: &str) -> Option<String> {
    let parts: Vec<String> = globs(list)
        .into_iter()
        .map(|(anchored, body)| {
            let body = body(Dialect::Posix);
            if anchored { format!("^(\\./)?{body}(/|$)") } else { format!("(^|/){body}(/|$)") }
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("|"))
}

/// Whether `rel` is one of the files the patterns name: an anchored one is
/// matched against the whole path or a folder it is under, the others
/// against any one name along it
fn named_by(rel: &str, pats: &[(bool, regex::Regex)]) -> bool {
    pats.iter().any(|(anchored, re)| {
        if *anchored {
            // The path itself, or a folder it is under
            let mut at = rel;
            loop {
                if re.is_match(at) {
                    return true;
                }
                match at.rsplit_once('/') {
                    Some((up, _)) => at = up,
                    None => return false,
                }
            }
        }
        rel.split('/').any(|part| re.is_match(part))
    })
}

/// Whether a file is looked in, by the patterns asked for
pub fn kept(rel: &str, include: &[(bool, regex::Regex)], exclude: &[(bool, regex::Regex)]) -> bool {
    (include.is_empty() || named_by(rel, include)) && !named_by(rel, exclude)
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// The places on one line, each with the line shown around it, no more than
/// `room` of them. Each place is counted on from the last, so a long line
/// with many matches is walked once, not once per match
fn spots_on(line: &str, n: u32, re: &regex::Regex, room: usize) -> Vec<Spot> {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut out = Vec::new();
    // Where the last place ended, in bytes, characters and UTF-16 units
    let (mut byte, mut chars, mut units) = (0usize, 0usize, 0usize);
    // Where each character starts, worked out once and only for a line that
    // has something on it
    let mut starts: Option<Vec<usize>> = None;
    for m in re.find_iter(line) {
        if out.len() >= room {
            break;
        }
        if m.as_str().is_empty() {
            continue;
        }
        let before = &line[byte..m.start()];
        chars += before.chars().count();
        units += before.encode_utf16().count();
        byte = m.start();
        let len = m.as_str().chars().count();
        let starts = starts.get_or_insert_with(|| line.char_indices().map(|(i, _)| i).collect());
        let (text, at) = shown_around(line, starts, chars, len);
        out.push(Spot { line: n, col: chars as u32 + 1, len: len as u32, col16: units as u32 + 1, text, at: at as u32 });
    }
    out
}

/// Enough of a line to recognise the place by, with the match in it: the
/// line's start trimmed of its indent, and a long line cut around the match
/// -- never more than [`SHOWN`] characters, however long the match. Says
/// where the match starts in what is shown. `starts` is where each
/// character of the line starts
fn shown_around(line: &str, starts: &[usize], col: usize, _len: usize) -> (String, usize) {
    let count = starts.len();
    let indent = line.chars().take(col).take_while(|c| c.is_whitespace()).count();
    let (mut start, mut end) = (indent, count);
    if end - start > SHOWN {
        start = col.saturating_sub(SHOWN / 3).max(indent);
        end = (start + SHOWN).min(count);
    }
    let at_byte = |i: usize| if i >= count { line.len() } else { starts[i] };
    let mut out = String::new();
    let mut at = col - start;
    if start > indent {
        out.push('\u{2026}');
        at += 1;
    }
    out.push_str(&line[at_byte(start)..at_byte(end)]);
    if end < count {
        out.push('\u{2026}');
    }
    (out, at)
}

/// Every place in the folder `root` the words are. `gone` says the search
/// is no longer wanted (a newer one started): it stops at once
pub fn search(root: &Path, ask: &Ask, gone: &(dyn Fn() -> bool + Sync)) -> Result<Spots, String> {
    let re = matcher(ask)?;
    let (include, exclude) = (patterns(&ask.include), patterns(&ask.exclude));
    let (files, mut capped) = candidates_up_to(root, SEARCH_FILES_CAP);
    let files: Vec<String> = files.into_iter().filter(|f| kept(f, &include, &exclude)).collect();
    let started = Instant::now();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let found = std::sync::atomic::AtomicUsize::new(0);
    let stopped = std::sync::atomic::AtomicBool::new(false);
    let too_big = std::sync::atomic::AtomicUsize::new(0);
    let out = std::sync::Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                use std::sync::atomic::Ordering::Relaxed;
                loop {
                    let i = next.fetch_add(1, Relaxed);
                    let Some(rel) = files.get(i) else { break };
                    if found.load(Relaxed) >= SPOTS_CAP || started.elapsed() > SEARCH_TIME_CAP || gone() {
                        stopped.store(true, Relaxed);
                        break;
                    }
                    let text = match read_whole(&root.join(rel)) {
                        Read::Text(t) => t,
                        Read::TooBig => {
                            too_big.fetch_add(1, Relaxed);
                            continue;
                        }
                        Read::Not => continue,
                    };
                    let mut spots = Vec::new();
                    for (n, line) in text.split('\n').enumerate() {
                        // The budget and the stop are minded inside a file
                        // too: one file can be a whole search's worth
                        let room = SPOTS_CAP.saturating_sub(found.load(Relaxed) + spots.len());
                        if room == 0 || (n % 512 == 0 && (gone() || started.elapsed() > SEARCH_TIME_CAP)) {
                            stopped.store(true, Relaxed);
                            break;
                        }
                        spots.extend(spots_on(line, n as u32 + 1, &re, room));
                    }
                    if !spots.is_empty() {
                        found.fetch_add(spots.len(), Relaxed);
                        out.lock().unwrap_or_else(|e| e.into_inner()).push(FileSpots { path: rel.clone(), spots });
                    }
                }
            });
        }
    });
    capped |= stopped.into_inner();
    let mut found = trimmed(out.into_inner().unwrap_or_else(|e| e.into_inner()), capped);
    found.too_big = too_big.into_inner();
    Ok(found)
}

/// The files in path order, cut at [`SPOTS_CAP`] places
fn trimmed(mut files: Vec<FileSpots>, mut capped: bool) -> Spots {
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let mut total = 0;
    let mut kept = Vec::new();
    for mut f in files {
        if total >= SPOTS_CAP {
            capped = true;
            break;
        }
        if total + f.spots.len() > SPOTS_CAP {
            f.spots.truncate(SPOTS_CAP - total);
            capped = true;
        }
        total += f.spots.len();
        kept.push(f);
    }
    Spots { files: kept, total, capped, too_big: 0 }
}

/// What a file is, to a search inside it
enum Read {
    Text(String),
    /// Bigger than the editor opens: not looked in, and said so
    TooBig,
    /// Not text, or not there
    Not,
}

/// A file's text when it is text and small enough for the editor to open:
/// what a match is shown in, and opened at
fn read_whole(path: &Path) -> Read {
    let Ok(meta) = std::fs::metadata(path) else { return Read::Not };
    if !meta.is_file() {
        return Read::Not;
    }
    if meta.len() > READ_LIMIT {
        return Read::TooBig;
    }
    let Ok(bytes) = std::fs::read(path) else { return Read::Not };
    // Binary files have no lines to show, and a match inside one is a
    // coincidence of bytes rather than something a person wrote
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Read::Not;
    }
    Read::Text(String::from_utf8_lossy(&bytes).into_owned())
}

/// The files under `root` worth looking in, up to `cap`, and whether the
/// list was cut short: git's, else the walk
fn candidates_up_to(root: &Path, cap: usize) -> (Vec<String>, bool) {
    if let Some(mut list) = asked_git(root) {
        let capped = list.len() > cap;
        list.truncate(cap);
        return (list, capped);
    }
    walked_up_to(root, cap)
}

// -- Opening a file by a few letters of its name -------------------------------

/// How many files a list for opening one by name is read up to
pub const QUICK_FILES_CAP: usize = 100_000;
/// How long a folder's list of files is used again before it is read anew:
/// the same few seconds a person types a name in, not long enough to miss a
/// file they have just made
const QUICK_FRESH: Duration = Duration::from_secs(15);

type Listed = std::sync::Arc<(Vec<String>, bool)>;

/// A folder's files, read again only when the last reading is older than
/// [`QUICK_FRESH`]: every letter typed asks
/// A reading that failed is not kept: the next letter typed asks again. The
/// reading itself is done outside the lock, so a slow machine holds up no
/// other folder's list
pub fn listing(key: &str, read: impl FnOnce() -> Result<(Vec<String>, bool), String>) -> Result<Listed, String> {
    static KEPT: std::sync::Mutex<Option<std::collections::HashMap<String, (Instant, Listed)>>> = std::sync::Mutex::new(None);
    {
        let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        let kept = kept.get_or_insert_with(Default::default);
        kept.retain(|_, (at, _)| at.elapsed() < QUICK_FRESH);
        if let Some((_, l)) = kept.get(key) {
            return Ok(l.clone());
        }
    }
    let l: Listed = std::sync::Arc::new(read()?);
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    kept.get_or_insert_with(Default::default).insert(key.to_string(), (Instant::now(), l.clone()));
    Ok(l)
}

/// This machine's files under `root`, for opening one by name
pub fn quick_list(root: &Path) -> Result<Listed, String> {
    listing(&root.display().to_string(), || Ok(candidates_up_to(root, QUICK_FILES_CAP)))
}

/// How well the letters typed fit a path: `None` when they do not. Every
/// word typed has to be in it, its letters in order; a word fits best whole
/// in the file's name, then with its letters at the starts of words and
/// side by side, and a shorter path beats a longer one
pub fn fit(path: &str, query: &str) -> Option<i64> {
    let low = path.to_lowercase();
    let name_at = low.rfind('/').map_or(0, |i| i + 1);
    let mut score = 0i64;
    for word in query.split_whitespace() {
        let word = word.to_lowercase();
        score += fit_word(&low, path, name_at, &word)?;
    }
    Some(score - (path.chars().count() as i64) / 4)
}

fn fit_word(low: &str, orig: &str, name_at: usize, word: &str) -> Option<i64> {
    let name = &low[name_at..];
    if let Some(i) = name.find(word) {
        // Whole in the name, best at its start
        return Some(1000 + if i == 0 { 200 } else { 0 } - i as i64);
    }
    if let Some(i) = low.find(word) {
        return Some(500 - i as i64 / 8);
    }
    // Letters in order, each placed where it fits best going forwards: the
    // start of a word earns, letters side by side earn, a gap costs
    let hay: Vec<char> = low.chars().collect();
    let raw: Vec<char> = orig.chars().collect();
    let mut at = 0usize;
    let mut last: Option<usize> = None;
    let mut score = 0i64;
    for c in word.chars() {
        let found = (at..hay.len()).find(|&i| hay[i] == c)?;
        let starts = found == 0
            || matches!(hay[found - 1], '/' | '_' | '-' | '.' | ' ')
            || (raw.get(found).is_some_and(|ch| ch.is_uppercase()) && raw.get(found - 1).is_some_and(|ch| ch.is_lowercase()));
        score += if starts { 12 } else { 0 };
        score += match last {
            Some(l) if l + 1 == found => 8,
            Some(l) => -((found - l - 1).min(20) as i64),
            None => 0,
        };
        if found >= name_at.min(hay.len()) {
            score += 3;
        }
        last = Some(found);
        at = found + 1;
    }
    Some(score)
}

/// The paths the letters fit best, best first, at most `limit`
pub fn best_fits(paths: &[String], query: &str, limit: usize) -> Vec<String> {
    let mut scored: Vec<(i64, &String)> = paths.iter().filter_map(|p| fit(p, query).map(|s| (s, p))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.len().cmp(&b.1.len())).then_with(|| a.1.cmp(b.1)));
    scored.into_iter().take(limit).map(|(_, p)| p.clone()).collect()
}

/// The same two searches, on a folder that is on another machine.
///
/// There is no walking a folder over there from here -- one listing is one
/// call, and a source tree is thousands of them. So the machine is asked to do
/// the walking, in one command for its own shell, and what comes back is read
/// here into the same answer a folder here gets. The set of files is the same
/// set as well: git's, where there is a repository, and every file but git's
/// own folder where there is not.
pub mod far {
    use super::{Ask, FileSpots, Found, Hit, SCAN_CAP, Spots, WALK_DEPTH, around, kept, matcher, named, patterns, spots_on, trimmed};
    use crate::ssh::sh_quote;

    /// How much of the list of files is read back. Enough for `SCAN_CAP` paths
    /// of any sensible length; a folder with more is one a search says it
    /// stopped in
    const LIST_BYTES: usize = 4 * 1024 * 1024;

    /// The files under the folder the command stands in, one after another
    /// with a zero byte after each. git first, as here; `find` when the folder
    /// is in no repository, or git is not on that machine
    fn listing() -> String {
        format!(
            "{{ git ls-files --cached --others --exclude-standard -z 2>/dev/null || \
             find . -maxdepth {} -path ./.git -prune -o -type f -print0; }}",
            WALK_DEPTH + 1
        )
    }

    /// What a command here ends with when the folder it was to stand in is
    /// not there: told apart from a search that found nothing
    pub const GONE: i32 = 3;

    /// What to run there to find files by name.
    pub fn names_command(root: &str) -> String {
        format!("cd {} || exit {GONE}; {} | head -c {LIST_BYTES}", sh_quote(root), listing())
    }

    /// The code [`copy_command`] ends with when the name it was to copy to is
    /// already taken: told apart from a copy that failed, so the person is
    /// told which
    pub const TAKEN: i32 = 17;

    /// What to run there to copy a file or a folder to `to`, both whole paths
    /// already fenced. A name already there is left alone; `-p` keeps when it
    /// was written, as the original says
    pub fn copy_command(from: &str, to: &str) -> String {
        let (from, to) = (sh_quote(from), sh_quote(to));
        format!("if [ -e {to} ] || [ -L {to} ]; then exit {TAKEN}; fi; cp -R -p -- {from} {to}")
    }

    /// What it printed, as the files whose name has the query in it.
    pub fn names(out: &str, query: &str, limit: usize) -> Found {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Found { hits: Vec::new(), capped: false };
        }
        let (files, capped) = listed(out, SCAN_CAP);
        named(files, capped, &needle, limit)
    }

    /// What [`names_command`] printed, as the files there: up to `cap`, and
    /// whether the list was cut short
    pub fn listed(out: &str, cap: usize) -> (Vec<String>, bool) {
        let mut files: Vec<String> = out
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(|p| p.strip_prefix("./").unwrap_or(p).to_string())
            .collect();
        files.sort();
        files.dedup();
        // A list that filled what was read back was cut, and its last name
        // may be half of one
        let mut capped = out.len() >= LIST_BYTES;
        if capped {
            files.pop();
        }
        if files.len() > cap {
            files.truncate(cap);
            capped = true;
        }
        (files, capped)
    }

    /// What to run there to find every place the words are: every matching
    /// line of every file, with the same choices a search here has. One line
    /// more than `limit` is asked for, which is how a list that went on is
    /// told from one that ended exactly there. A pattern of files is applied
    /// to what comes back
    pub fn spots_command(root: &str, ask: &Ask, limit: usize) -> String {
        let mut flags = String::from("-I -n -H --null");
        if !ask.case {
            flags.push_str(" -i");
        }
        if ask.word {
            flags.push_str(" -w");
        }
        // A regular expression is read as Perl's where the machine's grep
        // can (GNU's can), which is the reading this program gives one --
        // `\\d`, `(?:)` and the rest. Elsewhere it is read as POSIX's
        let dialect = if ask.regex { "$R" } else { "-F" };
        // Which files are looked in is decided before they are looked in, so
        // a narrower pattern is what lets a capped search reach the rest.
        // Where grep cannot read a list ended by zero bytes, every file is
        // looked in and what comes back is filtered here
        let filter = |list: &str, out: bool| match super::posix_paths(list) {
            Some(re) => format!(" | {{ if [ -n \"$Z\" ]; then grep -z{} -E -e {}; else cat; fi; }}", if out { "v" } else { "" }, sh_quote(&re)),
            None => String::new(),
        };
        format!(
            "cd {} || exit {GONE}; R=-E; echo | grep -P '' >/dev/null 2>&1 && R=-P; Z=; printf 'a\\0' | grep -qz a 2>/dev/null && Z=1; \
             {}{}{} | xargs -0 grep {flags} {dialect} -e {} -- 2>/dev/null | head -n {} | head -c {SPOTS_BYTES}",
            sh_quote(root),
            listing(),
            filter(&ask.include, false),
            filter(&ask.exclude, true),
            sh_quote(ask.q.trim()),
            limit + 1
        )
    }

    /// How much of what a search there printed is read back: the places it
    /// lists, with room for long lines, and no more however long they run
    const SPOTS_BYTES: usize = 4 * 1024 * 1024;

    /// What it printed -- a path, a zero byte, then the line number and the
    /// line -- as the same places a search here finds. Where on the line is
    /// worked out here, by the same reading of the words
    pub fn spots(out: &str, ask: &Ask, limit: usize) -> Result<Spots, String> {
        let re = matcher(ask)?;
        let (include, exclude) = (patterns(&ask.include), patterns(&ask.exclude));
        let mut files: Vec<FileSpots> = Vec::new();
        let mut lines = 0usize;
        let mut found = 0usize;
        // Read back to its limit in bytes: the last line may be cut
        let mut capped = out.len() >= SPOTS_BYTES;
        for row in out.lines() {
            let Some((path, rest)) = row.split_once('\0') else { continue };
            let Some((n, line)) = rest.split_once(':') else { continue };
            let Ok(n) = n.parse::<u32>() else { continue };
            lines += 1;
            if lines > limit {
                capped = true;
                break;
            }
            let path = path.strip_prefix("./").unwrap_or(path);
            if !kept(path, &include, &exclude) {
                continue;
            }
            let room = super::SPOTS_CAP.saturating_sub(found);
            if room == 0 {
                capped = true;
                break;
            }
            let spots = spots_on(line, n, &re, room);
            if spots.is_empty() {
                continue;
            }
            found += spots.len();
            match files.last_mut().filter(|f| f.path == path) {
                Some(f) => f.spots.extend(spots),
                None => files.push(FileSpots { path: path.to_string(), spots }),
            }
        }
        Ok(trimmed(files, capped))
    }

    /// What to run there to find files with this text inside them: the first
    /// line of each, as here, and never a file that is not text. One more line
    /// than `limit` is asked for, which is how a list that went on is told
    /// from one that ended exactly there
    pub fn text_command(root: &str, query: &str, limit: usize) -> String {
        format!(
            "cd {} && {} | xargs -0 grep -I -i -F -n -m 1 -H --null -e {} -- 2>/dev/null | head -n {}",
            sh_quote(root),
            listing(),
            sh_quote(query.trim()),
            limit + 1
        )
    }

    /// What it printed -- a path, a zero byte, then the line number and the
    /// line -- as the same hits a folder here gives.
    pub fn text(out: &str, query: &str, limit: usize) -> Found {
        let needle = query.trim().to_lowercase();
        let mut hits = Vec::new();
        let mut capped = false;
        for row in out.lines() {
            let Some((path, rest)) = row.split_once('\0') else { continue };
            let Some((n, line)) = rest.split_once(':') else { continue };
            let Ok(n) = n.parse::<u32>() else { continue };
            if hits.len() >= limit {
                capped = true;
                break;
            }
            hits.push(Hit {
                path: path.strip_prefix("./").unwrap_or(path).to_string(),
                line: Some(n),
                text: Some(around(line.trim(), &needle)),
            });
        }
        Found { hits, capped }
    }
}

/// How a file saved from the editor is written.
pub enum Save {
    /// In its encoding, exactly. Refused when a character in it cannot be
    Exact,
    /// In its encoding, with `?` for what cannot be written
    Replacing,
}

/// The bytes the editor's text is saved as, or the characters that stop it.
///
/// A file is written back in the encoding it was read in: a Shift_JIS CSV saved
/// as UTF-8 opens as nonsense in the spreadsheet that made it. What that
/// encoding cannot hold is not quietly dropped -- the characters come back, so
/// the person can choose to lose them or to change the file's encoding
pub fn save_bytes(text: &str, encoding: &'static encoding_rs::Encoding, how: Save) -> Result<Vec<u8>, Vec<char>> {
    match how {
        Save::Exact => crate::charset::write_as(text, encoding).ok_or_else(|| crate::charset::unwritable(text, encoding)),
        Save::Replacing => Ok(crate::charset::write_replacing(text, encoding)),
    }
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

    fn ask(q: &str) -> Ask {
        Ask { q: q.into(), ..Default::default() }
    }

    #[test]
    fn the_words_are_read_as_asked() {
        let plain = matcher(&ask("a.b")).unwrap();
        assert!(plain.is_match("A.B") && !plain.is_match("axb"), "plain words, either case");
        let case = matcher(&Ask { case: true, ..ask("Foo") }).unwrap();
        assert!(case.is_match("Foo") && !case.is_match("foo"));
        let word = matcher(&Ask { word: true, ..ask("log") }).unwrap();
        assert!(word.is_match("a log here") && !word.is_match("catalog"));
        let re = matcher(&Ask { regex: true, ..ask("fn \\w+_test") }).unwrap();
        assert!(re.is_match("fn a_test()"));
        assert!(matcher(&Ask { regex: true, ..ask("(") }).is_err(), "a broken pattern says so");
        assert!(matcher(&ask("  ")).is_err());
    }

    #[test]
    fn file_patterns_name_files_and_folders() {
        let inc = patterns("*.rs, docs/**");
        assert!(kept("src/main.rs", &inc, &[]));
        assert!(kept("docs/a/b.md", &inc, &[]));
        assert!(!kept("src/a.md", &inc, &[]));
        let exc = patterns("node_modules\ntarget/");
        assert!(!kept("web/node_modules/x/index.js", &[], &exc), "a name with no slash is a folder anywhere");
        assert!(!kept("target/debug/x", &[], &exc));
        assert!(kept("src/target.rs", &[], &exc), "only the name, not part of one");
        let deep = patterns("src/**/*.ts");
        assert!(kept("src/a/b/c.ts", &deep, &[]) && kept("src/c.ts", &deep, &[]) && !kept("lib/c.ts", &deep, &[]));
    }

    #[test]
    fn every_place_on_a_line_is_found_and_shown() {
        let re = matcher(&ask("ab")).unwrap();
        let s = spots_on("    xx ab yy AB\r", 7, &re, 100);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].line, s[0].col, s[0].len), (7, 8, 2));
        assert_eq!(s[0].text, "xx ab yy AB", "the indent and the line's end are not shown");
        assert_eq!(&s[0].text[s[0].at as usize..s[0].at as usize + 2], "ab");
        let long = format!("{}needle{}", "x".repeat(500), "y".repeat(500));
        let s = spots_on(&long, 1, &matcher(&ask("needle")).unwrap(), 100);
        let shown: Vec<char> = s[0].text.chars().collect();
        assert!(shown.len() <= SHOWN + 2 && shown[0] == '\u{2026}');
        assert_eq!(shown[s[0].at as usize..s[0].at as usize + 6].iter().collect::<String>(), "needle");
        assert_eq!(s[0].col, 501);
    }

    #[test]
    fn a_long_line_of_matches_is_bounded_in_count_and_in_size() {
        // A minified file: a hundred thousand matches on one line
        let line = "a".repeat(100_000);
        let s = spots_on(&line, 1, &matcher(&ask("a")).unwrap(), 5);
        assert_eq!(s.len(), 5, "no more than the room left");
        assert_eq!(s[4].col, 5);
        // One match the length of the whole line is shown cut
        let whole = spots_on(&line, 1, &matcher(&Ask { regex: true, ..ask(".*") }).unwrap(), 10);
        assert_eq!(whole.len(), 1);
        assert!(whole[0].text.chars().count() <= SHOWN + 2, "{} characters shown", whole[0].text.chars().count());
        assert_eq!(whole[0].len, 100_000, "the place itself keeps its length, for the editor");
    }

    #[test]
    fn a_place_is_also_counted_the_way_the_editor_counts() {
        let s = spots_on("\u{1F600} needle", 1, &matcher(&ask("needle")).unwrap(), 10);
        assert_eq!((s[0].col, s[0].col16), (3, 4), "a face is one character, and two for the editor");
    }

    #[test]
    fn file_patterns_are_written_for_another_machine_the_same_way() {
        let re = posix_paths("src/**, *.min.js").unwrap();
        assert!(re.contains("^(\\./)?src/.*(/|$)") || re.contains("^(\\./)?src/(.*/)?"), "{re}");
        assert!(re.contains("(^|/)[^/]*\\.min\\.js(/|$)"), "{re}");
        assert!(posix_paths(" , ").is_none());
        let cmd = far::spots_command("/w", &Ask { include: "src/**".into(), exclude: "dist".into(), ..ask("x") }, 10);
        assert!(cmd.contains("grep -z -E") && cmd.contains("grep -zv -E") && cmd.contains("|| exit 3"), "{cmd}");
        assert!(cmd.contains("head -c"), "what comes back is bounded in bytes: {cmd}");
    }

    #[test]
    fn a_file_too_big_to_open_is_said_rather_than_lost() {
        let dir = std::env::temp_dir().join(format!("sk-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut big = String::from("needle\n");
        big.push_str(&"x".repeat(READ_LIMIT as usize));
        std::fs::write(dir.join("big.txt"), big).unwrap();
        let found = search(&dir, &ask("needle"), &|| false).unwrap();
        assert_eq!((found.total, found.too_big), (0, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_is_searched_through_every_match_and_pattern() {
        let dir = std::env::temp_dir().join(format!("sk-spots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("skip")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "one hit\nhit two hit\n").unwrap();
        std::fs::write(dir.join("skip/b.rs"), "hit\n").unwrap();
        std::fs::write(dir.join("c.bin"), b"hit\0\0").unwrap();
        let found = search(&dir, &Ask { exclude: "skip".into(), ..ask("hit") }, &|| false).unwrap();
        assert_eq!(found.files.len(), 1, "{found:?}");
        assert_eq!(found.files[0].path, "src/a.rs");
        assert_eq!(found.total, 3, "every match, not the first of each file");
        assert!(!found.capped);
        let stopped = search(&dir, &ask("hit"), &|| true).unwrap();
        assert!(stopped.capped && stopped.total == 0, "a search no longer wanted stops");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_few_letters_find_the_file_meant() {
        let paths: Vec<String> = ["src/runtime.rs", "src/routine.rs", "docs/readme.md", "crates/core/src/files.rs", "src/run_time_tests.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(best_fits(&paths, "routine", 3)[0], "src/routine.rs");
        assert_eq!(best_fits(&paths, "runtime", 3)[0], "src/runtime.rs", "whole in the name beats letters spread out");
        assert_eq!(best_fits(&paths, "core files", 3), vec!["crates/core/src/files.rs".to_string()], "every word has to fit");
        assert_eq!(best_fits(&paths, "rtt", 5)[0], "src/run_time_tests.rs", "letters at the starts of words");
        assert!(best_fits(&paths, "zzz", 5).is_empty());
    }

    #[test]
    fn what_another_machine_found_reads_as_places() {
        let out = "./src/a.rs\x003:let hit = 1; // hit\nsrc/b.md\x0010:no match here for the words\nskip/c.rs\x001:hit\n";
        let s = far::spots(out, &Ask { exclude: "skip".into(), ..ask("hit") }, 100).unwrap();
        assert_eq!(s.files.len(), 1);
        assert_eq!(s.files[0].path, "src/a.rs");
        assert_eq!(s.total, 2);
        assert_eq!((s.files[0].spots[0].line, s.files[0].spots[0].col), (3, 5));
        let cut = far::spots(out, &ask("hit"), 1).unwrap();
        assert!(cut.capped, "a list that went past the limit says so");
        let cmd = far::spots_command("/w", &Ask { word: true, regex: true, case: true, ..ask("a b") }, 10);
        assert!(cmd.contains(" -w") && cmd.contains(" $R ") && cmd.contains("R=-P") && !cmd.contains(" -i") && cmd.contains("head -n 11"), "{cmd}");
    }

    /// A new name stays in the folder the file is in, and a name that would
    /// put it somewhere else is not a name
    #[test]
    fn a_new_name_is_one_name_in_the_same_folder() {
        assert_eq!(beside("src/a.rs", "b.rs").as_deref(), Some("src/b.rs"));
        assert_eq!(beside("a.rs", "b.rs").as_deref(), Some("b.rs"));
        assert_eq!(beside("src\\deep\\a.rs", " c.rs ").as_deref(), Some("src/deep/c.rs"));
        for bad in ["", " ", ".", "..", "x/y", "..\\up", "a\nb"] {
            assert_eq!(beside("src/a.rs", bad), None, "{bad:?} is not one name");
            assert_eq!(inside("src", bad), None, "{bad:?} is not one name");
        }
        assert_eq!(inside("", "new").as_deref(), Some("new"), "a new folder at the root");
        assert_eq!(inside("src/deep/", "new").as_deref(), Some("src/deep/new"));
    }

    /// A folder is copied whole beside itself, never over a name already there
    /// and never into itself
    #[test]
    fn a_folder_is_copied_whole_and_never_over_or_into_itself() {
        let dir = std::env::temp_dir().join(format!("shikisha-copy-{}", crate::random_hex(4)));
        let from = dir.join("a");
        std::fs::create_dir_all(from.join("in")).unwrap();
        std::fs::write(from.join("in").join("x.txt"), "x").unwrap();
        copy_tree(&from, &dir.join("b")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("b").join("in").join("x.txt")).unwrap(), "x");
        assert!(copy_tree(&from, &dir.join("b")).is_err(), "copied over a folder already there");
        assert!(copy_tree(&from, &from.join("in").join("again")).is_err(), "copied into itself");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file deleted from the list goes to the recycle bin, and one already
    /// gone is said as that. Ignored: it puts a file in the person's own
    /// recycle bin each time it runs
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn a_deleted_file_goes_to_the_recycle_bin() {
        let file = std::env::temp_dir().join(format!("shikisha-bin-{}.txt", crate::random_hex(4)));
        std::fs::write(&file, "x").unwrap();
        to_bin(&file).unwrap();
        assert!(!file.exists());
        assert!(to_bin(&file).is_err(), "a file already gone was binned");
    }

    /// The copy on another machine refuses a name already there with a code
    /// of its own, and every path goes quoted
    #[test]
    fn a_copy_there_names_a_taken_name_and_quotes_its_paths() {
        let c = far::copy_command("/w/it's", "/w/it's - Copy");
        assert!(c.contains(&format!("exit {}", far::TAKEN)));
        assert!(c.contains(r"cp -R -p -- '/w/it'\''s' '/w/it'\''s - Copy'"), "{c}");
    }

    /// A Shift_JIS CSV saved from the editor: back in Shift_JIS, byte for
    /// byte. A character it cannot hold stops the save and is named; written
    /// with question marks only when that is what was asked for
    #[test]
    fn a_file_is_saved_in_its_own_encoding_or_says_what_stops_it() {
        let sjis = encoding_rs::SHIFT_JIS;
        let text = "氏名,メモ\r\n山田,①済\r\n";
        assert_eq!(save_bytes(text, sjis, Save::Exact).unwrap(), crate::charset::write_as(text, sjis).unwrap());
        assert_eq!(save_bytes(text, encoding_rs::UTF_8, Save::Exact).unwrap(), text.as_bytes());

        let with_emoji = "山田,済😀👍😀\r\n";
        assert_eq!(save_bytes(with_emoji, sjis, Save::Exact).unwrap_err(), vec!['😀', '👍']);
        assert_eq!(
            save_bytes(with_emoji, sjis, Save::Replacing).unwrap(),
            crate::charset::write_as("山田,済???\r\n", sjis).unwrap()
        );
        assert_eq!(save_bytes(with_emoji, encoding_rs::UTF_8, Save::Exact).unwrap(), with_emoji.as_bytes());
    }

    /// A folder of this test's own. The process id is in the path because two
    /// test runs on one machine (another window, a watch loop) otherwise share
    /// it and delete each other's files mid-assertion.
    fn scratch(name: &str) -> PathBuf {
        let at = crate::test_temp("files-test").join(name);
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
            "found by part of the name, and regardless of case"
        );
        assert!(!found.capped);
    }

    #[test]
    fn text_comes_back_with_the_line_it_is_on() {
        let at = scratch("bytext");
        std::fs::write(at.join("a.txt"), "one\ntwo NEEDLE two\nthree\n").unwrap();
        std::fs::write(at.join("b.txt"), "nothing here").unwrap();
        let found = by_text(&at, "needle", 50);
        assert_eq!(found.hits.len(), 1, "only one hit by contents");
        let hit = &found.hits[0];
        assert_eq!(hit.path, "a.txt");
        assert_eq!(hit.line, Some(2), "line numbers count from 1");
        assert_eq!(hit.text.as_deref(), Some("two NEEDLE two"));
    }

    #[test]
    fn one_file_answers_once() {
        let at = scratch("once");
        std::fs::write(at.join("a.txt"), "hit\nhit\nhit\n").unwrap();
        assert_eq!(by_text(&at, "hit", 50).hits.len(), 1, "one line per file");
    }

    #[test]
    fn nothing_is_searched_for_an_empty_box() {
        let at = scratch("empty");
        std::fs::write(at.join("a.txt"), "anything").unwrap();
        assert!(by_name(&at, "   ", 50).hits.is_empty(), "an empty search is not everything");
        assert!(by_text(&at, "", 50).hits.is_empty());
    }

    #[test]
    fn a_long_line_is_cut_around_the_match() {
        let line = format!("{}NEEDLE{}", "x".repeat(400), "y".repeat(400));
        let shown = around(&line, "needle");
        assert!(shown.chars().count() < 200, "a long line is not shown as it is");
        assert!(shown.contains("NEEDLE"), "the hit is still there after cutting");
        assert!(shown.starts_with('…') && shown.ends_with('…'), "the side that was cut is marked");
    }

    #[test]
    fn the_mark_answers_one_question() {
        assert_eq!(mark_of(b"hello"), mark_of(b"hello"), "the same contents give the same mark");
        assert_ne!(mark_of(b"hello"), mark_of(b"hellp"), "one character different means different");
        assert_ne!(mark_of(b"hello"), mark_of(b"hello "), "a different length means different");
        assert_ne!(mark_of(b""), mark_of(b"x"));
    }

    #[test]
    fn a_stamp_moves_when_the_file_does() {
        let at = scratch("stamp").join("a.txt");
        std::fs::write(&at, "one").unwrap();
        let first = stamp_of(&at);
        assert!(!first.is_empty(), "a file that is there gets a mark");
        std::fs::write(&at, "one and a half").unwrap();
        assert_ne!(stamp_of(&at), first, "when the length changes the mark changes too");
        assert!(stamp_of(&at.with_file_name("nothing")).is_empty(), "what is not there has no mark");
    }

    /// What another machine prints for a search by name reads as the same
    /// answer a folder here gives: the `./` that `find` puts in front is not
    /// part of the name, and a name git and `find` both gave is one file
    #[test]
    fn a_far_listing_is_read_as_the_files_here_are() {
        let out = "./src/Payments.rs\0readme.md\0src/Payments.rs\0";
        let found = far::names(out, "payment", 50);
        assert_eq!(found.hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(), vec!["src/Payments.rs"]);
        assert!(!found.capped);
        assert!(far::names(out, "  ", 50).hits.is_empty(), "an empty search is not everything");
    }

    /// A search by what is inside, from another machine: the path, the line it
    /// is on and the line, and a list longer than asked for says it went on
    #[test]
    fn a_far_search_by_text_comes_back_with_its_lines() {
        let out = "./a.txt\x002:two NEEDLE: two\nb.txt\x0010:needle\n";
        let found = far::text(out, "needle", 50);
        assert_eq!(found.hits.len(), 2);
        assert_eq!(found.hits[0].path, "a.txt");
        assert_eq!(found.hits[0].line, Some(2));
        assert_eq!(found.hits[0].text.as_deref(), Some("two NEEDLE: two"), "a colon in the line stays in it");
        assert!(!found.capped);
        let one = far::text(out, "needle", 1);
        assert_eq!(one.hits.len(), 1);
        assert!(one.capped, "the second line was there, so the list was cut");
    }

    /// What a person typed reaches the far shell as text and nothing else: a
    /// `$( )` or a quote in the search box must not become a command there
    #[test]
    fn a_far_search_cannot_be_made_into_a_command() {
        let line = far::text_command("/home/user/it's", "$(rm -rf ~); 'x'", 300);
        assert!(line.starts_with("cd '/home/user/it'\\''s' && "), "{line}");
        assert!(line.contains("-e '$(rm -rf ~); '\\''x'\\''' --"), "{line}");
        assert!(line.ends_with("| head -n 301"), "{line}");
        assert!(far::names_command("/srv/p").starts_with("cd '/srv/p' || exit 3; "));
    }

    #[test]
    fn a_binary_file_is_not_read_as_lines() {
        let at = scratch("binary");
        std::fs::write(at.join("a.bin"), b"pre\0NEEDLE\0post").unwrap();
        assert!(by_text(&at, "needle", 50).hits.is_empty(), "binary files are not searched by contents");
    }
}

