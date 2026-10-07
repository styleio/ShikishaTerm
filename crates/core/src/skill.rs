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
pub const VERSION: u32 = 6;

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
description: Hand work to another tab in SHIKISHA-TERM -- another AI, a terminal, or a web page -- and get the result. Use when a message names a tab as <@ID> (for example "ask <@otter> to review this", "run the tests in <@shell>", "check the price on <@shop>"). Also use before making or changing a design or architecture choice in a project, and when asked why something was chosen: it says how to find and follow the project's decision records (ADR).
---

# Working with other SHIKISHA-TERM tabs

`<@ID>` in a message is another tab on this SHIKISHA-TERM desk, named by its id.
Which command reaches it depends on what the tab is (`shikisha tab_list` lists
them, with their kind):

    shikisha ask_tab ID "a line" "what to do"      # another AI: it does the work and replies
    shikisha tab_run ID "a command"                # a terminal: runs one command, prints its output
    shikisha browser_do ID "what to get done"      # a web page: driven toward the goal, prints what it found

Each waits for the tab to finish and prints the result. Read the last line:

- `[shikisha] DONE` -- the result is above it. Act on it.
- `[shikisha] STILL WORKING` -- it has not finished. End your turn now; when it
  is done you are told so in this tab, `shikisha inbox` gives you the result,
  and you carry on then.
- `[shikisha] WAITING` -- that tab is waiting for a person to approve or
  choose something. Tell the person.
- `[shikisha] NOT DONE` -- the reason is above it.
- Anything else says why nothing was sent. Using the wrong command for a tab
  says which one to use.

A terminal or a page is only driven when the person named it with @ in what
they asked you (`shikisha tab_list` marks those). If they did not, ask them to.
A page cannot be given passwords or other secrets: ask the person to do that
step themselves.

Write each request so it can be acted on alone: the other tab cannot see this
conversation. For an AI, say which files, branch or folder to look at and what
to answer. If the output says `same folder: no`, it cannot see your uncommitted
changes: commit them, or put the diff in the request. For a page, write only
what to do on it ("type Alice in the name box and send the form"). Do not ask
the page to report anything: what it shows at the end comes back to you.

The person watches the AIs confer as a chat. `ask_tab` takes one short line
for it before what you want done: what you would say to a colleague, one line,
in the person's language (for example `shikisha ask_tab otter "Could you review
the parser change?" "Review the diff in src/parser.rs on branch fix-parser ..."`).
A line too long or on more than one line is refused with the limit; say it
shorter. When another tab has asked you something, just answer it; as you
finish you are asked for your line, and you reply with that line alone.

To talk to the others without asking anything:

    shikisha say "a line"                       # one short line in the chat
    shikisha react ID 👍                        # mark the last line <@ID> said (👍 ❤️ 🎉 👀 ✅ ❓)
    shikisha share commit HEAD                  # a card: commit, pr URL, file PATH, or url URL

Share what the others should look at -- the commit you made, the pull request
you opened -- rather than pasting it into a line.

To repeat (for example "until the review finds nothing"): ask, act on the
result, ask again. Stop when it reports nothing significant, or when the round
shown in the output reaches its limit.

To see a whole job through with other tabs -- "have <@claude> implement it and
<@codex> review it until nothing is left", several tasks, reports, several
rounds -- run `shikisha skill orchestration` and follow it. It keeps the tasks
and the reports for you, and every answer says which command to run next.

Merge or push only when the person asked for it.

To read what was said in another tab without asking it anything -- its last
answer in full, or further back -- use
`shikisha tab_conversation ID '{{"want":3}}'`. Every word after `shikisha` is a
SHIKISHA-TERM command and its arguments (`shikisha list` shows the ones this
tab may use); an argument written as JSON is passed as that value.

# Decision records (ADR)

A project may keep its decisions as records: short Markdown files that say
what was decided and why, usually in docs/decisions or docs/adr. When the work
touches a design or architecture choice, or the person asks why something is
the way it is, read them first. `shikisha adr_list` says where this tab's
project keeps them and lists each one with its status.

- An accepted record is a decision that stands: follow it.
- A proposed one is not decided yet: read it as context.
- A superseded or deprecated one is history: follow the record that replaced it.
- If what you are asked to do goes against an accepted record, say which one
  and ask the person before doing it. Do not work around it.
- When the work changes a decision, tell the person that a new record should
  replace the old one. They write it from the ADR panel, or ask you to.

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

use crate::with_home as expand;

/// The file this app writes for a CLI, whether or not it is there
pub fn file_of(ai: &str) -> Option<PathBuf> {
    folder_of(ai).map(|d| d.join(NAME).join("SKILL.md"))
}

/// How a CLI stands with the skill
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// This CLI has no skills folder to put it in
    Unsupported,
    /// Not there
    Missing,
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
            Status::Installed => "in",
            Status::Old => "old",
        }
    }
}

pub fn status(ai: &str) -> Status {
    let Some(file) = file_of(ai) else {
        return Status::Unsupported;
    };
    status_at(&file)
}

fn status_at(file: &Path) -> Status {
    match std::fs::read_to_string(file) {
        Ok(had)
            if had.contains("written by SHIKISHA-TERM, skill version ")
                && !had.contains(&mark()) =>
        {
            Status::Old
        }
        Ok(_) => Status::Installed,
        Err(_) => Status::Missing,
    }
}

/// Write the skill for a CLI. Hands back where it went
pub fn install(ai: &str) -> Result<PathBuf> {
    let file = file_of(ai).ok_or_else(|| anyhow!("{ai} has no skills folder"))?;
    install_at(&file)?;
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
        if status_at(&file) == Status::Old && install_at(&file).is_ok() {
            done.push(ai.to_string());
        }
    }
    done
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
        assert_eq!(status_at(&file), Status::Missing);
        install_at(&file).unwrap();
        let had = std::fs::read_to_string(&file).unwrap();
        assert!(
            had.starts_with("---\nname: shikisha\n"),
            "the front matter is not first: {had}"
        );
        assert!(had.contains("shikisha ask_tab ID"));
        assert_eq!(status_at(&file), Status::Installed);
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
        assert_eq!(status_at(&file), Status::Old);
        std::fs::write(&file, "---\nname: shikisha\n---\nmy own notes\n").unwrap();
        assert_eq!(status_at(&file), Status::Installed);
        assert!(
            remove_at(&file).is_err(),
            "a file this app did not write was removed"
        );
        assert!(file.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
