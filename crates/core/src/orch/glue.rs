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

/// Whether a tab on another machine can run `shikisha` and reach this app:
/// the bridge on its machine is there and its line is up
pub fn reachable(t: &Tab) -> bool {
    t.machine().is_some_and(|at| crate::farlink::is_up(&at))
}

/// The tabs on this desk's screen, as orchestration needs to know them
pub fn scene(tabs: &[Tab], surfaces: &[Surface], profiles: &mut Profiles) -> Scene {
    let mut out = Vec::new();
    for s in surfaces {
        let Surface::Session(i) = s else { continue };
        let Some(t) = tabs.get(*i) else { continue };
        let (cli, typed_request) = profiles.of(t.profile_name());
        let far = t.machine().is_some();
        out.push(TabFact {
            id: tab_id(t),
            uid: t.uid().to_string(),
            called: t.called().to_string(),
            title: t.title.clone(),
            ai: t.is_ai() && !t.is_model(),
            cli,
            state: t.state,
            folder: t.cwd().map(|p| p.display().to_string()),
            far,
            reachable: !far || reachable(t),
            bridge: far && t.host_name().is_some_and(crate::farlink::agreed),
            incarnation: crate::api::incarnation_of(t.uid()),
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

/// Remember that this app typed `text` into `tab` (by uid). Called wherever
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
    composer: &HashMap<String, Named>,
) -> HashSet<String> {
    // What the person named to a tab is kept by who that tab is: a tab given
    // a closed tab's name is not given what the person named to that one
    let Some(t) = tabs.iter().find(|t| t.called() == called) else {
        return HashSet::new();
    };
    let heard = composer.get(t.uid());
    let from_bar = || heard.map(|n| n.values().cloned().collect()).unwrap_or_default();
    let (Some(file), Some(spec)) = (t.record(), t.resume.as_ref().and_then(|r| r.asks.as_ref())) else {
        return from_bar();
    };
    match last_asked(&file, spec, |a| !typed_here(t.uid(), a)) {
        // Each name as it was settled when the person was heard writing it;
        // one never heard (a request from before this run) as it is now
        Some(last) => crate::asktab::named_in(&last)
            .into_iter()
            .map(|name| heard.and_then(|n| n.get(&name).cloned()).unwrap_or_else(|| named_key(&name, tabs)))
            .collect(),
        None => from_bar(),
    }
}

/// What the person named to a tab in what they last sent it: each `<@ID>`
/// as written, and what it named then ([`named_key`])
pub type Named = HashMap<String, String>;

/// The names in `text`, each settled to what it names now
pub fn named_now(text: &str, tabs: &[Tab]) -> Named {
    crate::asktab::named_in(text).into_iter()
        .map(|name| {
            let key = named_key(&name, tabs);
            (name, key)
        })
        .collect()
}

/// What `name` names among `tabs`: the tab's uid -- so a tab closed since,
/// and another given its id, is not the one named -- or, for what is not a
/// tab (a page, by its key), the name itself
pub fn named_key(name: &str, tabs: &[Tab]) -> String {
    tabs.iter()
        .find(|t| t.id.as_deref() == Some(name) || t.called() == name)
        .map(|t| t.uid().to_string())
        .unwrap_or_else(|| name.to_string())
}

/// The last request in a record that `is_persons` accepts, read from the end
/// back. Not from a fixed stretch at the end: a job's lead reads files and runs
/// tests for an hour, and the person's request falls behind megabytes of what
/// the tools said -- read only from the end, it was lost, and with it every tab
/// the person had named
fn last_asked(file: &std::path::Path, spec: &crate::profile::AskSpec, is_persons: impl Fn(&str) -> bool) -> Option<String> {
    /// A stretch a time, overlapping so a line cut at a boundary is read whole
    /// in the stretch before it
    const STEP: u64 = 1 << 20;
    const OVERLAP: u64 = 256 * 1024;
    /// Far enough back for any conversation a person is still in
    const MOST: u64 = 512 << 20;
    let len = std::fs::metadata(file).ok()?.len();
    let mut end = len;
    while end > 0 && len - end < MOST {
        let start = end.saturating_sub(STEP);
        let (asked, _) = crate::asks::read_from(file, spec, start);
        if let Some(last) = asked.into_iter().rev().find(|a| is_persons(a)) {
            return Some(last);
        }
        if start == 0 {
            break;
        }
        // STEP is longer than OVERLAP, so every stretch starts further back
        end = start + OVERLAP;
    }
    None
}

/// The surface position a tab (by uid) is shown at (what the send queue
/// addresses)
fn position(tabs: &[Tab], surfaces: &[Surface], uid: &str) -> Option<(usize, usize)> {
    surfaces.iter().enumerate().find_map(|(p, s)| match s {
        Surface::Session(i) if tabs.get(*i).is_some_and(|t| t.uid() == uid) => Some((p + 1, *i)),
        _ => None,
    })
}

/// Carry out what orchestration decided, with the machinery every other
/// hand-off uses: the send queue for words, the tab's own keys for Esc, and
/// the same commands a person's automation would call to close a tab or tell
/// somebody. Every effect names its tab by uid: a tab closed since, and
/// another given its name, is not the one it meant
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
                    crate::append_hook_log(&format!("orchestration: nothing typed; tab {tab} is not on screen"));
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
                pending.push(PendingSend::new(pos, t.serial(), chunks, true, t.output_count(), now_ms, said.chars().count()));
                crate::append_hook_log(&format!("orchestration: typed into {} ({} chars)", tab_id(t), said.chars().count()));
            }
            Effect::Esc { tab } => {
                if let Some((_, i)) = position(tabs, surfaces, &tab) {
                    let _ = tabs[i].write_bytes(b"\x1b");
                }
            }
            Effect::Close { tab } => {
                // Closed by the name it goes by now, which is what the command
                // takes -- found from who it is, so it is that tab or none
                let Some(name) = position(tabs, surfaces, &tab).map(|(_, i)| tab_id(&tabs[i])) else { continue };
                if let Some(eng) = eng
                    && let Err(e) = eng.call_primitive_as(None, crate::grants::Subject::Human, "close_tab", &[serde_json::json!(name)])
                {
                    crate::append_hook_log(&format!("orchestration: could not close {name}: {e}"));
                }
            }
            Effect::Wake { tab } => {
                if let Some((_, i)) = position(tabs, surfaces, &tab)
                    && let Some(id) = tabs[i].cloud().and_then(|h| h.instance.as_deref())
                {
                    crate::e2b::shown(id);
                    crate::append_hook_log(&format!("orchestration: opened {}'s machine for its task", tab_id(&tabs[i])));
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
    fn the_persons_request_is_found_behind_megabytes_of_tool_output() {
        let spec = crate::profile::AskSpec {
            role_at: "/message/role".into(),
            role: "user".into(),
            text_at: "/message/content".into(),
            parts: Vec::new(),
            skip_when: vec!["/isMeta".into()],
        };
        let file = std::env::temp_dir().join(format!("glue-asked-{}.jsonl", std::process::id()));
        let mut body = String::new();
        body.push_str(r#"{"type":"user","message":{"role":"user","content":"have <@coder> fix it and <@reviewer> review it"}}"#);
        body.push('\n');
        // Three megabytes of what tools said and the model answered
        let filler = format!(r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"{}"}}]}}}}"#, "x".repeat(4000));
        for _ in 0..800 {
            body.push_str(&filler);
            body.push('\n');
        }
        body.push_str(r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"skill text naming <@otter>"}}"#);
        body.push('\n');
        std::fs::write(&file, body).unwrap();
        let got = last_asked(&file, &spec, |_| true).unwrap();
        assert!(got.contains("<@reviewer>"), "{got}");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn what_the_app_typed_is_not_taken_for_the_person() {
        let mut t = Typed::default();
        let line = super::super::text::mail_line(2);
        t.note("lead", &line);
        assert!(t.is_ours("lead", &line));
        assert!(!t.is_ours("lead", "Have <@codex> review it"));
        assert!(!t.is_ours("other", &line));
        // A long brief, trimmed or wrapped differently by the CLI
        let brief = super::super::text::brief(5, 3, "Fix the parser.", false);
        t.note("w", &brief);
        assert!(t.is_ours("w", &brief.replace('\n', " ")));
    }
}
