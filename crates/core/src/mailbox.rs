//! What a person did, waiting to be acted on.
//!
//! A shell -- the window here, a phone across the network -- reports what
//! happened as an [`Ev`](shikisha_shared::Ev). Sorting those reports into
//! queues, and handing each queue to the part of the loop that answers it, is
//! the runtime's work and not the window's: the same mailbox fills whether the
//! report came from a mouse on this desk or a thumb somewhere else.
//!
//! Every queue drains exactly once per turn, by `take_`: the loop takes what is
//! there and leaves the box empty, so nothing is acted on twice.

use crate::tab::RecordedStep;

/// Reports from a shell, sorted and waiting.
#[derive(Default)]
pub struct Mailbox {
    /// The window was closed. With nowhere left to draw, the loop has no choice but to shut down.
    pub closed: bool,
    /// The window's ✕ was pressed. The loop decides between putting the
    /// window away and quitting (a setting, and a question if an AI is at work)
    pub close_requested: bool,
    /// The notification-area icon asked for the window back
    pub tray_open: bool,
    /// "Quit" was chosen on the notification-area icon's menu
    pub tray_quit: bool,
    /// The sidebar gear (or a deep-link shortcut) was pressed. The loop opens the
    /// settings page. Carries an optional section to land on and whether to return
    /// to the board once saved (Some = requested, None = not requested).
    pub open_settings: Option<(Option<String>, bool, Option<String>, Option<u32>)>,
    /// The 🎯 panel's "save the replay" button. The loop copies the newest
    /// run's replay.lua into Downloads and answers with a flash message.
    pub replay_saves: bool,

    /// Panes clicked in the window. The loop moves focus to them
    pub focus_panes: Vec<u32>,
    /// Panes whose ✕ was pressed. The loop closes the view, not the tab
    pub close_panes: Vec<u32>,
    /// Dividers dragged in the window, as (pane, its split's new first share)
    pub pane_ratios: Vec<(usize, f32)>,
    /// Panes whose ⊞ / ⊟ caption button was pressed (pane, split downwards?)
    pub pane_splits: Vec<(u32, bool)>,
    /// ↻ / ⟲ pressed in a pane's caption: which pane, and whether to carry the
    /// conversation over
    pub restart_panes: Vec<(u32, bool)>,
    /// The size the terminal is now drawn at, when it has just been changed
    pub font_size: Option<u8>,
    /// The width the tab bar is now drawn at, when its edge has just been
    /// dragged. 0 = put away
    pub tab_width: Option<u16>,
    /// Pages placed in the window that have taken the keyboard since the last
    /// drain, by the name automation addresses them with
    pub touches: Vec<String>,
    /// Whether a placed page should be drawing the pen (the composer is shut)
    pub pen: Option<bool>,
    /// The pane that asked for a tab, if one did. Where the new tab lands
    pub add_tab_pane: Option<u32>,
    /// Names of pages whose bar button was pressed to signal "done" by a human
    pub presses: Vec<String>,
    /// Pages that finished loading (id, URL, whether refs are settled too)
    pub loads: Vec<(String, String, bool)>,
    /// Scroll-back requested via the wheel (positive = further into the past)
    pub scrolls: Vec<(i32, u16, u16)>,
    /// Navigation requested via the top bar
    pub gos: Vec<shikisha_shared::Go>,
    /// The answer to a location query we asked for (name inside the window, URL, can-go-back, can-go-forward)
    pub wheres: Vec<(String, String, bool, bool)>,
    /// Browser load start/end notifications (name inside the window, whether loading).
    /// The name is in "{ws}/{id}" form; converting to the id happens on the loop side
    /// (WinSurface doesn't know about caps). Same convention as `wheres`.
    pub loading: Vec<(String, bool)>,
    /// Relay-screen frames (JPEG byte buffers). The loop delivers these to phones.
    pub frames: Vec<Vec<u8>>,
    /// The settings page's "close settings" button was pressed. The loop closes the settings tab.
    pub close_settings: bool,
    /// The status bar's "remote connected" control was pressed. The loop cuts every
    /// remote session (rotates the token, drops the connections).
    pub remote_cut: bool,
    /// The first-run pointer that was closed, by step
    pub coach_done: Option<u8>,
    /// The thanks card was pressed: open the page, or just put it away
    pub thanks: Option<bool>,
    /// The update card was pressed: open the settings' Update card, or just
    /// put the card away
    pub update_card: Option<bool>,
    /// The `?` beside the gear was pressed
    pub help_site: bool,
    /// Tabs whose usage-limit notice was read, by screen number
    pub limit_acks: Vec<usize>,
    /// Lines a person finished in the composer, each with the tab it is for,
    /// awaiting delivery. Filled from both surfaces: the window's ipc and the
    /// phone's relay.
    pub says: Vec<(usize, String)>,
    /// Quick-action chips (Lua) fired from the bar, by index into config.actions.
    /// The loop looks up the code and runs it against the active tab.
    pub run_actions: Vec<usize>,
    /// "Operate a target tab" requests from the 🎯 panel: (target tab index, goal).
    /// target 0 = detach. The loop attaches the active AI as the target's operator.
    pub operates: Vec<(usize, String)>,
    /// 📼 record-mode toggles from the composer (true = arm the shown browser's
    /// recorder, false = silence recording everywhere).
    pub record_arms: Vec<bool>,
    /// ▶ Lua typed into the composer, awaiting a sandboxed run against the
    /// shown browser.
    pub run_luas: Vec<String>,
    /// What the git panel has asked for since the last drain: (panel, act, args)
    pub gits: Vec<(String, String, serde_json::Value)>,
    /// The same, for the file panel
    pub sftps: Vec<(String, String, serde_json::Value)>,
    /// Recorded steps reported by pages. The loop turns each into one Lua
    /// line for the composer.
    pub recorded: Vec<RecordedStep>,
    /// Text/keys typed into the composer while viewing a browser tab. The loop
    /// injects them into the shown browser — the very same caps.browser_inject the
    /// phone's relay uses, so the desktop composer and the phone share one path.
    pub injects: Vec<shikisha_shared::Input>,
    /// ✨ natural-language requests awaiting a command suggestion from the
    /// assistant AI, aimed at the active terminal tab.
    pub suggests: Vec<String>,
    /// 🔍 environment-survey button presses (the loop types the probe).
    pub surveys: usize,
    /// Vault searches awaiting an answer -- what to look for in past
    /// conversations. The loop runs the search and puts the hits into state
    pub vault_queries: Vec<String>,
    /// Past conversations asked to be reopened as resuming tabs
    pub vault_opens: Vec<shikisha_shared::Ev>,
    /// Branches asked about, and asked for: (folder cut from, branch, what to
    /// grow it from, make it, what to bring along)
    pub branches: Vec<shikisha_shared::BranchAsk>,
    /// Working folders asked about, and asked for: (the folder, the project
    /// chosen when one had to be, the branch, go ahead)
    pub repairs: Vec<(String, String, String, bool)>,
    /// Colours chosen for a project: (a folder in it, the colour)
    pub folder_colors: Vec<(String, String)>,
    /// Folders being looked through, and the one finally chosen
    pub browses: Vec<(String, bool)>,
    /// Folders renamed in the list: (folder, the new name)
    pub folder_names: Vec<(String, String)>,
    /// Folders taken out of the list. The files stay where they are
    pub folder_closes: Vec<String>,
    /// Branch folders thrown away for good
    pub folder_discards: Vec<String>,
}

impl Mailbox {
    /// Takes ownership of the names of pages whose bar button was pressed.
    /// The window only has a single report channel, so this is the only place that consumes it.
    pub fn take_presses(&mut self) -> Vec<String> {
        std::mem::take(&mut self.presses)
    }
    /// True if "close settings" was pressed (and clears the flag if so)
    pub fn take_focus_panes(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.focus_panes)
    }
    pub fn take_close_panes(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.close_panes)
    }
    pub fn take_pane_ratios(&mut self) -> Vec<(usize, f32)> {
        std::mem::take(&mut self.pane_ratios)
    }
    pub fn take_pane_splits(&mut self) -> Vec<(u32, bool)> {
        std::mem::take(&mut self.pane_splits)
    }
    pub fn take_restart_panes(&mut self) -> Vec<(u32, bool)> {
        std::mem::take(&mut self.restart_panes)
    }
    pub fn take_font_size(&mut self) -> Option<u8> {
        self.font_size.take()
    }
    pub fn take_tab_width(&mut self) -> Option<u16> {
        self.tab_width.take()
    }
    pub fn take_touches(&mut self) -> Vec<String> {
        std::mem::take(&mut self.touches)
    }
    pub fn take_pen(&mut self) -> Option<bool> {
        self.pen.take()
    }
    pub fn take_add_tab_pane(&mut self) -> Option<u32> {
        self.add_tab_pane.take()
    }
    pub fn take_close_settings(&mut self) -> bool {
        std::mem::take(&mut self.close_settings)
    }
    /// True if the "remote connected" control was pressed (and clears the flag if so)
    pub fn take_remote_cut(&mut self) -> bool {
        std::mem::take(&mut self.remote_cut)
    }
    pub fn take_coach_done(&mut self) -> Option<u8> {
        self.coach_done.take()
    }
    pub fn take_thanks(&mut self) -> Option<bool> {
        self.thanks.take()
    }
    pub fn take_update_card(&mut self) -> Option<bool> {
        self.update_card.take()
    }
    pub fn take_help_site(&mut self) -> bool {
        std::mem::take(&mut self.help_site)
    }
    pub fn take_limit_acks(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.limit_acks)
    }
    /// Takes ownership of pages that finished loading (id, URL, whether settled)
    pub fn take_loads(&mut self) -> Vec<(String, String, bool)> {
        std::mem::take(&mut self.loads)
    }
    /// Takes ownership of navigation requested via the top bar
    pub fn take_gos(&mut self) -> Vec<shikisha_shared::Go> {
        std::mem::take(&mut self.gos)
    }
    /// Takes ownership of wheel signals (tick count, row and column pointed at)
    pub fn take_scrolls(&mut self) -> Vec<(i32, u16, u16)> {
        std::mem::take(&mut self.scrolls)
    }
    /// Takes ownership of location answers
    pub fn take_wheres(&mut self) -> Vec<(String, String, bool, bool)> {
        std::mem::take(&mut self.wheres)
    }
    pub fn take_loading(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.loading)
    }
    /// Takes ownership of accumulated relay frames (the loop delivers them to phones)
    pub fn take_frames(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.frames)
    }
    /// Takes ownership of chat lines typed into model tabs
    pub fn take_says(&mut self) -> Vec<(usize, String)> {
        std::mem::take(&mut self.says)
    }
    /// Takes the indices of Lua quick-actions fired since the last drain.
    pub fn take_run_actions(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.run_actions)
    }
    /// Takes the 📼 record-mode toggles since the last drain.
    pub fn take_record_arms(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.record_arms)
    }
    /// Takes the composer Lua awaiting a sandboxed run (▶) since the last drain.
    pub fn take_run_luas(&mut self) -> Vec<String> {
        std::mem::take(&mut self.run_luas)
    }
    /// Takes what the git panel has asked for since the last drain
    pub fn take_gits(&mut self) -> Vec<(String, String, serde_json::Value)> {
        std::mem::take(&mut self.gits)
    }
    /// Takes what the file panel has asked for since the last drain
    pub fn take_sftps(&mut self) -> Vec<(String, String, serde_json::Value)> {
        std::mem::take(&mut self.sftps)
    }
    /// Takes the recorded steps reported by pages since the last drain.
    pub fn take_recorded(&mut self) -> Vec<RecordedStep> {
        std::mem::take(&mut self.recorded)
    }
    pub fn take_vault_queries(&mut self) -> Vec<String> {
        std::mem::take(&mut self.vault_queries)
    }
    pub fn take_vault_opens(&mut self) -> Vec<shikisha_shared::Ev> {
        std::mem::take(&mut self.vault_opens)
    }
    pub fn take_branches(&mut self) -> Vec<shikisha_shared::BranchAsk> {
        std::mem::take(&mut self.branches)
    }
    pub fn take_repairs(&mut self) -> Vec<(String, String, String, bool)> {
        std::mem::take(&mut self.repairs)
    }
    pub fn take_folder_colors(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.folder_colors)
    }
    pub fn take_browses(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.browses)
    }
    pub fn take_folder_names(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.folder_names)
    }
    pub fn take_folder_closes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.folder_closes)
    }
    pub fn take_folder_discards(&mut self) -> Vec<String> {
        std::mem::take(&mut self.folder_discards)
    }
    pub fn take_suggests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.suggests)
    }
    /// Takes the pending 🔍 survey presses since the last drain.
    pub fn take_surveys(&mut self) -> usize {
        std::mem::take(&mut self.surveys)
    }
    /// Takes the composer inputs bound for the shown browser since the last drain.
    pub fn take_injects(&mut self) -> Vec<shikisha_shared::Input> {
        std::mem::take(&mut self.injects)
    }
    /// Takes the pending operate-a-target requests (target index, goal).
    pub fn take_operates(&mut self) -> Vec<(usize, String)> {
        std::mem::take(&mut self.operates)
    }
}
