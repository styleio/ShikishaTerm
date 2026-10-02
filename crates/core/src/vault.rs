//! Past conversations, found by what was said in them, and picked back up.
//!
//! Every AI CLI keeps its own record of what it did — claude under
//! `~/.claude/projects`, codex under `~/.codex/sessions`. They are on the disk
//! already; what is missing is a way to ask "which of these was the one about
//! the payments bug" without opening dozens of files by hand.
//!
//! Nothing is indexed ahead of time. The search reads the records when asked --
//! every one of them, all the way through, newest first -- and stops once it
//! has enough, because the thing a person wants is almost always recent, and
//! building an index to keep in step with files another program writes would
//! be a second source of truth that drifts. Reading all of it is affordable:
//! 348 records, 814 MB, took a quarter to half a second to look through
//! (2026-09-29, measured), because only the lines the bytes turn up are ever
//! parsed (`reader::mention`). When it stops at enough, it says so, rather than
//! letting the page read as the whole of the past.
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

/// How much of a record's start is read for what it says about itself -- its
/// id and folder -- when a blank search only lists the recent ones
const READ_CAP: usize = 512 * 1024;

/// How many records to open when asking what was said in one folder. A folder
/// nothing was ever said in is the case this bounds: without it, every tab
/// that came up clean would read the whole of the past to learn nothing
const LOOK_CAP: usize = 120;

/// How much of a record to read to learn which folder it belongs to. Every one
/// of these writes that down within the first exchange
const FOLDER_CAP: usize = 64 * 1024;

/// One place a conversation can be found and resumed from.
///
/// Sent to a bridge as it is: the profiles that describe a CLI are this PC's
/// files, and the records they point at are on the bridge's machine
#[derive(Clone, serde::Serialize, serde::Deserialize)]
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
    /// holds, for saying what a conversation was about. Not sent to a bridge:
    /// nothing it does for this PC asks it
    #[serde(skip)]
    asks: Option<crate::profile::AskSpec>,
}

/// One conversation the search turned up.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
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
    /// For a line in an open tab: how far back in its terminal the line is,
    /// counted from the newest (0) -- what brings it into sight when pressed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The machine the record is on, by its settings entry: none is this PC.
    /// What reopening it goes by, with the folder
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Where in its record the line holding what was searched for starts, as
    /// a byte offset: where a reader opened on it begins. Absent when that is
    /// not known -- a blank search, or a record searched line by line on a
    /// machine with no bridge
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// Who said the words it was found in and when, when they were something
    /// said: what `annotate` finds who really sent them by
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<Said>,
    /// The tab whose conversation this is, by uid, when this app's own AI
    /// tab had it: one tab's conversation can run across several records (a
    /// `/clear`, a conversation picked back up), and the list shows it once
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// What that tab is called, or was when it was last seen: what the list
    /// shows it as
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_name: Option<String>,
    /// Who really sent a person's line the words were found in: the person
    /// (and from where), another tab, a job, automation -- as the
    /// conversation panel says it (`convo::read::origins`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<serde_json::Value>,
    /// Whether something in it is pinned
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// Whether it was found in a note written on it rather than in the record
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub noted: bool,
}

/// Who said the words a search found, and how they begin
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Said {
    pub who: crate::reader::Who,
    /// When, in milliseconds, when the record says
    pub when: Option<i64>,
    /// How the words begin: what the sends a person's line came from are
    /// found by (`convo::db::head`)
    pub head: String,
}

impl Said {
    fn of(m: &crate::reader::Mention) -> Option<Self> {
        let (who, when) = m.said?;
        Some(Said { who, when, head: m.words.chars().take(200).collect() })
    }
}

/// What a search came back with, and whether it saw everything.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
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
/// run of characters in what was said or in what a tool was asked and gave
/// back -- not in a record's bookkeeping, where the folder a conversation ran
/// in would make every conversation in it a match
pub fn search(query: &str, limit: usize) -> Found {
    search_until(query, limit, &|| false)
}

/// The same, given up as soon as `stop` says so: a search typed over by a
/// newer one is nobody's any more, and reading on would only hold the newer
/// one up
pub fn search_until(query: &str, limit: usize, stop: &dyn Fn() -> bool) -> Found {
    search_in(&sources(), query, limit, stop)
}

/// The same search over the records of these CLIs, so a test can supply its own
fn search_in(sources: &[Source], query: &str, limit: usize, stop: &dyn Fn() -> bool) -> Found {
    let needle = query.trim().to_lowercase();
    // Every record across every CLI, newest first, so the cap falls on the
    // oldest rather than on whichever CLI happens to be listed last
    let mut files: Vec<(SystemTime, &Source, PathBuf)> = Vec::new();
    for src in sources {
        for path in list(&src.verify) {
            let when = path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            files.push((when, src, path));
        }
    }
    files.sort_by_key(|(when, ..)| std::cmp::Reverse(*when));

    let mut hits = Vec::new();
    let mut read = 0;
    for (when, src, path) in &files {
        if hits.len() >= limit || stop() {
            break;
        }
        read += 1;
        let mut line = None;
        let mut said = None;
        // A blank search lists; it has nothing to read the records through for
        let (head, snip) = match needle.is_empty() {
            true => match read_head(path) {
                Some(head) => (head, String::new()),
                None => continue,
            },
            false => {
                let Ok(bytes) = std::fs::read(path) else { continue };
                let Some(found) = crate::reader::mention(&bytes, &needle) else { continue };
                let head = String::from_utf8_lossy(&bytes[..bytes.len().min(READ_CAP)]).into_owned();
                line = Some(found.line);
                said = Said::of(&found);
                (head, snippet(&found.words, found.at, needle.len()))
            }
        };
        let Some(id) = id_of(path, &head, src) else { continue };
        let cwd = cwd_of(&head, src);
        hits.push(Hit {
            program: src.program.clone(),
            id,
            title: title_of(cwd.as_deref(), &src.program),
            snippet: snip,
            cwd,
            when: when.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            tab: None,
            host: None,
            at: line,
            said,
            ..Hit::default()
        });
    }
    // Stopped at enough with records left unread: there may be older ones
    Found { capped: hits.len() >= limit && read < files.len(), hits }
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
pub fn search_far(at: &crate::elsewhere::Elsewhere, machine: &str, query: &str, limit: usize) -> FarFound {
    let needle = query.trim().to_lowercase();
    let sources = sources();
    // Searched over there by the bridge, when the person put one there and its
    // line is up: every record read all the way through, and judged the way
    // this PC judges its own. Otherwise, and when the bridge is too old to
    // know the search, by remote commands as before
    if let Some(found) = by_bridge::<Found>(at, "vault_search", serde_json::json!({
        "sources": sources, "query": query, "limit": limit,
    })) {
        return FarFound {
            hits: found
                .hits
                .into_iter()
                .map(|h| Hit {
                    title: format!("{machine}: {}", h.title),
                    host: Some(machine.to_string()),
                    ..h
                })
                .collect(),
            capped: found.capped,
            failed: None,
        };
    }
    let out = match crate::elsewhere::exec(at, &far_search_script(&sources, query), 120_000) {
        Ok(r) => r.out,
        Err(e) => {
            crate::append_hook_log(&format!("could not search the records on {}: {e:#}", at.address()));
            return FarFound { failed: Some(format!("{e:#}")), ..FarFound::default() };
        }
    };
    let mut more = |again: &[Again]| match crate::elsewhere::exec(at, &far_more_script(query, again), 120_000) {
        Ok(r) => Some(r.out),
        Err(e) => {
            crate::append_hook_log(&format!("could not search further on {}: {e:#}", at.address()));
            None
        }
    };
    let (hits, capped) = far_search_hits(&out, &sources, &needle, machine, limit, &mut more);
    FarFound { hits, capped, failed: None }
}

/// What a record notes about itself on every line, whose values a search on
/// another machine takes out before it looks: the folder a conversation ran in
/// is on every one of its lines, and a word in it would make every line a
/// candidate to bring over. Only a narrowing -- whatever is left is still
/// judged here the way a record on this PC is (`reader::mention`), so a key a
/// CLI adds later costs lines carried over, never a wrong match
const FAR_BOOKKEEPING: &[&str] = &[
    "cwd", "gitBranch", "sessionId", "session_id", "uuid", "parentUuid", "leafUuid", "id",
    "timestamp", "version", "cli_version", "requestId", "slug", "userType", "entrypoint",
    "originator", "model",
];

/// How many candidate lines of one record are brought over at a time, and at
/// most how much of them. A record whose candidates were none of them words a
/// reader sees -- the AI's own thinking mentions a word often, and it is never
/// shown -- is asked for its next ones, up to `FAR_ROUNDS` times
const FAR_CANDIDATE_LINES: usize = 20;
const FAR_CANDIDATE_BYTES: usize = 1024 * 1024;

/// How many times a record is asked for more candidates. Past that, the search
/// says it stopped before the end rather than calling the record a miss
const FAR_ROUNDS: usize = 10;

/// How many records of one CLI there are brought over as candidates. The
/// search wants a page of hits, and a common word would otherwise bring over
/// a candidate from every record the machine holds. Reaching it is said
/// (`@@CAP`), so the page says there may be older ones
const FAR_CANDIDATE_RECORDS: usize = 80;

/// What a search of another machine came back with
#[derive(Debug, Default, PartialEq)]
pub struct FarFound {
    pub hits: Vec<Hit>,
    /// Stopped before the end: a record or a CLI's records had more to look
    /// through than the search looks through
    pub capped: bool,
    /// Why the machine could not be searched at all
    pub failed: Option<String>,
}

/// The shell function every far search is made of: `one <which> <file> <skip>`
/// prints one record's candidate lines after the first `skip` -- with the
/// bookkeeping's values taken out, "key":"value" becoming "key":"", which
/// leaves every line the JSON it was -- as `@@F <which> <mtime> <skip> <path>`,
/// then the start of the record and the candidates, both in base64. One more
/// line than a round takes is asked for, so whether there are more is known
/// from what came back. Fails for a record with no candidates at all
fn far_search_prelude(query: &str) -> String {
    use base64::Engine as _;
    let q = base64::engine::general_purpose::STANDARD.encode(query.trim());
    let strip = format!(
        "s/\"({})\":\"([^\"\\\\]|\\\\.)*\"/\"\\1\":\"\"/g",
        FAR_BOOKKEEPING.join("|")
    );
    let take = FAR_CANDIDATE_LINES + 1;
    format!(
        "cd \"$HOME\" 2>/dev/null || exit 0; q=$(printf %s '{q}' | base64 -d); \
one() {{ if [ -z \"$q\" ]; then m=''; else \
m=$(sed -E '{strip}' \"$2\" 2>/dev/null | grep -i -F -m $(($3 + {take})) -e \"$q\" | tail -n +$(($3 + 1)) | head -c {FAR_CANDIDATE_BYTES}); \
[ -z \"$m\" ] && return 1; fi; \
printf '@@F %s %s %s %s\\n' \"$1\" \"$(stat -c %Y \"$2\" 2>/dev/null || echo 0)\" \"$3\" \"$2\"; \
head -c {FOLDER_CAP} \"$2\" | base64 -w0; echo; printf %s \"$m\" | base64 -w0; echo; }}; "
    )
}

/// The first command of a far search: every CLI's records there, newest first,
/// each that holds `query` anywhere, through `one`
fn far_search_script(sources: &[Source], query: &str) -> String {
    let mut script = far_search_prelude(query);
    for (i, src) in sources.iter().enumerate() {
        let Some(rest) = src.verify.strip_prefix("{home}/").map(crate::sessionfind::any_id) else { continue };
        if !rest.chars().all(|c| c.is_ascii_alphanumeric() || "/*._-".contains(c)) {
            continue;
        }
        script.push_str(&format!(
            "n=0; ls -t {rest} 2>/dev/null | while IFS= read -r f; do \
[ $n -ge {FAR_CANDIDATE_RECORDS} ] && {{ echo '@@CAP {i}'; break; }}; \
if [ -n \"$q\" ]; then grep -q -i -F -e \"$q\" \"$f\" 2>/dev/null || continue; fi; \
one {i} \"$f\" 0 && n=$((n+1)); done; "
        ));
    }
    script
}

/// A later command of a far search: the next candidates of the records whose
/// earlier ones were none of them words a reader sees
fn far_more_script(query: &str, more: &[Again]) -> String {
    let mut script = far_search_prelude(query);
    for (which, path, skip) in more {
        let quoted = crate::worktree::for_a_shell(std::slice::from_ref(path));
        script.push_str(&format!("one {which} {quoted} {skip}; "));
    }
    script
}

/// A record asked again for its next candidates: which CLI's, where it is,
/// and how many candidate lines to skip
type Again = (usize, String, usize);

/// One record as a far search printed it
struct FarRecord {
    which: usize,
    when: u64,
    skip: usize,
    path: String,
    head: String,
    candidates: Vec<u8>,
}

/// What a far search's command printed: the records, and whether a CLI's
/// records were more than it looks through
fn far_records(out: &str) -> (Vec<FarRecord>, bool) {
    use base64::Engine as _;
    let decode = |b: &str| base64::engine::general_purpose::STANDARD.decode(b.trim()).unwrap_or_default();
    let mut records = Vec::new();
    let mut capped = false;
    let mut lines = out.lines();
    while let Some(line) = lines.next() {
        if line.starts_with("@@CAP") {
            capped = true;
            continue;
        }
        let Some(rest) = line.strip_prefix("@@F ") else { continue };
        let mut parts = rest.splitn(4, ' ');
        let (Some(which), Some(when), Some(skip), Some(path)) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let head = String::from_utf8_lossy(&decode(lines.next().unwrap_or_default())).into_owned();
        let candidates = decode(lines.next().unwrap_or_default());
        let (Ok(which), Ok(skip)) = (which.parse(), skip.trim().parse()) else { continue };
        records.push(FarRecord { which, when: when.trim().parse().unwrap_or(0), skip, path: path.to_string(), head, candidates });
    }
    (records, capped)
}

/// A far search's hits, judged here the way a record here is: a record whose
/// candidates were only what it notes about itself is not a match. Asks `more`
/// for the next candidates of a record that has more, round after round
fn far_search_hits(
    first: &str,
    sources: &[Source],
    needle: &str,
    machine: &str,
    limit: usize,
    more: &mut dyn FnMut(&[Again]) -> Option<String>,
) -> (Vec<Hit>, bool) {
    let (mut records, mut capped) = far_records(first);
    let mut hits: Vec<Hit> = Vec::new();
    for round in 0..=FAR_ROUNDS {
        let mut again: Vec<Again> = Vec::new();
        for r in records.drain(..) {
            let Some(src) = sources.get(r.which) else { continue };
            let Some(id) = id_of(Path::new(&r.path), &r.head, src) else { continue };
            let snippet = match needle.is_empty() {
                true => String::new(),
                false => match crate::reader::mention(&r.candidates, needle) {
                    Some(found) => snippet(&found.words, found.at, needle.len()),
                    None => {
                        // One more line than a round takes, or cut at its
                        // size: there are more candidates after these
                        let lines = r.candidates.split(|b| *b == b'\n').filter(|l| !l.is_empty()).count();
                        if lines > FAR_CANDIDATE_LINES || r.candidates.len() >= FAR_CANDIDATE_BYTES {
                            again.push((r.which, r.path, r.skip + lines.saturating_sub(1).max(1)));
                        }
                        continue;
                    }
                },
            };
            if hits.iter().any(|h| h.program == src.program && h.id == id) {
                continue;
            }
            let cwd = cwd_of(&r.head, src);
            hits.push(Hit {
                program: src.program.clone(),
                id,
                title: format!("{machine}: {}", title_of(cwd.as_deref(), &src.program)),
                snippet,
                cwd,
                when: r.when,
                tab: None,
                host: Some(machine.to_string()),
                ..Hit::default()
            });
        }
        if again.is_empty() {
            break;
        }
        if round == FAR_ROUNDS || hits.len() >= limit {
            capped = true;
            break;
        }
        match more(&again) {
            Some(out) => records = far_records(&out).0,
            None => {
                capped = true;
                break;
            }
        }
    }
    // Newest first across every CLI there, as the search here orders them
    hits.sort_by_key(|h| std::cmp::Reverse(h.when));
    if hits.len() > limit {
        capped = true;
        hits.truncate(limit);
    }
    (hits, capped)
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
    record_folder(&src.verify, id, src.cwd_path.as_deref())
        .is_some_and(|at| crate::uistate::same_folder(Path::new(&at), cwd))
}

/// The local record's own folder, independent of the app's tab associations.
/// An unreadable or not-yet-written record establishes no folder.
pub(crate) fn record_folder(glob: &str, id: &str, field: Option<&str>) -> Option<String> {
    let path = crate::sessionfind::locate(glob, id)?;
    let head = read_some(&path, FOLDER_CAP)?;
    recorded_folder(&head, field)
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
            ..Hit::default()
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
    let rest = crate::sessionfind::any_id(verify.strip_prefix("{home}/")?);
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
            ..Hit::default()
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
    recorded_folder(text, src.cwd_path.as_deref())
}

fn recorded_folder(text: &str, field: Option<&str>) -> Option<String> {
    if let Some(p) = field
        && let Some(c) = first_line_field(text, p) {
            return Some(c);
        }
    // Format-blind fallback: the first cwd anywhere in the head. Every one of
    // these tools writes the folder into its records; they just disagree on
    // where, so this finds it without being told
    first_string(text, "\"cwd\":")
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

// -- Where a conversation was had ---------------------------------------------
//
// A conversation found by the search is read in the conversation panel, by
// the reader the panel reads every conversation with. What is left to this
// module is what picking it back up needs: the folder it was had in, the
// branch that folder was on, and whether the folder is still there.

/// Where one conversation was had, as its record says, and whether that
/// folder is still there on the machine it was had on
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Where {
    /// The folder it was had in, when the record says
    pub folder: Option<String>,
    /// The branch that folder was on, when the record says
    pub branch: Option<String>,
    /// Whether that folder is still there. `None` when that could not be asked
    pub exists: Option<bool>,
}

/// Whether an id is one a record will be looked up by: the characters ids are
/// made of, and nothing that climbs out of the folder. What names a
/// conversation can come from a phone, and it is this side that decides which
/// file that is
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && !id.contains("..")
        && id.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// Where one conversation was had, by the CLI and id a search hit carries,
/// here or on the machine `at`. Reads the start of the record only: that is
/// where every CLI writes the folder and the branch
pub fn where_is(program: &str, id: &str, at: Option<&crate::elsewhere::Elsewhere>) -> Result<Where, String> {
    use crate::i18n::t;
    let src = sources()
        .into_iter()
        .find(|s| s.program == program)
        .ok_or_else(|| t("err.vault.no_program"))?;
    if !plain_id(id) {
        return Err(t("err.vault.no_record"));
    }
    let Some(at) = at else {
        return where_here(&src, id);
    };
    if let Some(found) = by_bridge::<Result<Where, String>>(at, "vault_where", serde_json::json!({"source": src, "id": id})) {
        return found;
    }
    // The long way: the start of the record brought over, and the folder
    // looked for there
    use base64::Engine as _;
    let path = crate::reader::locate_far(at, &src.verify, id).ok_or_else(|| t("err.vault.no_record"))?;
    let quoted = crate::worktree::for_a_shell(std::slice::from_ref(&path));
    let ran = crate::elsewhere::exec(at, &format!("head -c {FOLDER_CAP} {quoted} | base64 -w0"), 60_000)
        .map_err(|e| format!("{}: {e:#}", t("err.vault.unreadable")))?;
    let head = base64::engine::general_purpose::STANDARD.decode(ran.out.trim()).unwrap_or_default();
    let head = String::from_utf8_lossy(&head);
    let folder = cwd_of(&head, &src);
    Ok(Where {
        exists: folder.as_deref().and_then(|f| folder_there(Some(at), f)),
        branch: branch_of(&head),
        folder,
    })
}

/// The same, for a record on this machine
fn where_here(src: &Source, id: &str) -> Result<Where, String> {
    use crate::i18n::t;
    let path = crate::sessionfind::locate(&src.verify, id).ok_or_else(|| t("err.vault.no_record"))?;
    let head = read_some(&path, FOLDER_CAP).ok_or_else(|| t("err.vault.unreadable"))?;
    let folder = cwd_of(&head, src);
    Ok(Where {
        exists: folder.as_deref().map(|f| Path::new(f).is_dir()),
        branch: branch_of(&head),
        folder,
    })
}

// -- The same, done by the bridge on another machine ---------------------------
//
// A server or MicroVM a person put the bridge on reads its own records: the
// whole of every one, with the same functions this PC uses on its own. What
// crosses the network is the answer, not the megabytes. Each is one line in
// `farops::OPS`.

/// Asked of the bridge on `at`, when its line is up. `None` when there is no
/// bridge to ask, when it could not answer, or when it is a version that does
/// not know `op` -- and then the caller does it the way it did before there
/// were bridges, which is what a machine without one is always given
fn by_bridge<T: serde::de::DeserializeOwned>(at: &crate::elsewhere::Elsewhere, op: &str, p: serde_json::Value) -> Option<T> {
    if !crate::farlink::is_up(at) {
        return None;
    }
    match crate::farlink::call(at, op, p).map(serde_json::from_value::<T>) {
        Ok(Ok(v)) => {
            // Said once per request, so which way a far record was read can
            // be told afterwards -- and a check can tell it
            crate::append_hook_log(&format!("vault: {op} answered by the bridge on {}", at.address()));
            Some(v)
        }
        Ok(Err(e)) => {
            crate::append_hook_log(&format!("vault: the bridge's answer to {op} could not be read ({e}); asking the long way"));
            None
        }
        Err(e) => {
            crate::append_hook_log(&format!("vault: the bridge could not {op} ({e}); asking the long way"));
            None
        }
    }
}

fn param<'a>(p: &'a serde_json::Value, key: &str) -> Result<&'a serde_json::Value, String> {
    p.get(key).ok_or_else(|| format!("missing {key}"))
}

/// `vault_search`, on the bridge's machine: the search this PC runs on its
/// own records (`search`), over the CLIs this PC sent
pub fn bridge_search(p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let sources: Vec<Source> = serde_json::from_value(param(p, "sources")?.clone()).map_err(|e| e.to_string())?;
    let query = param(p, "query")?.as_str().unwrap_or_default();
    let limit = p.get("limit").and_then(serde_json::Value::as_u64).unwrap_or(40) as usize;
    serde_json::to_value(search_in(&sources, query, limit, &|| false)).map_err(|e| e.to_string())
}

/// `vault_where`, on the bridge's machine: where one conversation was had
/// (`where_is`). A record it cannot read is said as an answer, not as a
/// failure of the bridge: this PC shows it as it is
pub fn bridge_where(p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let src: Source = serde_json::from_value(param(p, "source")?.clone()).map_err(|e| e.to_string())?;
    let id = param(p, "id")?.as_str().unwrap_or_default();
    let found: Result<Where, String> = match plain_id(id) {
        true => where_here(&src, id),
        false => Err(crate::i18n::t("err.vault.no_record")),
    };
    serde_json::to_value(found).map_err(|e| e.to_string())
}

// -- What this PC knows about a conversation besides its record ------------------
//
// The conversation panel keeps, for the AI tabs of this app, who really sent
// each thing a person's line holds, which records one tab's conversation ran
// across, and the pins and notes put on it (`convo`). The search is asked of
// the records themselves; this is laid over what it found.

/// Lay over hits what this PC knows about them: the tab whose conversation
/// each belongs to (`thread`, one tab's records being one conversation), who
/// sent the words a person's line was found in (`from`), and whether anything
/// in it is pinned
pub fn annotate(hits: &mut [Hit]) {
    let store = crate::convo::db::Store::open_read(&crate::convo::path()).ok();
    let pinned: std::collections::HashSet<String> = crate::convo::marks::all(&crate::convo::marks::path())
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m.pinned)
        .map(|m| m.record)
        .collect();
    for h in hits.iter_mut() {
        if h.tab.is_some() {
            continue;
        }
        h.pinned = pinned.contains(&h.id);
        let Some(store) = store.as_ref() else { continue };
        h.thread = store.tab_of(&h.id).ok().flatten();
        h.thread_name = h.thread.as_deref().and_then(|uid| store.name_of(uid).ok().flatten());
        if let Some(said) = h.said.as_ref().filter(|s| s.who == crate::reader::Who::You) {
            h.from = crate::convo::read::origins(&h.id, &[(said.when, said.head.as_str())]).into_iter().next().flatten();
        }
    }
}

/// The notes written on this PC's conversations that hold `query`, as hits:
/// a note is words a person wrote about a conversation, and finding the
/// conversation by them is what they were written for. Opened at the thing
/// said the note is on
pub fn note_hits(query: &str) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let marks = crate::convo::marks::all(&crate::convo::marks::path()).unwrap_or_default();
    let sources = sources();
    let mut hits: Vec<Hit> = Vec::new();
    for m in marks {
        let Some(at) = crate::reader::find_in(&m.note, &needle) else { continue };
        if hits.iter().any(|h| h.id == m.record) || !plain_id(&m.record) {
            continue;
        }
        // Which CLI's record it is, and where that conversation was had
        let Some((src, head)) = sources.iter().find_map(|s| {
            let path = crate::sessionfind::locate(&s.verify, &m.record)?;
            Some((s, read_some(&path, FOLDER_CAP)?))
        }) else {
            continue;
        };
        let cwd = cwd_of(&head, src);
        hits.push(Hit {
            program: src.program.clone(),
            id: m.record.clone(),
            title: title_of(cwd.as_deref(), &src.program),
            snippet: snippet(&m.note, at, needle.len()),
            cwd,
            when: (m.changed.max(m.made) / 1000).max(0) as u64,
            at: Some(m.at),
            pinned: m.pinned,
            noted: true,
            ..Hit::default()
        });
    }
    hits
}

/// Whether the folder a conversation was had in is still there, on the
/// machine it was had on. `None` when that could not be asked
pub fn folder_there(at: Option<&crate::elsewhere::Elsewhere>, folder: &str) -> Option<bool> {
    let Some(at) = at else {
        return Some(Path::new(folder).is_dir());
    };
    let quoted = crate::worktree::for_a_shell(&[folder.to_string()]);
    let ran = crate::elsewhere::exec(at, &format!("[ -d {quoted} ] && echo yes || echo no"), 30_000).ok()?;
    match ran.out.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// Where a branch could be cut into a folder again, among the folders of a
/// desk on this PC: each project's checkout that still has the branch, and
/// whether it has it itself (`true`) or only its remote does, as `origin/…`.
///
/// Asked when the folder a conversation was had in is gone. Removing a
/// worktree takes its folder and leaves its branch, so the work that was
/// committed there is still there to be picked back up
pub fn branch_homes(branch: &str, folders: &[PathBuf]) -> Vec<serde_json::Value> {
    // A name git would take as an option, or that is not one name, is not asked
    if branch.starts_with('-') || branch.contains("..") || branch.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Vec::new();
    }
    let has = |main: &Path, r: &str| {
        let mut asking = std::process::Command::new("git");
        asking.arg("-C").arg(main).args(["rev-parse", "--verify", "--quiet", r]);
        crate::detach_console(&mut asking).output().is_ok_and(|o| o.status.success())
    };
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut out = Vec::new();
    for folder in folders {
        let Some(main) = crate::repo::main_checkout(folder) else { continue };
        if seen.iter().any(|s| crate::uistate::same_folder(s, &main)) {
            continue;
        }
        seen.push(main.clone());
        let local = has(&main, &format!("refs/heads/{branch}^{{commit}}"));
        if local || has(&main, &format!("refs/remotes/origin/{branch}^{{commit}}")) {
            out.push(serde_json::json!({"dir": main.display().to_string(), "local": local}));
        }
    }
    out
}

/// The branch the folder was on, from the first place a record writes it
/// down: `gitBranch` on Claude's lines, `branch` in what Codex notes about
/// the repository when it starts. A detached checkout says `HEAD`, which names
/// no branch
fn branch_of(head: &str) -> Option<String> {
    ["\"gitBranch\":", "\"branch\":"]
        .iter()
        .find_map(|key| first_string(head, key))
        .filter(|b| b != "HEAD")
}

/// The string value after the first `key` in some JSON text, unescaped the
/// little a path or a name needs
fn first_string(text: &str, key: &str) -> Option<String> {
    let at = text.find(key)? + key.len();
    let rest = text[at..].trim_start().strip_prefix('"')?;
    let end = rest.find('"')?;
    let raw = &rest[..end];
    (!raw.is_empty()).then(|| raw.replace("\\\\", "\\").replace("\\/", "/"))
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
    let expanded = expand(&crate::sessionfind::any_id(pattern));
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
            ..Hit::default()
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
        assert!(script.contains("grep -i -F -m $(($3 + 21))"), "{script}");
        assert!(script.contains("\"(cwd|gitBranch|"), "the bookkeeping is not taken out before looking: {script}");
        assert!(script.contains("@@CAP 0"), "reaching the records' cap is not said: {script}");

        use base64::Engine as _;
        let b = |t: &str| base64::engine::general_purpose::STANDARD.encode(t);
        let head = r#"{"type":"user","cwd":"/home/user/site","sessionId":"aaa"}"#;
        let line = r#"{"type":"user","cwd":"","message":{"role":"user","content":"please fix the login bug in auth.rs"}}"#;
        let record = |id: &str, skip: usize, candidates: &str| {
            format!("@@F 0 1790430222 {skip} .claude/projects/-home-user-site/{id}.jsonl\n{}\n{}\n", b(head), b(candidates))
        };
        let mut never = |_: &[Again]| -> Option<String> { panic!("nothing more was needed") };
        let (hits, capped) = far_search_hits(&record("aaa", 0, line), &[claude()], "login bug", "vm", 10, &mut never);
        assert_eq!(hits.len(), 1);
        assert!(!capped);
        assert_eq!(hits[0].id, "aaa");
        assert!(hits[0].title.starts_with("vm: "), "{}", hits[0].title);
        assert!(hits[0].snippet.contains("login bug"), "{}", hits[0].snippet);
        assert_eq!(hits[0].cwd.as_deref(), Some("/home/user/site"));

        // A line whose words did not hold it -- it was only in what the
        // record notes about itself -- is not a match
        let noted = r#"{"type":"user","cwd":"","instructions":"login bug","message":{"role":"user","content":"hello"}}"#;
        let (hits, _) = far_search_hits(&record("bbb", 0, noted), &[claude()], "login bug", "vm", 10, &mut never);
        assert!(hits.is_empty());

        // A record whose first candidates are all thinking is asked for its
        // next ones, and the match after them is found
        let thought = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"the login bug again"}]}}"#;
        let first: String = std::iter::repeat_n(thought, FAR_CANDIDATE_LINES + 1).collect::<Vec<_>>().join("\n");
        let mut asked: Vec<Again> = Vec::new();
        let mut more = |again: &[Again]| {
            asked.extend_from_slice(again);
            Some(record("ccc", again[0].2, line))
        };
        let (hits, capped) = far_search_hits(&record("ccc", 0, &first), &[claude()], "login bug", "vm", 10, &mut more);
        assert_eq!(asked, vec![(0, ".claude/projects/-home-user-site/ccc.jsonl".to_string(), FAR_CANDIDATE_LINES)]);
        assert_eq!(hits.len(), 1, "the match after twenty thoughts was missed");
        assert!(!capped);

        // Asked for more past the rounds it may ask, the search says it
        // stopped before the end rather than calling the record a miss
        let mut forever = |again: &[Again]| Some(record("ddd", again[0].2, &first));
        let (hits, capped) = far_search_hits(&record("ddd", 0, &first), &[claude()], "login bug", "vm", 10, &mut forever);
        assert!(hits.is_empty());
        assert!(capped, "a search that stopped before the end did not say so");

        // ...and so does one whose machine had more records than it reads
        let (_, capped) = far_search_hits(&format!("{}@@CAP 0\n", record("aaa", 0, line)), &[claude()], "login bug", "vm", 10, &mut never);
        assert!(capped);
    }
}

#[cfg(test)]
mod whole_tests {
    use super::*;

    fn source(root: &Path) -> Source {
        Source {
            program: "claude".into(),
            with_id: vec!["--resume".into(), "{id}".into()],
            verify: format!("{}/*/{{id}}.jsonl", root.display()).replace('/', std::path::MAIN_SEPARATOR_STR),
            id_path: None,
            cwd_path: None,
            asks: None,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-vault-whole-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proj")).unwrap();
        d
    }

    fn said(who: &str, cwd: &str, text: &str) -> String {
        format!(
            r#"{{"type":"{who}","cwd":"{cwd}","gitBranch":"fix/login","message":{{"role":"{who}","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    /// The whole of every record is searched, not its start: a word said after
    /// megabytes of tool output is found. And only what a reader would see is
    /// matched -- the folder a conversation ran in is in every one of its
    /// lines, and a search for the folder's name is not a search for every
    /// conversation had there
    #[test]
    fn every_record_is_searched_all_the_way_through_and_only_where_words_are() {
        let root = tmp("through");
        let big = "x".repeat(3 * 1024 * 1024);
        let result = format!(
            r#"{{"type":"user","cwd":"D:/work/paymentsvc","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":"{big}"}}]}}}}"#
        );
        let late = [
            said("user", "D:/work/paymentsvc", "look at the logs"),
            result,
            said("assistant", "D:/work/paymentsvc", "The Refund handler double-counts"),
        ]
        .join("\n");
        std::fs::write(root.join("proj").join("aaaa-late.jsonl"), late).unwrap();
        std::fs::write(
            root.join("proj").join("bbbb-other.jsonl"),
            said("user", "D:/work/paymentsvc", "something else entirely"),
        )
        .unwrap();

        let src = [source(&root)];
        let found = search_in(&src, "refund HANDLER", 10, &|| false);
        let ids: Vec<&str> = found.hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, vec!["aaaa-late"], "a word past the first megabytes was not found");
        assert!(found.hits[0].snippet.contains("Refund handler"), "{}", found.hits[0].snippet);
        assert_eq!(found.hits[0].cwd.as_deref(), Some("D:/work/paymentsvc"));
        // ...and where its line starts, for a reader to open on it
        let record = std::fs::read(root.join("proj").join("aaaa-late.jsonl")).unwrap();
        let at = found.hits[0].at.expect("where the words are") as usize;
        assert!(record[at..].starts_with(said("assistant", "D:/work/paymentsvc", "The Refund handler double-counts").as_bytes()));

        let folder = search_in(&src, "paymentsvc", 10, &|| false);
        assert!(folder.hits.is_empty(), "the folder's name matched every conversation had in it: {:?}", folder.hits);

        // A search typed over by a newer one stops reading
        let stopped = search_in(&src, "refund", 10, &|| true);
        assert!(stopped.hits.is_empty());
    }

    /// Stopping at enough says there may be more; reading everything does not
    #[test]
    fn stopping_at_enough_says_so() {
        let root = tmp("enough");
        for i in 0..3 {
            std::fs::write(root.join("proj").join(format!("c{i}.jsonl")), said("user", "D:/w", "the same words")).unwrap();
        }
        let src = [source(&root)];
        assert!(search_in(&src, "same words", 2, &|| false).capped);
        assert!(!search_in(&src, "same words", 5, &|| false).capped);
    }

    /// The branch a conversation's folder was on, from the record, and a
    /// detached checkout names none
    #[test]
    fn the_branch_is_read_from_the_record() {
        assert_eq!(branch_of(&said("user", "D:/w", "hi")).as_deref(), Some("fix/login"));
        let codex = r#"{"type":"session_meta","payload":{"cwd":"/w","git":{"commit_hash":"abc","branch":"main"}}}"#;
        assert_eq!(branch_of(codex).as_deref(), Some("main"));
        assert_eq!(branch_of(r#"{"gitBranch":"HEAD"}"#), None);
    }

    /// What names a conversation can come from a phone. It is looked up as a
    /// file only when it is the characters an id is made of
    #[test]
    fn an_id_that_climbs_out_is_not_looked_up() {
        assert!(plain_id("0f44d745-174b-40ec-8181-4c932961e2c8"));
        assert!(plain_id("rollout-2026-04-28T10-54-47-019dd1cb"));
        assert!(!plain_id("../../secrets"));
        assert!(!plain_id("a/b"));
        assert!(!plain_id(""));
    }

    /// What a bridge does on its machine is what this PC does on its own: the
    /// same search, and the same answer to where a conversation was had --
    /// asked by name through the bridge's table and read back from what it sends
    #[test]
    fn a_bridge_answers_the_way_this_pc_reads() {
        let root = tmp("bridge");
        let gone = root.join("gone-worktree").display().to_string().replace('\\', "/");
        let lines = [
            said("user", &gone, "why is checkout slow"),
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"grep slow app.log"}}]}}"#.to_string(),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"slow query in pricing"}]}}"#.to_string(),
            said("assistant", &gone, "The pricing query is slow"),
        ]
        .join("\n");
        std::fs::write(root.join("proj").join("conv.jsonl"), &lines).unwrap();
        let src = source(&root);
        let as_json = serde_json::to_value(&src).unwrap();

        let far = crate::farops::run("vault_search", &serde_json::json!({"sources": [as_json], "query": "PRICING", "limit": 10})).unwrap();
        let far: Found = serde_json::from_value(far).unwrap();
        assert_eq!(far, search_in(std::slice::from_ref(&src), "PRICING", 10, &|| false));
        assert_eq!(far.hits.len(), 1);
        // Found in a tool's output: nobody said it
        assert_eq!(far.hits[0].said, None);

        let asked = |id: &str| {
            let v = crate::farops::run("vault_where", &serde_json::json!({"source": as_json, "id": id})).unwrap();
            serde_json::from_value::<Result<Where, String>>(v).unwrap()
        };
        let there = asked("conv").unwrap();
        assert_eq!(there, where_here(&src, "conv").unwrap());
        assert_eq!(there.folder.as_deref(), Some(gone.as_str()));
        assert_eq!(there.branch.as_deref(), Some("fix/login"));
        assert_eq!(there.exists, Some(false), "a worktree removed since is said to be gone");

        // A record that is not there is an answer, said as it is
        assert!(asked("nobody").is_err());
        // ...and an id that climbs out of the folder is not looked up at all
        assert!(asked("../x").is_err());
    }

    /// What a person said is found with who said it and when, for who really
    /// sent it to be looked up
    #[test]
    fn a_hit_in_something_said_says_who_said_it() {
        let root = tmp("said");
        let line = r#"{"type":"user","timestamp":"2026-09-29T01:02:03.000Z","cwd":"D:/w","message":{"role":"user","content":"please look at the marmot table"}}"#;
        std::fs::write(root.join("proj").join("c1.jsonl"), line).unwrap();
        let found = search_in(&[source(&root)], "marmot", 10, &|| false);
        let said = found.hits[0].said.as_ref().expect("who said it");
        assert_eq!(said.who, crate::reader::Who::You);
        assert!(said.head.starts_with("please look at the marmot"));
    }

    /// A folder gone from the disk is said to be gone
    #[test]
    fn a_folder_gone_is_said_to_be_gone() {
        let root = tmp("gone");
        assert_eq!(folder_there(None, &root.display().to_string()), Some(true));
        assert_eq!(folder_there(None, &root.join("removed").display().to_string()), Some(false));
    }
}

#[cfg(test)]
mod probe {
    /// How long a search of this machine's own records takes, and what it
    /// finds. Ignored by default because it reads the real records:
    ///
    ///   SHIKISHA_VAULT_PROBE=word cargo test --release -p shikisha-core vault::probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe() {
        let q = std::env::var("SHIKISHA_VAULT_PROBE").unwrap_or_else(|_| "worktree".into());
        let began = std::time::Instant::now();
        let found = super::search(&q, 40);
        println!("{} hits (capped={}) in {:?}", found.hits.len(), found.capped, began.elapsed());
        for h in found.hits.iter().take(5) {
            println!("  {} {}", h.title, h.snippet.chars().take(100).collect::<String>());
        }
    }
}
