//! The skill that teaches an AI to hand work to another tab.
//!
//! A person writes `@codex` in the input bar and the AI in front is sent
//! `<@otter>`. Nothing in that AI knows what it means until it has been told,
//! and the way these CLIs are told things for good is a skill: a `SKILL.md` in
//! a folder of theirs, read by name and description at the start of every
//! conversation and in full when it applies.
//!
//! **Written only when the person says so.** The folder is the CLI's, in the
//! person's home, shared by every program that runs that CLI. So the @ list
//! asks first, saying what will be written and where, and the settings say it
//! again with a way to take it out. What is kept here is the other half of
//! that promise: once agreed to, the file is this app's to keep current, and
//! a newer version replaces an older one without asking again.
//!
//! Where each CLI keeps skills comes from its profile (`skills`), not from
//! here: `{home}/.claude/skills` for Claude Code, `{home}/.agents/skills` for
//! Codex and Gemini CLI, which read that folder alike.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};

/// The folder this app's skill takes inside a CLI's skills folder
pub const NAME: &str = "shikisha";

/// Raised whenever the words below change, so a copy agreed to earlier is
/// brought up to date the next time the app starts
pub const VERSION: u32 = 1;

/// The line that says a file is this app's, and which version. Last, because
/// the front matter has to be the first thing in the file
fn mark() -> String {
    format!(
        "<!-- written by SHIKISHA-TERM, skill version {VERSION}; remove it from Settings > AI agents -->"
    )
}

/// The skill itself. English, as every instruction this app gives an AI is;
/// the AI answers the person in the person's own language
pub fn text() -> String {
    format!(
        r#"---
name: {NAME}
description: Hand work to another AI tab in SHIKISHA-TERM and get its reply. Use when a message names a tab as <@ID> (for example "ask <@otter> to review this"), or asks you to have another tab's AI do something.
---

# Asking another SHIKISHA-TERM tab

`<@ID>` in a message is another tab on this SHIKISHA-TERM desk, named by its id.
To hand it work, run:

    shikisha ask ID "what you want it to do"

This types the request into that tab, waits for its AI to finish, and prints
its reply. Read the last line of the output:

- `[shikisha] DONE` -- the reply is above it. Act on it.
- `[shikisha] STILL WORKING` -- it has not finished. End your turn now; its
  reply will be typed into this tab when it is done, and you carry on then.
- `[shikisha] WAITING` -- that tab is waiting for a person to approve or
  choose something. Tell the person.
- Anything else says why nothing was sent.

Write each request so the other AI can act on it alone: it cannot see this
conversation. Say which files, branch or folder to look at and what to answer.
If the output says `same folder: no`, it cannot see your uncommitted changes:
commit them, or put the diff in the request.

To repeat (for example "until the review finds nothing"): ask, act on the
reply, ask again. Stop when it reports nothing significant, or when the round
shown in the output reaches its limit.

Talk to the person in their own language.

{}
"#,
        mark()
    )
}

/// Where a CLI keeps skills, from its profile. `None`: it has no such folder
pub fn folder_of(ai: &str) -> Option<PathBuf> {
    let p = crate::profile::load_by_name(ai);
    let dir = p.skills.as_deref()?;
    Some(expand(dir))
}

/// `{home}` is the only thing a profile may stand in for (see `agenthook`)
fn expand(path: &str) -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    PathBuf::from(path.replace("{home}", &home))
}

/// The file this app writes for a CLI, whether or not it is there
pub fn file_of(ai: &str) -> Option<PathBuf> {
    folder_of(ai).map(|d| d.join(NAME).join("SKILL.md"))
}

/// How a CLI stands with the skill
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// This CLI has no skills folder to put it in
    Unsupported,
    /// Not there, and not turned down
    Missing,
    /// Not there, and the person said "not now"
    Later,
    /// There: this app's current one, or a file of that name somebody else
    /// wrote, which is theirs and is left alone
    Installed,
    /// This app's, from an earlier version
    Old,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Unsupported => "none",
            Status::Missing => "missing",
            Status::Later => "later",
            Status::Installed => "in",
            Status::Old => "old",
        }
    }
}

pub fn status(ai: &str) -> Status {
    let Some(file) = file_of(ai) else {
        return Status::Unsupported;
    };
    status_at(&file, later().iter().any(|a| a == ai))
}

fn status_at(file: &Path, turned_down: bool) -> Status {
    match std::fs::read_to_string(file) {
        Ok(had)
            if had.contains("written by SHIKISHA-TERM, skill version ")
                && !had.contains(&mark()) =>
        {
            Status::Old
        }
        Ok(_) => Status::Installed,
        Err(_) if turned_down => Status::Later,
        Err(_) => Status::Missing,
    }
}

/// Write the skill for a CLI. Hands back where it went
pub fn install(ai: &str) -> Result<PathBuf> {
    let file = file_of(ai).ok_or_else(|| anyhow!("{ai} has no skills folder"))?;
    install_at(&file)?;
    set_later(ai, false);
    Ok(file)
}

fn install_at(file: &Path) -> Result<()> {
    let dir = file
        .parent()
        .ok_or_else(|| anyhow!("no folder for {}", file.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("{} could not be made", dir.display()))?;
    std::fs::write(file, text()).with_context(|| format!("{} could not be written", file.display()))
}

/// Take it out again: the file, and its folder when nothing else is in it.
/// A file of that name that is not this app's is not touched
pub fn remove(ai: &str) -> Result<()> {
    let file = file_of(ai).ok_or_else(|| anyhow!("{ai} has no skills folder"))?;
    remove_at(&file)
}

fn remove_at(file: &Path) -> Result<()> {
    match std::fs::read_to_string(file) {
        Ok(had) if !had.contains("written by SHIKISHA-TERM") => Err(anyhow!(
            "{} was not written by this app, so it is left alone",
            file.display()
        )),
        Ok(_) => {
            std::fs::remove_file(file)
                .with_context(|| format!("{} could not be removed", file.display()))?;
            if let Some(dir) = file.parent() {
                let _ = std::fs::remove_dir(dir);
            }
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

/// Bring every copy this app wrote earlier up to the current words. Hands back
/// the CLIs whose copy was rewritten, for the person to be told. Only ever an
/// older copy of this app's own: a missing one stays missing
pub fn refresh() -> Vec<String> {
    let mut done = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ai in ["claude", "codex", "gemini"] {
        let Some(file) = file_of(ai) else { continue };
        if !seen.insert(file.clone()) {
            continue;
        }
        if status_at(&file, false) == Status::Old && install_at(&file).is_ok() {
            done.push(ai.to_string());
        }
    }
    done
}

/// The CLIs the person said "not now" for, kept beside the app's other state
fn later_file() -> PathBuf {
    crate::config::state_path("skill-later")
}

fn later() -> Vec<String> {
    std::fs::read_to_string(later_file())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn set_later(ai: &str, on: bool) {
    let mut list = later();
    list.retain(|a| a != ai);
    if on {
        list.push(ai.to_string());
    }
    let _ = std::fs::write(later_file(), list.join("\n"));
}

/// Every AI this app knows a skills folder for, with how it stands: what the
/// page reads to decide whether the @ list asks first
pub fn statuses() -> std::collections::BTreeMap<String, crate::uistate::SkillView> {
    ["claude", "codex", "gemini"]
        .into_iter()
        .map(|ai| {
            let view = crate::uistate::SkillView {
                state: status(ai).word().to_string(),
                file: file_of(ai)
                    .map(|f| f.display().to_string())
                    .unwrap_or_default(),
                name: crate::profile::load_by_name(ai).name,
            };
            (ai.to_string(), view)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("skill-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn it_is_written_read_as_current_and_taken_out_again() {
        let d = scratch("round");
        let file = d.join(NAME).join("SKILL.md");
        assert_eq!(status_at(&file, false), Status::Missing);
        assert_eq!(status_at(&file, true), Status::Later);
        install_at(&file).unwrap();
        let had = std::fs::read_to_string(&file).unwrap();
        assert!(
            had.starts_with("---\nname: shikisha\n"),
            "the front matter is not first: {had}"
        );
        assert!(had.contains("shikisha ask ID"));
        assert_eq!(status_at(&file, false), Status::Installed);
        remove_at(&file).unwrap();
        assert!(!file.exists());
        assert!(
            !file.parent().unwrap().exists(),
            "the empty folder was left behind"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_older_copy_is_seen_as_older_and_somebody_elses_file_is_left_alone() {
        let d = scratch("old");
        let file = d.join(NAME).join("SKILL.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "---\nname: shikisha\n---\n<!-- written by SHIKISHA-TERM, skill version 0; x -->\n",
        )
        .unwrap();
        assert_eq!(status_at(&file, false), Status::Old);
        std::fs::write(&file, "---\nname: shikisha\n---\nmy own notes\n").unwrap();
        assert_eq!(status_at(&file, false), Status::Installed);
        assert!(
            remove_at(&file).is_err(),
            "a file this app did not write was removed"
        );
        assert!(file.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
