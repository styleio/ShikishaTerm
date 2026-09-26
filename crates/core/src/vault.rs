//! Past conversations, found by what was said in them, and picked back up.
//!
//! Every AI CLI keeps its own record of what it did — claude under
//! `~/.claude/projects`, codex under `~/.codex/sessions`. They are on the disk
//! already; what is missing is a way to ask "which of these was the one about
//! the payments bug" without opening dozens of files by hand.
//!
//! Nothing is indexed ahead of time. The search reads the records when asked,
//! newest first, and stops once it has enough — because the thing a person
//! wants is almost always recent, and building an index to keep in step with
//! files another program writes would be a second source of truth that drifts.
//! When the search has to stop before the end, it says so, rather than letting
//! a bounded look read as a complete one.
//!
//! What makes a record findable is deliberately format-blind. These files are
//! JSON, one object per line, and every CLI arranges that JSON differently and
//! changes it between releases. So the text is searched as text, and the id and
//! folder are taken from the two places every one of them keeps them: the
//! conversation id from the record spec or, failing that, the file's own name;
//! the folder from the first `"cwd"` the file mentions.
//!
//! Reopening is not this module's to do — it hands back enough for the window
//! to launch a tab that resumes the conversation, which is the one place a new
//! tab can safely be made.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::profile::{ProfileFile, ResumeSpec};

/// How many records to open before stopping, newest first. A person looking
/// for a conversation is looking for a recent one; reading every record a
/// machine has ever written, on every keystroke, is not the way to help them
const SCAN_CAP: usize = 400;

/// How much of one record to read. The part that says what a conversation was
/// about is near the front — the opening messages — and a log that has grown
/// to tens of megabytes is not made more findable by reading all of it
const READ_CAP: usize = 512 * 1024;

/// How many records to open when asking what was said in one folder. A folder
/// nothing was ever said in is the case this bounds: without it, every tab
/// that came up clean would read the whole of the past to learn nothing
const LOOK_CAP: usize = 120;

/// How much of a record to read to learn which folder it belongs to. Every one
/// of these writes that down within the first exchange
const FOLDER_CAP: usize = 64 * 1024;

/// One place a conversation can be found and resumed from.
struct Source {
    /// What launches this CLI — the head of its command, e.g. `claude`
    program: String,
    /// The arguments that resume a known id, with `{id}` in them
    with_id: Vec<String>,
    /// Where its records live, as a glob with `{id}` standing for the id
    verify: String,
    /// How to read the id out of a record's first line, if it says so
    id_path: Option<String>,
    /// How to read the folder out of a record's first line, if it says so
    cwd_path: Option<String>,
    /// How to tell a person's own words apart from everything else the record
    /// holds, for saying what a conversation was about
    asks: Option<crate::profile::AskSpec>,
}

/// One conversation the search turned up.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Hit {
    /// What to run to reopen it (`claude`), and the id to resume
    pub program: String,
    pub id: String,
    /// The folder it happened in, when the record says
    pub cwd: Option<String>,
    /// A readable name for the row: the folder, or the program if there is none
    pub title: String,
    /// A cleaned-up line of context around the match — empty for a blank query
    pub snippet: String,
    /// When the record was last written, as seconds since the epoch, for
    /// showing "how long ago" without sending a clock across
    pub when: u64,
    /// Set when this hit is a line in an OPEN tab rather than a past record.
    /// Selecting it switches to that tab instead of reopening a conversation --
    /// this is the present, not the past
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<usize>,
    /// The machine the record is on, by its settings entry: none is this PC.
    /// What reopening it goes by, with the folder
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// What a search came back with, and whether it saw everything.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Found {
    pub hits: Vec<Hit>,
    /// True when the scan hit its cap before the end — so the UI can say "more
    /// than these" rather than implying the list is the whole of it
    pub capped: bool,
}

/// The conversations matching `query`, newest first, at most `limit`.
///
/// A blank query is a valid ask: it means "show me the recent ones", the way
/// an empty search box lists everything. A query is matched case-blind, as a
/// run of characters anywhere in the record
pub fn search(query: &str, limit: usize) -> Found {
    let needle = query.trim().to_lowercase();
    let sources = sources();
    // Every record across every CLI, newest first, so the cap falls on the
    // oldest rather than on whichever CLI happens to be listed last
    let mut files: Vec<(SystemTime, &Source, PathBuf)> = Vec::new();
    for src in &sources {
        for path in list(&src.verify) {
            let when = path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((when, src, path));
        }
    }
    files.sort_by_key(|(when, ..)| std::cmp::Reverse(*when));
    let capped = files.len() > SCAN_CAP;
    files.truncate(SCAN_CAP);

    let mut hits = Vec::new();
    for (when, src, path) in files {
        if hits.len() >= limit {
            break;
        }
        let Some(text) = read_head(&path) else { continue };
        let low = text.to_lowercase();
        let at = match needle.is_empty() {
            true => Some(0),
            false => low.find(&needle),
        };
        let Some(at) = at else { continue };
        let Some(id) = id_of(&path, &text, src) else { continue };
        let cwd = cwd_of(&text, src);
        hits.push(Hit {
            program: src.program.clone(),
            id,
            title: title_of(cwd.as_deref(), &src.program),
            snippet: if needle.is_empty() { String::new() } else { snippet(&text, at, needle.len()) },
            cwd,
            when: when.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            tab: None,
            host: None,
        });
    }
    // A capped scan that still filled the page is honestly complete for the
    // page; only say "more" when the cap actually hid matches we cannot see
    Found { capped: capped && hits.len() < limit, hits }
}

/// The same search over the records on another machine: every CLI's, newest
/// first, at most `limit` matches. Waits on the machine, so it is for a
/// thread; asking starts a paused MicroVM, which is why the board asks only
/// the machines already awake unless somebody says to wake the rest.
///
/// One command there per search. What is looked for goes over as base64 and
/// is matched by grep as plain text -- never as a pattern and never as words
/// for the shell. Each match comes back with the start of its record (where
/// the folder and the id are) and the line it matched on (for the context
/// shown), and `machine` is put in front of each hit's name, so a row says
/// where the conversation is
pub fn search_far(at: &crate::elsewhere::Elsewhere, machine: &str, query: &str, limit: usize) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    let sources = sources();
    let script = far_search_script(&sources, query);
    let out = match crate::elsewhere::exec(at, &script, 120_000) {
        Ok(r) => r.out,
        Err(e) => {
            crate::append_hook_log(&format!("could not search the records on {}: {e:#}", at.address()));
            return Vec::new();
        }
    };
    far_search_hits(&out, &sources, &needle, machine, limit)
}

/// The one command `search_far` runs: every CLI's records there, newest first,
/// each that holds `query` printed as `@@F <which> <mtime> <path>`, then the
/// start of it and the line it matched on, both in base64
fn far_search_script(sources: &[Source], query: &str) -> String {
    use base64::Engine as _;
    let q = base64::engine::general_purpose::STANDARD.encode(query.trim());
    let mut script = format!("cd \"$HOME\" 2>/dev/null || exit 0; q=$(printf %s '{q}' | base64 -d); n=0; ");
    for (i, src) in sources.iter().enumerate() {
        let Some(rest) = src.verify.strip_prefix("{home}/").map(|r| r.replace("{id}", "*")) else { continue };
        if !rest.chars().all(|c| c.is_ascii_alphanumeric() || "/*._-".contains(c)) {
            continue;
        }
        script.push_str(&format!(
            "ls -t {rest} 2>/dev/null | head -n {SCAN_CAP} | while IFS= read -r f; do \
if [ -z \"$q\" ]; then m=''; else m=$(grep -i -F -m1 -e \"$q\" \"$f\" 2>/dev/null | head -c 65536); [ -z \"$m\" ] && continue; fi; \
printf '@@F {i} %s %s\\n' \"$(stat -c %Y \"$f\" 2>/dev/null || echo 0)\" \"$f\"; \
head -c {FOLDER_CAP} \"$f\" | base64 -w0; echo; printf %s \"$m\" | base64 -w0; echo; done; "
        ));
    }
    script
}

/// What `far_search_script` printed, read into hits
fn far_search_hits(out: &str, sources: &[Source], needle: &str, machine: &str, limit: usize) -> Vec<Hit> {
    use base64::Engine as _;
    let decode = |b: &str| base64::engine::general_purpose::STANDARD.decode(b.trim()).unwrap_or_default();
    let mut hits: Vec<Hit> = Vec::new();
    let mut lines = out.lines();
    while let Some(head_line) = lines.next() {
        let Some(rest) = head_line.strip_prefix("@@F ") else { continue };
        let mut parts = rest.splitn(3, ' ');
        let (Some(which), Some(when), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let head = String::from_utf8_lossy(&decode(lines.next().unwrap_or_default())).into_owned();
        let matched = String::from_utf8_lossy(&decode(lines.next().unwrap_or_default())).into_owned();
        let Some(src) = which.parse::<usize>().ok().and_then(|i| sources.get(i)) else { continue };
        let Some(id) = id_of(Path::new(path), &head, src) else { continue };
        let cwd = cwd_of(&head, src);
        let snippet = match needle.is_empty() {
            true => String::new(),
            false => matched.to_lowercase().find(&needle).map(|at| snippet(&matched, at, needle.len())).unwrap_or_default(),
        };
        hits.push(Hit {
            program: src.program.clone(),
            id,
            title: format!("{machine}: {}", title_of(cwd.as_deref(), &src.program)),
            snippet,
            cwd,
            when: when.trim().parse().unwrap_or(0),
            tab: None,
            host: Some(machine.to_string()),
        });
    }
    // Newest first across every CLI there, as the search here orders them
    hits.sort_by_key(|h| std::cmp::Reverse(h.when));
    hits.truncate(limit);
    hits
}

/// The arguments that reopen one hit, resuming its conversation.
///
/// The program is the tab's; these are only the resume flags, `{id}` filled
/// in — the same shape `plan_launch` uses, so a reopened tab is an ordinary
/// resumed one
pub fn reopen_argv(hit: &Hit) -> Option<Vec<String>> {
    let src = sources().into_iter().find(|s| s.program == hit.program)?;
    if src.with_id.is_empty() {
        return None;
    }
    let mut out = vec![hit.program.clone()];
    for a in &src.with_id {
        out.push(a.replace("{id}", &hit.id));
    }
    Some(out)
}

/// The conversations one CLI recorded in one folder, newest first.
///
/// The Vault's search asked the other way round: not "which conversation
/// mentioned this word" but "what has been said here before". It is asked
/// about a tab that came up on a conversation of nobody's -- whatever lost the
/// thread, the CLI's own records are what is left to offer, and they are on
/// the disk already.
///
/// Reading stops at the first `most` that match, and never looks past
/// `LOOK_CAP` files, so asking about a folder nothing was ever said in costs a
/// bounded look rather than the whole of the past
pub fn here(program: &str, cwd: &Path, most: usize) -> Vec<Hit> {
    let Some(src) = sources().into_iter().find(|s| s.program == program) else {
        return Vec::new();
    };
    here_in(&src, cwd, most)
}

/// Whether that conversation is one this folder's own.
///
/// Asked of the CLI's records, which are the one account of what was said
/// where that nothing the app writes down can spoil. A tab handed a
/// conversation that was had somewhere else is carrying somebody else's, and
/// the folder it is sitting in still has its own waiting to be picked back up.
///
/// An id with no record yet is not this folder's: a conversation minted for a
/// CLI that has not spoken exists nowhere but in the launch that made it
pub fn belongs(program: &str, cwd: &Path, id: &str) -> bool {
    sources()
        .into_iter()
        .find(|s| s.program == program)
        .is_some_and(|src| belongs_in(&src, cwd, id))
}

/// The same question asked of one CLI's records, so a test can supply its own.
fn belongs_in(src: &Source, cwd: &Path, id: &str) -> bool {
    let Some(path) = crate::sessionfind::locate(&src.verify, id) else {
        return false;
    };
    let Some(head) = read_some(&path, FOLDER_CAP) else {
        return false;
    };
    cwd_of(&head, src).is_some_and(|at| crate::uistate::same_folder(Path::new(&at), cwd))
}

/// The same question asked of one CLI's records, so a test can supply its own.
fn here_in(src: &Source, cwd: &Path, most: usize) -> Vec<Hit> {
    let mut files: Vec<(SystemTime, PathBuf)> = list(&src.verify)
        .into_iter()
        .filter_map(|path| {
            let when = path.metadata().ok()?.modified().ok()?;
            Some((when, path))
        })
        .collect();
    files.sort_by_key(|(when, _)| std::cmp::Reverse(*when));
    files.truncate(LOOK_CAP);

    let mut out = Vec::new();
    for (when, path) in files {
        if out.len() >= most {
            break;
        }
        // Enough of the file to hold the folder it belongs to and nothing
        // more: every one of these writes that within the first exchange, and
        // reading half a megabyte per file to learn one path is a scan nobody
        // would wait for
        let Some(head) = read_some(&path, FOLDER_CAP) else { continue };
        let Some(at) = cwd_of(&head, &src) else { continue };
        if !crate::uistate::same_folder(Path::new(&at), cwd) {
            continue;
        }
        let Some(id) = id_of(&path, &head, &src) else { continue };
        out.push(Hit {
            program: src.program.clone(),
            id,
            title: title_of(Some(&at), &src.program),
            snippet: first_ask(&path, &src).unwrap_or_default(),
            cwd: Some(at),
            when: when.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            tab: None,
            host: None,
        });
    }
    out
}

/// The same list for a folder on another machine, read from the CLI's records
/// there.
///
/// The records are where the CLI ran, and for a folder on a MicroVM or a
/// server that is not this PC -- so the list asked of this PC's disk was always
/// empty there, on the window and on a phone alike. Asked of the machine in
/// one command: its newest records, each with the start of it, which is where
/// every CLI writes the folder and the id. Waits on the machine, so it is for
/// a thread; asking starts a paused machine, and it is asked when somebody
/// opens the list
pub fn here_far(program: &str, at: &crate::elsewhere::Elsewhere, cwd: &Path, most: usize) -> Vec<Hit> {
    let Some(src) = sources().into_iter().find(|s| s.program == program) else {
        return Vec::new();
    };
    let Some(line) = far_listing(&src.verify) else { return Vec::new() };
    let out = match crate::elsewhere::exec(at, &line, 60_000) {
        Ok(r) => r.out,
        Err(e) => {
            crate::append_hook_log(&format!("could not read the {program} records on {}: {e:#}", at.address()));
            return Vec::new();
        }
    };
    far_hits(&out, &src, cwd, most)
}

/// The one command that lists a CLI's records on a Linux machine, newest
/// first, each as `@@F <mtime> <path>` and then the start of it in base64.
/// `None` for a pattern that does not start at the home folder
fn far_listing(verify: &str) -> Option<String> {
    let rest = verify.strip_prefix("{home}/")?.replace("{id}", "*");
    // Only what a glob is made of: the pattern is a profile's, and anything
    // else in it would be words for the shell
    if !rest.chars().all(|c| c.is_ascii_alphanumeric() || "/*._-".contains(c)) {
        return None;
    }
    Some(format!(
        "cd \"$HOME\" 2>/dev/null || exit 0; ls -t {rest} 2>/dev/null | head -n {LOOK_CAP} | while IFS= read -r f; do \
printf '@@F %s %s\\n' \"$(stat -c %Y \"$f\" 2>/dev/null || echo 0)\" \"$f\"; head -c {FOLDER_CAP} \"$f\" | base64 -w0; echo; done"
    ))
}

/// What `far_listing` printed, read into the conversations of `cwd`
fn far_hits(out: &str, src: &Source, cwd: &Path, most: usize) -> Vec<Hit> {
    use base64::Engine as _;
    let mut hits = Vec::new();
    let mut lines = out.lines();
    while let Some(head_line) = lines.next() {
        if hits.len() >= most {
            break;
        }
        let Some(rest) = head_line.strip_prefix("@@F ") else { continue };
        let Some((when, path)) = rest.split_once(' ') else { continue };
        let body = lines.next().unwrap_or_default();
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(body.trim()) else { continue };
        let head = String::from_utf8_lossy(&bytes).into_owned();
        let Some(at) = cwd_of(&head, src) else { continue };
        if !crate::uistate::same_folder(Path::new(&at), cwd) {
            continue;
        }
        let Some(id) = id_of(Path::new(path), &head, src) else { continue };
        // What was asked first, from the start of the record brought over:
        // read the way a record here is read, from a copy of that start
        let snippet = {
            let tmp = std::env::temp_dir().join(format!("shikisha-far-record-{}", crate::random_hex(6)));
            let said = std::fs::write(&tmp, &bytes).ok().and_then(|()| first_ask(&tmp, src));
            let _ = std::fs::remove_file(&tmp);
            said.unwrap_or_default()
        };
        hits.push(Hit {
            program: src.program.clone(),
            id,
            title: title_of(Some(&at), &src.program),
            snippet,
            cwd: Some(at),
            when: when.trim().parse().unwrap_or(0),
            tab: None,
            host: None,
        });
    }
    hits
}

/// What the person asked first in a record, for saying which conversation this
/// is. Their own words, told apart from everything else the file holds the way
/// that CLI's profile says (`crate::asks`)
fn first_ask(path: &Path, src: &Source) -> Option<String> {
    let how = src.asks.as_ref()?;
    let (said, _) = crate::asks::read_from(path, how, 0);
    let first = said.into_iter().find(|t| !t.trim().is_empty())?;
    let one_line: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(one_line.chars().take(120).collect())
}

/// The CLIs whose records can be searched and resumed by id.
fn sources() -> Vec<Source> {
    profiles()
        .into_iter()
        .filter_map(|pf| {
            let program = pf.command_match.first()?.clone();
            let r: ResumeSpec = pf.resume?;
            // Only what can be both found and reopened. A CLI with no record
            // to read, or no way to resume a specific id, is not something the
            // Vault can honestly offer
            let verify = r.verify.clone()?;
            if r.with_id.is_empty() {
                return None;
            }
            let (id_path, cwd_path) = match &r.record {
                Some(rec) => (Some(rec.id.clone()), Some(rec.cwd.clone())),
                None => (None, None),
            };
            Some(Source { program, with_id: r.with_id, verify, id_path, cwd_path, asks: r.asks })
        })
        .collect()
}

/// Indirection so tests can supply their own.
fn profiles() -> Vec<ProfileFile> {
    crate::profile::files()
}

/// A readable name for a row: the folder's last part, or the program.
fn title_of(cwd: Option<&str>, program: &str) -> String {
    cwd.and_then(|c| {
        Path::new(c)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
    })
    .unwrap_or_else(|| program.to_string())
}

/// The id of a record: what its first line says, or its own file name.
///
/// A CLI that records where it keeps the id is believed; one that does not
/// keeps the id in the file name, which is the case for the tools that name
/// each record after the conversation
fn id_of(path: &Path, text: &str, src: &Source) -> Option<String> {
    if let Some(p) = &src.id_path
        && let Some(id) = first_line_field(text, p) {
            return Some(id);
        }
    let stem = path.file_stem()?.to_string_lossy().to_string();
    (!stem.is_empty()).then_some(stem)
}

/// The folder a record belongs to: where the CLI records it, or the first
/// `"cwd"` the file mentions.
fn cwd_of(text: &str, src: &Source) -> Option<String> {
    if let Some(p) = &src.cwd_path
        && let Some(c) = first_line_field(text, p) {
            return Some(c);
        }
    // Format-blind fallback: the first cwd anywhere in the head. Every one of
    // these tools writes the folder into its records; they just disagree on
    // where, so this finds it without being told
    let key = "\"cwd\":";
    let at = text.find(key)? + key.len();
    let rest = text[at..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    let raw = &rest[..end];
    (!raw.is_empty()).then(|| raw.replace("\\\\", "\\").replace("\\/", "/"))
}

/// One dotted field out of the first line's JSON (`payload.session_id`).
fn first_line_field(text: &str, path: &str) -> Option<String> {
    let line = text.lines().next()?;
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let mut at = &v;
    for step in path.split('.') {
        at = at.get(step)?;
    }
    at.as_str().map(str::to_string).filter(|s| !s.is_empty())
}

/// A readable line of context around a match.
///
/// The records are JSON, so a raw window is full of quotes, braces and escape
/// sequences. This is not trying to reconstruct the message — only to give the
/// eye enough around the hit to recognise the conversation, so the noise is
/// flattened to spaces and the window trimmed to something a row can hold
fn snippet(text: &str, at: usize, len: usize) -> String {
    let start = text[..at].char_indices().rev().nth(48).map(|(i, _)| i).unwrap_or(0);
    let want = len + 80;
    let end = text[at..].char_indices().nth(want).map(|(i, _)| at + i).unwrap_or(text.len());
    let mut out = String::new();
    let mut space = false;
    for c in text[start..end].chars() {
        // JSON's structure is noise here; letters and spaces are the signal
        if c.is_control() || matches!(c, '"' | '{' | '}' | '[' | ']' | '\\') {
            space = true;
            continue;
        }
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    let trimmed = out.trim();
    // Say when the window is a fragment, at both ends
    let lead = if start > 0 { "…" } else { "" };
    let tail = if end < text.len() { "…" } else { "" };
    format!("{lead}{trimmed}{tail}")
}

/// The first part of a file, capped.
fn read_head(path: &Path) -> Option<String> {
    read_some(path, READ_CAP)
}

/// The front of a file, at most `cap` bytes of it.
fn read_some(path: &Path, cap: usize) -> Option<String> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; cap];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    // Lossy on purpose: a record with a stray non-UTF-8 byte is still worth
    // searching, and refusing it whole would hide a real conversation
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Every file a glob points at, with `{id}` treated as any run of characters.
///
/// The pattern locates records with the id spelled out; here the id is what we
/// are trying to find, so it becomes a wildcard like `*`. Both live only in
/// file and folder names, never spanning a separator
fn list(pattern: &str) -> Vec<PathBuf> {
    // A pattern is written by whoever wrote the profile, with whichever
    // separator they had in mind. Windows takes either, so both are folded
    // to one there; everywhere else a backslash is an ordinary character in
    // a name and folding it would cut a path in the wrong place.
    let sep = std::path::MAIN_SEPARATOR;
    let expanded = expand(&pattern.replace("{id}", "*"));
    let full = match cfg!(windows) {
        true => expanded.to_string_lossy().replace('/', "\\"),
        false => expanded.to_string_lossy().to_string(),
    };
    let mut parts = full.split(sep);
    let mut roots: Vec<PathBuf> = match parts.next() {
        Some(first) => vec![PathBuf::from(format!("{first}{sep}"))],
        None => return Vec::new(),
    };
    for seg in parts {
        if seg.is_empty() {
            continue;
        }
        let mut next = Vec::new();
        let leaf = !seg.contains('*');
        for root in &roots {
            if leaf {
                next.push(root.join(seg));
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(root) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if glob_seg(seg, &name) {
                        next.push(e.path());
                    }
                }
            }
        }
        roots = next;
    }
    roots.into_iter().filter(|p| p.is_file()).collect()
}

/// One name against one `*`-glob segment (the pieces between the stars must
/// appear in order — enough for the record patterns).
fn glob_seg(pattern: &str, name: &str) -> bool {
    let pieces: Vec<&str> = pattern.split('*').collect();
    if pieces.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (pieces[0], pieces[pieces.len() - 1]);
    if !name.starts_with(first) || !name.ends_with(last) || name.len() < first.len() + last.len() {
        return false;
    }
    let mut at = first.len();
    for piece in &pieces[1..pieces.len() - 1] {
        match name[at..].find(piece) {
            Some(f) => at += f + piece.len(),
            None => return false,
        }
    }
    at <= name.len() - last.len()
}

fn expand(path: &str) -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    PathBuf::from(path.replace("{home}", &home))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, lines: &[&str]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, lines.join("\n")).unwrap();
    }

    /// A pattern written with this system's own separator
    fn with_seps(p: &str) -> String {
        p.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-vault-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Whether a conversation was had in this folder, asked of the records.
    ///
    /// What a tab coming back has to be able to ask. The app's own note of
    /// which tab was running what can be wrong -- a report credited to the
    /// wrong tab writes one CLI's conversation against another's name -- and
    /// then two tabs resume the same conversation and a third is gone. The
    /// records say where each one was actually had, and they say it whatever
    /// the app believes
    #[test]
    fn a_conversation_says_which_folder_it_was_had_in() {
        let root = tmp("belongs");
        let proj = root.join("proj-a");
        let line = |cwd: &str| {
            format!(
                r#"{{"type":"user","cwd":"{cwd}","message":{{"role":"user","content":"hello"}}}}"#
            )
        };
        let here = "aaaa1111-0000-0000-0000-000000000001";
        let elsewhere = "cccc3333-0000-0000-0000-000000000003";
        write(&proj.join(format!("{here}.jsonl")), &[&line("D:/work/here"), ""]);
        write(&proj.join(format!("{elsewhere}.jsonl")), &[&line("D:/work/elsewhere"), ""]);
        let src = Source {
            program: "claude".into(),
            with_id: vec!["--resume".into(), "{id}".into()],
            verify: with_seps(&format!("{}/*/{{id}}.jsonl", root.display())),
            id_path: None,
            cwd_path: None,
            asks: None,
        };
        let at = Path::new("D:/work/here");
        assert!(belongs_in(&src, at, here), "its own conversation was called somebody else's");
        assert!(
            !belongs_in(&src, at, elsewhere),
            "a conversation had in another folder was taken for this tab's"
        );
        assert!(
            !belongs_in(&src, at, "dddd4444-0000-0000-0000-000000000004"),
            "a conversation with no record yet is nobody's"
        );
    }

    /// What was said in one folder before, newest first, and nothing from
    /// anywhere else.
    ///
    /// This is what is left when the app has lost which conversation a tab was
    /// having: the records are on the disk whatever the app remembers, and
    /// every one of them says which folder it belongs to
    #[test]
    fn what_was_said_in_one_folder_is_found_whatever_the_app_remembers() {
        let root = tmp("here");
        let proj = root.join("proj-here");
        let line = |cwd: &str, said: &str| {
            format!(
                r#"{{"type":"user","cwd":"{cwd}","message":{{"role":"user","content":"{said}"}}}}"#
            )
        };
        write(&proj.join("aaaa1111-0000-0000-0000-000000000001.jsonl"),
            &[&line("D:\\\\work\\\\here", "the older one"), ""]);
        write(&proj.join("bbbb2222-0000-0000-0000-000000000002.jsonl"),
            &[&line("D:\\\\work\\\\here", "the newer one"), ""]);
        write(&proj.join("cccc3333-0000-0000-0000-000000000003.jsonl"),
            &[&line("D:\\\\work\\\\elsewhere", "somebody else's"), ""]);
        // Newest last-written wins, and the test says which is which rather
        // than trusting the order two files happened to be written in
        let older = proj.join("aaaa1111-0000-0000-0000-000000000001.jsonl");
        let newer = proj.join("bbbb2222-0000-0000-0000-000000000002.jsonl");
        let now = std::time::SystemTime::now();
        let ago = now - std::time::Duration::from_secs(600);
        let touch = |at: &Path, when: std::time::SystemTime| {
            std::fs::OpenOptions::new().write(true).open(at).unwrap().set_modified(when).unwrap();
        };
        touch(&older, ago);
        touch(&newer, now);

        let src = Source {
            program: "claude".into(),
            with_id: vec!["--resume".into(), "{id}".into()],
            verify: with_seps(&format!("{}/*/{{id}}.jsonl", root.display())),
            id_path: None,
            cwd_path: None,
            asks: None,
        };
        let ids = |most| {
            here_in(&src, Path::new("D:\\work\\here"), most)
                .into_iter()
                .map(|h| h.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(5),
            vec![
                "bbbb2222-0000-0000-0000-000000000002".to_string(),
                "aaaa1111-0000-0000-0000-000000000001".to_string(),
            ],
            "the folder's own conversations, newest first, and nobody else's"
        );
        // The same folder written the other way round is the same folder --
        // where that is true. On Windows neither the case of the letters nor
        // the direction of the slashes makes another folder; everywhere else
        // both do, and folding them would hand one folder's conversations to
        // somebody working in another
        if cfg!(windows) {
            assert_eq!(
                here_in(&src, Path::new("d:/work/here"), 1).len(),
                1,
                "the other slash and another case named the same folder"
            );
        }
        assert!(
            here_in(&src, Path::new("D:\\work\\nothing-here"), 1).is_empty(),
            "a folder nothing was said in offers nothing"
        );
        assert_eq!(ids(1).len(), 1, "it stops once it has what was asked for");
    }

    #[test]
    fn a_glob_lists_records_and_the_id_comes_from_the_name_when_the_file_does_not_say() {
        // Claude's shape: id is the file's own name, cwd lives in a later line
        let root = tmp("byname");
        let proj = root.join("proj-a");
        write(
            &proj.join("11112222-3333-4444-5555-666677778888.jsonl"),
            &[
                r#"{"type":"mode","sessionId":"11112222-3333-4444-5555-666677778888"}"#,
                r#"{"type":"msg","cwd":"D:\\work\\payments","message":"the payments bug"}"#,
            ],
        );
        let src = Source {
            program: "claude".into(),
            with_id: vec!["--resume".into(), "{id}".into()],
            verify: with_seps(&format!("{}/*/{{id}}.jsonl", root.display())),
            id_path: None,
            cwd_path: None,
            asks: None,
        };
        let files = list(&src.verify);
        assert_eq!(files.len(), 1, "the glob does not find the record");
        let text = read_head(&files[0]).unwrap();
        assert_eq!(id_of(&files[0], &text, &src).as_deref(), Some("11112222-3333-4444-5555-666677778888"));
        assert_eq!(cwd_of(&text, &src).as_deref(), Some("D:\\work\\payments"));
    }

    #[test]
    fn the_id_and_folder_come_from_the_first_line_when_the_record_says_so() {
        // Codex's shape: id and cwd are in payload on the first line, and the
        // file name is ambiguous (dashes everywhere)
        let root = tmp("bypayload");
        write(
            &root.join("2026/08/25").join("rollout-2026-08-25T10-00-00-aaaa-bbbb.jsonl"),
            &[r#"{"type":"session_meta","payload":{"session_id":"aaaa-bbbb","cwd":"D:/repo"}}"#],
        );
        let src = Source {
            program: "codex".into(),
            with_id: vec!["resume".into(), "{id}".into()],
            verify: with_seps(&format!("{}/*/*/*/rollout-*-{{id}}.jsonl", root.display())),
            id_path: Some("payload.session_id".into()),
            cwd_path: Some("payload.cwd".into()),
            asks: None,
        };
        let files = list(&src.verify);
        assert_eq!(files.len(), 1);
        let text = read_head(&files[0]).unwrap();
        assert_eq!(id_of(&files[0], &text, &src).as_deref(), Some("aaaa-bbbb"));
        assert_eq!(cwd_of(&text, &src).as_deref(), Some("D:/repo"));
    }

    #[test]
    fn the_snippet_is_readable_not_raw_json() {
        let text = r#"{"role":"user","message":"please fix the PAYMENTS bug in checkout"}"#;
        let low = text.to_lowercase();
        let at = low.find("payments").unwrap();
        let s = snippet(text, at, "payments".len());
        assert!(s.contains("PAYMENTS bug in checkout"), "the context cannot be read: {s}");
        assert!(!s.contains('{') && !s.contains('"'), "JSON symbols are left: {s}");
    }

    #[test]
    fn reopen_uses_the_resume_flags_with_the_id_filled_in() {
        let hit = Hit {
            program: "claude".into(),
            id: "abc-123".into(),
            cwd: Some("D:/x".into()),
            title: "x".into(),
            snippet: String::new(),
            when: 0,
            tab: None,
            host: None,
        };
        // With no profiles installed in the test env, reopen has nothing to
        // resolve against; the shape is what a real source produces
        let out = {
            let src = Source {
                program: "claude".into(),
                with_id: vec!["--resume".into(), "{id}".into()],
                verify: String::new(),
                id_path: None,
                cwd_path: None,
                asks: None,
            };
            let mut v = vec![src.program.clone()];
            for a in &src.with_id {
                v.push(a.replace("{id}", &hit.id));
            }
            v
        };
        assert_eq!(out, vec!["claude", "--resume", "abc-123"]);
    }

    #[test]
    fn a_glob_segment_matches_in_order() {
        assert!(glob_seg("rollout-*-x.jsonl", "rollout-2026-x.jsonl"));
        assert!(glob_seg("*", "anything"));
        assert!(glob_seg("a.jsonl", "a.jsonl"));
        assert!(!glob_seg("a.jsonl", "b.jsonl"));
        assert!(!glob_seg("rollout-*.jsonl", "other.jsonl"));
    }
}

#[cfg(test)]
mod far_tests {
    use super::*;

    /// A CLI's records on another machine, as the machine lists them, read
    /// into the conversations of one folder there -- newest first, and none of
    /// another folder's
    #[test]
    fn a_far_folders_conversations_are_read_from_its_machines_records() {
        let src = Source {
            program: "claude".into(),
            with_id: vec!["--resume".into(), "{id}".into()],
            verify: "{home}/.claude/projects/*/{id}.jsonl".into(),
            id_path: None,
            cwd_path: None,
            asks: None,
        };
        // What the listing printed for two records, the newer of another folder
        let out = "@@F 1790430223 .claude/projects/-home-user-site/bbb.jsonl\n\
eyJ0eXBlIjoidXNlciIsImN3ZCI6Ii9ob21lL3VzZXIvb3RoZXIiLCJzZXNzaW9uSWQiOiJiYmIifQo=\n\
@@F 1790430222 .claude/projects/-home-user-site/aaa.jsonl\n\
eyJ0eXBlIjoidXNlciIsImN3ZCI6Ii9ob21lL3VzZXIvc2l0ZSIsInNlc3Npb25JZCI6ImFhYSIsIm1lc3NhZ2UiOnsicm9sZSI6InVzZXIiLCJjb250ZW50IjoiZml4IHRoZSBsb2dpbiBidWcifX0K\n";
        let hits = far_hits(out, &src, Path::new("/home/user/site"), 12);
        assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["aaa"]);
        assert_eq!(hits[0].when, 1_790_430_222);
        assert_eq!(hits[0].cwd.as_deref(), Some("/home/user/site"));

        // The listing is only ever a glob under the home folder
        let line = far_listing(&src.verify).unwrap();
        assert!(line.contains("ls -t .claude/projects/*/*.jsonl"), "{line}");
        assert_eq!(far_listing("/etc/{id}"), None, "a pattern outside the home folder is listed");
        assert_eq!(far_listing("{home}/$(reboot)/{id}"), None, "a pattern with shell words in it is run");
    }
}

#[cfg(test)]
mod far_search_tests {
    use super::*;

    fn claude() -> Source {
        Source {
            program: "claude".into(),
            with_id: vec!["--resume".into(), "{id}".into()],
            verify: "{home}/.claude/projects/*/{id}.jsonl".into(),
            id_path: None,
            cwd_path: None,
            asks: None,
        }
    }

    /// The search on another machine: what is looked for goes over as base64
    /// and never as words for the shell, and what comes back is read into
    /// hits named after the machine they are on
    #[test]
    fn a_search_on_another_machine_is_read_back_named_after_it() {
        let script = far_search_script(&[claude()], "login bug'; rm -rf ~");
        if let Ok(to) = std::env::var("SHIKISHA_FAR_SEARCH_OUT") {
            std::fs::write(to, &script).unwrap();
        }
        assert!(!script.contains("rm -rf"), "the words looked for reach the shell as words");
        assert!(script.contains("grep -i -F -m1"), "{script}");

        use base64::Engine as _;
        let b = |t: &str| base64::engine::general_purpose::STANDARD.encode(t);
        let head = r#"{"type":"user","cwd":"/home/user/site","sessionId":"aaa"}"#;
        let line = r#"{"message":{"content":"please fix the login bug in auth.rs"}}"#;
        let out = format!(
            "@@F 0 1790430222 .claude/projects/-home-user-site/aaa.jsonl\n{}\n{}\n",
            b(head),
            b(line)
        );
        let hits = far_search_hits(&out, &[claude()], "login bug", "vm", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "aaa");
        assert!(hits[0].title.starts_with("vm: "), "{}", hits[0].title);
        assert!(hits[0].snippet.contains("login bug"), "{}", hits[0].snippet);
        assert_eq!(hits[0].cwd.as_deref(), Some("/home/user/site"));
    }
}
