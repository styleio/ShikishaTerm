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
/// The file as this start found it, before anything this run does is written
/// over it. See [`keep_as_read`]
const AS_READ: &str = "last-session.prev";

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

/// Keep a copy of what this start was handed, and say how much it was.
///
/// The file is rewritten within seconds of every start, so what the start read
/// is gone by the time anybody asks why a tab came up without its
/// conversation. That question has been asked again and again, and every time
/// the one piece of evidence that could settle it -- did the file know about
/// the tab at all -- had already been written over. The copy is taken once,
/// here, and is replaced only by the next start's
fn keep_as_read(text: &str, read: &Saved) {
    let tabs: usize = read.desks.iter().map(|d| d.tabs.len()).sum();
    crate::append_hook_log(&format!(
        "last session: {tabs} tab(s) remembered across {} desk(s); a copy is kept as {AS_READ}",
        read.desks.len()
    ));
    if let Err(e) = crate::crypto::write_atomic(&crate::config::state_path(AS_READ), text) {
        crate::append_hook_log(&format!("could not keep a copy of the last session: {e}"));
    }
}

/// Whether two written-down folders are the same folder.
///
/// Compared the way every other place-comparison in this app is, because
/// Windows hands the same folder back in whatever spelling it likes and none
/// of the differences mean anything: the settings keep whatever was typed, and
/// a path that came back from somewhere else may carry the other slash, other
/// case or a separator on the end. Compared as written, one rewriting of the
/// settings turned every conversation on a desk into a stranger, all at once
/// and without a word -- a tab is matched to a running process by this same
/// question (`TabOptions::same_place`), and only what is remembered asked it
/// more strictly than the rest
fn same_folder(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            crate::uistate::same_folder(std::path::Path::new(a), std::path::Path::new(b))
        }
        (a, b) => a == b,
    }
}

/// Whether the CLI's own record of this conversation is still on this computer.
///
/// A CLI that keeps no record we know how to find is taken at its word: there
/// is nothing to check, and doubting it would throw away conversations that are
/// perfectly alive
fn kept_on_this_computer(t: &Tab, id: &str) -> bool {
    match t.resume.as_ref().and_then(|r| r.verify.as_deref()) {
        Some(pattern) => crate::sessionfind::exists(pattern, id),
        None => true,
    }
}

/// Which conversation to remember for one tab: the one it holds now, or the one
/// this file already holds for it.
///
/// Normally the one it holds now, and there is nothing to weigh. The exception
/// is the one that loses work: a tab is handed a brand new id the moment it
/// starts, and until somebody speaks in it that id names nothing — no record,
/// nowhere. Remembering it in place of a conversation that does exist trades
/// something for nothing, and the trade is silent: the empty id cannot be
/// resumed next time, so the tab comes up clean and what it was saying is no
/// longer written down anywhere the app looks.
///
/// So a conversation with a record outranks one without. Nothing is held on to
/// once the tab has a real conversation of its own again -- speaking in the new
/// one gives it a record, and it wins the moment it has one
fn worth_keeping(
    live: Option<&Session>,
    had: Option<&Session>,
    kept: impl Fn(&str) -> bool,
) -> Option<Session> {
    match (live, had) {
        (Some(now), Some(before))
            if now.id != before.id && !kept(&now.id) && kept(&before.id) =>
        {
            Some(before.clone())
        }
        (Some(now), _) => Some(now.clone()),
        (None, before) => before.cloned(),
    }
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
            Ok(s) if s.version <= VERSION => {
                keep_as_read(&text, &s);
                s
            }
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
        let saved = self.remembered_of(desk, program, cwd, id, title)?;
        Some(Session {
            id: saved.session.clone(),
            source: match saved.source.as_str() {
                "Minted" => SessionSource::Minted,
                "Hook" => SessionSource::Hook,
                _ => SessionSource::Store,
            },
        })
    }

    /// Which remembered tab this one is.
    ///
    /// **A tab is its program and its folder.** Both are chosen deliberately,
    /// both are written down as they are, and neither changes on its own --
    /// so when one conversation was remembered for this CLI in this folder,
    /// it is this tab's, whatever either of them is called today.
    ///
    /// The name used to be part of the test, and it is not a name a person
    /// gave: a tab without one of its own is handed the one its title
    /// suggests, and a second tab of the same name gets `-2` after it
    /// (`config::unique_id`). That makes the names positional. Close the first
    /// of three `claude` tabs and the other two are renamed under the
    /// remembered file, which then matches nothing -- every one of that desk's
    /// conversations dropped at the next start, silently, because one tab was
    /// closed.
    ///
    /// The name is kept for the one thing it can honestly settle: two tabs of
    /// one CLI in one folder, where nothing else tells them apart. When it
    /// cannot settle that either, nothing is handed over -- resuming the wrong
    /// conversation is worse than starting a new one
    fn remembered_of(
        &self,
        desk: &crate::config::Desk,
        program: &str,
        cwd: Option<&str>,
        id: Option<&str>,
        title: &str,
    ) -> Option<&SavedTab> {
        let desk = self.desk(desk)?;
        let here: Vec<&SavedTab> = desk
            .tabs
            .iter()
            .filter(|s| s.program == program && same_folder(s.cwd.as_deref(), cwd))
            .collect();
        if let [only] = here.as_slice() {
            return Some(only);
        }
        // More than one, so a name is all that is left. Asked twice because
        // the two names answer different questions: the automation name is the
        // handle that survives a rename, and the title is what a person reads.
        // Either only answers when it picks out exactly one
        let mut by_id =
            here.iter().copied().filter(|s| matches!((&s.id, id), (Some(a), Some(b)) if a == b));
        if let (Some(only), None) = (by_id.next(), by_id.next()) {
            return Some(only);
        }
        let mut by_title = here.iter().copied().filter(|s| s.title == title);
        match (by_title.next(), by_title.next()) {
            (Some(only), None) => Some(only),
            _ => None,
        }
    }

    /// Whether this desk was remembered at all.
    ///
    /// Asked by a launch that found nothing for its tab, because the two ways
    /// of finding nothing are different faults: a desk that is here but has no
    /// entry for this tab, and a desk the file does not know -- its id changed,
    /// or another start wrote the file without it
    pub fn knows_desk(&self, desk: &crate::config::Desk) -> bool {
        self.desk(desk).is_some()
    }

    /// How many conversations this desk remembers for one CLI in one folder.
    ///
    /// Asked by a launch that is about to start clean, so that it can tell the
    /// ordinary case -- a tab that is new since last time, and nothing was
    /// ever written down for it -- from the one worth saying out loud: there
    /// were conversations here and none of them could be given to this tab
    pub fn remembered_here(
        &self,
        desk: &crate::config::Desk,
        program: &str,
        cwd: Option<&str>,
    ) -> usize {
        self.desk(desk)
            .map(|d| {
                d.tabs
                    .iter()
                    .filter(|s| s.program == program && same_folder(s.cwd.as_deref(), cwd))
                    .count()
            })
            .unwrap_or(0)
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
                let had = self.conversation_for(desk, t);
                let s = worth_keeping(t.conversation_to_keep(), had.as_ref(), |id| {
                    kept_on_this_computer(t, id)
                })?;
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

    /// Put it on the disk, unless what is already there says the same thing.
    ///
    /// The skipping is done here, against the file, rather than by the caller
    /// against a mark made of the screen's division and each tab's
    /// conversation. Neither of those is what decides these contents:
    /// `worth_keeping` also asks whether the id a tab holds has a record yet,
    /// and that answer changes on its own, a while after the tab started, when
    /// the CLI first writes the conversation down. So a caller skipping on its
    /// own mark wrote the file once, at the moment the answer was still "no",
    /// and then never again -- a tab restarted at noon stayed remembered under
    /// the conversation it had at eleven, and that dead id is what the next
    /// start handed the CLI. Asked of the contents themselves, the question
    /// cannot be asked of the wrong thing
    pub fn write(&self) {
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        if std::fs::read_to_string(path()).is_ok_and(|had| had == text) {
            return;
        }
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

    /// Closing one tab does not take the other tabs' conversations with it.
    ///
    /// A tab without an automation name of its own is given one: the slug of
    /// its title, and `-2`, `-3`... after that when the name is taken
    /// (`config::unique_id`). So the names are positional. Close the first of
    /// three `claude` tabs and the remaining two are renamed -- under a file
    /// that still calls them by the names they had -- and a test that turned on
    /// those names matched nothing at the next start. Every conversation on the
    /// desk was dropped at once, silently, because one tab was closed.
    ///
    /// The folder and the program do not move like that. One conversation
    /// remembered for this CLI in this folder is this tab's, whatever either of
    /// them is called today
    #[test]
    fn closing_one_tab_leaves_the_others_their_conversations() {
        let tab = |id: &str, cwd: &str, session: &str| SavedTab {
            title: "claude".into(),
            id: Some(id.into()),
            cwd: Some(cwd.into()),
            program: "claude".into(),
            session: session.into(),
            source: "Minted".into(),
        };
        let saved = Saved {
            version: VERSION,
            desks: vec![SavedWs {
                name: "work".into(),
                id: None,
                panes: None,
                tabs: vec![
                    tab("claude", "D:\\Gone", "one"),
                    tab("claude-2", "D:\\Left", "two"),
                    tab("claude-3", "D:\\Right", "three"),
                ],
            }],
        };
        // The first tab is closed, so the two behind it are renamed one step
        // forward. What they are is what they were: same CLI, same folders
        let found = |cwd, id| {
            saved
                .conversation_of(&named("work"), "claude", Some(cwd), Some(id), "claude")
                .map(|s| s.id)
        };
        assert_eq!(found("D:\\Left", "claude"), Some("two".into()));
        assert_eq!(found("D:\\Right", "claude-2"), Some("three".into()));
        // A tab added since is not handed somebody else's conversation
        assert_eq!(found("D:\\New", "claude-3"), None);
        // And the launch can tell "nothing was ever written down here" from
        // "something was, and it could not be given to this tab"
        assert_eq!(saved.remembered_here(&named("work"), "claude", Some("D:\\Left")), 1);
        assert_eq!(saved.remembered_here(&named("work"), "claude", Some("D:\\New")), 0);
    }

    /// Two tabs of one CLI in one folder: the name is all there is, and when it
    /// does not answer either, nothing is handed over. Putting the wrong
    /// conversation back is worse than starting a new one
    #[test]
    fn two_tabs_in_one_folder_are_told_apart_by_name_or_not_at_all() {
        let tab = |id: &str, title: &str, session: &str| SavedTab {
            title: title.into(),
            id: Some(id.into()),
            cwd: Some("D:\\Work".into()),
            program: "claude".into(),
            session: session.into(),
            source: "Minted".into(),
        };
        let saved = Saved {
            version: VERSION,
            desks: vec![SavedWs {
                name: "work".into(),
                id: None,
                panes: None,
                tabs: vec![tab("coder", "build", "one"), tab("reviewer", "review", "two")],
            }],
        };
        let found = |id, title| {
            saved
                .conversation_of(&named("work"), "claude", Some("D:\\Work"), id, title)
                .map(|s| s.id)
        };
        // The automation name settles it, and the title settles it when there
        // is no automation name
        assert_eq!(found(Some("reviewer"), "何とでも"), Some("two".into()));
        assert_eq!(found(None, "build"), Some("one".into()));
        // Neither answers: nothing is handed over rather than a guess
        assert_eq!(found(Some("nobody"), "nothing"), None);
        assert_eq!(saved.remembered_here(&named("work"), "claude", Some("D:\\Work")), 2);
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

    /// One folder written two ways is still one folder.
    ///
    /// Windows opens `C:\x`, `C:/x` and `c:\x\` as the same folder, and the
    /// settings keep whatever was typed. What is remembered used to be looked
    /// up by the spelling alone, so one rewriting of the settings turned every
    /// conversation on a desk into a stranger, all at once and in silence
    #[test]
    fn a_folder_written_the_other_way_round_is_still_that_folder() {
        let saved = Saved {
            version: VERSION,
            desks: vec![SavedWs {
                name: "work".into(),
                id: None,
                panes: None,
                tabs: vec![SavedTab {
                    title: "claude".into(),
                    id: Some("claude".into()),
                    cwd: Some(r"C:\Users\me\Work".into()),
                    program: "claude".into(),
                    session: "abc".into(),
                    source: "Minted".into(),
                }],
            }],
        };
        let found = |cwd: &str| {
            saved
                .conversation_of(&named("work"), "claude", Some(cwd), Some("claude"), "claude")
                .map(|s| s.id)
        };
        assert_eq!(found(r"C:\Users\me\Work"), Some("abc".into()));
        if cfg!(windows) {
            assert_eq!(found("C:/Users/me/Work"), Some("abc".into()), "the other slash");
            assert_eq!(found(r"c:\users\me\work"), Some("abc".into()), "another case");
            assert_eq!(found(r"C:\Users\me\Work\"), Some("abc".into()), "a separator on the end");
        }
        assert_eq!(found(r"C:\Users\me\Elsewhere"), None, "another folder is another folder");
    }

    /// An empty conversation does not take a real one's place.
    ///
    /// A tab is handed a brand new id the moment it starts, and until somebody
    /// speaks in it that id names nothing. Written down in place of the
    /// conversation the tab was having, it loses it twice over: the new id
    /// cannot be resumed, so the tab comes up clean next time, and the real
    /// conversation is no longer written down anywhere the app looks
    #[test]
    fn a_conversation_nobody_has_spoken_in_does_not_replace_one_that_exists() {
        let of = |id: &str| Session { id: id.into(), source: SessionSource::Minted };
        let (real, empty) = (of("written-down"), of("never-used"));
        let on_disk = |id: &str| id == "written-down";

        // The tab holds an id nothing was ever said in, and the file holds the
        // conversation it was having: the conversation wins
        assert_eq!(
            worth_keeping(Some(&empty), Some(&real), on_disk).map(|s| s.id),
            Some("written-down".into())
        );
        // Once the tab has a conversation of its own again, it wins at once --
        // nothing is held on to for longer than it is the only one there is
        assert_eq!(
            worth_keeping(Some(&real), Some(&of("older")), on_disk).map(|s| s.id),
            Some("written-down".into())
        );
        // Neither has a record: there is nothing to weigh, so the tab's own
        // stands. Refusing both would forget a conversation that is about to
        // have a record the moment somebody types in it
        assert_eq!(
            worth_keeping(Some(&empty), Some(&of("also-gone")), on_disk).map(|s| s.id),
            Some("never-used".into())
        );
        // A tab with nothing of its own keeps what the file holds for it
        assert_eq!(
            worth_keeping(None, Some(&real), on_disk).map(|s| s.id),
            Some("written-down".into())
        );
        assert!(worth_keeping(None, None, on_disk).is_none());
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
