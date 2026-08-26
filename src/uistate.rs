//! Carries the state the screen needs, separated from how it looks.
//!
//! Nothing here says "how to display it." It only holds what's happening.
//! The receiving side decides the appearance.
//!
//! The reason for that separation: the same state is used by several places.
//!   - the window (our own native window)
//!   - the local terminal (the TUI we've had all along)
//!   - the phone (remote display)
//!
//! Previously the phone display was built separately, and as a result only
//! one side ended up broken. The ASCII art getting mangled, the lines all
//! running together — both happened because "show the screen" was written
//! twice. Route everything through here and it's written once.

use serde::Serialize;

/// State of a single tab
#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct TabState {
    /// 1-based tab number (same number a person presses)
    pub index: usize,
    pub name: String,
    /// Name referenced from automation
    pub id: Option<String>,
    /// WAIT / BUSY / DONE / ASK / EXIT. Used to pick the display
    pub state: String,
    /// Human-readable state name (translated). Use this one for display
    pub state_label: String,
    /// Detection profile name (Codex CLI, etc). Lets you eyeball whether it matched correctly
    pub profile: String,
    pub locked: bool,
    /// Chain depth. 0 = a conversation a human started
    pub depth: u32,
    /// Recent output volume (old to new, each 0..=7). Rendered as a bar graph
    pub activity: Vec<u8>,
    /// "pty" or "browser". Changes how it's displayed
    pub kind: String,
    /// This pty tab is a model bridge (OpenAI-compatible API). The shell offers
    /// a chat input box for it instead of leaving it as a silent idle screen.
    #[serde(default)]
    pub model: bool,
    /// A chat reply is being generated right now (drives the shell's spinner).
    #[serde(default)]
    pub busy: bool,
    /// The settings page. It rides in the pane list like a browser, but the
    /// shell keeps it out of the tab strip and reaches it via a fixed gear.
    #[serde(default)]
    pub settings: bool,
    /// Which AI this tab runs (claude / codex / gemini / deepseek / …), if any.
    /// A plain identity, not a look: the display side turns it into a brand
    /// colour so the "run several AIs side by side" story reads at a glance.
    #[serde(default)]
    pub ai: Option<String>,
    /// Whether this tab acts without pausing for confirmation — the prerequisite
    /// for driving another tab (operate). The shell greys out / blocks picking an
    /// operate target when the operator (active tab) can't. Always false for a
    /// browser or the settings pane.
    #[serde(default)]
    pub auto: bool,
    /// What the thing in this tab says it is doing, in its own words.
    ///
    /// The state dot is read off the screen and can only ever say "busy" or
    /// "waiting"; this is the agent telling us "running tests, 3 of 5". The
    /// newest one is what the tab row has room for
    #[serde(default)]
    pub status: Option<String>,
    /// How far along, 0..=1, when it has said. Shown beside the status
    #[serde(default)]
    pub progress: Option<f32>,
    /// Where this tab is: the branch, its pull request, the ports it opened.
    ///
    /// A different kind of thing from `status`, which is why it gets its own
    /// line. That one is what the tab last *said*; this is where it *is*, and
    /// it stays true while nothing is being said at all.
    ///
    /// Sent in pieces rather than as one line, because how they share a narrow
    /// row is the display's business: a branch name can be any length, and the
    /// short precious parts beside it must not be the ones that get cut
    #[serde(default)]
    pub place: Option<PlaceState>,
    /// What it is costing right now: processor and memory. Its own value, not
    /// part of `place` -- where a tab is and what it is spending are different
    /// questions, and this one changes every couple of seconds
    #[serde(default)]
    pub cost: Option<String>,
}

/// The Vault overlay's contents: what was searched and what turned up.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct VaultState {
    pub query: String,
    pub hits: Vec<crate::vault::Hit>,
    /// True when the search stopped before the end -- so the overlay can say
    /// "more than these" rather than implying it is the whole of the past
    pub capped: bool,
}

/// Where a tab is, in the parts it is made of.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct PlaceState {
    #[serde(default)]
    pub branch: Option<String>,
    /// Already written the way it reads: `#12`, `#12 merged`
    #[serde(default)]
    pub pr: Option<String>,
    #[serde(default)]
    pub ports: Vec<u16>,
}

/// Current position of the automation ring
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct BallState {
    /// Tab currently holding it (0 = the human)
    pub holder: usize,
    /// Where it was thrown from most recently
    pub from: usize,
    pub depth: u32,
    pub max: u32,
    /// "idle" / "flying" / "caught" / "held"
    pub phase: String,
    /// Progress while in flight, 0.0..=1.0
    pub progress: f32,
    /// Waiting for a human to add to the draft
    pub awaiting_human: bool,
}

/// Row of controls shown above the browser.
///
/// Whether to show them at all is decided by settings or Lua; whether each
/// one is pressable is answered by the browser. If something is shown as
/// pressable when it can't go back, the person who pressed it thinks
/// something broke
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct NavState {
    pub back: bool,
    pub forward: bool,
    pub reload: bool,
    /// URL field (how a person navigates to an arbitrary page)
    pub edit: bool,
    pub can_back: bool,
    pub can_forward: bool,
    /// Where it's currently open
    pub at: String,
    /// Whether it's loading (only for top-frame navigation; doesn't fire for in-SPA navigation)
    #[serde(default)]
    pub loading: bool,
}

/// Everything shown on screen, all in one place.
///
/// Words about appearance (color, width, symbols) don't belong here.
/// The moment one goes in, every receiving side is locked into the same look
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct UiState {
    pub workspace: String,
    pub workspaces: Vec<String>,
    pub ws_index: usize,
    /// What the focused pane is showing (0 = nothing is in it yet)
    pub active: usize,
    /// Whether INDEX is covering the window. A screen, not a pane: the board
    /// is a view OF the running things rather than one of them, so it has no
    /// place in a layout of them
    #[serde(default)]
    pub board: bool,
    /// Whether the settings form is covering the window. Also a screen rather
    /// than a pane: it asks about the whole app, not about one corner of it
    #[serde(default)]
    pub settings_open: bool,
    pub auto_enabled: bool,
    pub remote_on: bool,
    /// Whether at least one phone/browser is currently connected over the remote
    /// link. Drives the window's "remote connected — click to disconnect" pill.
    #[serde(default)]
    pub remote_conn: bool,
    /// Whether the pairing is a fixed token (config remote.sticky_token). The
    /// disconnect cuts the same either way; what differs is what comes after,
    /// so the button has to say which one it is rather than claim the other.
    #[serde(default)]
    pub remote_sticky: bool,
    /// First launch, before any settings exist yet
    pub first_run: bool,
    pub tabs: Vec<TabState>,
    pub ball: BallState,
    /// Transient notification (saved, emergency stop, etc.)
    pub flash: Option<String>,
    /// Whether help is being shown
    pub help_open: bool,
    /// What this whole app is costing the machine, for the board header --
    /// honest about our own weight rather than leaving it to a task manager
    #[serde(default)]
    pub self_cost: Option<String>,
    /// The Vault, when its overlay is open: a query and what it found. Absent
    /// the rest of the time, so the state stays small
    #[serde(default)]
    pub vault: Option<VaultState>,
    /// The help itself: the keys in force, paired with the dictionary key for
    /// the line describing each. Built from the same table the window
    /// dispatches on, so a rebound key cannot leave the help telling people to
    /// press something that no longer does anything
    #[serde(default)]
    pub help_rows: Vec<(String, String)>,
    /// Whether the workspace picker is being shown
    pub ws_open: bool,
    /// If a QR for phone pairing is being shown, the destination it encodes
    pub qr: Option<String>,
    /// The QR image itself (inline SVG). Making it a separate image request
    /// meant that of the two servers serving the same screen (window and
    /// phone), only one could actually render the image — opening it from
    /// the phone produced a broken link. Carrying it in `state` lets either
    /// side draw the same thing, and no second request is needed
    #[serde(default)]
    pub qr_svg: Option<String>,
    /// Controls shown above the browser being viewed (None = don't show)
    pub nav: Option<NavState>,
    /// How many lines back from the current screen we're scrolled (0 = current).
    /// Without knowing we've scrolled back, it looks like output has stopped
    pub scrolled: usize,
    /// Which build this is (lets you confirm you're not looking at a stale executable)
    pub build: String,
    /// Whether what's being viewed can be put back the way it started: a session
    /// relaunches its command, a page reopens exactly as it was opened. False on
    /// the board, and on the app's own screens (settings, results). The screen
    /// shows its restart button from this rather than working it out again
    #[serde(default)]
    pub restartable: bool,
    /// If the current workspace is an AI-vs-AI discussion, the session number
    /// (1-based) of the opening speaker. The dashboard shows a "start the
    /// discussion" card that sends the typed topic there. None = not a discussion.
    #[serde(default)]
    pub discuss_start: Option<usize>,
    /// The opening speaker's display name, for the start card's label.
    #[serde(default)]
    pub discuss_start_name: Option<String>,
    /// Whether the discussion is currently at rest: nobody is generating and
    /// the automation ring is idle. When true, the shell floats a prominent
    /// "pose a topic" banner over whatever tab is in view, so you never have to
    /// hunt for the opening speaker. While a participant is speaking it hides,
    /// so the AI screens are never covered. Only meaningful with `discuss_start`.
    #[serde(default)]
    pub discuss_idle: bool,
}

impl TabState {
    /// Build from a running tab
    pub fn of(index: usize, t: &crate::tab::Tab) -> Self {
        Self {
            index,
            name: t.title.clone(),
            id: t.id.clone(),
            state: t.state.label().to_string(),
            state_label: t.state.display(),
            profile: t.profile_name().to_string(),
            locked: t.locked,
            depth: t.chain_depth,
            activity: t.activity().to_vec(),
            kind: "pty".into(),
            model: t.is_model(),
            busy: t.is_generating(),
            settings: false,
            ai: t.ai_kind(),
            auto: t.auto_runs(),
            status: t.status_line(),
            progress: t.progress.as_ref().map(|(p, _)| *p),
            place: (t.place != crate::repo::Place::default()).then(|| PlaceState {
                branch: t.place.branch.clone(),
                pr: t.place.pr.clone(),
                ports: t.place.ports.clone(),
            }),
            cost: t.usage.line(),
        }
    }

    /// Build from a browser placed inside the window.
    ///
    /// It isn't a session, so it has no state and no output volume.
    /// Rather than pad it out to look similar, it's more readable left as-is
    pub fn browser(index: usize, key: &str, name: &str) -> Self {
        Self {
            index,
            name: name.to_string(),
            id: Some(key.to_string()),
            state: "WEB".into(),
            state_label: crate::i18n::t("tui.state.web"),
            profile: String::new(),
            locked: false,
            depth: 0,
            activity: Vec::new(),
            kind: "browser".into(),
            status: None,
            progress: None,
            // A browser is not in a folder and starts nothing, so it has
            // nowhere to be, and nothing of its own to cost
            place: None,
            cost: None,
            model: false,
            busy: false,
            settings: key == "settings",
            ai: None,
            auto: false,
        }
    }
}

impl BallState {
    pub fn of(b: &crate::ball::Ball, max: u32, now_ms: u64) -> Self {
        use crate::ball::Phase;
        let (phase, progress) = match b.phase(now_ms) {
            Phase::Idle => ("idle", 0.0),
            Phase::Flying { progress, .. } => ("flying", progress),
            Phase::Caught { .. } => ("caught", 1.0),
            Phase::Held { .. } => ("held", 1.0),
        };
        Self {
            holder: b.holder,
            from: b.from,
            depth: b.depth,
            max,
            phase: phase.into(),
            progress,
            awaiting_human: b.awaiting_human,
        }
    }
}

impl UiState {
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms a browser placed inside the window gets the next number in
    /// the tab sequence.
    ///
    /// It used to have no row in either the list or the board, and couldn't
    /// be switched to. Showing/hiding it was a separate operation, Ctrl+B o,
    /// which called it "part of the tabs" while not actually being a tab.
    ///
    /// If the numbering isn't contiguous, the number a person presses no longer matches the contents
    #[test]
    fn a_browser_takes_the_next_tab_number() {
        let a = TabState::browser(3, "shop", "通販サイト");
        let b = TabState::browser(4, "mail", "メール");
        assert_eq!((a.index, b.index), (3, 4));
        assert_eq!(a.kind, "browser", "セッションと同じ見せ方になっている");
        assert_eq!(a.id.as_deref(), Some("shop"), "自動化から指す名前が違う");
        // The human-readable name and the name automation refers to are different things
        assert_eq!(a.name, "通販サイト", "設定した表示名が出ていない");
        // It isn't a session, so don't pad it out to look like one
        assert!(a.activity.is_empty() && a.profile.is_empty() && a.depth == 0);
        assert!(!a.locked);
    }


    fn tab(index: usize, name: &str) -> TabState {
        TabState {
            index,
            name: name.into(),
            id: None,
            state: "WAIT".into(),
            state_label: "WAIT".into(),
            profile: "GENERIC".into(),
            locked: false,
            depth: 0,
            activity: vec![0; 4],
            kind: "pty".into(),
            model: false,
            busy: false,
            settings: false,
            ai: None,
            auto: false,
            status: None,
            progress: None,
            place: None,
            cost: None,
        }
    }

    /// Confirms appearance never leaks into state.
    ///
    /// Putting a color or symbol here would lock the window, TUI, and phone
    /// all into the same look. Keeping them separate lets each pick its own
    #[test]
    fn the_state_carries_no_appearance() {
        let s = UiState {
            workspace: "検証".into(),
            tabs: vec![tab(1, "実装")],
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        for looky in ["color", "#", "rgb", "▁", "●", "width", "px"] {
            assert!(
                !json.contains(looky),
                "見た目が混ざっている ({looky}): {json}"
            );
        }
    }



    /// Confirms identical states compare equal.
    ///
    /// If we sent every frame, an unchanged screen would still trigger a
    /// redraw. Making it comparable lets us send only when it actually changed
    #[test]
    fn identical_states_compare_equal() {
        let a = UiState {
            tabs: vec![tab(1, "実装")],
            ..Default::default()
        };
        let mut b = a.clone();
        assert_eq!(a, b);
        b.tabs[0].state = "BUSY".into();
        assert_ne!(a, b, "状態が変わったのに同じと判定された");
    }
}
