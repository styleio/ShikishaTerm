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
use shikisha_shared::Ev;

/// What "open the settings" is asking for: which section to land on,
/// whether to come back to the board once saved, which item to look at,
/// and which tab of it. Four maybes with no names between them was one
/// too many
pub type SettingsWanted = (Option<String>, bool, Option<String>, Option<u32>, Option<String>);

/// Reports from a shell, sorted and waiting.
#[derive(Default)]
pub struct Mailbox {
    /// The folder the person was looking at when they asked for a new tab, if
    /// they asked from inside one
    pub add_tab_folder: Option<String>,
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
    pub open_settings: Option<SettingsWanted>,
    /// The 🎯 panel's "save the replay" button. The loop copies the newest
    /// run's replay.lua into Downloads and answers with a flash message.
    pub replay_saves: bool,

    /// Panes clicked in the window. The loop moves focus to them
    pub focus_panes: Vec<u32>,
    /// Panes whose ✕ was pressed. The loop closes the view, not the tab
    pub close_panes: Vec<u32>,
    /// Tabs asked to close: (screen number, what the row was, already asked)
    pub close_tabs: Vec<(usize, String, bool)>,
    /// The question a tab's ✕ asked was answered "no"
    pub close_tab_back: bool,
    /// Closed tabs asked back: which one, or the one closed last
    pub reopen_tabs: Vec<Option<u64>>,
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
    /// The width the right-hand column is now drawn at, when its edge has just
    /// been dragged or it has been put away. 0 = put away
    pub side_width: Option<u16>,
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
    /// The name is in "{desk}/{id}" form; converting to the id happens on the loop side
    /// (WinSurface doesn't know about caps). Same convention as `wheres`.
    pub loading: Vec<(String, bool)>,
    /// Relay-screen frames (JPEG byte buffers). The loop delivers these to phones.
    pub frames: Vec<Vec<u8>>,
    /// The settings page's "close settings" button was pressed. The loop closes the settings tab.
    pub close_settings: bool,
    /// The add-a-tab dialog asked for the whole settings page. The loop gives
    /// the same page the whole window
    pub settings_full: bool,
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
    /// "How to install it" was pressed on a tab that could not start
    pub install_help: bool,
    /// "How to install it" was pressed beside a named program (its command)
    pub install_pages: Vec<String>,
    /// The first-start setup was answered: the AI to prefer, and Yolo mode
    pub setup: Option<(Option<String>, bool)>,
    /// The setup's "Refresh" was pressed, on which of its pages: look for
    /// what is installed again
    pub setup_refresh: Option<u8>,
    /// A project to clone or make new, or the clone under way to stop:
    /// (how, the URL or the name, the folder it goes in)
    pub add_projects: Vec<(String, String, String, u64, String)>,
    /// Folders on another machine to list: (host, path, the dialog's number)
    pub remote_lists: Vec<(String, String, u64)>,
    /// Machines to write into the settings: (name, address, key file, number)
    pub add_hosts: Vec<(String, String, String, u64)>,
    /// Answers about a project's found worktrees: (its shared git folder, act)
    pub found: Vec<(String, String)>,
    /// Answers from the row of a worktree being made: (its number, act)
    pub makings: Vec<(u64, String)>,
    /// Tabs whose usage-limit notice was read, by screen number
    pub limit_acks: Vec<usize>,
    /// Tabs somebody asked to look at, by screen number (0 is the board): a
    /// row pressed in the list or the bar, a notification clicked, a number
    /// pressed after the prefix. By number rather than as the keystroke it
    /// would be, because a keystroke is one digit and a desk can hold more
    /// than nine things
    pub selects: Vec<usize>,
    /// Lines a person finished in the composer, each with the tab it is for,
    /// awaiting delivery. Filled from both surfaces: the window's ipc and the
    /// phone's relay.
    pub says: Vec<(usize, String)>,
    /// Quick commands pressed, by id, each with the tab it is for (0 = the one
    /// in view). Filled from both surfaces, like `says`
    pub quicks: Vec<(String, usize)>,
    /// The window's quick-command launcher went up (true) or came down
    pub quick_shown: Option<bool>,
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
    /// Git accounts chosen in the menu at the top of the git column: (panel,
    /// account)
    pub git_accounts: Vec<(String, String)>,
    /// The same, for the transfer panel (this machine and a server)
    pub sftps: Vec<(String, String, serde_json::Value)>,
    /// The same, for the column's file list (one machine, one folder)
    pub files: Vec<(String, String, serde_json::Value)>,
    /// What the Issue tab has asked for since the last drain: (act, args)
    pub issues: Vec<(String, serde_json::Value)>,
    /// The Issue row in the list was pressed
    pub open_issues: bool,
    /// Files pressed in that list since the last drain: (panel, relative path,
    /// which change of it to show -- empty for the file itself). An empty path
    /// means "put this editor's file away"
    pub edits: Vec<(String, String, String)>,
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
    /// Folders whose project was offered an environment file and told to keep
    /// it. One act, once, because somebody read it and said yes
    pub keep_envs: Vec<String>,
    /// Working folders asked about, and asked for: (the folder, the project
    /// chosen when one had to be, the branch, go ahead)
    pub repairs: Vec<(String, String, String, bool)>,
    /// Colours chosen for a project: (a folder in it, the colour)
    pub folder_colors: Vec<(String, String)>,
    /// Folders being looked through, the one finally chosen, and the name of a
    /// folder to make where the list is standing: (path, open, make)
    pub browses: Vec<(String, bool, String)>,
    /// Working folders pressed in the list: bring back what was on screen the
    /// last time each was the one being looked at
    pub folder_views: Vec<String>,
    /// Folders renamed in the list: (folder, the new name)
    pub folder_names: Vec<(String, String)>,
    /// Tabs renamed where they stand: (screen number, name)
    pub tab_names: Vec<(usize, String)>,
    /// Folders taken out of the list. The files stay where they are
    pub folder_closes: Vec<String>,
    /// Branch folders thrown away for good: (folder, and whether the person
    /// asked not to be asked about it again)
    pub folder_discards: Vec<(String, bool)>,
}

impl Mailbox {
    /// Post one report from a page into the queue that answers it.
    ///
    /// Only what a page is allowed to report (see
    /// [`shikisha_shared::allowed_from_page`]) reaches here, and every one of
    /// them names the page it came from. A shell that shows pages some other
    /// way -- the window does, with a good deal else to do per report -- sorts
    /// them itself; this is for the shell that has nothing else to do with
    /// them.
    pub fn page_report(&mut self, ev: Ev) {
        match ev {
            Ev::Ready { from: Some(name), url, complete } => self.loads.push((name, url, complete)),
            Ev::Loading { from: Some(name), busy } => self.loading.push((name, busy)),
            Ev::Touched { from: Some(name) } => self.touches.push(name),
            Ev::Button { from: Some(name) } => self.presses.push(name),
            Ev::Where { from: Some(name), url, can_back, can_forward } => {
                self.wheres.push((name, url, can_back, can_forward));
            }
            Ev::Recorded { from: Some(child), act, sel, value, xpath, hint } => {
                self.recorded.push(RecordedStep { child, act, sel, value, xpath, hint });
            }
            // A frame of a page being watched from somewhere else. Decoded
            // here because what goes out to a phone is bytes
            Ev::Frame { data, .. } => {
                use base64::Engine as _;
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data.as_bytes())
                {
                    self.frames.push(bytes);
                }
            }
            _ => {}
        }
    }

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
    pub fn take_close_tabs(&mut self) -> Vec<(usize, String, bool)> {
        std::mem::take(&mut self.close_tabs)
    }
    pub fn take_close_tab_back(&mut self) -> bool {
        std::mem::take(&mut self.close_tab_back)
    }
    pub fn take_reopen_tabs(&mut self) -> Vec<Option<u64>> {
        std::mem::take(&mut self.reopen_tabs)
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
    pub fn take_side_width(&mut self) -> Option<u16> {
        self.side_width.take()
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
    pub fn take_settings_full(&mut self) -> bool {
        std::mem::take(&mut self.settings_full)
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
    pub fn take_install_help(&mut self) -> bool {
        std::mem::take(&mut self.install_help)
    }
    pub fn take_install_pages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.install_pages)
    }
    pub fn take_setup(&mut self) -> Option<(Option<String>, bool)> {
        self.setup.take()
    }
    pub fn take_found(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.found)
    }
    pub fn take_makings(&mut self) -> Vec<(u64, String)> {
        std::mem::take(&mut self.makings)
    }
    pub fn take_add_projects(&mut self) -> Vec<(String, String, String, u64, String)> {
        std::mem::take(&mut self.add_projects)
    }
    pub fn take_remote_lists(&mut self) -> Vec<(String, String, u64)> {
        std::mem::take(&mut self.remote_lists)
    }
    pub fn take_add_hosts(&mut self) -> Vec<(String, String, String, u64)> {
        std::mem::take(&mut self.add_hosts)
    }
    pub fn take_setup_refresh(&mut self) -> Option<u8> {
        self.setup_refresh.take()
    }
    pub fn take_help_site(&mut self) -> bool {
        std::mem::take(&mut self.help_site)
    }
    pub fn take_selects(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.selects)
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
    /// Takes the quick commands pressed since the last drain
    pub fn take_quicks(&mut self) -> Vec<(String, usize)> {
        std::mem::take(&mut self.quicks)
    }
    pub fn take_quick_shown(&mut self) -> Option<bool> {
        self.quick_shown.take()
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
    /// Takes the git accounts chosen in the git column since the last drain
    pub fn take_git_accounts(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.git_accounts)
    }
    pub fn take_issues(&mut self) -> Vec<(String, serde_json::Value)> {
        std::mem::take(&mut self.issues)
    }
    pub fn take_open_issues(&mut self) -> bool {
        std::mem::take(&mut self.open_issues)
    }
    pub fn take_files(&mut self) -> Vec<(String, String, serde_json::Value)> {
        std::mem::take(&mut self.files)
    }
    pub fn take_edits(&mut self) -> Vec<(String, String, String)> {
        std::mem::take(&mut self.edits)
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
    pub fn take_keep_envs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.keep_envs)
    }
    pub fn take_repairs(&mut self) -> Vec<(String, String, String, bool)> {
        std::mem::take(&mut self.repairs)
    }
    pub fn take_folder_colors(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.folder_colors)
    }
    pub fn take_browses(&mut self) -> Vec<(String, bool, String)> {
        std::mem::take(&mut self.browses)
    }
    pub fn take_folder_views(&mut self) -> Vec<String> {
        std::mem::take(&mut self.folder_views)
    }
    pub fn take_tab_names(&mut self) -> Vec<(usize, String)> {
        std::mem::take(&mut self.tab_names)
    }
    pub fn take_folder_names(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.folder_names)
    }
    pub fn take_folder_closes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.folder_closes)
    }
    pub fn take_folder_discards(&mut self) -> Vec<(String, bool)> {
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
