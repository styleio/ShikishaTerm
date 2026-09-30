//! Asking an AI CLI to report the conversation it is running.
//!
//! Every CLI worth resuming can run a command of your choosing when a
//! conversation starts. That command is this app in hook mode (`--hook
//! session`), which reports the id back through the API pipe — see
//! `main::hook_mode`. What this module does is put that command into the CLI's
//! own config file, and take it out again.
//!
//! Three rules, because this writes into a file that belongs to another
//! program and to the person using it:
//!
//!   - **Nothing else in the file is touched.** Only entries this app put there
//!     are replaced or removed; every other hook the person has set up survives
//!   - **A file that doesn't parse is left exactly as it is.** Rewriting a
//!     config we cannot read would destroy settings to install a convenience
//!   - **The previous contents are kept** beside it as `.bak` before the first
//!     change. It costs nothing and it is the difference between "undo it" and
//!     "what did it just do to my settings"

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::profile::{HookFormat, HookTrust};

/// How this app's own hook entry is recognised again later.
///
/// Not a comment or a marker field — the command itself. A marker can be
/// dropped by an editor that rewrites the file; the command cannot, because
/// removing it removes the hook
const MARK: &str = "--hook";

/// How long a CLI should wait for one of these, in seconds.
///
/// Nothing here answers back, so the number only matters as a ceiling. Three
/// rather than five because Codex caps two of its events at three and warns,
/// on every single launch, about anything higher — a line of complaint in
/// someone's terminal forever, to allow a wait we do not want in the first
/// place
const TIMEOUT_S: u32 = 3;

/// What the one hook that answers back is run with: the end of a turn, where
/// a tab that was asked something by another tab is asked for its line for
/// the chat (`main::hook_mode`, `asktab::Ask::hear_stop`)
pub const LINE_ARG: &str = "line";

/// How long a CLI waits for that one, in seconds. It is waited for -- it
/// decides whether the turn ends -- and the app answers it at once; this is
/// the most a stuck app can cost a turn
const LINE_TIMEOUT_S: u32 = 10;

/// One event we ask a CLI to report, and what our end makes of it.
///
/// The meaning travels in the command line rather than being looked up when
/// the event lands. A person opening their own settings file has to be able to
/// see what this app will do with each event, and `--hook state:QUESTION`
/// under `PermissionRequest` says it without a manual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The CLI's own name for the event
    pub event: String,
    /// What we ask to be run for it: `session`, or `state:<STATE>`
    pub arg: String,
}

/// One CLI that can be asked to report on itself.
#[derive(Debug, Clone)]
pub struct Target {
    /// The CLI's display name, from its profile
    pub name: String,
    pub file: PathBuf,
    pub format: HookFormat,
    /// How long the CLI may wait, already in the unit that CLI counts in
    pub timeout: u32,
    /// The same, for the hook that answers back ([`LINE_ARG`])
    pub line_timeout: u32,
    pub entries: Vec<Entry>,
    /// How the CLI is told a hook written for it is agreed to ([`approve`])
    pub trust: Option<HookTrust>,
}

/// Where the target stands right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The CLI keeps no config here — most likely it isn't installed
    NoConfig,
    /// A config exists, without our entry
    Absent,
    /// Our entry is there and points at this app where it now lives
    Installed,
    /// Our entry is there but names a different path — the app was moved or
    /// copied. It would run the old one, or nothing at all
    Stale,
    /// The file exists and is not JSON we can read. Nothing will be written
    Unreadable(String),
}

/// Every CLI in the profiles that says how to hook it.
pub fn targets() -> Vec<Target> {
    crate::profile::all()
        .into_iter()
        .filter_map(|p| {
            let hook = p.resume.as_ref()?.hook.as_ref()?;
            let mut entries: Vec<Entry> = hook
                .events
                .iter()
                .map(|event| Entry { event: event.clone(), arg: "session".into() })
                .collect();
            // A state this app has no name for would install a hook that fires
            // into nothing, so the profile is checked here rather than trusted
            for (event, state) in &hook.states {
                match crate::detect::TabState::from_label(state) {
                    Some(s) => entries.push(Entry {
                        event: event.clone(),
                        arg: format!("state:{}", s.label()),
                    }),
                    None => crate::append_hook_log(&format!(
                        "profile {}: {event} says {state:?}, which is not a state",
                        p.name
                    )),
                }
            }
            // A helper beginning or ending. Only the two words this app acts
            // on, for the same reason as the states: anything else would be a
            // hook that fires into nothing
            for (event, way) in &hook.helpers {
                match Helper::of(way) {
                    Some(h) => entries.push(Entry { event: event.clone(), arg: h.arg().to_string() }),
                    None => crate::append_hook_log(&format!(
                        "profile {}: {event} says {way:?} of a helper, which is neither \"up\" nor \"down\"",
                        p.name
                    )),
                }
            }
            if let Some(event) = hook.turn_end.as_ref() {
                entries.push(Entry { event: event.clone(), arg: LINE_ARG.to_string() });
            }
            Some(Target {
                name: p.name.clone(),
                file: expand(&hook.file),
                format: hook.format,
                timeout: hook.timeout_unit.from_seconds(TIMEOUT_S),
                line_timeout: hook.timeout_unit.from_seconds(LINE_TIMEOUT_S),
                entries,
                trust: hook.trust,
            })
        })
        .collect()
}

/// `{home}` is the only thing a profile may stand in for. A profile that could
/// name any path would be naming a file to overwrite
fn expand(path: &str) -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    expand_at(path, &home)
}

fn expand_at(path: &str, home: &str) -> PathBuf {
    // Put together a piece at a time, so the path is spelled the way this
    // system spells one: it is shown to the person, in the question that asks
    // to write there
    path.replace("{home}", home).split(['/', '\\']).fold(PathBuf::new(), |mut at, piece| {
        match at.as_os_str().is_empty() {
            true => at.push(format!("{piece}{}", std::path::MAIN_SEPARATOR)),
            false => at.push(piece),
        }
        at
    })
}

/// This app, as the CLI will have to spell it.
fn me() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("SHIKISHA-TERM.exe"))
}

/// The same path with no space in it, for a CLI that will not take quotes.
///
/// A path that has none already is its own answer, on every system. `None`
/// means there is no way to name this program without a space, and saying so
/// is the point: the alternative is installing a hook that can never run and
/// finding out from a dot that never moves.
///
/// Nothing but Windows keeps a second name for a file, so on everything else
/// a path with a space in it has no answer at all. In practice that is rare
/// there, and a person can move the program; on Windows a space is the normal
/// case, which is why the other half of this exists.
#[cfg(not(windows))]
fn spaceless(path: &Path) -> Option<String> {
    let long = path.display().to_string();
    (!long.contains(' ')).then_some(long)
}

/// Windows has kept a second, space-free name for every file since the days
/// when eight characters was the whole allowance (`C:\PROGRA~1\...`), and it
/// is the only way to name a program in a command line that cannot be quoted.
/// `None` when the path still has a space afterwards — that happens where the
/// short names have been turned off for a volume.
#[cfg(windows)]
fn spaceless(path: &Path) -> Option<String> {
    let long = path.display().to_string();
    if !long.contains(' ') {
        return Some(long);
    }
    let wide: Vec<u16> = long.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: both slices are ours, and the call is told the size of the one
    // it writes into
    let n = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
            wide.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
        )
    } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let short = String::from_utf16_lossy(&buf[..n]);
    (!short.contains(' ')).then_some(short)
}

/// One handler, in the spelling this CLI accepts.
///
/// `async` is not a nicety. A hook is a program the CLI runs and waits for,
/// and these fire on every turn and every permission dialog -- a fifth of a
/// second of process startup, charged to the person's turn, for a report
/// nobody is waiting on. The one exception is [`LINE_ARG`]: it answers back,
/// and a CLI ignores what a hook it did not wait for says. The timeout stays
/// for the CLIs that still honour one.
fn handler(format: HookFormat, timeout: u32, arg: &str, program: &Path) -> serde_json::Value {
    let mut h = handler_of(format, timeout, arg, program);
    if arg == LINE_ARG
        && let Some(o) = h.as_object_mut()
    {
        o.shift_remove("async");
    }
    h
}

fn handler_of(format: HookFormat, timeout: u32, arg: &str, program: &Path) -> serde_json::Value {
    let exe = program.display().to_string();
    match format {
        // Nothing quoted, and a path chosen so that nothing needs to be
        HookFormat::Bare => serde_json::json!({
            "type": "command",
            "command": format!("{} --hook {arg}", spaceless(program).unwrap_or(exe)),
            "timeout": timeout,
            "async": true,
        }),
        HookFormat::Args => serde_json::json!({
            "type": "command",
            "command": exe,
            "args": ["--hook", arg],
            "timeout": timeout,
            "async": true,
        }),
        // One command line, so the path is quoted: on Windows it usually has a
        // space in it, and an unquoted one would be read as two arguments
        HookFormat::Shell => serde_json::json!({
            "type": "command",
            "command": format!("\"{exe}\" --hook {arg}"),
            "timeout": timeout,
            "async": true,
        }),
    }
}

/// Whether this handler is one of ours, wherever the app now lives.
fn is_ours(h: &serde_json::Value) -> bool {
    let line = h.get("command").and_then(|c| c.as_str()).unwrap_or_default();
    let args = h
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let both = format!("{line} {args}");
    both.contains(MARK) && both.to_ascii_lowercase().contains("shikisha")
}

/// Whether a handler of ours runs this app with `arg` (`--hook <arg>`)
fn carries(h: &serde_json::Value, arg: &str) -> bool {
    let line = h.get("command").and_then(|c| c.as_str()).unwrap_or_default();
    let args = h
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let both = format!("{line} {args}");
    both.match_indices(MARK).any(|(at, _)| {
        both[at + MARK.len()..]
            .trim_start()
            .split(|c: char| c.is_whitespace() || matches!(c, ';' | '"' | '\''))
            .next()
            == Some(arg)
    })
}

/// Whether a handler of ours is exactly one of the ones we would write today
/// -- same place, same argument, same way of running it, same patience.
///
/// Asked against this target's whole wish for that event rather than against
/// one argument, because one event can be asked for more than one thing.
/// Asked against this target rather than against any spelling we know, so that
/// an entry written for a CLI that has since changed its mind reads as out of
/// date and gets rewritten
fn is_current(wanted: &[serde_json::Value], h: &serde_json::Value) -> bool {
    wanted.contains(h)
}

/// What the file says about us, without changing anything.
pub fn status(t: &Target) -> Status {
    status_of(t, &me())
}

/// The same, asked about a copy of this app somewhere else.
///
/// `Stale` means "the entry names a different program", and which program is
/// the right one depends on who is asking: the app asks about itself, while a
/// tool repairing an install asks about that install. Asked with `me()` from a
/// tool, every correctly installed entry reads as stale -- which is how it
/// would report a repair it had just made successfully
pub fn status_of(t: &Target, program: &Path) -> Status {
    let Ok(text) = std::fs::read_to_string(&t.file) else {
        return Status::NoConfig;
    };
    let doc: serde_json::Value = match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
        Ok(v) => v,
        Err(e) => return Status::Unreadable(e.to_string()),
    };
    let mut seen = 0;
    let mut current = 0;
    // Once per event, not once per thing wanted from it: an event asked for
    // two things was being read twice, and counted both of its handlers each
    // time -- so a correctly installed entry never added up
    for (event, hs) in wanted(t, program) {
        for group in doc
            .pointer(&format!("/hooks/{event}"))
            .and_then(|g| g.as_array())
            .into_iter()
            .flatten()
        {
            for h in group.pointer("/hooks").and_then(|h| h.as_array()).into_iter().flatten() {
                if is_ours(h) {
                    seen += 1;
                    if is_current(&hs, h) {
                        current += 1;
                    }
                }
            }
        }
    }
    match (seen, current) {
        (0, _) => Status::Absent,
        (s, c) if s == c && c == t.entries.len() => Status::Installed,
        _ => Status::Stale,
    }
}

/// The events this app now asks the CLI to report that its file does not yet
/// carry an entry of ours for, while it does carry ours for others: what an
/// update of this app adds to a hook somebody already agreed to.
///
/// Told apart from a hook that is merely out of date (it names another copy
/// of this app, or an older way of writing the same thing), which is kept
/// right without asking: agreeing to a hook was agreeing to what it was shown
/// to do, and a new event is something it was not shown doing. Empty for a
/// file with none of ours in it -- that is a hook not set up at all, asked
/// about the ordinary way -- and for one that cannot be read
pub fn new_events(t: &Target) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(&t.file) else { return Vec::new() };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else {
        return Vec::new();
    };
    new_events_in(&doc, t)
}

/// [`new_events`] of a file already read, from this PC or another machine
fn new_events_in(doc: &serde_json::Value, t: &Target) -> Vec<String> {
    let ours_in = |event: &str| -> Vec<serde_json::Value> {
        doc.pointer(&format!("/hooks/{event}"))
            .and_then(|g| g.as_array())
            .into_iter()
            .flatten()
            .flat_map(|group| group.pointer("/hooks").and_then(|h| h.as_array()).cloned().unwrap_or_default())
            .filter(is_ours)
            .collect()
    };
    if !t.entries.iter().any(|e| !ours_in(&e.event).is_empty()) {
        return Vec::new();
    }
    // An event is new when something is asked of it that no entry of ours
    // there does: a whole event, or a new thing asked of one already carried
    // (the end of a turn asked to answer back as well as to report)
    let mut added: Vec<String> = Vec::new();
    for e in &t.entries {
        let new = !ours_in(&e.event).iter().any(|h| carries(h, &e.arg));
        if new && !added.contains(&e.event) {
            added.push(e.event.clone());
        }
    }
    added
}

/// Everything we want from one event, gathered.
///
/// One event can be asked for more than one thing -- Gemini CLI has four
/// events in total, so the start of a turn has to carry both "this is the
/// conversation" and "it is working" -- and writing them one at a time meant
/// each one removed the last. Gathering first is what makes that impossible
fn wanted(t: &Target, program: &Path) -> Vec<(String, Vec<serde_json::Value>)> {
    let mut out: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    for entry in &t.entries {
        let wait = if entry.arg == LINE_ARG { t.line_timeout } else { t.timeout };
        let h = handler(t.format, wait, &entry.arg, program);
        match out.iter_mut().find(|(event, _)| *event == entry.event) {
            Some((_, list)) => list.push(h),
            None => out.push((entry.event.clone(), vec![h])),
        }
    }
    out
}

/// One line of [`preview`]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Line {
    pub text: String,
    /// Written by this app: the rest is the person's, as it already is
    pub ours: bool,
}

/// What stands in for a group of ours while [`preview`] lays the file out
const MARKED: &str = "\u{1}shikisha:";

/// The CLI's settings file as it would be once ours is in, a line at a time,
/// with the lines this app adds told from the ones already there.
///
/// Shown rather than described: this writes into someone else's config, and
/// "trust me" is not an acceptable substitute for the lines involved -- nor is
/// showing ours alone, which says nothing of what happens to theirs. A file
/// that cannot be read is not written to (see [`edit`]); shown for it is what
/// ours is on its own
pub fn preview(t: &Target) -> Vec<Line> {
    let existing = std::fs::read_to_string(&t.file).ok();
    preview_of(existing.as_deref(), wanted(t, &me()))
}

/// [`preview`] of a file's text as read, wherever it was read from, with
/// `wanted` put into it
fn preview_of(existing: Option<&str>, wanted: Vec<(String, Vec<serde_json::Value>)>) -> Vec<Line> {
    let mut doc = match existing {
        None => serde_json::json!({}),
        Some(text) => match serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) {
            Ok(v) if v.is_object() => v,
            _ => {
                let mut hooks = serde_json::Map::new();
                for (event, hs) in wanted {
                    hooks.insert(event, serde_json::json!([{ "hooks": hs }]));
                }
                return pretty_lines(&serde_json::json!({ "hooks": hooks }), true);
            }
        },
    };
    let had: Option<Vec<String>> = doc.get("hooks").and_then(|h| h.as_object()).map(|h| h.keys().cloned().collect());
    let mut marks = Vec::new();
    merge_with(&mut doc, wanted, true, Some(&mut marks));
    // The stand-in as the text spells it: quoted, the mark escaped
    let needle = serde_json::to_string(MARKED).unwrap_or_default();
    let needle = needle.trim_end_matches('"');
    let mut out: Vec<Line> = Vec::new();
    for line in pretty_lines(&doc, existing.is_none()) {
        // A stand-in for a group of ours: the group, where it stood, indented
        // as it stood and carrying its comma
        let Some(at) = line.text.find(needle) else {
            out.push(line);
            continue;
        };
        let lead = &line.text[..at];
        let rest = &line.text[at + 1..];
        let n: usize = rest.split('"').next().and_then(|m| m.rsplit(':').next()).and_then(|n| n.parse().ok()).unwrap_or(0);
        let comma = if line.text.trim_end().ends_with(',') { "," } else { "" };
        let body = serde_json::to_string_pretty(&marks.get(n).cloned().unwrap_or_default()).unwrap_or_default();
        let count = body.lines().count();
        for (i, piece) in body.lines().enumerate() {
            let text = match i {
                0 => format!("{lead}{piece}"),
                _ => format!("{}{piece}", " ".repeat(lead.len())),
            };
            out.push(Line { text: if i + 1 == count { format!("{text}{comma}") } else { text }, ours: true });
        }
    }
    // An event, or the whole list of hooks, that the file did not have: the
    // brackets around ours are ours too
    let opens = |text: &str, depth: usize| text.len() > depth && text[..depth].trim().is_empty() && !text[depth..].starts_with(' ');
    let mut i = 0;
    while i < out.len() {
        let text = out[i].text.clone();
        let key = text.trim_start().split('"').nth(1).unwrap_or_default().to_string();
        let new = match &had {
            None => opens(&text, 2) && key == "hooks",
            Some(keys) => opens(&text, 4) && text.trim_end().ends_with('[') && !keys.contains(&key),
        };
        if new {
            let depth = text.len() - text.trim_start().len();
            let mut j = i;
            while j < out.len() {
                out[j].ours = true;
                let t = out[j].text.trim_start();
                if j > i && (out[j].text.len() - t.len()) == depth && (t.starts_with(']') || t.starts_with('}')) {
                    break;
                }
                j += 1;
            }
            i = j;
        }
        i += 1;
    }
    out
}

fn pretty_lines(v: &serde_json::Value, ours: bool) -> Vec<Line> {
    serde_json::to_string_pretty(v)
        .unwrap_or_default()
        .lines()
        .map(|l| Line { text: l.to_string(), ours })
        .collect()
}

/// Put our entry in (or bring it up to date), leaving everything else alone.
pub fn install(t: &Target) -> Result<()> {
    edit(t, true, &me())
}

/// The same, naming the program to be run instead of this one.
///
/// For the times this is not being asked by the app a person uses: a tool that
/// puts the hook back into an install somewhere else, after it was found
/// missing. The path is written into somebody's settings and will be run, so
/// it is given rather than guessed -- the obvious guess, "whatever is running
/// now", is exactly the one that writes the wrong program in
pub fn install_as(t: &Target, program: &Path) -> Result<()> {
    edit(t, true, program)
}

/// Take our entry out, leaving everything else alone.
pub fn uninstall(t: &Target) -> Result<()> {
    edit(t, false, &me())
}

/// The program the first entry of ours in `t`'s file runs, as written there
fn program_named_in(t: &Target) -> Option<PathBuf> {
    let text = std::fs::read_to_string(&t.file).ok()?;
    let doc: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let h = doc
        .pointer("/hooks")?
        .as_object()?
        .values()
        .filter_map(|g| g.as_array())
        .flatten()
        .filter_map(|g| g.pointer("/hooks").and_then(|h| h.as_array()))
        .flatten()
        .find(|h| is_ours(h))?;
    program_of(h)
}

/// The program one of our handlers runs: its `command` when the arguments
/// are apart, else the command line up to where ours begin, unquoted
fn program_of(h: &serde_json::Value) -> Option<PathBuf> {
    let line = h.get("command")?.as_str()?;
    if h.get("args").is_some() {
        return Some(PathBuf::from(line));
    }
    let program = line.split(&format!(" {MARK}")).next()?.trim().trim_matches('"');
    (!program.is_empty()).then(|| PathBuf::from(program))
}

/// Whether the CLI this hook is for is used on this PC: the folder it keeps
/// its settings in is there. A CLI never run here has none -- and nothing is
/// written into, or asked about, a CLI nobody uses
pub fn in_use(t: &Target) -> bool {
    t.file.parent().is_some_and(|d| d.is_dir())
}

/// The CLIs to ask about as the program starts: used on this PC, and not
/// answered yet. `answers` is what the settings hold ([`crate::config::Config::agent_hooks`])
pub fn unasked(answers: &std::collections::BTreeMap<String, String>) -> Vec<Target> {
    among(targets(), answers, None)
}

/// The CLIs the person said to set up, that are used on this PC
pub fn agreed(answers: &std::collections::BTreeMap<String, String>) -> Vec<Target> {
    among(targets(), answers, Some(crate::config::HOOK_ON))
}

/// The CLIs agreed to, split by what may be done about them without asking:
/// kept right as they are (`.0`), or asked about again first because this
/// version adds events to their hook ([`new_events`]; `.1`)
pub fn agreed_split(answers: &std::collections::BTreeMap<String, String>) -> (Vec<Target>, Vec<Target>) {
    agreed(answers).into_iter().partition(|t| new_events(t).is_empty())
}

/// Of `all`, the ones used here whose answer is `answer` (`None`: none yet)
fn among(all: Vec<Target>, answers: &std::collections::BTreeMap<String, String>, answer: Option<&str>) -> Vec<Target> {
    all.into_iter()
        .filter(|t| in_use(t) && answers.get(&t.name).map(String::as_str) == answer)
        .collect()
}

/// The question about `clis`, as the board draws it
pub fn question(seq: u64, clis: &[Target]) -> crate::uistate::HookAskState {
    crate::uistate::HookAskState {
        seq,
        clis: clis
            .iter()
            .map(|t| crate::uistate::HookAskCli {
                name: t.name.clone(),
                file: t.file.display().to_string(),
                preview: preview(t),
                approval: approval_file(t).map(|f| f.display().to_string()).unwrap_or_default(),
                added: new_events(t),
            })
            .collect(),
        machine: String::new(),
        found: false,
    }
}

/// Where the CLI keeps its approval of a hook, for one that holds new hooks
/// back ([`HookTrust`])
pub fn approval_file(t: &Target) -> Option<PathBuf> {
    match t.trust? {
        HookTrust::CodexAppServer => Some(t.file.with_file_name("config.toml")),
    }
}

/// What [`keep_right`] had to do
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// The entry was written: it was missing, or named another copy of this app
    pub written: bool,
    /// How many hooks the CLI was told are approved
    pub approved: usize,
}

/// Bring one agreed CLI's hook to what it should be: written where it is
/// missing or names another copy of this app, and approved by a CLI that
/// wants that. Slow for a CLI that is asked to approve, so called off the
/// thread that draws. An error leaves what was already done in place
pub fn keep_right(t: &Target) -> Result<Kept> {
    let mut kept = Kept::default();
    match status(t) {
        Status::Installed => {}
        // Another copy of this app on this PC, complete and still there: it
        // runs a hook exactly as well as this one does. Rewritten, two copies
        // that both keep hooks right would take the file from each other at
        // every start
        Status::Stale if program_named_in(t).is_some_and(|p| p.is_file() && status_of(t, &p) == Status::Installed) => {}
        Status::Absent | Status::NoConfig | Status::Stale => {
            install(t)?;
            kept.written = true;
        }
        Status::Unreadable(why) => anyhow::bail!(
            "{}: {why}",
            crate::i18n::tp("err.hookfile.unreadable", &[("path", &t.file.display().to_string())])
        ),
    }
    kept.approved = approve(t)?;
    Ok(kept)
}

/// Have the CLI approve the hooks of ours in `t`'s file, for a CLI that holds
/// a new hook back until it is approved ([`HookTrust`]). Only ours: a hook
/// the person put there themselves is theirs to approve.
///
/// `Ok(n)`: `n` approved just now, 0 when every one of them already was. Slow
/// (a program is started and asked), so called off the thread that draws
pub fn approve(t: &Target) -> Result<usize> {
    match t.trust {
        None => Ok(0),
        Some(HookTrust::CodexAppServer) => codex_approve(&t.file),
    }
}

/// How long Codex's app server may take over any one answer. It starts in
/// about a second here; a first run after an update takes longer
const APP_SERVER_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// The approval, asked of Codex itself (`codex app-server`, JSON-RPC over its
/// input and output): which hooks it knows and whether each is approved
/// (`hooks/list`), the approval of ours written the way its own "trust"
/// button writes it (`config/batchWrite` into `hooks.state`, the hash being
/// the one Codex computed), and asked again to be sure it took
fn codex_approve(file: &Path) -> Result<usize> {
    // Told which of its homes this is rather than left to find its own:
    // Codex finds its home by asking Windows, not by the USERPROFILE the
    // file was found by, and the approval has to land beside the hooks it is
    // for (measured 2026-09-30 with a home of a test's own)
    let codex_home = file.parent().context("the hook file has no folder")?;
    let mut codex = AppServer::start(codex_home)?;
    codex.call("initialize", serde_json::json!({ "clientInfo": { "name": "shikisha", "version": env!("CARGO_PKG_VERSION") } }))?;
    codex.note("initialized")?;
    let home = file.parent().and_then(|d| d.parent()).unwrap_or(file);
    let ask = serde_json::json!({ "cwds": [home.display().to_string()] });
    let wanted = to_approve(&codex.call("hooks/list", ask.clone())?, file);
    if wanted.is_empty() {
        return Ok(0);
    }
    let value: serde_json::Map<String, serde_json::Value> = wanted
        .iter()
        .map(|(key, hash)| (key.clone(), serde_json::json!({ "trusted_hash": hash })))
        .collect();
    codex.call(
        "config/batchWrite",
        serde_json::json!({
            "edits": [{ "keyPath": "hooks.state", "value": value, "mergeStrategy": "upsert" }],
            "reloadUserConfig": true,
        }),
    )?;
    let left = to_approve(&codex.call("hooks/list", ask)?, file);
    if !left.is_empty() {
        anyhow::bail!(crate::i18n::tp("err.hook.trust.not_taken", &[("n", &left.len().to_string())]));
    }
    crate::append_hook_log(&format!("Codex approved {} hook(s) of ours in {}", wanted.len(), file.display()));
    Ok(wanted.len())
}

/// Of the hooks a `hooks/list` answer names, the ones of ours in `file` that
/// Codex does not yet run, as (key, the hash to approve). Pure, so what is
/// picked can be checked: nobody else's hook, and no hook of another file
fn to_approve(listed: &serde_json::Value, file: &Path) -> Vec<(String, String)> {
    let same = |p: &str| {
        let a = Path::new(p);
        a == file || a.to_string_lossy().eq_ignore_ascii_case(&file.to_string_lossy())
    };
    listed
        .pointer("/data")
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
        .flat_map(|at| at.pointer("/hooks").and_then(|h| h.as_array()).into_iter().flatten())
        .filter(|h| h.get("sourcePath").and_then(|p| p.as_str()).is_some_and(same))
        .filter(|h| is_ours(h))
        .filter(|h| h.get("trustStatus").and_then(|s| s.as_str()) != Some("trusted"))
        .filter_map(|h| Some((h.get("key")?.as_str()?.to_string(), h.get("currentHash")?.as_str()?.to_string())))
        .fold(Vec::new(), |mut out, kh| {
            if !out.contains(&kh) {
                out.push(kh);
            }
            out
        })
}

/// Codex's app server, started for one conversation and ended with it.
///
/// Held in a job object with everything it starts, so an npm shim's `node`
/// ends with it rather than staying behind
struct AppServer {
    child: std::process::Child,
    input: Option<std::process::ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
    next: u64,
    _job: Option<crate::job::Job>,
}

impl AppServer {
    fn start(codex_home: &Path) -> Result<Self> {
        use std::process::{Command, Stdio};
        let path = crate::tab::resolve_command("codex").unwrap_or_else(|| PathBuf::from("codex"));
        let script = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"));
        let mut cmd = if script {
            let mut c = Command::new("cmd.exe");
            c.arg("/c").arg(&path);
            c
        } else {
            Command::new(&path)
        };
        cmd.arg("app-server")
            .env("CODEX_HOME", codex_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::detach_console(&mut cmd);
        let mut child = cmd
            .spawn()
            .with_context(|| crate::i18n::tp("err.hook.trust.no_codex", &[("path", &path.display().to_string())]))?;
        let job = crate::job::Job::new();
        if let Some(j) = &job {
            j.take(child.id());
        }
        let out = child.stdout.take().context("no output from codex app-server")?;
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead as _;
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let input = child.stdin.take();
        Ok(Self { child, input, lines, next: 0, _job: job })
    }

    fn write(&mut self, message: serde_json::Value) -> Result<()> {
        use std::io::Write as _;
        let input = self.input.as_mut().context("codex app-server closed its input")?;
        writeln!(input, "{message}")?;
        input.flush()?;
        Ok(())
    }

    /// A request, and its answer. Whatever else it says meanwhile (notices
    /// of its own) is passed over
    fn call(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        self.next += 1;
        let id = self.next;
        self.write(serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let until = std::time::Instant::now() + APP_SERVER_WAIT;
        loop {
            let left = until.saturating_duration_since(std::time::Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .map_err(|_| anyhow::anyhow!(crate::i18n::tp("err.hook.trust.no_answer", &[("what", method)])))?;
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if v.get("id").and_then(|i| i.as_u64()) != Some(id) {
                continue;
            }
            if let Some(e) = v.get("error") {
                anyhow::bail!("codex app-server, {method}: {}", e.get("message").and_then(|m| m.as_str()).unwrap_or("error"));
            }
            return Ok(v.get("result").cloned().unwrap_or_default());
        }
    }

    fn note(&mut self, method: &str) -> Result<()> {
        self.write(serde_json::json!({ "jsonrpc": "2.0", "method": method }))
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        // Its input closed is its cue to go; the job ends whatever is left
        self.input = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn edit(t: &Target, want: bool, program: &Path) -> Result<()> {
    // Said before anything is written, not discovered later by a dot that
    // never moves: a CLI that will not take quotes cannot be handed a path
    // with a space in it, and on a volume with no short names there is no
    // second spelling to fall back on
    if want && t.format == HookFormat::Bare && spaceless(program).is_none() {
        anyhow::bail!(crate::i18n::tp(
            "err.hook.path_has_space",
            &[("name", &t.name), ("path", &program.display().to_string())]
        ));
    }
    let existing = std::fs::read_to_string(&t.file).ok();
    let mut doc: serde_json::Value = match existing.as_deref() {
        Some(text) => serde_json::from_str(text.trim_start_matches('\u{feff}')).with_context(|| {
            crate::i18n::tp(
                "err.hookfile.unreadable",
                &[("path", &t.file.display().to_string())],
            )
        })?,
        None if want => serde_json::json!({}),
        // Nothing to remove from a file that isn't there
        None => return Ok(()),
    };
    if !doc.is_object() {
        anyhow::bail!(crate::i18n::tp(
            "err.hookfile.unreadable",
            &[("path", &t.file.display().to_string())]
        ));
    }

    merge(&mut doc, t, want, program, None);

    if let Some(dir) = t.file.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    // The way back, before the change. Promised in the question that asks
    // to write here, so a way back that could not be kept stops the change
    if let Some(text) = existing.as_deref() {
        let bak = t.file.with_extension("bak");
        std::fs::write(&bak, text).with_context(|| {
            crate::i18n::tp("err.hookfile.no_backup", &[("path", &bak.display().to_string())])
        })?;
    }
    crate::crypto::write_atomic(&t.file, &serde_json::to_string_pretty(&doc)?)?;
    crate::append_hook_log(&format!(
        "{} hook {} in {}",
        t.name,
        if want { "installed" } else { "removed" },
        t.file.display()
    ));
    Ok(())
}

/// Put ours into a CLI's settings as read (`want`), or take ours out of them,
/// leaving everything else where it was. With `marks`, each group of ours is
/// put in as a stand-in naming its place in `marks` instead, for [`preview`]
/// to find in the text and draw as ours
fn merge(doc: &mut serde_json::Value, t: &Target, want: bool, program: &Path, marks: Option<&mut Vec<serde_json::Value>>) {
    merge_with(doc, wanted(t, program), want, marks)
}

/// [`merge`], given what is wanted of each event rather than working it out
/// for this PC: the same edit serves a hook file on another machine, whose
/// entries are spelled for that machine ([`far_wanted`])
fn merge_with(
    doc: &mut serde_json::Value,
    wanted: Vec<(String, Vec<serde_json::Value>)>,
    want: bool,
    mut marks: Option<&mut Vec<serde_json::Value>>,
) {
    for (event, hs) in wanted {
        let list = doc
            .as_object_mut()
            .and_then(|o| o.entry("hooks").or_insert_with(|| serde_json::json!({})).as_object_mut())
            .map(|h| h.entry(event).or_insert_with(|| serde_json::json!([])));
        let Some(slot) = list else { continue };
        if !slot.is_array() {
            *slot = serde_json::json!([]);
        }
        let groups = slot.as_array_mut().expect("just made an array");
        // Ours goes, whether we are replacing it or removing it. Everyone
        // else's stays exactly where it was
        for group in groups.iter_mut() {
            if let Some(hs) = group.pointer_mut("/hooks").and_then(|h| h.as_array_mut()) {
                hs.retain(|h| !is_ours(h));
            }
        }
        groups.retain(|g| {
            g.pointer("/hooks")
                .and_then(|h| h.as_array())
                .map(|h| !h.is_empty())
                .unwrap_or(true)
        });
        if want {
            let group = serde_json::json!({ "hooks": hs });
            match marks.as_deref_mut() {
                Some(marks) => {
                    groups.push(serde_json::json!(format!("{MARKED}{}", marks.len())));
                    marks.push(group);
                }
                None => groups.push(group),
            }
        }
    }
    // Leave no empty scaffolding behind after a removal — including the map
    // itself when we were the only thing in it, which is the whole file for a
    // CLI whose hook config exists because we made it
    if !want
        && let Some(hooks) = doc.get_mut("hooks").and_then(|h| h.as_object_mut())
    {
        hooks.retain(|_, v| !v.as_array().map(|a| a.is_empty()).unwrap_or(false));
        let empty = hooks.is_empty();
        if let Some(o) = doc.as_object_mut().filter(|_| empty) {
            o.shift_remove("hooks");
        }
    }
}

/// What one hook event is worth keeping, once the CLI's JSON has been read.
///
/// Pure, so it can be tested: this is the only place on the hook path that
/// makes a judgment, and it runs inside a child process of the agent that is
/// not allowed to fail loudly.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The conversation this is, when the event says
    pub id: Option<String>,
    /// What to tell the tab it is doing, in this app's own vocabulary
    pub state: Option<String>,
    /// What a person just asked, when the event is the one carrying it
    pub prompt: Option<String>,
    /// A helper that began (`true`) or ended (`false`), by the CLI's id for it
    pub helper: Option<(String, bool)>,
    /// Everything the CLI says is still running beside the conversation, when
    /// the event carries its list: the helpers by id, and whether anything
    /// that is not a helper (a command left running) is still going. The
    /// whole picture as of that moment, so it replaces what was known
    pub running: Option<Running>,
}

/// What a CLI said is still running beside a conversation (see [`Report::running`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Running {
    pub helpers: Vec<String>,
    pub other: bool,
}

/// Which way a helper went, as a hook entry for it says (`--hook helper:up`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Helper {
    Up,
    Down,
}

impl Helper {
    /// The profile's word for it
    pub fn of(word: &str) -> Option<Helper> {
        match word.trim() {
            "up" => Some(Helper::Up),
            "down" => Some(Helper::Down),
            _ => None,
        }
    }

    /// What the hook entry asks to be run for it
    pub fn arg(self) -> &'static str {
        match self {
            Helper::Up => "helper:up",
            Helper::Down => "helper:down",
        }
    }
}

/// The CLI's id for the helper an event is about, when it is about one.
///
/// Every event a helper sends carries the parent conversation's id and this
/// one beside it: it is how a helper's events are told from the parent's
fn helper_id(v: &serde_json::Value) -> Option<String> {
    v.get("agent_id").and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// The kinds of thing a CLI lists as running that are helpers of the kind
/// that ends: a subagent the conversation started. A teammate is not among
/// them -- it is listed as running for as long as the team exists, whether or
/// not it is doing anything, so counting it would hold the tab at work for
/// good. A command left running is counted as other work
const HELPER_KINDS: &[&str] = &["subagent", "local_agent", "local_subagent"];
const STANDING_KINDS: &[&str] = &["teammate"];

/// A status that says the thing has ended, however it ended. Anything else --
/// a word this list does not know included -- is read as still running: a
/// tab held at work a little too long is put right by the next list, while
/// one let go of too early says "done" under work that is still going
fn ended(status: &str) -> bool {
    matches!(
        status,
        "done" | "finished" | "complete" | "completed" | "succeeded" | "success" | "stopped" | "exited"
            | "terminated" | "killed" | "cancelled" | "canceled" | "aborted" | "failed" | "error" | "errored"
            | "crashed" | "timed_out" | "timeout" | "expired" | "skipped" | "idle"
    )
}

/// What is running beside the conversation, from the list a CLI hands its
/// hook as the turn ends. `None` when the event carries no list (an older CLI,
/// or an event that never does): then nothing is known, and what was known
/// stands
pub fn running_of(v: &serde_json::Value) -> Option<Running> {
    let list = v.get("background_tasks")?.as_array()?;
    let mut out = Running::default();
    for item in list {
        let kind = item.get("type").and_then(|x| x.as_str()).unwrap_or("").trim().to_ascii_lowercase();
        let status = item.get("status").and_then(|x| x.as_str()).unwrap_or("").trim().to_ascii_lowercase();
        if !status.is_empty() && ended(&status) {
            continue;
        }
        if STANDING_KINDS.contains(&kind.as_str()) {
            continue;
        }
        let id = item.get("id").and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty());
        match (HELPER_KINDS.contains(&kind.as_str()), id) {
            (true, Some(id)) => out.helpers.push(id.to_string()),
            // A helper with no id cannot be told apart from the next one, and
            // anything of a kind nobody here knows is something still going
            _ => out.other = true,
        }
    }
    Some(out)
}

/// `kind` is what the hook entry asked for: `session`, or `state:<STATE>`.
pub fn report_of(kind: &str, v: &serde_json::Value) -> Report {
    // The same fact goes by several names across the CLIs that report it, and a
    // CLI is free to rename it in its next release. Read every spelling anyone
    // is known to use rather than one and a shrug
    let id = ["session_id", "sessionId", "conversation_id", "conversationId"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    if let Some(way) = kind.strip_prefix("helper:") {
        let helper = Helper::of(way).zip(helper_id(v)).map(|(h, id)| (id, h == Helper::Up));
        return Report { helper, ..Default::default() };
    }
    let Some(state) = kind.strip_prefix("state:") else {
        // A conversation begun afresh has nothing running beside it yet; one
        // taken up again, or packed down to save room, carries on with what
        // it had
        let fresh = kind == "session"
            && matches!(v.get("source").and_then(|s| s.as_str()), Some("startup") | Some("clear"));
        return Report {
            id: id.filter(|_| kind == "session"),
            running: fresh.then(Running::default),
            ..Default::default()
        };
    };
    // A subagent's events carry its parent's session id, so its "finished"
    // would put the whole tab back to rest while the real turn runs on. The
    // one thing a subagent has to say that cannot wait is that it is asking
    // for permission — that dialog is in front of the person either way.
    //
    // The id is left to the event that exists to carry it: reporting it from
    // every event would write the same line into the log all day
    let sub = ["agent_id", "agent_type"]
        .iter()
        .any(|k| v.get(*k).is_some_and(|x| !x.is_null()));
    let keep = !sub || state.eq_ignore_ascii_case("QUESTION");
    // The request itself rides on the event that says one was sent. A
    // subagent's prompt is the main agent talking to it, not a person
    let prompt = (!sub)
        .then(|| v.get("prompt").and_then(|p| p.as_str()))
        .flatten()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    // A helper whose turn failed (its request to the model refused, a limit
    // hit) sends no "ended" of its own -- this failure is its end
    let helper = (sub && state.eq_ignore_ascii_case("FAILED")).then(|| helper_id(v)).flatten().map(|id| (id, false));
    // What is still running, from the parent's own end of its turn only: a
    // helper's list is not the conversation's
    let running = (!sub).then(|| running_of(v)).flatten();
    Report { id: None, state: keep.then(|| state.to_string()), prompt, helper, running }
}

// ── On another machine ───────────────────────────────────────────────────
// An AI in a folder on a MicroVM or a server has no pipe back to this app:
// the program it would run to report (`--hook`) is on this PC, not there. So
// the hook written there says it to the terminal the AI is running in --
// an escape of this app's own, written to /dev/tty -- and the terminal here,
// which reads everything that terminal shows, takes it off the stream
// (`tab::QueryResponder::unhandled_osc`). Nothing is asked of the machine to
// hear it, so a report costs no request, and wakes nothing.

/// The escape a hook on another machine reports with:
/// `ESC ] 7727 ; shikisha-hook ; <kind> ; <sent ms> ; <base64 of the event> BEL`
pub const FAR_OSC: &str = "7727";
pub const FAR_OSC_TAG: &str = "shikisha-hook";

/// The line a hook on another machine runs: the event's JSON, as the CLI hands
/// it over, said to the terminal. `: shikisha --hook <arg>` in front is how it
/// is known again as this app's (see `is_ours`), and does nothing
pub fn far_line(arg: &str) -> String {
    format!(
        ": shikisha {MARK} {arg}; b=$(base64 | tr -d '\\n'); \
printf '\\033]{FAR_OSC};{FAR_OSC_TAG};%s;%s;%s\\007' '{arg}' \"$(date +%s%3N)\" \"$b\" > /dev/tty 2>/dev/null; true"
    )
}

fn far_handler(format: HookFormat, timeout: u32, arg: &str) -> serde_json::Value {
    let line = far_line(arg);
    match format {
        HookFormat::Args => serde_json::json!({
            "type": "command",
            "command": "sh",
            "args": ["-c", line],
            "timeout": timeout,
            "async": true,
        }),
        HookFormat::Bare | HookFormat::Shell => serde_json::json!({
            "type": "command",
            "command": line,
            "timeout": timeout,
            "async": true,
        }),
    }
}

/// A CLI's hook file on another machine: where it is there, from the
/// profile's `{home}` -- a MicroVM's home is fixed, a server's is where its
/// file commands start
pub fn far_file(t: &Target, profile_file: &str, on_microvm: bool) -> Option<String> {
    let _ = t;
    let rest = profile_file.strip_prefix("{home}")?.trim_start_matches(['/', '\\']);
    let rest = rest.replace('\\', "/");
    Some(match on_microvm {
        true => format!("/home/user/{rest}"),
        false => rest,
    })
}

/// What this app writes into a CLI's hook file on another machine, per event
fn far_wanted(t: &Target) -> Vec<(String, Vec<serde_json::Value>)> {
    let mut wanted: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
    for entry in &t.entries {
        let h = far_handler(t.format, t.timeout, &entry.arg);
        match wanted.iter_mut().find(|(event, _)| *event == entry.event) {
            Some((_, list)) => list.push(h),
            None => wanted.push((entry.event.clone(), vec![h])),
        }
    }
    wanted
}

/// The hook file's text with this app's entries for another machine in it,
/// everything else in it as it was. `None` when there is nothing to change,
/// or when the file is not JSON this can read -- a file that does not parse
/// is left exactly as it is, there as here
pub fn far_edited(t: &Target, existing: Option<&str>) -> Option<String> {
    let mut doc: serde_json::Value = match existing {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?,
        _ => serde_json::json!({}),
    };
    if !doc.is_object() {
        return None;
    }
    let before = doc.clone();
    for (event, hs) in far_wanted(t) {
        let Some(slot) = doc
            .as_object_mut()
            .and_then(|o| o.entry("hooks").or_insert_with(|| serde_json::json!({})).as_object_mut())
            .map(|h| h.entry(event).or_insert_with(|| serde_json::json!([])))
        else {
            continue;
        };
        if !slot.is_array() {
            *slot = serde_json::json!([]);
        }
        let groups = slot.as_array_mut().expect("just made an array");
        // Already exactly what would be written: left alone
        let ours: Vec<&serde_json::Value> = groups
            .iter()
            .filter_map(|g| g.pointer("/hooks").and_then(|h| h.as_array()))
            .flatten()
            .filter(|h| is_ours(h))
            .collect();
        if ours.len() == hs.len() && ours.iter().all(|h| hs.contains(h)) {
            continue;
        }
        for group in groups.iter_mut() {
            if let Some(list) = group.pointer_mut("/hooks").and_then(|h| h.as_array_mut()) {
                list.retain(|h| !is_ours(h));
            }
        }
        groups.retain(|g| g.pointer("/hooks").and_then(|h| h.as_array()).is_none_or(|h| !h.is_empty()));
        groups.push(serde_json::json!({ "hooks": hs }));
    }
    (doc != before).then(|| serde_json::to_string_pretty(&doc).unwrap_or_default())
}

/// The hook file's text with every entry of this app's taken out, whatever
/// event it sits under and whichever version wrote it, and everything else
/// as it was. `None` when there is none of ours in it, or when it is not JSON
/// this can read (left exactly as it is)
pub fn far_without(existing: Option<&str>) -> Option<String> {
    let text = existing.filter(|t| !t.trim().is_empty())?;
    let mut doc: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let before = doc.clone();
    let hooks = doc.get_mut("hooks").and_then(|h| h.as_object_mut())?;
    for groups in hooks.values_mut().filter_map(|g| g.as_array_mut()) {
        for group in groups.iter_mut() {
            if let Some(list) = group.pointer_mut("/hooks").and_then(|h| h.as_array_mut()) {
                list.retain(|h| !is_ours(h));
            }
        }
        groups.retain(|g| g.pointer("/hooks").and_then(|h| h.as_array()).is_none_or(|h| !h.is_empty()));
    }
    // An event that only held ours goes with it, and the map when nothing is left
    hooks.retain(|_, v| !v.as_array().is_some_and(|a| a.is_empty()));
    let empty = hooks.is_empty();
    if let Some(o) = doc.as_object_mut().filter(|_| empty) {
        o.shift_remove("hooks");
    }
    (doc != before).then(|| serde_json::to_string_pretty(&doc).unwrap_or_default())
}

/// What a hook file on another machine says about this app, read without
/// changing anything
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FarSeen {
    /// An entry of this app's is in it
    pub ours: bool,
    /// Events this version asks for that the file carries none of ours for,
    /// while it carries ours for others: what an update adds to a hook that
    /// was already there ([`new_events`])
    pub added: Vec<String>,
}

pub fn far_seen(t: &Target, existing: Option<&str>) -> FarSeen {
    let Some(doc) = existing
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s.trim_start_matches('\u{feff}')).ok())
    else {
        return FarSeen::default();
    };
    let ours = doc
        .get("hooks")
        .and_then(|h| h.as_object())
        .into_iter()
        .flat_map(|h| h.values())
        .filter_map(|g| g.as_array())
        .flatten()
        .filter_map(|g| g.pointer("/hooks").and_then(|h| h.as_array()))
        .flatten()
        .any(is_ours);
    FarSeen { ours, added: new_events_in(&doc, t) }
}

/// The hook file on another machine as it would be once this app's entries
/// are in, a line at a time, the lines this app adds told apart ([`preview`])
pub fn far_preview(t: &Target, existing: Option<&str>) -> Vec<Line> {
    preview_of(existing.filter(|s| !s.trim().is_empty()), far_wanted(t))
}

/// The profiles' hook targets, with the file each names as the profile wrote
/// it (`{home}/...`), for placing on another machine
pub fn far_targets() -> Vec<(Target, String)> {
    crate::profile::all()
        .into_iter()
        .filter_map(|p| {
            let file = p.resume.as_ref()?.hook.as_ref()?.file.clone();
            let mut t = targets().into_iter().find(|t| t.name == p.name)?;
            // What a hook there says reaches this app one way, as a line on
            // the terminal: nothing goes back to it, so nothing is asked of
            // it that would need an answer
            t.entries.retain(|e| e.arg != LINE_ARG);
            Some((t, file))
        })
        .collect()
}

// ── Consent, per machine ──────────────────────────
//
// A hook on another machine is written into somebody's settings on a machine
// that is not this one, so it is asked about first, the way this PC's are
// (`question`), and only ever written after "yes". The answer is kept per
// machine -- by the name its entry in the settings has, as the bridge is
// (`config::Config::bridges`) -- and per CLI: agreeing for Claude Code on a
// server is not agreeing for Codex there, whose file was never shown.
//
// Nothing here writes on its own account. `far_look` reads the file and
// decides: kept right when agreed, taken out when refused, asked about when
// there is no answer -- including a file an older version of this app wrote
// into without asking, which is asked about once (keep or take out) rather
// than kept or deleted silently.

/// The answers, as the settings last said ([`crate::config::Config::far_hooks`])
static FAR_ANSWERS: std::sync::Mutex<std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Take the answers the settings hold now. A machine whose answers changed
/// is looked at again (see [`far_look`]): unticked in the settings, its hook
/// comes out the next time the machine is reached
pub fn far_answers_now(answers: &std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>) {
    let Ok(mut now) = FAR_ANSWERS.lock() else { return };
    if *now == *answers {
        return;
    }
    let changed: Vec<String> = now
        .keys()
        .chain(answers.keys())
        .filter(|h| now.get(*h) != answers.get(*h))
        .cloned()
        .collect();
    *now = answers.clone();
    drop(now);
    if let Ok(mut looked) = FAR_LOOKED.lock() {
        looked.retain(|k, _| !changed.iter().any(|h| k.0 == *h));
    }
}

/// What the person said about `cli` on the machine `host`: `None` when they
/// have not been asked
pub fn far_answer(host: &str, cli: &str) -> Option<bool> {
    FAR_ANSWERS
        .lock()
        .ok()?
        .get(host)?
        .get(cli)
        .map(|a| a == crate::config::HOOK_ON)
}

/// The CLIs whose hook the person refused, or unticked, on the machine
/// `host`: looked at whenever any tab reaches that machine, not only an AI's,
/// so that an entry to take out does not wait for an AI to be started there
pub fn far_refused(host: &str) -> Vec<String> {
    FAR_ANSWERS
        .lock()
        .ok()
        .and_then(|a| a.get(host).cloned())
        .map(|m| m.into_iter().filter(|(_, a)| a == crate::config::HOOK_OFF).map(|(cli, _)| cli).collect())
        .unwrap_or_default()
}

/// A question to put to the person about one CLI's hook on one machine
#[derive(Debug, Clone)]
pub struct FarAsk {
    /// The machine's entry name, the key its answer is kept under
    pub host: String,
    pub at: crate::elsewhere::Elsewhere,
    pub cli: Target,
    /// The file there, as that machine spells it
    pub file: String,
    pub preview: Vec<Line>,
    /// An older version already wrote ours there without asking: the answers
    /// are "keep" and "take out"
    pub found: bool,
    /// Agreed to already, asked again because this version adds these events
    pub added: Vec<String>,
}

/// The question about `ask`, as the board draws it ([`question`]'s far twin)
pub fn far_question(seq: u64, ask: &FarAsk) -> crate::uistate::HookAskState {
    crate::uistate::HookAskState {
        seq,
        clis: vec![crate::uistate::HookAskCli {
            name: ask.cli.name.clone(),
            // A path a server's file commands take from its home, said the way
            // a person reads one there
            file: if ask.file.starts_with('/') { ask.file.clone() } else { format!("~/{}", ask.file) },
            preview: ask.preview.clone(),
            approval: String::new(),
            added: ask.added.clone(),
        }],
        machine: ask.host.clone(),
        found: ask.found,
    }
}

/// Questions found by [`far_look`], waiting for the board
static FAR_ASKS: std::sync::Mutex<Vec<FarAsk>> = std::sync::Mutex::new(Vec::new());

/// The questions found since the last call
pub fn take_far_asks() -> Vec<FarAsk> {
    FAR_ASKS.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default()
}

/// Which machine and CLI pairs have been looked at in this run, and when a
/// look that could not read the file may be tried again
static FAR_LOOKED: std::sync::Mutex<std::collections::BTreeMap<(String, String, String), Option<std::time::Instant>>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// How long after a failed read the same file is tried again. A machine that
/// did not answer is usually still starting or briefly away; asking it again
/// on every pass of the loop would only fill the log
const FAR_RETRY: std::time::Duration = std::time::Duration::from_secs(120);

/// Look at the CLI `profile`'s hook file on the machine `host` (reached as
/// `at`; `machine` tells one of its machines from another) once in this run,
/// on a thread, and do what the person's answer says -- or ask. Reads only,
/// unless the answer is on record
pub fn far_look(at: crate::elsewhere::Elsewhere, host: String, machine: String, profile: String) {
    let key = (host.clone(), machine, profile.clone());
    {
        let Ok(mut looked) = FAR_LOOKED.lock() else { return };
        match looked.get(&key) {
            Some(None) => return,
            Some(Some(again)) if std::time::Instant::now() < *again => return,
            _ => {}
        }
        looked.insert(key.clone(), None);
    }
    std::thread::spawn(move || {
        if far_look_now(&at, &host, &profile).is_err()
            && let Ok(mut looked) = FAR_LOOKED.lock()
        {
            looked.insert(key, Some(std::time::Instant::now() + FAR_RETRY));
        }
    });
}

fn far_look_now(at: &crate::elsewhere::Elsewhere, host: &str, profile: &str) -> Result<(), String> {
    let on_microvm = matches!(at, crate::elsewhere::Elsewhere::Cloud(_));
    for (t, file) in far_targets().into_iter().filter(|(t, _)| t.name == profile) {
        let Some(path) = far_file(&t, &file, on_microvm) else { continue };
        let existing = far_read(at, &path, &t.name)?;
        let seen = far_seen(&t, existing.as_deref());
        match far_answer(host, &t.name) {
            // Agreed: kept right -- unless this version would add events,
            // which were not shown when it was agreed to
            Some(true) if seen.added.is_empty() => far_put(at, &t, &path, existing.as_deref())?,
            Some(true) => far_ask(host, at, &t, &path, existing.as_deref(), false, seen.added),
            // Refused, here or in the settings: ours comes out
            Some(false) => {
                if seen.ours {
                    far_take_out(at, &t, &path, existing.as_deref())?;
                }
            }
            None => far_ask(host, at, &t, &path, existing.as_deref(), seen.ours, Vec::new()),
        }
    }
    Ok(())
}

fn far_ask(
    host: &str,
    at: &crate::elsewhere::Elsewhere,
    t: &Target,
    path: &str,
    existing: Option<&str>,
    found: bool,
    added: Vec<String>,
) {
    let ask = FarAsk {
        host: host.to_string(),
        at: at.clone(),
        cli: t.clone(),
        file: path.to_string(),
        preview: far_preview(t, existing),
        found,
        added,
    };
    crate::append_hook_log(&format!(
        "far hooks: asking about {} on {}{}",
        t.name,
        host,
        if found { " (already written there by an earlier version)" } else { "" }
    ));
    if let Ok(mut asks) = FAR_ASKS.lock() {
        asks.push(ask);
    }
}

/// The person's answer about `t` on the machine `at`, carried out there: put
/// in (every event this version asks for) or taken out. On a thread; what
/// happened goes to the log, and `said` hears one line for the screen
pub fn far_apply(at: crate::elsewhere::Elsewhere, t: Target, path: String, on: bool, said: std::sync::mpsc::Sender<String>) {
    std::thread::spawn(move || {
        let done = far_read(&at, &path, &t.name).and_then(|existing| match on {
            true => far_put(&at, &t, &path, existing.as_deref()),
            false => far_take_out(&at, &t, &path, existing.as_deref()),
        });
        let line = match (&done, on) {
            (Ok(()), true) => crate::i18n::tp("msg.hooks.far.set_up", &[("name", &t.name), ("machine", &at.address())]),
            (Ok(()), false) => crate::i18n::tp("msg.hooks.far.off", &[("name", &t.name), ("machine", &at.address())]),
            (Err(why), _) => crate::i18n::tp("msg.hooks.far.failed", &[("why", why)]),
        };
        let _ = said.send(line);
    });
}

/// Take this app's entries out of every CLI's hook file on the machine a
/// settings entry names, there and then. For a server about to be taken out
/// of the settings, whose hooks would otherwise stay behind with nothing left
/// that could reach them. Answers what could not be done
pub fn far_take_out_all(host: &crate::config::HostSpec) -> Result<(), String> {
    let at = crate::elsewhere::Elsewhere::of(host).map_err(|e| format!("{e:#}"))?;
    let on_microvm = matches!(at, crate::elsewhere::Elsewhere::Cloud(_));
    let mut failed = Vec::new();
    for (t, file) in far_targets() {
        let Some(path) = far_file(&t, &file, on_microvm) else { continue };
        let done = far_read(&at, &path, &t.name).and_then(|existing| {
            if far_seen(&t, existing.as_deref()).ours {
                far_take_out(&at, &t, &path, existing.as_deref())
            } else {
                Ok(())
            }
        });
        if let Err(why) = done {
            failed.push(why);
        }
    }
    match failed.is_empty() {
        true => Ok(()),
        false => Err(failed.join("; ")),
    }
}

/// Put this app's hooks for the CLI `profile` into its file on the machine,
/// before the AI is started there, and waited for -- when the person agreed
/// to it for that machine, and only then. The AI reads its hooks when it
/// starts, so hooks written after the line that starts it reach only its next
/// start. Not agreed (or not asked yet): nothing is written, and the question
/// is asked on the screen as the tab runs. `line` is what is about to be
/// typed; the CLI is its first word
pub fn ensure_far_before(at: &crate::elsewhere::Elsewhere, host: &str, line: &str) {
    let head = line.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    if head.is_empty() {
        return;
    }
    let Some(profile) = crate::profile::files()
        .into_iter()
        .find(|p| p.command_match.iter().any(|m| m.trim().eq_ignore_ascii_case(&head)))
        .map(|p| p.name)
    else {
        return;
    };
    if far_answer(host, &profile) != Some(true) {
        return;
    }
    let on_microvm = matches!(at, crate::elsewhere::Elsewhere::Cloud(_));
    for (t, file) in far_targets().into_iter().filter(|(t, _)| t.name == profile) {
        let Some(path) = far_file(&t, &file, on_microvm) else { continue };
        let Ok(existing) = far_read(at, &path, &t.name) else { return };
        // Events this version adds were not what was agreed to: they wait
        // for the question, and what was agreed stays as it was
        if far_seen(&t, existing.as_deref()).added.is_empty() {
            let _ = far_put(at, &t, &path, existing.as_deref());
        }
    }
}

/// The hook file on the machine: its text, `None` when it is known not to be
/// there, an error when it could not be read -- a read that failed on the way
/// is not an empty file, and writing over it would take the person's own
/// settings with it
fn far_read(at: &crate::elsewhere::Elsewhere, path: &str, name: &str) -> Result<Option<String>, String> {
    match crate::elsewhere::files(at, crate::ssh::FileJob::Read { path: path.to_string() }, 30_000) {
        Ok(crate::ssh::FileAnswer::Bytes(b)) => Ok(Some(String::from_utf8_lossy(&b).to_string())),
        _ => {
            let quoted = crate::worktree::for_a_shell(&[path.to_string()]);
            match crate::elsewhere::exec(at, &format!("test -e {quoted}"), 30_000) {
                Ok(r) if !r.ok() => Ok(None),
                _ => {
                    let why = format!("could not read the {name} hook file {path} on {}; left alone", at.address());
                    crate::append_hook_log(&why);
                    Err(why)
                }
            }
        }
    }
}

/// Where the way back is kept beside a hook file, as on this PC
/// (`settings.json` -> `settings.bak`)
fn far_bak(path: &str) -> String {
    match path.strip_suffix(".json") {
        Some(stem) => format!("{stem}.bak"),
        None => format!("{path}.bak"),
    }
}

/// Write `text` over the hook file, the file as it was kept beside it first.
/// A way back that could not be kept stops the change, as it does here
fn far_write(at: &crate::elsewhere::Elsewhere, t: &Target, path: &str, existing: Option<&str>, text: String, what: &str) -> Result<(), String> {
    if let Some(old) = existing {
        let bak = far_bak(path);
        if let Err(e) = crate::elsewhere::files(at, crate::ssh::FileJob::Write { to: bak.clone(), bytes: old.as_bytes().to_vec() }, 30_000) {
            let why = format!("could not keep {bak} on {} before changing the {} hook, so it was not changed: {e:#}", at.address(), t.name);
            crate::append_hook_log(&why);
            return Err(why);
        }
    }
    match crate::elsewhere::files(at, crate::ssh::FileJob::Write { to: path.to_string(), bytes: text.into_bytes() }, 30_000) {
        Ok(_) => {
            crate::append_hook_log(&format!("{} hook {what} in {path} on {}", t.name, at.address()));
            Ok(())
        }
        Err(e) => {
            let why = format!("could not change the {} hook in {path} on {}: {e:#}", t.name, at.address());
            crate::append_hook_log(&why);
            Err(why)
        }
    }
}

fn far_put(at: &crate::elsewhere::Elsewhere, t: &Target, path: &str, existing: Option<&str>) -> Result<(), String> {
    far_change(at, t, path, existing, &|now| far_edited(t, now), "written")
}

fn far_take_out(at: &crate::elsewhere::Elsewhere, t: &Target, path: &str, existing: Option<&str>) -> Result<(), String> {
    far_change(at, t, path, existing, &far_without, "taken out")
}

/// How many times a hook file that changed between being read and being
/// written is read again before giving up. A CLI rewrites its own settings
/// once per change it makes; a file that is different on each of three reads
/// is being written by something else that keeps at it, and a fourth read
/// would only race it again
const FAR_TRIES: usize = 3;

/// What to write, worked out from the file as it is at the moment of writing.
///
/// A hook file on another machine can be changed by the CLI there, or by the
/// person, between this program reading it and writing it back -- and
/// writing the text worked out from the earlier read would take that change
/// away, with a way back that holds the earlier file too. So it is read again
/// just before the write, and the write goes ahead only when it is still the
/// file the text was worked out from; otherwise the text is worked out again
/// from what is there now. `Ok(None)`: nothing to change. The moment between
/// the last read and the write itself cannot be closed from here -- the file
/// is written over a connection with no lock on it -- only made as short as
/// one round trip
fn far_settle(
    first: Option<&str>,
    edit: &dyn Fn(Option<&str>) -> Option<String>,
    mut read_again: impl FnMut() -> Result<Option<String>, String>,
) -> Result<Option<(Option<String>, String)>, FarRace> {
    let mut seen = first.map(str::to_string);
    for _ in 0..FAR_TRIES {
        let Some(text) = edit(seen.as_deref()) else { return Ok(None) };
        let now = read_again().map_err(FarRace::Unread)?;
        if now == seen {
            return Ok(Some((seen, text)));
        }
        seen = now;
    }
    Err(FarRace::KeptChanging)
}

/// Why a hook file was left alone while it was being changed
#[derive(Debug, PartialEq, Eq)]
enum FarRace {
    /// Reading it again failed
    Unread(String),
    /// It was different on every read
    KeptChanging,
}

/// Change a hook file on another machine to what `edit` makes of it, as it is
/// when written (`far_settle`), the file as it was then kept beside it
fn far_change(
    at: &crate::elsewhere::Elsewhere,
    t: &Target,
    path: &str,
    existing: Option<&str>,
    edit: &dyn Fn(Option<&str>) -> Option<String>,
    what: &str,
) -> Result<(), String> {
    match far_settle(existing, edit, || far_read(at, path, &t.name)) {
        Ok(None) => Ok(()),
        Ok(Some((base, text))) => far_write(at, t, path, base.as_deref(), text, what),
        Err(FarRace::Unread(why)) => Err(why),
        Err(FarRace::KeptChanging) => {
            let why = format!(
                "the {} hook file {path} on {} kept changing while it was being {what}, so it was left alone; try again when it is quiet",
                t.name,
                at.address()
            );
            crate::append_hook_log(&why);
            Err(why)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hook on another machine says to its terminal what the terminal here
    /// takes off the stream, and nothing else of the file is touched to put
    /// it there; a second pass changes nothing
    #[test]
    fn a_far_hook_is_written_beside_the_persons_own_and_only_once() {
        let line = far_line("state:BUSY");
        if let Ok(to) = std::env::var("SHIKISHA_FAR_LINE_OUT") {
            std::fs::write(to, &line).unwrap();
        }
        assert!(line.contains("/dev/tty") && line.contains(FAR_OSC) && line.contains(FAR_OSC_TAG));
        assert!(is_ours(&serde_json::json!({"type": "command", "command": line})), "our own line is not known again");

        let t = Target {
            name: "Test CLI".into(),
            file: PathBuf::from("unused"),
            format: HookFormat::Shell,
            timeout: TIMEOUT_S,
            line_timeout: LINE_TIMEOUT_S,
            trust: None,
            entries: vec![
                Entry { event: "SessionStart".into(), arg: "session".into() },
                Entry { event: "Stop".into(), arg: "state:DONE".into() },
            ],
        };
        let theirs = r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#;
        let once = far_edited(&t, Some(theirs)).expect("nothing was written");
        let v: serde_json::Value = serde_json::from_str(&once).unwrap();
        assert_eq!(v["model"], "opus", "the person's own setting is gone");
        assert!(once.contains("say done"), "the person's own hook is gone");
        assert!(once.contains("state:DONE") && once.contains("SessionStart"));
        assert_eq!(far_edited(&t, Some(&once)), None, "a second pass writes again");
        assert_eq!(far_edited(&t, Some("{ not json")), None, "a file that does not parse is written over");
    }

    /// A hook file changed over there between the read and the write is not
    /// written over with what was worked out from the older read: it is read
    /// again and worked out again, the person's change kept -- and a file
    /// that is different every time is left alone rather than raced. Before,
    /// the text from the first read went straight back, taking the change
    /// with it, and the way back kept beside it was the older file as well
    #[test]
    fn a_far_hook_file_changed_meanwhile_is_worked_out_again() {
        let t = far_test_target(&[("Stop", "state:DONE")]);
        let first = r#"{"model":"opus"}"#;
        let changed = r#"{"model":"opus","theme":"dark"}"#;
        let edit = |now: Option<&str>| far_edited(&t, now);
        // Changed once, then still: the write is made from the changed file
        let mut reads = vec![Ok(Some(changed.to_string())), Ok(Some(changed.to_string()))].into_iter();
        let (base, text) = far_settle(Some(first), &edit, || reads.next().unwrap()).unwrap().unwrap();
        assert_eq!(base.as_deref(), Some(changed), "the way back is the file as it was when written");
        assert!(text.contains("dark") && text.contains("state:DONE"), "the change made meanwhile is kept: {text}");
        // Unchanged: one read again, and the write goes ahead
        let mut reads = vec![Ok(Some(first.to_string()))].into_iter();
        assert!(far_settle(Some(first), &edit, || reads.next().unwrap()).unwrap().is_some());
        // Different every time: left alone
        let mut n = 0;
        let busy = far_settle(Some(first), &edit, || {
            n += 1;
            Ok(Some(format!(r#"{{"n":{n}}}"#)))
        });
        assert_eq!(busy, Err(FarRace::KeptChanging));
        // Nothing to change: not read again at all
        let done = far_edited(&t, Some(first)).unwrap();
        assert_eq!(far_settle(Some(&done), &edit, || panic!("read again for nothing")), Ok(None));
    }

    fn far_test_target(events: &[(&str, &str)]) -> Target {
        Target {
            name: "Test CLI".into(),
            file: PathBuf::from("unused"),
            format: HookFormat::Shell,
            timeout: TIMEOUT_S,
            line_timeout: LINE_TIMEOUT_S,
            trust: None,
            entries: events.iter().map(|(e, a)| Entry { event: (*e).into(), arg: (*a).into() }).collect(),
        }
    }

    /// Taking ours out of a file on another machine leaves the person's own
    /// hooks and settings exactly, takes ours out of every event (including
    /// one a newer version no longer asks for), and drops what only held ours
    #[test]
    fn a_far_hook_comes_out_and_nothing_of_the_persons_goes_with_it() {
        let t = far_test_target(&[("Stop", "state:DONE"), ("SessionStart", "session")]);
        let theirs = r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}"#;
        let with = far_edited(&t, Some(theirs)).unwrap();
        // One more of ours under an event this version does not ask for
        let mut v: serde_json::Value = serde_json::from_str(&with).unwrap();
        v["hooks"]["Old"] = serde_json::json!([{ "hooks": [far_handler(HookFormat::Shell, TIMEOUT_S, "state:BUSY")] }]);
        let with = serde_json::to_string_pretty(&v).unwrap();

        let out = far_without(Some(&with)).expect("nothing was taken out");
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(back["model"], "opus", "the person's own setting went");
        assert_eq!(back["hooks"]["Stop"][0]["hooks"][0]["command"], "say done", "the person's own hook went");
        assert!(!out.contains(MARK), "an entry of ours is still there");
        assert!(back["hooks"].get("SessionStart").is_none() && back["hooks"].get("Old").is_none(), "an event that only held ours stayed");

        assert_eq!(far_without(Some(theirs)), None, "a file with none of ours is written");
        assert_eq!(far_without(Some("{ not json")), None, "a file that does not parse is written over");
        assert_eq!(far_without(None), None);
        // Ours alone: the list of hooks goes with it
        let only = far_edited(&t, None).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&far_without(Some(&only)).unwrap()).unwrap(), serde_json::json!({}));
    }

    /// What a file on another machine says: none of ours, ours as agreed, or
    /// ours from before an update that asks for more
    #[test]
    fn a_far_file_says_whether_ours_is_there_and_what_an_update_adds() {
        let older = far_test_target(&[("Stop", "state:DONE")]);
        let newer = far_test_target(&[("Stop", "state:DONE"), ("SubagentStart", "helper:start")]);
        assert_eq!(far_seen(&newer, None), FarSeen::default());
        assert_eq!(far_seen(&newer, Some(r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"say"}]}]}}"#)), FarSeen::default());
        let written = far_edited(&older, None).unwrap();
        assert_eq!(far_seen(&older, Some(&written)), FarSeen { ours: true, added: vec![] });
        assert_eq!(far_seen(&newer, Some(&written)), FarSeen { ours: true, added: vec!["SubagentStart".into()] });
        // The way back sits beside the file, as on this PC
        assert_eq!(far_bak(".claude/settings.json"), ".claude/settings.bak");
        assert_eq!(far_bak("/home/user/.gemini/hooks"), "/home/user/.gemini/hooks.bak");
        // What would be shown before writing: the person's lines and ours, told apart
        let lines = far_preview(&older, Some(r#"{"model":"opus"}"#));
        assert!(lines.iter().any(|l| !l.ours && l.text.contains("opus")));
        assert!(lines.iter().any(|l| l.ours && l.text.contains(MARK)));
    }

    /// The answers are kept per machine and per CLI, and a machine whose
    /// answers change is looked at again -- that is how unticking it in the
    /// settings takes the hook out
    #[test]
    fn far_answers_are_per_machine_and_a_change_looks_again() {
        let mut answers: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> = Default::default();
        answers.entry("far-test-a".into()).or_default().insert("Claude Code".into(), crate::config::HOOK_ON.into());
        answers.entry("far-test-b".into()).or_default().insert("Claude Code".into(), crate::config::HOOK_OFF.into());
        far_answers_now(&answers);
        assert_eq!(far_answer("far-test-a", "Claude Code"), Some(true));
        assert_eq!(far_answer("far-test-b", "Claude Code"), Some(false));
        assert_eq!(far_answer("far-test-a", "Codex CLI"), None, "an answer for one CLI was taken for another");
        assert_eq!(far_refused("far-test-b"), vec!["Claude Code".to_string()]);
        assert!(far_refused("far-test-a").is_empty());

        FAR_LOOKED.lock().unwrap().insert(("far-test-a".into(), "m1".into(), "Claude Code".into()), None);
        FAR_LOOKED.lock().unwrap().insert(("far-test-b".into(), "m2".into(), "Claude Code".into()), None);
        answers.get_mut("far-test-a").unwrap().insert("Claude Code".into(), crate::config::HOOK_OFF.into());
        far_answers_now(&answers);
        let looked = FAR_LOOKED.lock().unwrap();
        assert!(!looked.contains_key(&("far-test-a".into(), "m1".into(), "Claude Code".into())), "a changed machine is not looked at again");
        assert!(looked.contains_key(&("far-test-b".into(), "m2".into(), "Claude Code".into())), "an unchanged machine was looked at again");
    }

    fn target(dir: &std::path::Path, format: HookFormat) -> Target {
        Target {
            name: "Test CLI".into(),
            file: dir.join("hooks.json"),
            format,
            timeout: TIMEOUT_S,
            line_timeout: LINE_TIMEOUT_S,
            trust: None,
            entries: vec![Entry { event: "SessionStart".into(), arg: "session".into() }],
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-hook-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A hook set up by an earlier version is asked about again when this one
    /// adds events to it -- and only then: missing altogether is the first
    /// question's business, and complete is nothing to ask about
    #[test]
    fn events_this_version_adds_to_an_agreed_hook_are_named() {
        let dir = tmp("grown");
        let older = Target {
            name: "Test CLI".into(),
            file: dir.join("settings.json"),
            format: HookFormat::Args,
            timeout: TIMEOUT_S,
            line_timeout: LINE_TIMEOUT_S,
            trust: None,
            entries: vec![
                Entry { event: "SessionStart".into(), arg: "session".into() },
                Entry { event: "Stop".into(), arg: "state:DONE".into() },
            ],
        };
        // Not set up at all: not this question's
        assert!(new_events(&older).is_empty());
        install(&older).unwrap();
        assert!(new_events(&older).is_empty(), "a complete hook was said to have new events");
        let newer = Target {
            entries: [
                older.entries.clone(),
                vec![
                    Entry { event: "SubagentStart".into(), arg: "helper:up".into() },
                    Entry { event: "SubagentStop".into(), arg: "helper:down".into() },
                ],
            ]
            .concat(),
            ..older.clone()
        };
        assert_eq!(new_events(&newer), vec!["SubagentStart".to_string(), "SubagentStop".into()]);
        install(&newer).unwrap();
        assert!(new_events(&newer).is_empty(), "written, the events are still said to be new");
        // Something new asked of an event already carried is new too: the
        // end of a turn asked to answer back, as well as to report
        let answers = Target {
            entries: [newer.entries.clone(), vec![Entry { event: "Stop".into(), arg: LINE_ARG.into() }]].concat(),
            ..newer.clone()
        };
        assert_eq!(new_events(&answers), vec!["Stop".to_string()]);
        install(&answers).unwrap();
        assert!(new_events(&answers).is_empty());
        assert_eq!(status(&answers), Status::Installed);
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&answers.file).unwrap()).unwrap();
        let stop = doc["hooks"]["Stop"][0]["hooks"].as_array().unwrap();
        let line = stop.iter().find(|h| carries(h, LINE_ARG)).expect("the line hook is written");
        assert!(line.get("async").is_none(), "a hook that answers back is waited for");
        assert_eq!(line["timeout"], LINE_TIMEOUT_S);
        assert!(stop.iter().any(|h| carries(h, "state:DONE") && h["async"] == true), "the report stays out of the way");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installing_leaves_every_other_hook_exactly_where_it_was() {
        let dir = tmp("keep");
        let t = target(&dir, HookFormat::Args);
        let theirs = serde_json::json!({
            "model": "opus",
            "hooks": {
                "SessionStart": [{ "hooks": [{ "type": "command", "command": "their-tool" }] }],
                "Stop": [{ "hooks": [{ "type": "command", "command": "beep" }] }]
            }
        });
        std::fs::write(&t.file, serde_json::to_string_pretty(&theirs).unwrap()).unwrap();

        assert_eq!(status(&t), Status::Absent);
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(after["model"], "opus", "unrelated settings stay as they are");
        assert_eq!(after["hooks"]["Stop"], theirs["hooks"]["Stop"], "other events are untouched");
        let starts = after["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(starts.len(), 2, "it is added beside the other hooks");
        assert_eq!(starts[0], theirs["hooks"]["SessionStart"][0]);

        // The file it replaced is still there to go back to
        assert!(t.file.with_extension("bak").exists());

        // Installing twice does not stack up
        install(&t).unwrap();
        let again: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(again["hooks"]["SessionStart"].as_array().unwrap().len(), 2);

        // ...and removing ours puts the file back the way it was
        uninstall(&t).unwrap();
        let back: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(back, theirs, "it takes away only its own");
        assert_eq!(status(&t), Status::Absent);
    }

    /// Another app that installs a hook of its own into the same file keeps it.
    ///
    /// These files are shared ground: more than one program asks a CLI to
    /// report on itself, and they all write into the one settings file. The
    /// word "hook" is what every one of them is about, so a rule that removed
    /// anything mentioning it would quietly disable a neighbour -- and the
    /// person would see it as that other app going deaf, with nothing naming
    /// the app that did it. What makes an entry ours is our own name in it.
    #[test]
    fn another_apps_hook_in_the_same_file_is_left_alone() {
        let dir = tmp("neighbour");
        let t = target(&dir, HookFormat::Args);
        // Shaped like the ones really found in the wild: a script of theirs,
        // under the events we want too, and the word "hook" all through it
        let theirs = serde_json::json!({
            "hooks": {
                "SessionStart": [{ "hooks": [
                    { "type": "command", "command": "C:/Users/me/.other/agent-hooks/their-hook.cmd || echo {}", "timeout": 10 }
                ]}],
                "Stop": [{ "hooks": [
                    { "type": "command", "command": "other-tool --hook stop", "timeout": 10 }
                ]}]
            }
        });
        std::fs::write(&t.file, serde_json::to_string_pretty(&theirs).unwrap()).unwrap();

        install(&t).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        for event in ["SessionStart", "Stop"] {
            let theirs_here = &theirs["hooks"][event][0]["hooks"][0];
            let found = after["hooks"][event]
                .as_array()
                .unwrap()
                .iter()
                .any(|g| g["hooks"].as_array().is_some_and(|hs| hs.contains(theirs_here)));
            assert!(found, "the other app's {event} hook was taken out: {after}");
        }
        // ...and taking ours away leaves theirs exactly as it was written
        uninstall(&t).unwrap();
        let back: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(back, theirs, "removing ours disturbed the other app's");
    }

    /// A repair tool writes in the app it is repairing, not itself.
    ///
    /// What goes into these files is a command line the CLI runs on every
    /// turn. Taken from "whatever process is asking", a tool that puts the
    /// hook back would write ITSELF in -- and every turn afterwards would run
    /// a one-shot diagnostic instead of the app.
    #[test]
    fn the_program_written_in_is_the_one_named() {
        let dir = tmp("named");
        let t = target(&dir, HookFormat::Args);
        let elsewhere = dir.join("SomeWhereElse.exe");
        install_as(&t, &elsewhere).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        let said = after["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str().unwrap();
        assert_eq!(said, elsewhere.display().to_string(), "it wrote a different program in");
        // And the entry reads as installed for THAT app, while this build --
        // which is not what will run -- sees it as naming somewhere else
        assert_eq!(status_of(&t, &elsewhere), Status::Installed);
        assert_eq!(status(&t), Status::Stale);
    }

    /// A path with no space in it is its own space-free spelling, wherever
    /// this is running.
    ///
    /// Only Windows keeps a second name for a file, and the half that said so
    /// answered `None` to everything -- so on every other system a hook that
    /// cannot be quoted refused to install, whatever it was being pointed at,
    /// and told the person their path had a space in it when it did not.
    /// Nothing caught it because no test had installed that spelling before
    #[test]
    fn a_path_already_free_of_spaces_needs_no_second_name() {
        let plain = std::env::temp_dir().join("shikisha-plain").join("app");
        assert!(!plain.display().to_string().contains(' '), "assumption: {plain:?}");
        assert_eq!(spaceless(&plain), Some(plain.display().to_string()));

        // And this program's own path answers, or nothing that needs the
        // unquoted spelling could ever be installed from here
        assert!(spaceless(&me()).is_some(), "it cannot name itself: {:?}", me());
    }

    /// One event can be asked for more than one thing, and all of it has to
    /// survive.
    ///
    /// Gemini CLI has four events in total, so the start of a turn is the only
    /// place to say both "this is the conversation" and "it is working".
    /// Written one at a time, each removed the one before it: only the last
    /// was installed, the session was never reported, and the entry could
    /// never read as up to date because one of the three was always missing
    #[test]
    fn an_event_asked_for_two_things_keeps_both() {
        let dir = tmp("twice");
        let t = Target {
            name: "Test CLI".into(),
            file: dir.join("hooks.json"),
            format: HookFormat::Bare,
            timeout: 3_000,
            line_timeout: 10_000,
            trust: None,
            entries: vec![
                Entry { event: "BeforeAgent".into(), arg: "session".into() },
                Entry { event: "BeforeAgent".into(), arg: "state:BUSY".into() },
                Entry { event: "AfterAgent".into(), arg: "state:DONE".into() },
            ],
        };
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed, "three written means all three are there");

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        let said = |event: &str| {
            after["hooks"][event]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|g| g["hooks"].as_array().unwrap().clone())
                .map(|h| h["command"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<_>>()
        };
        let first = said("BeforeAgent");
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(first.iter().any(|c| c.ends_with("--hook session")), "{first:?}");
        assert!(first.iter().any(|c| c.ends_with("--hook state:BUSY")), "{first:?}");
        assert_eq!(said("AfterAgent").len(), 1);

        // What the person was shown is what was written
        let shown = preview(&t).into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n");
        let shown: serde_json::Value = serde_json::from_str(&shown).unwrap();
        assert_eq!(shown["hooks"]["BeforeAgent"][0]["hooks"].as_array().unwrap().len(), 2);

        // And taking it out takes out both
        uninstall(&t).unwrap();
        assert_eq!(status(&t), Status::Absent);
    }

    /// Each event carries what this app will make of it, in the entry itself.
    /// A person reading their own settings file should not have to be told
    /// which of their CLI's events this app treats as "waiting for you"
    #[test]
    fn every_event_is_written_with_the_meaning_it_was_given() {
        let dir = tmp("states");
        let t = Target {
            name: "Test CLI".into(),
            file: dir.join("hooks.json"),
            format: HookFormat::Args,
            timeout: TIMEOUT_S,
            line_timeout: LINE_TIMEOUT_S,
            trust: None,
            entries: vec![
                Entry { event: "SessionStart".into(), arg: "session".into() },
                Entry { event: "UserPromptSubmit".into(), arg: "state:BUSY".into() },
                Entry { event: "PermissionRequest".into(), arg: "state:QUESTION".into() },
                Entry { event: "Stop".into(), arg: "state:DONE".into() },
            ],
        };
        assert_eq!(status(&t), Status::NoConfig);
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed);

        let made: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        let arg_of = |event: &str| -> String {
            made["hooks"][event][0]["hooks"][0]["args"][1].as_str().unwrap().to_string()
        };
        assert_eq!(arg_of("UserPromptSubmit"), "state:BUSY");
        assert_eq!(arg_of("PermissionRequest"), "state:QUESTION");
        assert_eq!(arg_of("Stop"), "state:DONE");
        assert_eq!(arg_of("SessionStart"), "session");
        // Nothing that fires on every turn may be waited for
        assert_eq!(made["hooks"]["Stop"][0]["hooks"][0]["async"], serde_json::json!(true));

        // An entry that says the right thing for the wrong event is not ours
        // to keep: it would report "working" where a turn ends
        let mut crossed = made.clone();
        crossed["hooks"]["Stop"][0]["hooks"][0]["args"] =
            serde_json::json!(["--hook", "state:BUSY"]);
        std::fs::write(&t.file, serde_json::to_string_pretty(&crossed).unwrap()).unwrap();
        assert_eq!(status(&t), Status::Stale, "one with a different meaning is due to be reinstalled");
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed);
        assert_eq!(
            std::fs::read_to_string(&t.file)
                .map(|s| s.matches("--hook").count())
                .unwrap(),
            4,
            "reinstalling does not add more"
        );

        uninstall(&t).unwrap();
        let back: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(back, serde_json::json!({}), "it is taken out of every event");
    }

    /// The profiles that ship with the app are read here rather than described:
    /// a state name nobody implements would install a hook that fires into
    /// nothing, and the failure would be a dot that simply never moves
    #[test]
    fn the_profiles_that_ship_name_states_this_app_knows() {
        for p in crate::profile::all() {
            let Some(hook) = p.resume.as_ref().and_then(|r| r.hook.as_ref()) else {
                continue;
            };
            for (event, state) in &hook.states {
                assert!(
                    crate::detect::TabState::from_label(state).is_some(),
                    "{}: {event} says {state:?}, which is not a state",
                    p.name
                );
            }
            for (event, way) in &hook.helpers {
                assert!(Helper::of(way).is_some(), "{}: {event} says {way:?} of a helper", p.name);
            }
        }
    }

    /// A helper's beginning and end, from the events that say them, by the
    /// helper's own id -- and nothing about the conversation's state, which
    /// is the parent's to say
    #[test]
    fn a_helper_is_heard_beginning_and_ending() {
        let started = serde_json::json!({"session_id": "s", "hook_event_name": "SubagentStart", "agent_id": "a1f2", "agent_type": "general-purpose"});
        let r = report_of("helper:up", &started);
        assert_eq!(r.helper, Some(("a1f2".into(), true)));
        assert_eq!(r.state, None);
        let ended = serde_json::json!({"session_id": "s", "hook_event_name": "SubagentStop", "agent_id": "a1f2"});
        assert_eq!(report_of("helper:down", &ended).helper, Some(("a1f2".into(), false)));
        // Without the helper's id there is nothing to keep track of
        assert_eq!(report_of("helper:up", &serde_json::json!({"session_id": "s"})).helper, None);
        // A helper whose turn failed sends no end of its own: the failure is it
        let failed = serde_json::json!({"session_id": "s", "hook_event_name": "StopFailure", "agent_id": "a1f2"});
        let r = report_of("state:FAILED", &failed);
        assert_eq!(r.helper, Some(("a1f2".into(), false)));
        assert_eq!(r.state, None, "a helper's failure was taken for the conversation's");
    }

    /// The list a turn's end carries is read for what is still running: the
    /// helpers of the kind that end, by id, and whether anything else is
    /// going. Ended things are left out; a teammate -- listed as running for
    /// as long as its team stands -- does not count; a kind or a status
    /// nobody here knows counts as still going
    #[test]
    fn what_still_runs_is_read_from_the_end_of_the_turn() {
        let stop = |tasks: serde_json::Value| {
            report_of("state:DONE", &serde_json::json!({"session_id": "s", "hook_event_name": "Stop", "background_tasks": tasks}))
        };
        let r = stop(serde_json::json!([
            {"id": "a1", "type": "subagent", "status": "running"},
            {"id": "a2", "type": "subagent", "status": "completed"},
            {"id": "team-x", "type": "teammate", "status": "running"},
        ]));
        assert_eq!(r.state.as_deref(), Some("DONE"), "the parent's own turn still ended");
        assert_eq!(r.running, Some(Running { helpers: vec!["a1".into()], other: false }));
        let shell = stop(serde_json::json!([{"id": "b1", "type": "shell", "status": "running"}]));
        assert_eq!(shell.running, Some(Running { helpers: vec![], other: true }));
        let odd = stop(serde_json::json!([{"id": "c1", "type": "something_new"}]));
        assert!(odd.running.unwrap().other, "an unknown kind with no status was taken as ended");
        let unknown_status = stop(serde_json::json!([{"id": "a3", "type": "subagent", "status": "warming_up"}]));
        assert_eq!(unknown_status.running.unwrap().helpers, vec!["a3".to_string()]);
        let clear = stop(serde_json::json!([]));
        assert_eq!(clear.running, Some(Running::default()), "an empty list is news too: nothing runs");
        // No list at all says nothing -- what was known stands
        let none = report_of("state:DONE", &serde_json::json!({"session_id": "s", "hook_event_name": "Stop"}));
        assert_eq!(none.running, None);
        // A helper's own end of turn carries a list that is not the conversation's
        let helpers = report_of(
            "state:DONE",
            &serde_json::json!({"session_id": "s", "agent_id": "a1", "background_tasks": [{"id": "z", "type": "shell", "status": "running"}]}),
        );
        assert_eq!(helpers.running, None);
    }

    /// A conversation begun afresh has nothing beside it; one taken up again
    /// or packed down keeps what it had
    #[test]
    fn a_fresh_conversation_has_nothing_beside_it() {
        let begun = |source: &str| report_of("session", &serde_json::json!({"session_id": "s", "source": source}));
        assert_eq!(begun("startup").running, Some(Running::default()));
        assert_eq!(begun("clear").running, Some(Running::default()));
        assert_eq!(begun("resume").running, None);
        assert_eq!(begun("compact").running, None);
        assert_eq!(begun("startup").id.as_deref(), Some("s"));
    }

    /// Measured against Codex CLI 0.150/0.151 on Windows, which splits the
    /// command line itself: a quoted path never ran, the same path bare ran,
    /// and neither spelling ran when the path contained a space. So the bare
    /// form must carry no quotes at all, and a path with a space in it has to
    /// be written in Windows' second, space-free spelling.
    #[test]
    fn the_bare_form_carries_no_quotes() {
        let h = handler(HookFormat::Bare, TIMEOUT_S, "state:DONE", &me());
        let line = h["command"].as_str().unwrap();
        assert!(!line.contains('"'), "with quotes it does not start: {line}");
        assert!(line.ends_with(" --hook state:DONE"), "{line}");
        assert!(h.get("args").is_none(), "args are not written in the one-line form");
        // A CLI that waits on this is waiting for nothing; and above three
        // seconds Codex complains at every launch
        assert_eq!(h["async"], serde_json::json!(true));
        assert_eq!(h["timeout"], serde_json::json!(3));
        // The program part is the half that must survive being split on
        // spaces. (Where the exe lives during a test run has none, so this
        // checks the rule rather than the workaround)
        let program = line.trim_end_matches(" --hook state:DONE");
        assert!(!program.contains(' '), "a path with a space is split and lost: {program}");
    }

    #[test]
    fn a_config_that_cannot_be_read_is_not_written_over() {
        let dir = tmp("broken");
        let t = target(&dir, HookFormat::Args);
        std::fs::write(&t.file, "{ this is not json").unwrap();
        assert!(matches!(status(&t), Status::Unreadable(_)));
        assert!(install(&t).is_err(), "settings that cannot be read are not rewritten");
        assert_eq!(
            std::fs::read_to_string(&t.file).unwrap(),
            "{ this is not json",
            "not one character has changed"
        );
    }

    #[test]
    fn a_missing_config_is_created_from_nothing() {
        let dir = tmp("fresh");
        let t = target(&dir.join("deeper"), HookFormat::Shell);
        assert_eq!(status(&t), Status::NoConfig);
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed);
        let made: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        let h = &made["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(h["type"], "command");
        let line = h["command"].as_str().unwrap();
        assert!(line.starts_with('"') && line.contains("--hook session"), "{line}");
        assert!(h.get("args").is_none(), "args are not written in the one-line form");
    }

    #[test]
    fn an_entry_pointing_at_the_old_place_is_noticed() {
        let dir = tmp("moved");
        let t = target(&dir, HookFormat::Args);
        let old = serde_json::json!({
            "hooks": { "SessionStart": [{ "hooks": [{
                "type": "command",
                "command": "C:\\\\somewhere\\\\else\\\\SHIKISHA-TERM.exe",
                "args": ["--hook", "session"],
                "timeout": 5
            }] }] }
        });
        std::fs::write(&t.file, serde_json::to_string_pretty(&old).unwrap()).unwrap();
        assert_eq!(status(&t), Status::Stale, "after moving, it still points at the old place");
        install(&t).unwrap();
        assert_eq!(status(&t), Status::Installed, "reinstalling points at the current place");
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(
            after["hooks"]["SessionStart"].as_array().unwrap().len(),
            1,
            "the old one is not kept"
        );
    }

    /// Asked about as the program starts: a CLI used here and never answered
    /// about. Not one answered either way, and not one this PC does not use --
    /// nothing is written for, or asked about, a CLI nobody runs
    #[test]
    fn it_asks_about_a_cli_used_here_and_never_answered_about() {
        let dir = tmp("asked");
        let cli = |name: &str, used: bool| {
            let home = dir.join(name);
            if used {
                std::fs::create_dir_all(&home).unwrap();
            }
            let mut t = target(&home, HookFormat::Bare);
            t.name = name.into();
            t
        };
        let all = || vec![cli("new", true), cli("yes", true), cli("no", true), cli("absent", false)];
        let answers: std::collections::BTreeMap<String, String> =
            [("yes".to_string(), "on".to_string()), ("no".to_string(), "off".to_string())].into();
        let names = |v: Vec<Target>| v.into_iter().map(|t| t.name).collect::<Vec<_>>();
        assert_eq!(names(among(all(), &answers, None)), vec!["new"], "asked about");
        assert_eq!(names(among(all(), &answers, Some("on"))), vec!["yes"], "kept right");
    }

    /// The file is shown as it will be written, the person's lines as they
    /// are and ours told apart: the group added beside their hook, and an
    /// event they had nothing under, brackets and all
    #[test]
    fn the_preview_is_the_file_to_be_written_with_ours_told_apart() {
        let dir = tmp("preview");
        let mut t = target(&dir, HookFormat::Args);
        t.entries.push(Entry { event: "Stop".into(), arg: "state:DONE".into() });
        let theirs = serde_json::json!({
            "model": "opus",
            "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "their-beep" }] }] }
        });
        std::fs::write(&t.file, serde_json::to_string_pretty(&theirs).unwrap()).unwrap();
        let lines = preview(&t);
        let text = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        install(&t).unwrap();
        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&t.file).unwrap()).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&text).unwrap(), written, "shown is not what was written:\n{text}");
        let ours = |needle: &str| lines.iter().filter(|l| l.text.contains(needle)).all(|l| l.ours);
        let theirs_line = |needle: &str| lines.iter().filter(|l| l.text.contains(needle)).all(|l| !l.ours);
        assert!(theirs_line("\"model\"") && theirs_line("their-beep") && theirs_line("\"Stop\""), "{lines:#?}");
        assert!(ours("--hook") && ours("\"SessionStart\""), "{lines:#?}");
        assert!(lines.iter().filter(|l| l.ours).count() > 8 && lines.iter().any(|l| !l.ours));
        // The brackets closing the event the file did not have are ours; the
        // ones closing theirs are not
        let start = lines.iter().position(|l| l.text.contains("\"SessionStart\"")).unwrap();
        let close = lines[start..].iter().position(|l| l.text.trim_start().starts_with(']')).unwrap() + start;
        assert!(lines[start..=close].iter().all(|l| l.ours), "{lines:#?}");
        // A file that is not there yet is all ours
        std::fs::remove_file(&t.file).unwrap();
        assert!(preview(&t).iter().all(|l| l.ours));
    }

    /// Two copies of the app on one PC that both keep hooks right do not take
    /// the file from each other: a complete entry naming another copy that is
    /// still there is left as it is, and one naming a copy that is gone is
    /// written again for this one
    #[test]
    fn another_copy_still_there_keeps_its_hook() {
        for format in [HookFormat::Bare, HookFormat::Args, HookFormat::Shell] {
            let dir = tmp(&format!("copies-{format:?}"));
            let other = dir.join("other").join("SHIKISHA-TERM.exe");
            std::fs::create_dir_all(other.parent().unwrap()).unwrap();
            std::fs::write(&other, b"").unwrap();
            let t = target(&dir, format);
            install_as(&t, &other).unwrap();
            assert_eq!(status(&t), Status::Stale, "{format:?}: it names another copy");
            assert_eq!(keep_right(&t).unwrap(), Kept::default(), "{format:?}: the other copy's hook was taken");
            assert_eq!(program_named_in(&t).as_deref(), Some(other.as_path()), "{format:?}");
            std::fs::remove_file(&other).unwrap();
            assert!(keep_right(&t).unwrap().written, "{format:?}: a hook naming a copy that is gone was left");
            assert_eq!(status(&t), Status::Installed, "{format:?}");
        }
    }

    /// A settings file is named the way this system names one: it is shown
    /// to the person in the question that asks to write there
    #[test]
    fn a_hook_file_is_spelled_the_way_this_system_spells_a_path() {
        #[cfg(windows)]
        assert_eq!(
            expand_at("{home}/.codex/hooks.json", r"C:\Users\a").display().to_string(),
            r"C:\Users\a\.codex\hooks.json"
        );
        #[cfg(not(windows))]
        assert_eq!(expand_at("{home}/.codex/hooks.json", "/home/a").display().to_string(), "/home/a/.codex/hooks.json");
    }

    /// Codex is told to run the hooks of ours in our file that it holds back
    /// -- and nothing else: not a hook the person put there, not one in
    /// another file, not one it already runs, not one twice
    #[test]
    fn only_our_held_back_hooks_are_approved() {
        let file = Path::new(r"C:\Users\a\.codex\hooks.json");
        let hook = |key: &str, path: &str, command: &str, trust: &str| {
            serde_json::json!({ "key": key, "sourcePath": path, "command": command,
                "currentHash": format!("sha256:{key}"), "trustStatus": trust })
        };
        let ours = r"C:\PROGRA~1\SHIKISHA\SHIKISHA-TERM.exe --hook state:DONE";
        let listed = serde_json::json!({ "data": [{ "hooks": [
            hook("a", r"C:\Users\a\.codex\hooks.json", ours, "untrusted"),
            hook("b", r"c:\users\a\.codex\HOOKS.json", ours, "modified"),
            hook("c", r"C:\Users\a\.codex\hooks.json", ours, "trusted"),
            hook("d", r"C:\Users\a\.codex\hooks.json", "notify-send done", "untrusted"),
            hook("e", r"C:\work\.codex\hooks.json", ours, "untrusted"),
        ] }, { "hooks": [hook("a", r"C:\Users\a\.codex\hooks.json", ours, "untrusted")] }] });
        let keys: Vec<String> = to_approve(&listed, file).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["a", "b"]);
    }
}
