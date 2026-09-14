//! Closing a tab from the tab bar, and opening it again.
//!
//! A tab lives in the settings, so closing one is taking its line out of them
//! -- the same thing deleting it on the settings screen does, which is why the
//! reload that follows is what actually ends it. What this adds is the way
//! back: the line is kept, with the place it stood and the conversation it
//! was having, so that opening it again puts back the tab and not just its
//! name.
//!
//! What does not come back is what was on its screen. The process is ended
//! when the tab goes, and a picture of a live thing restored later would invite
//! people to trust it (the same line `lastsession` draws). The conversation is
//! different: it is the CLI's own record, and it is resumed rather than shown.
//!
//! The one pause is for a tab whose work would be cut off: an AI still busy,
//! or waiting for an answer. Everything else closes on the press, the way a
//! browser's tab does -- a question on every close teaches people to click
//! through questions.

use serde::{Deserialize, Serialize};

use crate::config::{self, TabMark, TakenTab};
use crate::detect::TabState;
use crate::i18n;
use crate::tab::{Session, SessionSource, Tab};
use crate::uistate::{CloseAskState, ClosedState};
use crate::view::{surface_key, Surface};

/// The file's shape, versioned so a later one can refuse to read this rather
/// than half-understand it
const VERSION: u32 = 1;
const FILE: &str = "closed-tabs";
/// How many are kept. Enough for "the one closed a minute ago" and the few
/// before it; further back than that, the Vault is where past work is found
const KEEP: usize = 20;
/// How many the list offers
const SHOWN: usize = 10;

/// One closed tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClosedTab {
    /// What the screen asks for it by. Never reused, so a press on a list
    /// that has since changed cannot open a different one
    pub id: u64,
    pub desk: String,
    /// The name it had on screen
    pub name: String,
    /// When it was closed (seconds since 1970)
    pub at: i64,
    /// The conversation to resume when it is opened again, when there is one
    /// that can be resumed by name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    pub place: Place,
}

/// Where a closed tab was written, and so where it goes back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Place {
    /// A tab written in a folder: a terminal, a page, a panel
    Folder { taken: TakenTab },
    /// A page in the desk's older `browsers` list
    Listed { at: usize, line: serde_json::Value },
    /// A page nothing wrote down: automation opened it, or it is the result
    /// view. Kept for this run only -- its address can carry a key that means
    /// nothing after a restart, and is nothing to leave lying in a file
    Page { key: String, url: String, profile: shikisha_shared::BrowserProfile },
}

/// The closed tabs, oldest first.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Closed {
    version: u32,
    #[serde(default)]
    items: Vec<ClosedTab>,
    /// The next id to hand out. Kept in the file so ids stay unique across
    /// runs
    #[serde(default)]
    next: u64,
    /// Where it is read from and written to. None for a store held only in
    /// memory
    #[serde(skip)]
    file: Option<std::path::PathBuf>,
}

impl Closed {
    pub fn load() -> Closed {
        Self::load_from(config::state_path(FILE))
    }

    fn load_from(file: std::path::PathBuf) -> Closed {
        let fresh = |file| Closed { version: VERSION, items: Vec::new(), next: 1, file };
        let Ok(text) = std::fs::read_to_string(&file) else {
            return fresh(Some(file));
        };
        match serde_json::from_str::<Closed>(text.trim_start_matches('\u{feff}')) {
            Ok(mut c) if c.version <= VERSION => {
                c.items.retain(|i| !matches!(i.place, Place::Page { .. }));
                c.next = c.next.max(c.items.iter().map(|i| i.id + 1).max().unwrap_or(1));
                c.file = Some(file);
                c
            }
            // A file from a later version is left alone rather than
            // overwritten: the person may go back to that build
            Ok(_) => fresh(None),
            Err(e) => {
                crate::append_hook_log(&format!("closed tabs unreadable: {e}"));
                fresh(Some(file))
            }
        }
    }

    fn write(&self) {
        let Some(file) = &self.file else { return };
        let kept = Closed {
            version: VERSION,
            items: self
                .items
                .iter()
                .filter(|i| !matches!(i.place, Place::Page { .. }))
                .cloned()
                .collect(),
            next: self.next,
            file: None,
        };
        let Ok(text) = serde_json::to_string_pretty(&kept) else { return };
        if let Err(e) = crate::crypto::write_atomic(file, &text) {
            crate::append_hook_log(&format!("could not write the closed tabs: {e}"));
        }
    }

    /// Keeps one, newest, and forgets the oldest past what is kept.
    fn remember(&mut self, desk: &str, name: &str, conversation: Option<String>, place: Place) {
        let id = self.next.max(1);
        self.next = id + 1;
        self.items.push(ClosedTab {
            id,
            desk: desk.to_string(),
            name: name.to_string(),
            at: now(),
            conversation,
            place,
        });
        let over = self.items.len().saturating_sub(KEEP);
        self.items.drain(..over);
        self.write();
    }

    /// Takes one out: the one asked for, or the newest on this desk.
    fn take(&mut self, desk: &str, which: Option<u64>) -> Option<ClosedTab> {
        let at = self
            .items
            .iter()
            .rposition(|i| i.desk == desk && which.is_none_or(|w| w == i.id))?;
        let item = self.items.remove(at);
        self.write();
        Some(item)
    }

    /// Puts back one that could not be opened, where it was in the order, so
    /// a failed attempt costs nothing. By id, which goes up in the order tabs
    /// were closed, where two closed in one second share a time
    fn keep_again(&mut self, item: ClosedTab) {
        let at = self.items.iter().position(|i| i.id > item.id).unwrap_or(self.items.len());
        self.items.insert(at, item);
        self.write();
    }

    /// The ones the list offers for a desk, newest first.
    pub fn shown(&self, desk: &str) -> Vec<ClosedState> {
        self.items
            .iter()
            .rev()
            .filter(|i| i.desk == desk)
            .take(SHOWN)
            .map(|i| ClosedState {
                id: i.id,
                name: i.name.clone(),
                folder: match &i.place {
                    Place::Folder { taken } => taken
                        .folder
                        .as_deref()
                        .map(|c| config::resolve_folder_cwd(c).display().to_string()),
                    _ => None,
                },
                at: i.at,
            })
            .collect()
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What ends with a closed tab right away, rather than with the reload.
#[derive(Debug, PartialEq, Eq)]
pub enum Ends {
    Nothing,
    /// A running tab, by serial
    Tab(u64),
    /// The throwaway editor, by its name
    Editor(String),
}

/// What came of a press on a tab's ✕.
#[derive(Debug)]
pub enum Closing {
    /// Nothing to close: the row is not there, is not the row the press was
    /// aimed at any more, or is not one that closes (the settings form)
    Nothing,
    /// Its work would be cut off, so the person is asked first
    Ask(CloseAskState),
    /// Closed. `settings` says the settings were changed, and the reload that
    /// takes that up is what finishes the job
    Closed { note: String, settings: bool, ends: Ends },
    Failed(String),
}

/// Closes the row at screen number `at`.
///
/// `key` is what the press saw there (`view::surface_key`); a row that has
/// moved since is left alone, so a press cannot land on the tab that slid
/// into its place. `sure` is the answer to the question, when it was asked.
#[allow(clippy::too_many_arguments)]
pub fn close(
    at: usize,
    key: &str,
    sure: bool,
    rows: &[(Surface, Option<usize>)],
    tabs: &[Tab],
    desk: Option<&config::Desk>,
    caps: &crate::hooks::Caps,
    closed: &mut Closed,
    seq: u64,
) -> Closing {
    let Some((surface, written)) = at.checked_sub(1).and_then(|i| rows.get(i)) else {
        return Closing::Nothing;
    };
    let here = surface_key(surface, tabs);
    if !key.is_empty() && here != key {
        return Closing::Nothing;
    }
    let desk_name = desk.map(|d| d.name.as_str()).unwrap_or_default();
    let said = |name: &str| i18n::tp("msg.tab.closed", &[("name", name)]);
    // Taking a written line out, for anything that has one
    let take = |closed: &mut Closed, name: &str, conversation: Option<String>, ends: Ends| {
        let (Some(w), Some(desk)) = (written, desk) else {
            return None;
        };
        let ft = desk.tabs.get(*w)?;
        Some(match config::take_tab(desk_name, *w, &TabMark::of(desk, ft)) {
            Ok(taken) => {
                closed.remember(desk_name, name, conversation, Place::Folder { taken });
                Closing::Closed { note: said(name), settings: true, ends }
            }
            Err(e) => Closing::Failed(format!("{e:#}")),
        })
    };
    match surface {
        Surface::Session(i) => {
            let Some(t) = tabs.get(*i) else {
                return Closing::Nothing;
            };
            let conversation = conversation_of(t);
            if !sure && matches!(t.state, TabState::Busy | TabState::Question) {
                return Closing::Ask(CloseAskState {
                    seq,
                    tab: at,
                    key: here,
                    name: t.title.clone(),
                    state: t.state.label().to_string(),
                    ai: t.is_ai(),
                    comes_back: conversation.is_some(),
                });
            }
            take(closed, &t.title, conversation, Ends::Tab(t.serial())).unwrap_or_else(|| {
                // Nothing wrote it down, so there is no line to keep and no
                // way to open it again: it simply ends
                Closing::Closed {
                    note: i18n::tp("msg.tab.closed_for_good", &[("name", &t.title)]),
                    settings: false,
                    ends: Ends::Tab(t.serial()),
                }
            })
        }
        Surface::Git { name, .. } | Surface::Sftp { name, .. } | Surface::Failed { name, .. } => {
            take(closed, name, None, Ends::Nothing).unwrap_or(Closing::Nothing)
        }
        Surface::Editor { key: editor, name, .. } => take(closed, name, None, Ends::Nothing)
            // The throwaway editor is written nowhere and holds no file of its
            // own to keep: closing it is putting it away
            .unwrap_or_else(|| Closing::Closed {
                note: i18n::tp("msg.tab.closed_for_good", &[("name", name)]),
                settings: false,
                ends: Ends::Editor(editor.clone()),
            }),
        // Not one that closes, like INDEX: its row in the list is always there,
        // and pressing another tab is the way out of it
        Surface::Issues { .. } => Closing::Nothing,
        Surface::Browser { key: page, name } => {
            if page == crate::runtime::SETTINGS_TAB {
                return Closing::Nothing;
            }
            if let Some(done) = take(closed, name, None, Ends::Nothing) {
                return done;
            }
            // Written in the older list beside the desk
            if desk.is_some_and(|d| d.browsers.iter().any(|b| &b.id == page)) {
                return match config::take_browser(desk_name, page) {
                    Ok((spot, line)) => {
                        let _ = caps.browser_close(page);
                        closed.remember(desk_name, name, None, Place::Listed { at: spot, line });
                        Closing::Closed { note: said(name), settings: true, ends: Ends::Nothing }
                    }
                    Err(e) => Closing::Failed(format!("{e:#}")),
                };
            }
            // Written nowhere: opened by automation, or the result view
            let spec = caps.browser_spec(page);
            if let Err(e) = caps.browser_close(page) {
                return Closing::Failed(format!("{e:#}"));
            }
            match spec {
                Some((url, profile)) => {
                    closed.remember(desk_name, name, None, Place::Page { key: page.clone(), url, profile });
                    Closing::Closed { note: said(name), settings: false, ends: Ends::Nothing }
                }
                None => Closing::Closed {
                    note: i18n::tp("msg.tab.closed_for_good", &[("name", name)]),
                    settings: false,
                    ends: Ends::Nothing,
                },
            }
        }
    }
}

/// The conversation a tab would come back into, if it can come back into one
/// at all: a CLI that can be told which conversation to resume, and a record
/// of that conversation that is really there. An id handed out at launch to a
/// tab nobody spoke to names a conversation that was never written, and
/// handing that to the CLI later would be an error in red on reopening.
fn conversation_of(t: &Tab) -> Option<String> {
    let s = t.conversation_to_keep()?;
    let spec = t.resume.as_ref()?;
    if spec.with_id.is_empty() {
        return None;
    }
    if spec.verify.as_ref().is_some_and(|v| !crate::sessionfind::exists(v, &s.id)) {
        return None;
    }
    Some(s.id.clone())
}

/// What came of asking for a closed tab back.
#[derive(Debug)]
pub enum Reopening {
    /// Back. `settings` as for `Closing`; `reveal` is the name the row will
    /// answer to, so the screen can go to it once it is there; `resume` is the
    /// conversation to hand its launch, under the tab's automation name
    Reopened {
        note: String,
        settings: bool,
        reveal: String,
        resume: Option<(String, Session)>,
    },
    Nothing(String),
    Failed(String),
}

/// Opens a closed tab again: the one asked for, or the one closed last.
pub fn reopen(
    which: Option<u64>,
    desk: &config::Desk,
    caps: &crate::hooks::Caps,
    closed: &mut Closed,
) -> Reopening {
    let Some(item) = closed.take(&desk.name, which) else {
        return Reopening::Nothing(i18n::t("msg.tab.none_closed"));
    };
    let said = |talk: bool| {
        i18n::tp(
            if talk { "msg.tab.reopened_talk" } else { "msg.tab.reopened" },
            &[("name", &item.name)],
        )
    };
    let back = match &item.place {
        Place::Folder { taken } => config::put_tab_back(&desk.name, taken).map(|id| Reopening::Reopened {
            note: said(item.conversation.is_some()),
            settings: true,
            resume: item
                .conversation
                .clone()
                .map(|c| (id.clone(), Session { id: c, source: SessionSource::Store })),
            reveal: id,
        }),
        Place::Listed { at, line } => config::put_browser_back(&desk.name, *at, line).map(|()| Reopening::Reopened {
            note: said(false),
            settings: true,
            reveal: line.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            resume: None,
        }),
        Place::Page { key, url, profile } => {
            // Opened again by something else since: it is here already
            let here = caps.hosted_names().iter().any(|h| h == key);
            let opened = if here { Ok(()) } else { caps.browser_open(key, url, profile.clone()) };
            opened.map(|()| Reopening::Reopened {
                note: said(false),
                settings: false,
                reveal: key.clone(),
                resume: None,
            })
        }
    };
    back.unwrap_or_else(|e| {
        let why = format!("{e:#}");
        closed.keep_again(item);
        Reopening::Failed(why)
    })
}

/// The row a reopened tab answers to, once it is on screen: a running tab by
/// its automation name, anything else by its own.
pub fn row_named(rows: &[Surface], tabs: &[Tab], name: &str) -> Option<usize> {
    rows.iter()
        .position(|s| match s {
            Surface::Session(i) => tabs.get(*i).and_then(|t| t.id.as_deref()) == Some(name),
            Surface::Browser { key, .. }
            | Surface::Git { key, .. }
            | Surface::Sftp { key, .. }
            | Surface::Editor { key, .. }
            | Surface::Failed { key, .. }
            | Surface::Issues { key } => key == name,
        })
        .map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Closed {
        Closed { version: VERSION, items: Vec::new(), next: 1, file: None }
    }

    fn line(name: &str) -> Place {
        Place::Folder {
            taken: TakenTab {
                line: serde_json::json!({ "name": name, "command": "claude" }),
                folder: Some("D:\\work".into()),
                path: vec![0],
                id: Some(name.into()),
            },
        }
    }

    #[test]
    fn the_newest_comes_back_first_and_only_on_its_own_desk() {
        let mut c = store();
        c.remember("A", "one", None, line("one"));
        c.remember("B", "elsewhere", None, line("elsewhere"));
        c.remember("A", "two", Some("abc".into()), line("two"));
        let shown = c.shown("A");
        assert_eq!(shown.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["two", "one"]);
        assert_eq!(c.take("A", None).map(|i| i.name), Some("two".into()), "the last closed should come first");
        assert_eq!(c.take("A", None).map(|i| i.name), Some("one".into()));
        assert!(c.take("A", None).is_none(), "a tab from another desk came back here");
    }

    /// Like INDEX, the Issue tab is not closed -- from its tab, the keyboard or
    /// a phone -- and nothing of it is kept to bring back
    #[test]
    fn the_issue_tab_does_not_close() {
        let rows = vec![(Surface::Issues { key: crate::view::ISSUES_KEY.to_string() }, None)];
        let caps: crate::hooks::Caps = std::rc::Rc::new(crate::caps::Capabilities::disabled());
        let mut c = store();
        let got = close(1, "", true, &rows, &[], None, &caps, &mut c, 1);
        assert!(matches!(got, Closing::Nothing), "{got:?}");
        assert!(c.items.is_empty(), "it was kept as something closed");
    }

    #[test]
    fn one_can_be_asked_for_by_id_and_ids_are_never_reused() {
        let mut c = store();
        c.remember("A", "one", None, line("one"));
        c.remember("A", "two", None, line("two"));
        let first = c.shown("A")[1].id;
        let item = c.take("A", Some(first)).expect("the one asked for did not come out");
        assert_eq!(item.name, "one");
        c.remember("A", "three", None, line("three"));
        assert!(c.shown("A").iter().all(|s| s.id != first), "an id was handed out twice");
        // A failed attempt puts it back where it was
        c.keep_again(item);
        assert_eq!(c.shown("A").last().map(|s| s.name.as_str()), Some("one"));
    }

    #[test]
    fn only_so_many_are_kept() {
        let mut c = store();
        for n in 0..KEEP + 5 {
            c.remember("A", &format!("t{n}"), None, line("t"));
        }
        assert_eq!(c.items.len(), KEEP);
        assert_eq!(c.items[0].name, "t5", "the oldest were not the ones forgotten");
        assert_eq!(c.shown("A").len(), SHOWN);
    }

    /// A page nothing wrote down is kept for the run and never reaches the
    /// file: its address can carry a key
    #[test]
    fn a_page_nobody_wrote_down_is_not_written_to_disk() {
        let dir = std::env::temp_dir().join(format!("shikisha-closed-{}", crate::random_hex(6)));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("closed-tabs");
        let mut c = Closed::load_from(file.clone());
        c.remember("A", "tab", Some("abc".into()), line("tab"));
        c.remember(
            "A",
            "result",
            None,
            Place::Page {
                key: "result".into(),
                url: "http://127.0.0.1:1/result?token=secret".into(),
                profile: shikisha_shared::BrowserProfile::shared_default(),
            },
        );
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("secret"), "an address carrying a key was written to disk");
        let again = Closed::load_from(file.clone());
        assert_eq!(again.shown("A").len(), 1);
        assert_eq!(again.items[0].conversation.as_deref(), Some("abc"), "the conversation was not kept");
        assert!(again.next > 2, "ids start over on the next run");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
