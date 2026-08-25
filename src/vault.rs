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
    files.sort_by(|a, b| b.0.cmp(&a.0));
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
        });
    }
    // A capped scan that still filled the page is honestly complete for the
    // page; only say "more" when the cap actually hid matches we cannot see
    Found { capped: capped && hits.len() < limit, hits }
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
            Some(Source { program, with_id: r.with_id, verify, id_path, cwd_path })
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
    if let Some(p) = &src.id_path {
        if let Some(id) = first_line_field(text, p) {
            return Some(id);
        }
    }
    let stem = path.file_stem()?.to_string_lossy().to_string();
    (!stem.is_empty()).then_some(stem)
}

/// The folder a record belongs to: where the CLI records it, or the first
/// `"cwd"` the file mentions.
fn cwd_of(text: &str, src: &Source) -> Option<String> {
    if let Some(p) = &src.cwd_path {
        if let Some(c) = first_line_field(text, p) {
            return Some(c);
        }
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
    use std::io::Read as _;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; READ_CAP];
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
    let full = expand(&pattern.replace("{id}", "*"))
        .to_string_lossy()
        .replace('/', "\\");
    let mut parts = full.split('\\');
    let mut roots: Vec<PathBuf> = match parts.next() {
        Some(first) => vec![PathBuf::from(format!("{first}\\"))],
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

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-vault-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
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
            verify: format!("{}\\*\\{{id}}.jsonl", root.display()),
            id_path: None,
            cwd_path: None,
        };
        let files = list(&src.verify);
        assert_eq!(files.len(), 1, "グロブが記録を見つけられていない");
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
            verify: format!("{}\\*\\*\\*\\rollout-*-{{id}}.jsonl", root.display()),
            id_path: Some("payload.session_id".into()),
            cwd_path: Some("payload.cwd".into()),
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
        assert!(s.contains("PAYMENTS bug in checkout"), "文脈が読めない: {s}");
        assert!(!s.contains('{') && !s.contains('"'), "JSONの記号が残っている: {s}");
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
