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
//! What it is FOR is the restart nobody planned: the app was closed, or the
//! machine was, in the middle of something. Putting the panes back and leaving
//! them empty restores the furniture and not the work, so by default each tab
//! is launched back into the conversation it was having -- the CLI is handed
//! the id and asked to resume it, and what comes up is the conversation, not a
//! prompt. `restore_conversations` turns that off for the person who closes
//! the app to be rid of what was in it; with it off this is still read, and
//! the key that means "carry the conversation over" (Ctrl+B r) can then reach
//! back across the restart on a tab where nothing has happened yet.

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
    pub desks: Vec<SavedWs>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedWs {
    pub name: String,
    /// The desk's id, which renaming leaves alone. Absent in files written
    /// before it was kept, and those are found by name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
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
        let fallback = Saved { version: VERSION, desks: Vec::new() };
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
    pub fn conversation_for(&self, desk: &crate::config::Desk, t: &Tab) -> Option<Session> {
        self.conversation_of(
            desk,
            t.program(),
            t.cwd().map(|c| c.display().to_string()).as_deref(),
            t.id.as_deref(),
            &t.title,
        )
    }

    /// The same question asked of a tab that does not exist yet.
    ///
    /// At startup the answer is needed BEFORE the process is launched -- that
    /// is the whole point of carrying a conversation over -- so the test is
    /// written against the four things that identify a tab rather than against
    /// a live one. `conversation_for` is the same test, asked later
    pub fn conversation_of(
        &self,
        desk: &crate::config::Desk,
        program: &str,
        cwd: Option<&str>,
        id: Option<&str>,
        title: &str,
    ) -> Option<Session> {
        let desk = self.desk(desk)?;
        let saved = desk.tabs.iter().find(|s| {
            s.program == program
                && s.cwd.as_deref() == cwd
                && match (&s.id, id) {
                    // An automation name is the handle that survives renaming,
                    // so when there is one it is the whole test
                    (Some(a), Some(b)) => a == b,
                    _ => s.title == title,
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

    /// The division of the screen this desk had last time.
    pub fn panes_for(&self, desk: &crate::config::Desk) -> Option<crate::layout::Layout> {
        self.desk(desk)?.panes.clone()
    }

    /// What was remembered about this desk.
    ///
    /// By its id when one was written down, because a desk renamed since the
    /// app closed is still that desk: found by name, its AI tabs came back as
    /// new conversations, and what they had been saying was left behind under
    /// a name nothing is called any more. A remembered id that is not this
    /// one is another desk, whatever it was called -- a new desk given a
    /// deleted one's name does not inherit its conversations. Only a file
    /// written before ids were kept is read by name
    fn desk(&self, desk: &crate::config::Desk) -> Option<&SavedWs> {
        let by_id = self
            .desks
            .iter()
            .find(|w| !desk.id.is_empty() && w.id.as_deref() == Some(desk.id.as_str()));
        by_id.or_else(|| self.desks.iter().find(|w| w.id.is_none() && w.name == desk.name))
    }

    /// Replace what is remembered about one desk, leaving the others.
    ///
    /// Desks are updated one at a time because that is how they are used:
    /// switching away should not forget where you were, and a desk that
    /// has not been opened this run has nothing newer to say about itself
    pub fn remember(
        &mut self,
        desk: &crate::config::Desk,
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
                let s = t.conversation_to_keep()?;
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
        let id = (!desk.id.is_empty()).then(|| desk.id.clone());
        let entry = SavedWs {
            name: desk.name.clone(),
            id: id.clone(),
            panes: panes.cloned(),
            tabs: saved,
        };
        // Filed under the id. What this desk left under its name before ids
        // were kept goes: it is the same desk, and left behind it would be
        // found again by a new desk that takes the name
        self.desks.retain(|w| match (&w.id, &id) {
            (Some(saved), Some(now)) => saved != now,
            (Some(_), None) => true,
            (None, _) => w.name != desk.name,
        });
        self.desks.push(entry);
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

    /// A desk as the settings would give it: its name, and an id it was never
    /// remembered under
    fn named(name: &str) -> crate::config::Desk {
        crate::config::Desk { name: name.into(), id: format!("{name}-id"), ..Default::default() }
    }

    #[test]
    fn a_tab_is_recognised_by_what_it_is_not_only_by_its_name() {
        let saved = Saved {
            version: VERSION,
            desks: vec![SavedWs {
                name: "work".into(),
                id: None,
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
        let found = |program, cwd, id, title| {
            saved
                .conversation_of(&named("work"), program, Some(cwd), id, title)
                .map(|s| s.id)
        };
        // The automation name is the handle that survives renaming
        assert_eq!(
            found("claude", "D:\\Test", Some("coder"), "何とでも"),
            Some("abc".into())
        );
        // The same name pointing at a different program is not the same tab:
        // resuming a conversation into another CLI is nonsense, not a courtesy
        assert_eq!(found("codex", "D:\\Test", Some("coder"), "AGENT"), None);
        // Nor is the same tab set up in another folder
        assert_eq!(found("claude", "D:\\Other", Some("coder"), "AGENT"), None);
        // And a desk that was never remembered has nothing to say
        assert!(saved
            .conversation_of(&named("elsewhere"), "claude", Some("D:\\Test"), Some("coder"), "AGENT")
            .is_none());
        assert!(saved.panes_for(&named("work")).is_none());
        assert!(saved.panes_for(&named("elsewhere")).is_none());
    }

    /// A desk renamed since the app closed brings its conversations back.
    ///
    /// Remembered by name, a renamed desk's AI tabs came back as new
    /// conversations. Remembered by id, it is still that desk -- and a new desk
    /// that took a deleted desk's name does not inherit what that one said
    #[test]
    fn a_renamed_desk_is_still_remembered_and_a_new_one_of_the_same_name_is_not() {
        let tab = |session: &str| SavedTab {
            title: "claude".into(),
            id: Some("claude".into()),
            cwd: Some("D:/Work".into()),
            program: "claude".into(),
            session: session.into(),
            source: "Minted".into(),
        };
        let entry = |id: Option<&str>, name: &str, session: &str| SavedWs {
            name: name.into(),
            id: id.map(str::to_string),
            panes: None,
            tabs: vec![tab(session)],
        };
        let desk = |id: &str, name: &str| crate::config::Desk { id: id.into(), name: name.into(), ..Default::default() };
        let said = |saved: &Saved, d: &crate::config::Desk| {
            saved.conversation_of(d, "claude", Some("D:/Work"), Some("claude"), "claude").map(|s| s.id)
        };

        let saved = Saved { version: VERSION, desks: vec![entry(Some("default"), "DEFAULT", "abc")] };
        assert_eq!(said(&saved, &desk("default", "ワイアード＆エコ")), Some("abc".into()), "renaming lost the conversation");
        assert_eq!(said(&saved, &desk("other", "DEFAULT")), None, "another desk of the same name took it");

        // A file from before ids were kept is read by name, and the entry
        // written from then on replaces it rather than standing beside it
        let mut saved = Saved { version: VERSION, desks: vec![entry(None, "DEFAULT", "old")] };
        assert_eq!(said(&saved, &desk("default", "DEFAULT")), Some("old".into()), "an older file is not read");
        saved.remember(&desk("default", "DEFAULT"), &[], None);
        assert_eq!(saved.desks.len(), 1, "the desk is remembered twice");
        assert_eq!(saved.desks[0].id.as_deref(), Some("default"));
    }

    /// Frozen on purpose: this is the file as the released 0.5.1 wrote it, and
    /// it has to keep loading, because the people who install the next version
    /// are the people who already had that one. Their tabs come back or they
    /// do not, and there is no second chance to notice.
    ///
    /// So do not edit this string to make it pass. A change here means a shape
    /// somebody already has on disk stopped being readable, and the fix is
    /// either to keep reading it or to raise VERSION and say what happens to
    /// the old one.
    #[test]
    fn the_file_an_earlier_release_wrote_still_loads() {
        const AS_0_5_1_WROTE_IT: &str = r#"{
          "version": 1,
          "desks": [{
            "name": "work",
            "panes": null,
            "tabs": [{
              "title": "AGENT",
              "id": "coder",
              "cwd": "D:\\Test",
              "program": "claude",
              "session": "abc",
              "source": "Minted"
            }]
          }]
        }"#;

        let saved: Saved = serde_json::from_str(AS_0_5_1_WROTE_IT).expect("an earlier file still parses");
        assert!(saved.version <= VERSION, "load() would refuse anything above");
        assert_eq!(
            saved
                .conversation_of(&named("work"), "claude", Some("D:\\Test"), Some("coder"), "AGENT")
                .map(|s| s.id),
            Some("abc".into()),
            "the conversation that tab was having comes back"
        );
    }

    #[test]
    fn a_file_from_a_newer_version_is_not_guessed_at() {
        let dir = std::env::temp_dir().join("shikisha-lastsession");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("last-session");
        std::fs::write(&f, r#"{"version":99,"desks":[{"name":"x","tabs":[]}]}"#).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        let parsed: Saved = serde_json::from_str(&text).unwrap();
        assert!(parsed.version > VERSION, "it can tell the file is from a later version");
    }
}
