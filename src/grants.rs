//! Who may call which automation command.
//!
//! `caps.rs` answers "which door" -- named file/HTTP gateways, so a script can
//! never build its own path or URL. This module answers the other half of the
//! same question: **who is calling**. A command can be open to a person's own
//! automation and closed to an AI, and that is decided per command in the
//! settings screen, not buried in whichever door the call arrived through.
//!
//! Two rules keep it honest:
//!
//! 1. **The defaults live here, in code -- the config file stores only what a
//!    person changed.** If the defaults lived in the file, every command added
//!    after that file was written would inherit "whatever the file happens to
//!    say", which is either silently open or silently broken. Owning the
//!    defaults means a new command starts life with the answer its author
//!    chose, on every machine, including ones with old config files.
//! 2. **Nothing may exist without a row here.** `hooks` builds its table in
//!    Lua; a test walks that table and this catalog and fails if either has a
//!    name the other doesn't, or a row with no words for the screen. Adding a
//!    command and forgetting the permission screen is therefore not something
//!    anyone has to remember -- it is a build failure.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Who is running the code that is calling.
///
/// Decided at the door, never claimed by the caller: the external API mints a
/// token per tab, so "which tab is this" is authenticated rather than asserted,
/// and code an AI wrote arrives only through `run_scoped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// A person: their own hooks and scripts, the composer's run button, an
    /// external program they started themselves
    Human,
    /// An AI: a call carrying an AI tab's token, and Lua an AI wrote
    Ai,
}

impl Subject {
    pub fn id(self) -> &'static str {
        match self {
            Subject::Human => "human",
            Subject::Ai => "ai",
        }
    }
}

/// Which heading a command is filed under. Sixty-odd checkboxes in one column
/// is a wall nobody reads; these are what it folds into
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Tabs,
    Report,
    Browser,
    Handoff,
    Files,
    Layout,
    Basics,
    Advanced,
}

impl Group {
    pub fn id(self) -> &'static str {
        match self {
            Group::Tabs => "tabs",
            Group::Report => "report",
            Group::Browser => "browser",
            Group::Handoff => "handoff",
            Group::Files => "files",
            Group::Layout => "layout",
            Group::Basics => "basics",
            Group::Advanced => "advanced",
        }
    }

    /// The order they appear on screen: what automation touches most first,
    /// what can hurt last
    pub const ORDER: [Group; 8] = [
        Group::Tabs,
        Group::Report,
        Group::Browser,
        Group::Handoff,
        Group::Layout,
        Group::Basics,
        Group::Files,
        Group::Advanced,
    ];
}

/// One command, and who may call it when nobody has said otherwise
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub name: &'static str,
    pub group: Group,
    /// Open to a person's own automation by default
    pub human: bool,
    /// Open to an AI by default.
    ///
    /// False only where a command can (a) step outside this table -- the
    /// unwalled evaluator, raw paths and raw URLs -- or (b) destroy something
    /// the person owns. Everything else stays open: a guardrail that closes
    /// what nobody was afraid of only teaches people to switch it off.
    pub ai: bool,
    /// Part of the vocabulary AI-written Lua gets inside `run_scoped`.
    ///
    /// The walled evaluator is a narrower place on purpose (one page, nothing
    /// else), so this is a smaller set than "everything an AI may call". The
    /// permission table is the ceiling; this is the scope under it.
    pub scoped: bool,
}

const fn e(name: &'static str, group: Group, human: bool, ai: bool, scoped: bool) -> Entry {
    Entry { name, group, human, ai, scoped }
}

/// Every command that exists, with its group and its defaults.
///
/// Kept in the order it is written rather than sorted: the screen groups it
/// anyway, and a diff of this file should read like a list of decisions.
pub const CATALOG: &[Entry] = &[
    // -- Tabs: look at them, hand them work ---------------------------------
    e("state", Group::Tabs, true, true, false),
    e("tab_output", Group::Tabs, true, true, false),
    e("tab_screen", Group::Tabs, true, true, false),
    e("tab_read", Group::Tabs, true, true, false),
    e("send_to_tab", Group::Tabs, true, true, false),
    e("draft_to_tab", Group::Tabs, true, true, false),
    e("send", Group::Tabs, true, true, false),
    e("show", Group::Tabs, true, true, false),
    e("wait", Group::Tabs, true, true, false),
    e("wait_state", Group::Tabs, true, true, false),
    // Restarting a tab throws away the conversation running in it
    e("restart", Group::Tabs, true, false, false),
    // -- Reporting, and telling a person ------------------------------------
    e("note", Group::Report, true, true, false),
    e("notify", Group::Report, true, true, false),
    e("remote_url", Group::Report, true, true, false),
    // An AI CLI's own hooks report through these. Closing them would leave the
    // state dot guessing from the screen for the tools that were willing to say
    e("set_state", Group::Report, true, true, false),
    e("set_status", Group::Report, true, true, false),
    e("set_progress", Group::Report, true, true, false),
    e("set_session", Group::Report, true, true, false),
    // -- Browser -------------------------------------------------------------
    e("browser_open", Group::Browser, true, true, true),
    e("browser_close", Group::Browser, true, true, false),
    e("browser_go", Group::Browser, true, true, true),
    e("browser_nav", Group::Browser, true, true, false),
    e("browser_unnav", Group::Browser, true, true, false),
    e("browser_find", Group::Browser, true, true, true),
    e("browser_click", Group::Browser, true, true, true),
    e("browser_fill", Group::Browser, true, true, true),
    e("browser_press", Group::Browser, true, true, true),
    e("browser_fill_secret", Group::Browser, true, true, true),
    e("browser_auth", Group::Browser, true, true, true),
    e("browser_text", Group::Browser, true, true, true),
    e("browser_html", Group::Browser, true, true, true),
    e("browser_digest", Group::Browser, true, true, true),
    e("browser_fetch", Group::Browser, true, true, true),
    e("browser_state_save", Group::Browser, true, true, true),
    e("browser_state_load", Group::Browser, true, true, true),
    e("browser_snapshot", Group::Browser, true, true, true),
    e("browser_ask", Group::Browser, true, true, true),
    e("browser_pressed", Group::Browser, true, true, true),
    e("browser_unask", Group::Browser, true, true, true),
    e("browser_wait", Group::Browser, true, true, false),
    // -- Handing a run between participants ----------------------------------
    e("contract", Group::Handoff, true, true, false),
    e("exchange_new", Group::Handoff, true, true, false),
    e("exchange_write", Group::Handoff, true, true, false),
    e("exchange_append", Group::Handoff, true, true, false),
    e("exchange_take", Group::Handoff, true, true, false),
    e("record", Group::Handoff, true, true, false),
    e("record_reset", Group::Handoff, true, true, false),
    e("take_replay", Group::Handoff, true, true, false),
    e("set_result", Group::Handoff, true, true, false),
    e("open_result", Group::Handoff, true, true, false),
    e("skip", Group::Handoff, true, true, false),
    e("lint", Group::Handoff, true, true, false),
    // -- Splitting the screen -------------------------------------------------
    e("split_pane", Group::Layout, true, true, false),
    // Closing a pane takes away a place the person was looking at
    e("close_pane", Group::Layout, true, false, false),
    e("focus_pane", Group::Layout, true, true, false),
    e("equalize_panes", Group::Layout, true, true, false),
    // -- Everyday tools -------------------------------------------------------
    e("get_var", Group::Basics, true, true, false),
    e("set_var", Group::Basics, true, true, false),
    e("log", Group::Basics, true, true, true),
    e("now", Group::Basics, true, true, false),
    e("epoch_ms", Group::Basics, true, true, false),
    e("sleep", Group::Basics, true, true, false),
    e("t", Group::Basics, true, true, false),
    e("tf", Group::Basics, true, true, false),
    e("list", Group::Basics, true, true, false),
    // -- Files and the network ------------------------------------------------
    // Through a registered gateway: the destination was chosen by a person
    e("read_file", Group::Files, true, true, false),
    e("write_file", Group::Files, true, true, false),
    e("http", Group::Files, true, true, false),
    // Raw paths and raw URLs. The allowed folders and hosts are a person's own
    // escape hatch for their own scripts, and start out empty
    e("read_path", Group::Files, true, false, false),
    e("write_path", Group::Files, true, false, false),
    e("http_raw", Group::Files, true, false, false),
    // -- The evaluators --------------------------------------------------------
    // Walled: one page, and only what this table allows an AI anyway
    e("run_scoped", Group::Advanced, true, true, false),
    // Unwalled. Open to an AI, this table would be a suggestion
    e("lua", Group::Advanced, true, false, false),
];

/// The catalog as the settings screen wants it: grouped, in the order the
/// groups are shown, each command carrying the answer it has when nobody has
/// said otherwise.
///
/// Poured into the page like the dictionary is, so the screen is drawn from
/// the same list the app enforces -- there is no second copy to fall behind
pub fn catalog_json() -> String {
    #[derive(Serialize)]
    struct Command {
        name: &'static str,
        /// The dictionary key holding this command's one line of description.
        /// Spelled here rather than rebuilt in the page, so the shape of the
        /// key lives in one place
        text: String,
        human: bool,
        ai: bool,
    }
    #[derive(Serialize)]
    struct Section {
        group: &'static str,
        label: String,
        commands: Vec<Command>,
    }
    let sections: Vec<Section> = Group::ORDER
        .iter()
        .map(|g| Section {
            group: g.id(),
            label: group_key(*g),
            commands: CATALOG
                .iter()
                .filter(|c| c.group == *g)
                .map(|c| Command {
                    name: c.name,
                    text: text_key(c.name),
                    human: c.human,
                    ai: c.ai,
                })
                .collect(),
        })
        .collect();
    serde_json::to_string(&sections).unwrap_or_else(|_| "[]".into())
}

/// The row for a command, if it has one
pub fn entry(name: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.name == name)
}

/// What the settings screen says about a command. Keys live in `lang/en.json`
/// and are layered over by the other languages like everything else
pub fn text_key(name: &str) -> String {
    format!("grant.{name}")
}

pub fn group_key(group: Group) -> String {
    format!("grant.group.{}", group.id())
}

/// One command's answer, as written in the config file. Absent means "the
/// default", which is why both halves are optional: a file records decisions,
/// not a snapshot of everything
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Rule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<bool>,
}

/// What the config file holds: only the rows a person changed
pub type GrantSpec = BTreeMap<String, Rule>;

/// The decision, asked the same way from every door
#[derive(Debug, Clone, Default)]
pub struct Grants {
    spec: GrantSpec,
}

impl Grants {
    pub fn new(spec: GrantSpec) -> Self {
        Self { spec }
    }

    /// May `subject` call `name`?
    ///
    /// A name with no row is refused to an AI and allowed to a person. That
    /// case is a build failure (see the tests), so this is not a policy -- it
    /// is what happens in the seconds before someone notices, and it fails in
    /// the direction that cannot hurt.
    pub fn allows(&self, name: &str, subject: Subject) -> bool {
        let (dh, da) = match entry(name) {
            Some(e) => (e.human, e.ai),
            None => (true, false),
        };
        let rule = self.spec.get(name).copied().unwrap_or_default();
        match subject {
            Subject::Human => rule.human.unwrap_or(dh),
            Subject::Ai => rule.ai.unwrap_or(da),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_with_no_row_is_closed_to_an_ai_and_open_to_a_person() {
        let g = Grants::default();
        assert!(g.allows("db_query", Subject::Human));
        assert!(!g.allows("db_query", Subject::Ai));
    }

    #[test]
    fn the_config_file_only_has_to_carry_what_changed() {
        let mut spec = GrantSpec::new();
        spec.insert("lua".into(), Rule { human: None, ai: Some(true) });
        let g = Grants::new(spec);
        // The one row that was changed
        assert!(g.allows("lua", Subject::Ai));
        // ...and everything else still answers from the catalog
        assert!(!g.allows("write_path", Subject::Ai));
        assert!(g.allows("send_to_tab", Subject::Ai));
        assert!(g.allows("write_path", Subject::Human));
    }

    #[test]
    fn every_row_is_written_once() {
        let mut seen = std::collections::HashSet::new();
        for e in CATALOG {
            assert!(seen.insert(e.name), "{} is in the catalog twice", e.name);
        }
    }

    #[test]
    fn what_an_ai_is_refused_by_default_is_a_short_and_deliberate_list() {
        // Not a lock on the list -- a reminder that widening it is a decision.
        // Every name here can either step outside this table or destroy
        // something the person owns
        let closed: Vec<&str> = CATALOG.iter().filter(|e| !e.ai).map(|e| e.name).collect();
        assert_eq!(
            closed,
            vec!["restart", "close_pane", "read_path", "write_path", "http_raw", "lua"]
        );
    }
}
