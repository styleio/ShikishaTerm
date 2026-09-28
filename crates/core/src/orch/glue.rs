//! Between the record and the tabs: the picture of the tabs the runtime hands
//! [`Orchestra`](super::Orchestra), and what it does with the answer.
//!
//! Kept out of `runtime.rs` so the loop only calls in: it owns the tabs and
//! the send queue, and everything this module needs of them is here, in one
//! place.

use std::collections::{HashMap, HashSet, VecDeque};

use super::{Effect, Scene, TabFact};
use crate::hooks::HookEngine;
use crate::send::{PendingSend, paste_chunks};
use crate::tab::Tab;
use crate::view::Surface;

/// The id a tab is named by (`<@ID>`), as `tab_list` gives it
pub fn tab_id(t: &Tab) -> String {
    t.id.clone().unwrap_or_else(|| t.called().to_string())
}

/// What each profile says about a CLI, looked up once: which CLI it is (the
/// command its profile matches first) and whether a paste has to be asked for
/// in typed words first
#[derive(Default)]
pub struct Profiles {
    known: HashMap<String, (String, bool)>,
}

impl Profiles {
    fn of(&mut self, profile: &str) -> (String, bool) {
        if let Some(v) = self.known.get(profile) {
            return v.clone();
        }
        let found = crate::profile::files()
            .into_iter()
            .find(|pf| pf.name == profile)
            .map(|pf| {
                (
                    pf.command_match.first().map(|c| c.trim().to_string()).unwrap_or_default(),
                    pf.paste_needs_typed_request,
                )
            })
            .unwrap_or_default();
        self.known.insert(profile.to_string(), found.clone());
        found
    }
}

/// The tabs on this desk's screen, as orchestration needs to know them.
/// `reachable` says whether a tab on another machine can run `shikisha` here
/// (the relay is there and connected)
pub fn scene(tabs: &[Tab], surfaces: &[Surface], profiles: &mut Profiles, reachable: impl Fn(&Tab) -> bool) -> Scene {
    let mut out = Vec::new();
    for s in surfaces {
        let Surface::Session(i) = s else { continue };
        let Some(t) = tabs.get(*i) else { continue };
        let (cli, typed_request) = profiles.of(t.profile_name());
        let far = t.machine().is_some();
        out.push(TabFact {
            id: tab_id(t),
            called: t.called().to_string(),
            title: t.title.clone(),
            ai: t.is_ai() && !t.is_model(),
            cli,
            state: t.state,
            folder: t.cwd().map(|p| p.display().to_string()),
            far,
            reachable: !far || reachable(t),
            incarnation: crate::api::incarnation_of(t.called()),
            typed_request,
        });
    }
    Scene { tabs: out }
}

/// What this app typed into each tab itself, recently: a brief, a line saying
/// there is mail, what another tab asked it. In a CLI's record these look
/// exactly like a person's words, and they must not be taken for them -- a
/// person's words are what names the tabs an AI may drive
#[derive(Default)]
pub struct Typed {
    by_tab: HashMap<String, VecDeque<String>>,
}

/// How much of a text is compared: enough to tell two apart, short enough to
/// survive a CLI trimming a long paste
const KEPT: usize = 80;

fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(KEPT).collect()
}

static TYPED: std::sync::Mutex<Option<Typed>> = std::sync::Mutex::new(None);

/// Remember that this app typed `text` into `tab` (by id). Called wherever
/// words go into a tab that are not a person's: a brief, a line about mail,
/// what another tab sent with `send_to_tab`
pub fn note_typed(tab: &str, text: &str) {
    if let Ok(mut g) = TYPED.lock() {
        g.get_or_insert_with(Typed::default).note(tab, text);
    }
}

fn typed_here(tab: &str, text: &str) -> bool {
    TYPED.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|t| t.is_ours(tab, text)))
}

impl Typed {
    pub fn note(&mut self, tab: &str, text: &str) {
        let q = self.by_tab.entry(tab.to_string()).or_default();
        q.push_back(flat(text));
        while q.len() > 32 {
            q.pop_front();
        }
    }

    pub fn is_ours(&self, tab: &str, text: &str) -> bool {
        let f = flat(text);
        if f.is_empty() {
            return false;
        }
        let head: String = f.chars().take(60).collect();
        self.by_tab
            .get(tab)
            .is_some_and(|q| q.iter().any(|t| t.starts_with(&head) || f.starts_with(t.as_str())))
    }
}

/// The tabs the person named (`<@ID>`) in what they last asked the AI in
/// `called`. Read from the CLI's own record of the conversation when it keeps
/// one -- which is where a person's words land whether they came through the
/// input bar or were typed straight at the CLI's prompt -- skipping anything
/// this app typed there itself. A CLI with no record the app can read falls
/// back on what the input bar last sent it
pub fn named_for(
    called: &str,
    tabs: &[Tab],
    composer: &HashMap<String, HashSet<String>>,
) -> HashSet<String> {
    let from_bar = || composer.get(called).cloned().unwrap_or_default();
    let Some(t) = tabs.iter().find(|t| t.called() == called) else {
        return from_bar();
    };
    let (Some(file), Some(spec)) = (t.record(), t.resume.as_ref().and_then(|r| r.asks.as_ref())) else {
        return from_bar();
    };
    let (asked, _) = crate::asks::read_from(&file, spec, crate::asks::begin_at(&file));
    let id = tab_id(t);
    match asked.iter().rev().find(|a| !typed_here(&id, a)) {
        Some(last) => crate::asktab::named_in(last),
        None => from_bar(),
    }
}

/// The surface position a tab is shown at (what the send queue addresses)
fn position(tabs: &[Tab], surfaces: &[Surface], id: &str) -> Option<(usize, usize)> {
    surfaces.iter().enumerate().find_map(|(p, s)| match s {
        Surface::Session(i) if tabs.get(*i).is_some_and(|t| tab_id(t) == id) => Some((p + 1, *i)),
        _ => None,
    })
}

/// Carry out what orchestration decided, with the machinery every other
/// hand-off uses: the send queue for words, the tab's own keys for Esc, and
/// the same commands a person's automation would call to close a tab or tell
/// somebody
pub fn apply(
    effects: Vec<Effect>,
    tabs: &mut [Tab],
    surfaces: &[Surface],
    pending: &mut Vec<PendingSend>,
    eng: Option<&HookEngine>,
    now_ms: u64,
) {
    for e in effects {
        match e {
            Effect::Type { tab, lead, text } => {
                let Some((pos, i)) = position(tabs, surfaces, &tab) else {
                    crate::append_hook_log(&format!("orchestration: nothing typed; <@{tab}> is not on screen"));
                    continue;
                };
                let t = &tabs[i];
                let mut chunks = Vec::new();
                let mut said = String::new();
                if let Some(l) = &lead {
                    chunks.push(t.encode_out(&format!("{l} ")));
                    said.push_str(l);
                    said.push(' ');
                }
                chunks.extend(paste_chunks(t, &text));
                said.push_str(&text);
                note_typed(&tab, &said);
                if lead.is_some() {
                    // A CLI may keep the typed line and the paste apart in its
                    // record; either may be what it shows as the request
                    note_typed(&tab, &text);
                }
                pending.push(PendingSend::new(pos, chunks, true, t.output_count(), now_ms, said.chars().count()));
                crate::append_hook_log(&format!("orchestration: typed into {tab} ({} chars)", said.chars().count()));
            }
            Effect::Esc { tab } => {
                if let Some((_, i)) = position(tabs, surfaces, &tab) {
                    let _ = tabs[i].write_bytes(b"\x1b");
                }
            }
            Effect::Close { tab } => {
                if let Some(eng) = eng
                    && let Err(e) = eng.call_primitive_as(None, crate::grants::Subject::Human, "close_tab", &[serde_json::json!(tab)])
                {
                    crate::append_hook_log(&format!("orchestration: could not close {tab}: {e}"));
                }
            }
            Effect::Person { text } => {
                if let Some(eng) = eng
                    && let Err(e) = eng.call_primitive_as(None, crate::grants::Subject::Human, "notify", &[serde_json::json!(text)])
                {
                    crate::append_hook_log(&format!("orchestration: could not notify: {e}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_app_typed_is_not_taken_for_the_person() {
        let mut t = Typed::default();
        t.note("lead", "[shikisha] You have 2 messages. Run: shikisha inbox");
        assert!(t.is_ours("lead", "[shikisha] You have 2 messages. Run: shikisha inbox"));
        assert!(!t.is_ours("lead", "Have <@codex> review it"));
        assert!(!t.is_ours("other", "[shikisha] You have 2 messages. Run: shikisha inbox"));
        // A long brief, trimmed or wrapped differently by the CLI
        let brief = "You are a worker in SHIKISHA-TERM, on assignment d5 (task t3).\nThe tab that assigned it cannot see this terminal.";
        t.note("w", brief);
        assert!(t.is_ours("w", &brief.replace('\n', " ")));
    }
}
