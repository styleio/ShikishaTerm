//! What was on screen when the app last closed.
//!
//! Two things are worth keeping, and they are the two this app owns: **which
//! conversation each tab was having**, and **how the screen was divided**.
//!
//! Nothing else. A terminal's contents, a shell's history, what a program had
//! half-typed — none of that is ours to promise back, and pretending otherwise
//! would be worse than not offering it: a restored screen that is a photograph
//! of a live thing invites people to trust it.
//!
//! And it is never used on its own. Someone who quit to be rid of a
//! conversation should not find it waiting for them. What this does is let the
//! key that already means "carry the conversation over" reach across a restart
//! of the whole app, on a tab where nothing has happened yet.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::tab::{Session, SessionSource, Tab};

/// The file's shape, versioned so a later one can refuse to read this rather
/// than half-understand it
const VERSION: u32 = 1;
const FILE: &str = "last-session";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    pub version: u32,
    #[serde(default)]
    pub workspaces: Vec<SavedWs>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedWs {
    pub name: String,
    /// How the content area was divided. Absent when it was not
    #[serde(default)]
    pub panes: Option<crate::layout::Layout>,
    #[serde(default)]
    pub tabs: Vec<SavedTab>,
}

/// One tab's conversation, with enough beside it to be sure it is the same tab.
///
/// A name alone is not enough: the same name can be a different program in a
/// different folder tomorrow, and resuming a conversation into the wrong CLI
/// would be a strange kind of nonsense
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTab {
    pub title: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    pub program: String,
    pub session: String,
    /// How that id was come by, kept because it says how far to trust it
    pub source: String,
}

fn path() -> PathBuf {
    crate::config::state_path(FILE)
}

impl Saved {
    pub fn load() -> Saved {
        let fallback = Saved { version: VERSION, workspaces: Vec::new() };
        let Ok(text) = std::fs::read_to_string(path()) else {
            return fallback;
        };
        match serde_json::from_str::<Saved>(text.trim_start_matches('\u{feff}')) {
            // A file from a later version is not ours to interpret. Left alone
            // rather than overwritten: the person may go back to that build
            Ok(s) if s.version <= VERSION => s,
            Ok(s) => {
                crate::append_hook_log(&format!(
                    "last session was written by a newer version ({}); leaving it alone",
                    s.version
                ));
                fallback
            }
            Err(e) => {
                crate::append_hook_log(&format!("last session unreadable: {e}"));
                fallback
            }
        }
    }

    /// The conversation this tab was having last time, if this is recognisably
    /// the same tab.
    pub fn conversation_for(&self, workspace: &str, t: &Tab) -> Option<Session> {
        let ws = self.workspaces.iter().find(|w| w.name == workspace)?;
        let cwd = t.cwd().map(|c| c.display().to_string());
        let saved = ws.tabs.iter().find(|s| {
            s.program == t.program()
                && s.cwd == cwd
                && match (&s.id, &t.id) {
                    // An automation name is the handle that survives renaming,
                    // so when there is one it is the whole test
                    (Some(a), Some(b)) => a == b,
                    _ => s.title == t.title,
                }
        })?;
        Some(Session {
            id: saved.session.clone(),
            source: match saved.source.as_str() {
                "Minted" => SessionSource::Minted,
                "Hook" => SessionSource::Hook,
                _ => SessionSource::Store,
            },
        })
    }

    /// The division of the screen this workspace had last time.
    pub fn panes_for(&self, workspace: &str) -> Option<crate::layout::Layout> {
        self.workspaces
            .iter()
            .find(|w| w.name == workspace)?
            .panes
            .clone()
    }

    /// Replace what is remembered about one workspace, leaving the others.
    ///
    /// Workspaces are updated one at a time because that is how they are used:
    /// switching away should not forget where you were, and a workspace that
    /// has not been opened this run has nothing newer to say about itself
    pub fn remember(
        &mut self,
        workspace: &str,
        tabs: &[Tab],
        panes: Option<&crate::layout::Layout>,
    ) {
        let saved: Vec<SavedTab> = tabs
            .iter()
            .filter_map(|t| {
                // The conversation worth keeping is the one that was actually
                // used. A tab nobody spoke to this run was handed an id at
                // launch and never put anything in it, so what it had BEFORE
                // is the one still worth coming back to — otherwise opening
                // the app and closing it again would quietly forget everything
                let s = match t.spoke() {
                    true => t.session.as_ref().or(t.previous.as_ref())?,
                    false => t.previous.as_ref().or(t.session.as_ref())?,
                };
                Some(SavedTab {
                    title: t.title.clone(),
                    id: t.id.clone(),
                    cwd: t.cwd().map(|c| c.display().to_string()),
                    program: t.program().to_string(),
                    session: s.id.clone(),
                    source: format!("{:?}", s.source),
                })
            })
            .collect();
        let entry = SavedWs {
            name: workspace.to_string(),
            panes: panes.cloned(),
            tabs: saved,
        };
        match self.workspaces.iter_mut().find(|w| w.name == workspace) {
            Some(w) => *w = entry,
            None => self.workspaces.push(entry),
        }
    }

    pub fn write(&self) {
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        // A conversation id names a conversation; it is not a credential, but
        // it is nobody else's business either. Beside the exe with the rest of
        // the app's own state
        if let Err(e) = crate::crypto::write_atomic(&path(), &text) {
            crate::append_hook_log(&format!("could not write the last session: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_is_recognised_by_what_it_is_not_only_by_its_name() {
        let saved = Saved {
            version: VERSION,
            workspaces: vec![SavedWs {
                name: "work".into(),
                panes: None,
                tabs: vec![SavedTab {
                    title: "AGENT".into(),
                    id: Some("coder".into()),
                    cwd: Some("D:\\Test".into()),
                    program: "claude".into(),
                    session: "abc".into(),
                    source: "Minted".into(),
                }],
            }],
        };
        // The same name pointing at a different program is not the same tab:
        // resuming a conversation into another CLI is nonsense, not a courtesy
        let other = SavedTab { program: "codex".into(), ..saved.workspaces[0].tabs[0].clone() };
        assert_ne!(other.program, saved.workspaces[0].tabs[0].program);
        assert!(saved.panes_for("work").is_none());
        assert!(saved.panes_for("elsewhere").is_none());
    }

    #[test]
    fn a_file_from_a_newer_version_is_not_guessed_at() {
        let dir = std::env::temp_dir().join("shikisha-lastsession");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("last-session");
        std::fs::write(&f, r#"{"version":99,"workspaces":[{"name":"x","tabs":[]}]}"#).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        let parsed: Saved = serde_json::from_str(&text).unwrap();
        assert!(parsed.version > VERSION, "後の版のファイルだと分かる");
    }
}
