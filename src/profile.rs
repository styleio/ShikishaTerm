//! Agent profiles: declaratively define state-detection rules per tool, in
//! external files. DESIGN.md section 4.3. Users can support a new tool just
//! by editing ./profiles/*.json.

use anyhow::{Context, Result};
use serde::Deserialize;

/// The raw shape of profiles/*.json
#[derive(Debug, Clone, Deserialize)]
pub struct ProfileFile {
    pub name: String,
    /// Applied when the command name contains this string (case-insensitive)
    #[serde(default)]
    pub command_match: Vec<String>,
    /// Regex: if it matches the screen, treat as BUSY (in progress)
    #[serde(default)]
    pub busy_patterns: Vec<String>,
    /// Regex: if it matches the screen, treat as QUESTION (waiting on a choice)
    #[serde(default)]
    pub question_patterns: Vec<String>,
    /// If the screen doesn't change for this long (ms), treat as done/idle
    #[serde(default = "default_silence_ms")]
    pub silence_ms: u64,
    /// Number of bottom rows excluded from the screen-change check.
    /// Used to ignore status bars (e.g. byobu/tmux, whose clock updates
    /// every second)
    #[serde(default = "default_ignore_bottom_rows")]
    pub ignore_bottom_rows: u16,
    /// How long to wait, after DONE first appears, before it's confirmed as
    /// truly finished.
    ///
    /// AI output can pause mid-stream, so mere silence isn't proof of
    /// completion. It's fine to switch the on-screen display right away
    /// (worst case it just flips back), but handing off to another tab
    /// can't be undone, so only that path waits for confirmation.
    ///
    /// If unset, uses the base config value. Override here only when a
    /// particular AI has its own quirks.
    #[serde(default)]
    pub done_confirm_ms: Option<u64>,
    /// How this CLI carries a conversation across a restart. Absent means it
    /// has none — an editor-style tool with no conversation to keep
    #[serde(default)]
    pub resume: Option<ResumeSpec>,
}

/// Everything about picking a conversation back up, as data.
///
/// Which arguments to pass is a fact about a CLI, not about this app, and CLIs
/// change theirs between releases. Keeping it in the profile means a new
/// release is a text edit rather than a build.
///
/// **The program is never named here.** These are arguments added to the
/// command the tab already runs, so a profile can change how a CLI is asked to
/// resume but can never point the tab at a different program. That is what
/// makes it safe to run one unattended, with no approval to click.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ResumeSpec {
    /// Arguments that hand OUR id over at launch, for CLIs that accept one.
    /// The strongest form: nothing has to be attributed afterwards
    #[serde(default)]
    pub new_id: Vec<String>,
    /// Arguments that resume a conversation we know the id of
    #[serde(default)]
    pub with_id: Vec<String>,
    /// Arguments that continue whatever ran here last. A guess by definition —
    /// used only when this folder and CLI belong to exactly one tab
    #[serde(default)]
    pub newest_here: Vec<String>,
    /// Where this CLI keeps its own record of conversations, for CLIs that
    /// won't take an id and can't be hooked
    #[serde(default)]
    pub record: Option<RecordSpec>,
    /// How to ask this CLI to report its conversation as it starts
    #[serde(default)]
    pub hook: Option<HookSpec>,
}

/// A CLI's own record of its conversations, as far as we need to read it:
/// one file per conversation, the first line of which says which one it is.
#[derive(Debug, Clone, Deserialize)]
pub struct RecordSpec {
    /// Folder to look in. `{home}` is the user's home folder
    pub dir: String,
    /// Field holding the conversation id, in the first line's JSON.
    /// A dotted path steps into nested objects
    pub id: String,
    /// Field holding the folder that conversation was started in
    pub cwd: String,
}

/// How to install a "tell us your session id" hook into a CLI's own config.
#[derive(Debug, Clone, Deserialize)]
pub struct HookSpec {
    /// The CLI's hook config file. `{home}` is the user's home folder
    pub file: String,
    /// The shape of that file. Two are known; both are JSON
    pub format: HookFormat,
    /// The events to register for. One is usually enough — the moment a
    /// conversation starts — but a CLI may name it differently
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookFormat {
    /// `hooks.<Event>[] = { matcher?, hooks: [ { type, command, args?, timeout? } ] }`
    Grouped,
    /// `hooks.<Event>[] = { command }`, plus a `version` beside it
    Flat,
}

fn default_silence_ms() -> u64 {
    2000
}

fn default_ignore_bottom_rows() -> u16 {
    2
}

/// Default wait time before response completion is confirmed.
///
/// The cost of waiting is only a few seconds' delay in hand-off, which
/// doesn't compare to the damage of jumping the gun and handing off a
/// mid-response answer. Err on the long side.
pub const DEFAULT_DONE_CONFIRM_MS: u64 = 10_000;

/// A compiled profile
pub struct Profile {
    pub name: String,
    pub busy: Vec<regex::Regex>,
    pub question: Vec<regex::Regex>,
    pub silence_ms: u64,
    pub ignore_bottom_rows: u16,
    pub done_confirm_ms: Option<u64>,
    pub resume: Option<ResumeSpec>,
}

impl Profile {
    /// Generic fallback for when nothing matches any profile
    /// (judged only by the silence timer, bell, and process exit).
    pub fn generic() -> Self {
        Self {
            name: "GENERIC".into(),
            busy: Vec::new(),
            question: Vec::new(),
            silence_ms: default_silence_ms(),
            ignore_bottom_rows: default_ignore_bottom_rows(),
            done_confirm_ms: None,
            resume: None,
        }
    }

    pub fn compile(f: ProfileFile) -> Result<Self> {
        let compile_all = |patterns: &[String]| -> Result<Vec<regex::Regex>> {
            patterns
                .iter()
                .map(|p| {
                    regex::Regex::new(p).with_context(|| {
                        crate::i18n::tp("err.profile.bad_regex", &[("p", p)])
                    })
                })
                .collect()
        };
        Ok(Self {
            busy: compile_all(&f.busy_patterns)?,
            question: compile_all(&f.question_patterns)?,
            silence_ms: f.silence_ms,
            done_confirm_ms: f.done_confirm_ms,
            ignore_bottom_rows: f.ignore_bottom_rows,
            resume: f.resume,
            name: f.name,
        })
    }
}

/// Search beside the exe (portable layout) first, then directly under the
/// current directory.
fn candidate_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("profiles")))
    {
        dirs.push(d);
    }
    // Also check beside the config file (so it's found even in setups where
    // the exe and data live in different places)
    if let Some(d) = crate::config::config_file_path().parent() {
        dirs.push(d.join("profiles"));
    }
    dirs.push(std::path::PathBuf::from("profiles"));
    dirs
}

fn find_profile<F>(pred: F) -> Option<Profile>
where
    F: Fn(&std::path::Path, &ProfileFile) -> bool,
{
    for dir in candidate_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(pf) = serde_json::from_str::<ProfileFile>(&text) else {
                continue;
            };
            if pred(&path, &pf) {
                if let Ok(p) = Profile::compile(pf) {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Return the profile matching the command name; falls back to generic if
/// none match.
pub fn load_for_command(cmd: &str) -> Profile {
    let needle = std::path::Path::new(cmd)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(cmd)
        .to_lowercase();
    find_profile(|_, pf| {
        pf.command_match
            .iter()
            .any(|m| needle.contains(&m.to_lowercase()))
    })
    .unwrap_or_else(Profile::generic)
}

/// Load a profile by explicit name (the file name, or its `name` field).
/// Used by config.json's tab definition "profile": "claude" (for cases like
/// an AI over ssh, where the command name alone can't identify it).
pub fn load_by_name(name: &str) -> Profile {
    let needle = name.to_lowercase();
    find_profile(|path, pf| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.to_lowercase() == needle)
            || pf.name.to_lowercase() == needle
    })
    .unwrap_or_else(Profile::generic)
}
