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
    /// WAIT / BUSY / DONE / QUESTION / EXIT, exactly as `TabState::label`
    /// spells them — the page uses this as a CSS class. Used to pick the display
    pub state: String,
    /// Human-readable state name (translated). Use this one for display
    pub state_label: String,
    /// When that state began, as seconds since 1970, so a row can say how long
    /// ago a tab finished. Absent for what has no state of its own (a page)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
    /// Detection profile name (Codex CLI, etc). Lets you eyeball whether it matched correctly
    pub profile: String,
    pub locked: bool,
    /// Chain depth. 0 = a conversation a human started
    pub depth: u32,
    /// Recent output volume (old to new, each 0..=7). Rendered as a bar graph
    pub activity: Vec<u8>,
    /// Which folder it works in, as a position in `UiState::groups`. Absent for
    /// a tab that is in no folder, which is drawn under no heading
    #[serde(default)]
    pub group: Option<usize>,
    /// "pty" or "browser". Changes how it's displayed
    pub kind: String,
    /// A page that cannot be driven in plain words yet: one of the two models
    /// it needs is not chosen, on the tab or on its desk. The board asks for
    /// them before 🗣 is used rather than after
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub words_unset: bool,
    /// A page whose next move is picked by a service built for deciding --
    /// quick. Otherwise a model that writes picks it, a good deal slower,
    /// and the 🗣 panel says so beside its ⚙ ([低速] / [高速])
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub words_fast: bool,
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
    /// What the CLI last said about its usage limit, while it still stands.
    /// Shown beside the tab only while it is the one being looked at
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
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
    /// Whether this tab came up on a conversation of nobody's while its folder
    /// has been worked in before. The caption offers the way back while it is
    /// true, which is until somebody speaks here: from then on this tab has a
    /// conversation of its own, and the past is the Vault's business.
    ///
    /// Except on a tab that lost one (`lost`), where the offer stays: somebody
    /// whose conversation did not come back usually notices after they have
    /// typed, and an offer that goes away exactly then is not an offer
    #[serde(default)]
    pub past: bool,
    /// Whether the conversation written down for this tab is the one it did
    /// NOT come back to. Said in the caption's colour and in what it offers,
    /// because a tab that lost one is not the same as a tab that never had one
    #[serde(default)]
    pub lost: bool,
    /// Whether relaunching this makes any sense. A session always can be; a
    /// placed page can be reopened at the URL it started on; the app's own
    /// furniture (the settings form, the result view) cannot, because there is
    /// nothing behind it to put back.
    ///
    /// Per tab rather than one flag for the focused one, because the ↻ lives
    /// in each pane's caption now: a control that appears where it would do
    /// nothing is worse than one that is not offered.
    #[serde(default)]
    pub restartable: bool,
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
    /// A message being typed into this tab on somebody's behalf right now,
    /// while it is still going in.
    ///
    /// A terminal takes a paste a chunk at a time and sets its own pace
    /// (`send.rs`), so a long one is seconds of work. Without this the
    /// composer empties and nothing else happens, and a wait nobody was told
    /// about reads as a message that was lost
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sending: Option<SendingState>,
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
    /// Whether what was said here can be read back as text (reader.rs). The
    /// display offers the reader only where there is something to read
    #[serde(default)]
    pub readable: bool,
    /// A script is asking the person something about this page (a browser):
    /// the words and the button. The board draws the bar under the page from
    /// this; the page itself never sees it, so it cannot press it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<AskState>,
    /// The file an editor is showing, relative to its folder. Absent for
    /// every other kind of tab, and for an editor with nothing open in it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// What the disk says about that file as of this frame. The editor watches
    /// it: unchanged means nobody else has written, and a change means somebody
    /// did -- which is the whole reason this travels
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_stamp: Option<String>,
    /// Which change of that file the editor is showing instead of the file
    /// itself: `work`, `staged` or `commit:<hash>`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_diff: Option<String>,
    /// The device this page is drawn on, when that is not this machine at all
    /// (the `browser_draw` setting). Such a page is already in front of the
    /// person whose machine it is and there is no picture of it to send
    /// anywhere else, so the screen says where it is instead of showing a relay
    /// that can never fill in
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away: Option<String>,
    /// Words to put in the input bar the first time this tab is looked at --
    /// the address of the issue a worktree was just made for -- and not send
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    /// For a tab that could not be started: why, and where to read how to
    /// install what it needs
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<FailedState>,
    /// For a tab that is waiting for somewhere to work: what is the matter, in
    /// the words its own screen says it. The page turns it into the folder to
    /// pick and the button that moves the tab there, because being told what
    /// is wrong and left to find the settings is not being helped
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<HoldState>,
    /// What this row is, whatever number it has (`view::surface_key`). Sent
    /// back with a press on its ✕, so a press cannot land on the tab that slid
    /// into its place in the meantime
    #[serde(default)]
    pub key: String,
    /// The name a person gave the server this tab is on ("Production"), when
    /// it is on one and they did. Worn wherever the tab is named
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<MarkState>,
}

/// A tab whose ✕ was pressed while its work would be cut off, waiting for the
/// person to say whether to close it anyway.
///
/// Held by the app rather than by the page that pressed it, so that every way
/// of closing a tab -- the ✕, the middle button, the key, the phone -- goes
/// through the same question, and so that pages placed in the window step
/// aside while it is asked
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct CloseAskState {
    /// Which asking this is. The page opens the question once per number
    pub seq: u64,
    /// The row, and what it has to still be for the answer to apply
    pub tab: usize,
    pub key: String,
    pub name: String,
    /// BUSY or QUESTION, as `TabState::label` spells them
    pub state: String,
    /// Whether an AI is what runs there, which is what the question talks about
    pub ai: bool,
    /// Whether opening it again would bring its conversation back
    pub comes_back: bool,
}

/// A closed tab the list offers to open again.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct ClosedState {
    pub id: u64,
    pub name: String,
    /// The folder it goes back into, when it goes back into one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// When it was closed (seconds since 1970)
    pub at: i64,
}

#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct FailedState {
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_url: Option<String>,
}

/// A tab held back for want of a folder, as the page draws it.
///
/// Both lines come from the tab itself (`tab::Held`), so the card on its screen
/// and the panel over it cannot say two different things. `folder` is the
/// folder it was given and could not have, which is what the picker starts
/// from -- somewhere near it is usually the answer
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct HoldState {
    pub head: String,
    pub say: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// What a script is asking the person about a page, for the bar the board
/// draws under it: the words on the left, the button on the right
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct AskState {
    pub text: String,
    pub label: String,
}

/// What has been said in one tab's folder before, for the tab that came up on
/// a conversation of nobody's: the list the person picks the way back from.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct PastState {
    /// The tab this was asked about, by the number a person presses
    pub tab: usize,
    /// What that tab is called, so the overlay can say which tab it is about
    pub name: String,
    pub hits: Vec<crate::vault::Hit>,
    /// Still being asked of the machine the folder is on
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asking: bool,
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

/// A message on its way into a tab, as the screen says it.
///
/// Two numbers rather than one: a bar on its own is a riddle, and the length
/// of what was written is what turns "still going" into "this is a long one"
#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct SendingState {
    /// How much of the text has gone over, 0..=1
    pub share: f32,
    /// How many characters the whole message is
    pub chars: usize,
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
    /// `owner/name` on GitHub, when that is where this folder pushes to. What
    /// the screen asks before it asks GitHub anything at all: with this
    /// missing, the git column has no way to tell a folder whose server has
    /// pull requests and CI from one whose server has neither
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// The colours a project is given when nobody has chosen one. Eight, because
/// past that they stop being telling apart and start being decoration
pub const PALETTE: [&str; 8] = [
    "#d97757", "#19c37d", "#4285f4", "#a06bff",
    "#e0a80a", "#12b3a8", "#e5644d", "#7f8cff",
];

/// The name a person gave a server, ready to wear: on a tab, on a file panel's
/// connection, on a question about something that cannot be undone.
///
/// Only ever made for a server with a name. A colour with no word beside it
/// does not say "production" to somebody seeing it for the first time, so a
/// mark with no name is not a quieter mark -- it is none
#[derive(Clone, Serialize, PartialEq, Eq, Debug, Default)]
pub struct MarkState {
    pub name: String,
    /// Ready to draw: what was chosen, or one worked out from the name
    pub color: String,
    /// Whether something that cannot be undone there waits for the name to be typed
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub careful: bool,
    /// Which server it is, written the way the settings file it
    /// ([`crate::ssh::Spec::machine`]), so the settings can be opened on it
    pub machine: String,
}

impl MarkState {
    /// The mark for this server, if anybody named it.
    ///
    /// Looked up the one way the settings write it, whatever case a person
    /// editing the file by hand used for the address
    pub fn of(
        machine: &str,
        marks: &std::collections::HashMap<String, crate::config::ServerMark>,
    ) -> Option<Self> {
        let key = crate::ssh::machine_key(machine);
        let mark = marks
            .get(&key)
            .or_else(|| marks.iter().find(|(k, _)| crate::ssh::machine_key(k) == key).map(|(_, m)| m))?;
        let name = mark.name.trim();
        if name.is_empty() {
            return None;
        }
        Some(Self {
            name: name.to_string(),
            color: mark
                .color
                .as_deref()
                .map(str::trim)
                .filter(|c| is_hex_colour(c))
                .map(str::to_string)
                .unwrap_or_else(|| palette_of(name)),
            careful: mark.careful,
            machine: key,
        })
    }

    /// The same, for a machine a tab or a panel is on
    pub fn of_place(
        at: &crate::elsewhere::Elsewhere,
        marks: &std::collections::HashMap<String, crate::config::ServerMark>,
    ) -> Option<Self> {
        match at {
            crate::elsewhere::Elsewhere::Ssh(spec) => Self::of(&spec.machine(), marks),
            // A sandbox is made when it is wanted and thrown away after: there
            // is no lasting machine for a name to belong to
            crate::elsewhere::Elsewhere::Cloud(_) => None,
        }
    }
}

/// `#rgb` or `#rrggbb`, and nothing else. The colour goes straight into the
/// page's styles, on the window and on a phone, so what is not plainly a colour
/// is not handed over
fn is_hex_colour(c: &str) -> bool {
    let Some(hex) = c.strip_prefix('#') else { return false };
    matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// One of the colours projects are given, worked out from a name so that the
/// same name always comes out the same colour
fn palette_of(name: &str) -> String {
    let mut h: u32 = 2166136261;
    for b in name.to_lowercase().bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    PALETTE[(h % PALETTE.len() as u32) as usize].to_string()
}

/// A folder's summary and whether it is written for it, as the settings have
/// them (see [`GroupState::describe`])
#[derive(Clone, PartialEq, Debug, Default)]
pub struct FolderLabel {
    pub folder: std::path::PathBuf,
    pub summary: Option<String>,
    pub auto: bool,
}

/// A folder, as a heading over the tabs working in it.
///
/// A group is a folder, so this is worked out from where the tabs actually
/// are rather than from what the settings say -- a tab that ended up
/// somewhere else is somewhere else, and a list that insisted otherwise
/// would be a list that lies.
///
/// Nothing is drawn when there is only one: someone who has never asked for a
/// second folder should not have to learn that the first one has a name.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct GroupState {
    /// The heading: what someone named it; else, for a worktree, the branch it
    /// was cut for; else the folder's own name
    pub name: String,
    /// The project this folder is a piece of, in words: what the settings call
    /// it, or the folder its repository is checked out in. Absent outside a
    /// repository. Worked out here rather than on the page, because the page
    /// can only see headings -- and a heading is not the project's name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The whole path: what the page knows the folder by, and says it by
    pub folder: String,
    /// The colour of this folder's project, ready to draw. Folders sharing one
    /// are branches of one repository, and the list draws them as a family.
    /// Absent when the folder is not in a repository at all -- there is no
    /// family to belong to, so there is nothing for a colour to say
    #[serde(default)]
    pub color: Option<String>,
    /// Whether this folder is a branch cut from the family's checkout
    #[serde(default)]
    pub linked: bool,
    /// The family itself: the git folder this checkout and every branch cut
    /// from it share, spelled one way. Two headings holding the same one are
    /// drawn as one household -- the checkout first, its branches under it --
    /// which is the only depth the list draws, because it is the only depth
    /// git has
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// The branch this folder is on. Worn by the checkout's heading when it
    /// has branches under it, so the row that is the project says which
    /// branch the project itself is standing on
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Whether the folder is actually on this machine. Settings travel between
    /// PCs on a sync folder or a stick, and a path that is right on one of them
    /// is simply not there on the other -- which the list has to say, because
    /// the tabs in that folder are being held back rather than run
    #[serde(default)]
    pub health: crate::folders::Health,
    /// How far this folder's branch is from the remote, as of the last fetch.
    /// A number the row can wear, so "somebody else has pushed" is something
    /// you notice rather than something you find out
    #[serde(default)]
    pub drift: crate::folders::Drift,
    /// Whether nothing runs in it yet. The list is worked out from tabs, so
    /// a folder with none would not be on it at all -- and a folder somebody
    /// just added is exactly that folder. It is shown so its + can be pressed
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub empty: bool,
    /// The issue or pull request this folder was made for, as the settings
    /// wrote it (`issue:owner/name#12`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_item: Option<String>,
    /// The machine the folder is on, when it is not this one: the card says
    /// it where a folder here says its branch
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// And the name a person gave that machine, worn beside it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<MarkState>,
    /// What is being done in it, in a few sentences: over the name when the
    /// pointer rests on it, and on a card where there is no pointer
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Whether the name and summary are written from what its AIs are asked,
    /// rather than by a person. The name is drawn a shade quieter then
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
}

impl GroupState {
    /// The folders these tabs are in, in the order they first appear, each
    /// paired with the folder itself so a tab can find its own.
    ///
    /// Tabs that are in no folder at all -- a browser is in none -- get no
    /// heading and belong to nothing, which is why the answer is looked up by
    /// path rather than handed out by position
    ///
    /// `configured` is what the settings say the desk's folders are, by
    /// path and name. Any of them no tab is in comes last, marked empty:
    /// added a minute ago, or emptied out, either way somewhere with a + to
    /// press and nothing else to say for it
    pub fn all(
        tabs: &[crate::tab::Tab],
        chosen: &std::collections::HashMap<String, String>,
        configured: &[(std::path::PathBuf, String)],
    ) -> Vec<(std::path::PathBuf, GroupState)> {
        let mut out: Vec<(std::path::PathBuf, GroupState)> = Vec::new();
        for t in tabs {
            let Some(cwd) = t.cwd() else { continue };
            if out.iter().any(|(k, _)| k == cwd) {
                continue;
            }
            // What to call it: what someone typed, else -- for a worktree --
            // the branch it was cut for, else the folder's own name. The branch
            // for a worktree because with several of them open, the branch is
            // the thing that tells them apart. Not for a project's own
            // checkout: that folder is the project, and headed by its branch
            // two projects standing on main read as the same folder twice
            let name = t
                .group_name()
                .map(str::to_string)
                .filter(|n| !n.trim().is_empty())
                .or_else(|| t.place.branch.clone().filter(|_| t.place.linked))
                .or_else(|| cwd.file_name().map(|n| n.to_string_lossy().to_string()))
                .or_else(|| t.place.branch.clone())
                .unwrap_or_default();
            out.push((
                cwd.to_path_buf(),
                GroupState {
                    name,
                    project: None,
                    folder: cwd.display().to_string(),
                    color: t
                        .place
                        .family
                        .as_deref()
                        .map(|f| Self::color_of(f, chosen)),
                    linked: t.place.linked,
                    family: t.place.family.as_ref().map(|f| f.display().to_string()),
                    branch: t.place.branch.clone(),
                    // Filled in by whoever is drawing: whether a folder is
                    // here, and how far it has drifted, are questions for the
                    // disk, and the disk is asked away from the list being built
                    health: Default::default(),
                    drift: Default::default(),
                    empty: false,
                    work_item: None,
                    host: None,
                    mark: None,
                    // Put on by whoever is drawing, from the settings
                    // (`describe`), the same as the project's name
                    summary: None,
                    auto: false,
                },
            ));
        }
        for (cwd, name) in configured {
            if out.iter().any(|(k, _)| same_folder(k, cwd)) {
                continue;
            }
            // Which household an empty folder belongs to is read off its
            // path alone. Nothing is running in it, so nothing has looked at
            // its git folder -- and this list is built on every frame, so it
            // must not start looking now: a drive that has stopped answering
            // would stop the drawing
            let family = family_by_path(cwd).map(|f| f.display().to_string());
            out.push((
                cwd.clone(),
                GroupState {
                    name: Some(name.trim())
                        .filter(|n| !n.is_empty())
                        .map(str::to_string)
                        .or_else(|| cwd.file_name().map(|n| n.to_string_lossy().to_string()))
                        .unwrap_or_default(),
                    project: None,
                    folder: cwd.display().to_string(),
                    // No colour: which project it belongs to is read off a
                    // running tab's place, and nothing is running here yet
                    color: None,
                    linked: family.is_some(),
                    family,
                    branch: None,
                    health: Default::default(),
                    drift: Default::default(),
                    empty: true,
                    work_item: None,
                    host: None,
                    mark: None,
                    // Put on by whoever is drawing, from the settings
                    // (`describe`), the same as the project's name
                    summary: None,
                    auto: false,
                },
            ));
        }
        adopt_checkouts(&mut out);
        for (_, g) in out.iter_mut() {
            g.project = g.family.as_deref().and_then(project_by_family);
        }
        by_family(out)
    }

    /// Put the names the settings give projects over the ones read off disk.
    ///
    /// `named` is (a folder, the project it says it is in). A name written on
    /// one folder of a household names the whole household: the worktrees cut
    /// from a checkout do not each repeat what project they are
    /// Mark each folder with the issue or pull request it was made for
    pub fn name_work_items(groups: &mut [(std::path::PathBuf, GroupState)], items: &[(std::path::PathBuf, String)]) {
        for (at, g) in groups.iter_mut() {
            g.work_item = items.iter().find(|(k, _)| same_folder(k, at)).map(|(_, w)| w.clone());
        }
    }

    /// Give each folder what the settings say is being done in it, and
    /// whether that is written for it (see [`FolderLabel`])
    pub fn describe(groups: &mut [(std::path::PathBuf, GroupState)], labels: &[FolderLabel]) {
        for (at, g) in groups.iter_mut() {
            let Some(l) = labels.iter().find(|l| same_folder(&l.folder, at)) else { continue };
            g.summary = l.summary.clone();
            g.auto = l.auto;
        }
    }

    pub fn name_projects(groups: &mut [(std::path::PathBuf, GroupState)], named: &[(std::path::PathBuf, String)]) {
        let mut by_family: Vec<(String, String)> = Vec::new();
        for (at, name) in named {
            let Some((_, g)) = groups.iter().find(|(k, _)| same_folder(k, at)) else { continue };
            if let Some(f) = g.family.clone()
                && !by_family.iter().any(|(k, _)| *k == f)
            {
                by_family.push((f, name.clone()));
            }
        }
        for (at, g) in groups.iter_mut() {
            let own = named.iter().find(|(k, _)| same_folder(k, at)).map(|(_, n)| n.clone());
            let kin = g
                .family
                .as_ref()
                .and_then(|f| by_family.iter().find(|(k, _)| k == f))
                .map(|(_, n)| n.clone());
            if let Some(n) = own.or(kin) {
                g.project = Some(n);
            }
        }
    }

    /// The colour a project is drawn in: the one someone chose for it, or one
    /// picked from its own name.
    ///
    /// Derived when nobody has said, so a family has a colour without anyone
    /// being asked for one, and the same colour every time it is asked. Chosen
    /// when someone has -- the answer is kept against the folder git shares,
    /// so every branch of the project changes together
    pub fn color_of(family: &std::path::Path, chosen: &std::collections::HashMap<String, String>) -> String {
        let key = family.display().to_string();
        if let Some(c) = chosen.get(&key).or_else(|| chosen.get(&key.to_lowercase()))
            && !c.trim().is_empty() {
                return c.trim().to_string();
            }
        palette_of(&key)
    }
}

/// What making a branch would do, answered while the name is being typed.
///
/// The dialog shows where the folder will be and the command that will make
/// it, and both come from here -- worked out by the same code that will run
/// it, never by the page guessing at the same rules a second time.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct BranchPlan {
    /// The folder it would be cut from, as the page named it
    pub from: String,
    /// What has been typed so far, so a late answer to an old question can be
    /// told from the answer to this one
    pub branch: String,
    pub folder: String,
    /// The command, exactly as it will run
    pub line: String,
    /// The name that was asked about, exactly as it was sent -- empty when
    /// nobody has typed one. What came back is only about this question, and
    /// the dialog has to be able to tell that
    #[serde(default)]
    pub asked: String,
    /// Which opening of the dialog asked (see `Ev::Branch`)
    #[serde(default)]
    pub seq: u64,
    /// What the new branch will grow from
    #[serde(default)]
    pub base: String,
    /// The others it could grow from instead, best first
    #[serde(default)]
    pub bases: Vec<String>,
    /// What the new folder will not have and cannot get from git -- the
    /// ignored things that are actually there, offered to come along
    #[serde(default)]
    pub carry: Vec<crate::worktree::Carry>,
    /// The ignore lines behind `carry`, each once with how the project brings
    /// what it matches -- to be chosen by line instead of one thing at a time
    #[serde(default)]
    pub carry_lines: Vec<crate::worktree::CarryLine>,
    /// Why it cannot be done, when it cannot
    #[serde(default)]
    pub error: Option<String>,
    /// Set once it has actually been made
    #[serde(default)]
    pub done: bool,
    /// Every command, when more than one folder is being made at once (one
    /// per AI). Empty when it is one folder and `line` says it
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    /// The project this is cut from: what it is called, and where its own
    /// folder is. Not a choice -- which project it is was settled by the row
    /// the person pressed -- but the thing to check before pressing, and the
    /// only way to tell two projects of the same name apart
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub project_at: String,
    /// The machines this can be made on besides this one: the ones the project
    /// has a checkout on, and the ones it could have one on. Names and kinds
    /// only: the addresses and what is filed under them are the settings'
    #[serde(default)]
    pub hosts: Vec<HostOffer>,
    /// Whether this PC is one of the places: the project has a checkout here
    #[serde(default)]
    pub here: bool,
    /// Which of them is chosen. Empty is this machine
    #[serde(default)]
    pub host: String,
    /// On a MicroVM: what it will sign in to the project's git server as, and
    /// what kind of token that is. Absent while it is being asked for
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<SignInNote>,
    /// On a MicroVM with a checkout there: whether the AI on the checkout's
    /// machine is signed in, since the worktree is a copy of that machine
    /// and a copy made before the sign-in has none. Absent where there is
    /// nothing to say (no AI, no checkout there yet)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_sign_in: Option<AiSignInNote>,
    /// The AI the project's MicroVM checkouts are given, as it says
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub machine_ai: String,
    /// The file this project's own preparation came from, when it has one
    #[serde(default)]
    pub setup_from: String,
    /// The parts of it that need another tool, named rather than dropped
    #[serde(default)]
    pub setup_unresolved: Vec<String>,
    /// A file this project could have, when it has none and one can be worked
    /// out. Shown whole and written only when somebody says so
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<crate::devcontainer::Draft>,
    /// What the settings call this project, when they have been told. Empty
    /// means nobody has written it down and it is still being worked out
    #[serde(default)]
    pub project_name: String,
    /// The branch asked for is already open in a folder: which one, whether
    /// this desk already lists it, and a name that is free instead. The dialog
    /// asks one question with it -- open that folder, or make this one under
    /// the other name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_use: Option<BranchInUse>,
}

/// A machine a worktree could be cut on, as the branch dialog lists it.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct HostOffer {
    pub name: String,
    /// `ssh` or `microvm`
    pub kind: String,
    /// Where the project is checked out there. Empty when it has no checkout
    /// there yet: a MicroVM makes one, a server has to be told where it is
    #[serde(default)]
    pub at: String,
}

/// Whether the AI on a project's checkout machine is signed in.
///
/// Said in the worktree dialog, because a worktree on a MicroVM is a copy of
/// the checkout's machine: one copied before the sign-in has none, and every
/// one made after it has it
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct AiSignInNote {
    /// The AI, by the command that starts it (`claude`)
    pub ai: String,
    /// What it is called
    pub name: String,
    /// The checkout on that machine, whose tab is where the sign-in is done
    pub checkout: String,
    /// `asking` until the machine has answered; then `yes`, `no`, or `error`
    pub state: String,
    /// Why the machine could not be asked, when that is what happened
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// What a MicroVM will sign in to the project's git server as.
///
/// Said before anything is made, because the token goes with every request
/// the machine sends there, and what runs in it -- an AI included -- can use
/// all of what the token allows
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct SignInNote {
    /// The account, as the project settings name it
    pub account: String,
    /// `fine`, `classic`, `oauth`, `app`, `unknown` (see
    /// [`crate::config::token_kind`]), or `none` when there is nothing to sign
    /// in with
    pub kind: String,
    /// Why there is none to be had, when that is what happened
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// One of the app's git accounts, as a dialog on the board offers it
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct GitAccountChoice {
    /// Its name, which is what a choice is written as
    pub name: String,
    /// What it is called on screen: its label, else its name
    pub label: String,
    /// The owners (users, organisations) it says it is for, lowercased
    pub owners: Vec<String>,
}

/// An AI a MicroVM can be given
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct MachineAiChoice {
    /// Its command, which is what the project writes down
    pub key: String,
    pub name: String,
}

/// The sign-in step of a project just cloned onto a MicroVM: the checkout's
/// machine has the AI, and a worktree is a copy of that machine, so the
/// sign-in is done here, once, before the first worktree is cut. Shown only
/// once the machine has said the AI is not signed in yet; a machine that is
/// (a key given by the machine setup) goes straight on
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct LoginStepState {
    /// Counts up each time the step is opened, so the board opens it once
    pub seq: u64,
    /// The checkout on the machine, whose AI tab the step shows
    pub folder: String,
    /// The MicroVM's name in the settings
    pub host: String,
    /// The AI, by the command that starts it, and what it is called
    pub ai: String,
    pub name: String,
    /// `asking`, `yes`, `no` or `error` -- what the machine says about the
    /// sign-in, looked at again every few seconds while the step is open
    pub state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// The checkout's AI terminal as it is now, one row per line (the same
    /// markup a pane is drawn from), so the step shows that terminal itself
    /// wherever the board is and whatever the pane behind it is showing
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub screen: String,
    /// The first web address on that terminal, wrapped rows joined: the one
    /// an AI prints for a sign-in done in a browser. A help beside the
    /// terminal, never the way -- an AI that stops printing one leaves the
    /// terminal itself, which is the way
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// What is being signed in to: empty for an AI on a MicroVM, `git` for a
    /// server's git to GitHub, which a clone there could not read without
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// For `git`: the commands drafted for that server, in order -- shown
    /// with a copy button each, and run by the person in the terminal
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// For `git`: the GitHub account the clone signs in as, when one was
    /// chosen -- the one to sign in as in the browser
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account: String,
}

/// The public addresses of a folder on a MicroVM, as asked for from its menu
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct FarPortsState {
    /// The folder it is about
    pub folder: String,
    /// Still being asked
    pub busy: bool,
    pub ports: Vec<FarPort>,
    /// Why they could not be had
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// One port something listens on in a MicroVM, and where it answers from
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct FarPort {
    pub port: u16,
    pub url: String,
}

/// A branch already open elsewhere, as the branch dialog asks about it.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct BranchInUse {
    pub branch: String,
    pub folder: String,
    /// The folder is already one of this desk's
    pub listed: bool,
    /// The name to use instead, free now
    pub instead: String,
}

/// What an AI subscription has left, as the status line draws it.
///
/// Numbers and words apart, so the page can draw a bar for the number and
/// put the words beside it: "20% used · resets in 3h 45m" reads; "5h 20%"
/// does not. Each window is absent when the service withheld it
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct UsageState {
    /// Whose allowance: "Claude", "Codex"
    pub who: String,
    /// The 5-hour window
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five: Option<UsageWindow>,
    /// The 7-day window
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week: Option<UsageWindow>,
    /// The whole reading in one sentence, for hover
    pub title: String,
}

#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct UsageWindow {
    /// What the window is called: "5h" / "7d", in the person's language
    pub name: String,
    /// Whole percent used
    pub pct: u32,
    /// "20% used"
    pub used: String,
    /// The time until the window resets, bare: "3h 45m", "9m", "4d 10h".
    /// Beside the bar the span alone is enough -- what else would a time
    /// next to "20% used" be -- and "resets in" is said only on hover, where
    /// there is room. Absent when the service did not say when
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets: Option<String>,
}

impl UsageState {
    /// Worded from a reading, at `now` (seconds since the epoch).
    ///
    /// The words are English in every language on purpose (the shipped
    /// Japanese carries no `tui.usage.*` entries, so they fall through to
    /// the English). This row has one job -- keep STOP within reach -- and
    /// the Japanese translation of it ran the reading to twice the
    /// width of "5h 24% used 9m", pushing the button toward the edge. A
    /// person's decision to translate these keys anyway is honoured, but it
    /// is a decision to make the row wider
    ///
    /// `who` is the AI's name as the hover says it: "Claude", "Codex"
    pub fn of(who: &str, l: &crate::limits::Limits, now: i64) -> Self {
        let window = |w: &Option<crate::limits::Window>, name_key: &str| {
            w.as_ref().map(|w| UsageWindow {
                name: crate::i18n::t(name_key),
                pct: w.pct,
                used: crate::i18n::tp("tui.usage.used", &[("pct", &w.pct.to_string())]),
                resets: w.resets_at.map(|at| until(at - now)),
            })
        };
        let five = window(&l.five_hour, "tui.usage.five");
        let week = window(&l.seven_day, "tui.usage.week");
        let say = |w: &Option<UsageWindow>| {
            w.as_ref()
                .map(|w| match &w.resets {
                    Some(r) => format!(
                        "{}: {} · {}",
                        w.name,
                        w.used,
                        crate::i18n::tp("tui.usage.resets", &[("t", r)])
                    ),
                    None => format!("{}: {}", w.name, w.used),
                })
                .unwrap_or_else(|| crate::i18n::t("tui.usage.unknown"))
        };
        UsageState {
            title: crate::i18n::tp(
                "tui.usage.title",
                &[("who", who), ("five", &say(&five)), ("week", &say(&week))],
            ),
            who: who.to_string(),
            five,
            week,
        }
    }
}

/// A span of seconds as a person reads it: days and hours, or hours and
/// minutes, and never a zero in front ("9m", not "0h 9m" -- the zero is a
/// character the row pays for and the reader gains nothing from). Never
/// negative -- a reset already past is "0m"
fn until(secs: i64) -> String {
    let s = secs.max(0);
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    let (d, h, m) = (d.to_string(), h.to_string(), m.to_string());
    match (d.as_str(), h.as_str()) {
        ("0", "0") => crate::i18n::tp("tui.usage.m", &[("m", &m)]),
        ("0", _) => crate::i18n::tp("tui.usage.hm", &[("h", &h), ("m", &m)]),
        (_, "0") => crate::i18n::tp("tui.usage.d", &[("d", &d)]),
        _ => crate::i18n::tp("tui.usage.dh", &[("d", &d), ("h", &h)]),
    }
}

/// An AI this machine can start, as the dialog offers it.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct AiChoice {
    /// The command, which is also the word the dialog sends back
    pub key: String,
    /// What the profile calls it
    pub name: String,
    /// The whole launch line, flag and all. Not the page's business
    #[serde(skip)]
    pub command: String,
}

/// The setup a first start asks before anything else: which AI to prefer, then
/// whether GitHub CLI is here.
///
/// Asked because a PC with none of the AIs installed meets nothing but tabs
/// that cannot start, and nobody can tell from those what is missing. The
/// same three the Assistant AI setting offers, split by whether this PC has
/// each one.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct SetupState {
    /// The ones installed here, to pick from. The first is picked until
    /// somebody picks another
    pub installed: Vec<SetupAi>,
    /// The ones this PC does not have, each with the way to its install page
    pub missing: Vec<SetupAi>,
    /// Whether GitHub CLI (`gh`) is installed here, for the setup's second page
    pub gh: bool,
}

/// Worktrees of a project on this desk that git knows about and the desk does
/// not list: made from a terminal, by another tool, or on another desk.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct DiscoveredState {
    /// The repository's shared git folder, which is what the project is known by
    pub family: String,
    /// Each one: its folder and the branch it is on
    pub found: Vec<FoundWorktree>,
    /// Whether somebody chose to keep them hidden. The row is not drawn then;
    /// the project's heading still offers to show them
    pub kept: bool,
}

/// A worktree on its way onto the desk or off it, drawn as a row under its
/// project's heading. One being made stays until it is a card of its own there;
/// one being deleted stays until its folder is gone, or until the person answers
/// for a folder that would not go.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct MakingState {
    /// Its own number, which its row's buttons answer with
    pub id: u64,
    /// The project's shared git folder, which puts the row under its heading
    pub family: String,
    /// The worktree's name (its branch)
    pub name: String,
    /// Where it is being made
    pub folder: String,
    /// Being made: `preparing`, `creating`, `setting_up` or `stopping`,
    /// and `failed` once it failed. Being deleted: `removing`, and
    /// `unremoved` once its folder would not go. `untrusted` is the made
    /// folder git will not work in until it is written down as trusted, and
    /// `unlinked` the made folder waiting to be told whether to copy in what
    /// could not be shared with the project
    pub stage: String,
    /// Why it failed, in git's words where git said
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// The folder git asked to have written down as one to trust, spelled the
    /// way git asked for it. Empty when git is happy, which is nearly always
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub trust: String,
    /// What would be written, exactly as it will appear in the file
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub trust_line: String,
    /// The file it would be written into: the person's own git settings
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub trust_file: String,
    /// The folders that could not be given a second name here, waiting for
    /// the person to say whether to copy them in instead. Empty nearly always
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unlinked: Vec<String>,
}

#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct FoundWorktree {
    pub folder: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// A project on its way from a URL or being made new, as the dialog shows it.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct AddProjectState {
    /// The dialog's own number for the attempt this is about
    pub ask: u64,
    /// Still going
    pub running: bool,
    /// git's word for the stage a clone is at, and how far through it
    #[serde(skip_serializing_if = "String::is_empty")]
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    /// Why it did not happen
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The project it made, once it is on the desk
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    /// Made on another machine: its name
    #[serde(skip_serializing_if = "String::is_empty")]
    pub host: String,
    /// Made on a MicroVM, where the dialog goes on to the project's first
    /// worktree, cut on that machine
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub microvm: bool,
    /// Under way on the board, as a row under the project: the dialog closes,
    /// and the row says how far it has got
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub started: bool,
    /// For a clone onto a MicroVM: what it will sign in to the git server as,
    /// said before it is pressed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<SignInNote>,
    /// The GitHub accounts GitHub CLI on a server is signed in to, answered
    /// to the SSH clone page for the server chosen there. Present once the
    /// server has answered, empty when it holds none
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounts: Option<Vec<String>>,
}

/// A machine a project can be on, besides this PC: one reached over SSH.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct HostChoice {
    pub name: String,
    /// Its address as written, `ssh://user@host:port`. Empty for a MicroVM
    pub at: String,
    /// `ssh` or `microvm`: one already there, or one made when wanted
    pub kind: String,
    /// The checkout of the project last written down on it, where looking
    /// starts
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project: String,
}

/// "Now make its first worktree", said by the settings page about a project.
///
/// The settings are a page of their own, in a window or a frame the board
/// does not own, and the dialog that makes a worktree is the board's. This is
/// the one way from the first to the second that works the same in the window
/// and on a phone: the page tells the app, and the app tells whichever board
/// is looking
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct BranchNext {
    /// Counts up with every ask, from 1; the board opens the dialog for a
    /// count it has not seen
    pub seq: u64,
    /// The project's checkout, as the board lists the folder
    pub folder: String,
    /// Through the project's rules first, as a project just added goes: the
    /// app says it of a project it has just cloned onto a MicroVM
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rules: bool,
}

/// A folder on another machine, listed for the add-a-project dialog.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct RemoteListState {
    /// The dialog's own number for the listing this answers
    pub ask: u64,
    pub host: String,
    /// Still being asked
    pub busy: bool,
    /// Where it is, the way that machine spells it
    #[serde(skip_serializing_if = "String::is_empty")]
    pub at: String,
    pub dirs: Vec<String>,
    /// Whether it is a git repository's own folder
    pub git: bool,
    /// Why it could not be listed: the machine could not be reached, or the
    /// folder is not there
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One AI in the first-start setup.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct SetupAi {
    /// Its command, which is also what the page sends back
    pub id: String,
    /// What it is called
    pub name: String,
    /// Whether there is a page on how to install it. The address stays with
    /// the app; the page only asks for it to be opened
    pub install: bool,
}

/// Putting a working folder back on this machine.
///
/// The dialog does not decide anything: it shows what will happen and asks. All
/// of it is worked out by the same code that carries it out, so the line on
/// screen is the line that runs.
///
/// Most of the time there is nothing to ask. Where a folder came from was
/// written down when it was made, so the answer is already known and the
/// question is only whether to go ahead. The asking case is the one where
/// nothing was written down — settings typed by hand, or a folder that predates
/// any of this — and the answer is written into the settings so that this
/// machine, and every machine after it, never asks again.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct RepairPlan {
    /// The folder this is about, as the page named it
    pub folder: String,
    /// What it is called in the list
    pub name: String,
    /// What is wrong with it, in one line
    pub trouble: String,
    /// Everything that has to happen, in order, each carrying the command
    #[serde(default)]
    pub steps: Vec<crate::folders::Step>,
    /// What stands in the way, when something does
    #[serde(default)]
    pub blocked: Option<crate::folders::Blocked>,
    /// The same, in the person's language
    #[serde(default)]
    pub said: String,
    /// Whether this is the one case that has to ask which project the folder
    /// belongs to
    #[serde(default)]
    pub asking: bool,
    /// The projects on this machine it could belong to
    #[serde(default)]
    pub projects: Vec<Project>,
    /// The branch to propose, when asking
    #[serde(default)]
    pub branch: String,
    /// Why it could not be done, after trying
    #[serde(default)]
    pub error: Option<String>,
    /// Set once the folder is actually there
    #[serde(default)]
    pub done: bool,
    /// Whether the steps are running now. Cloning takes as long as the network
    /// does, so it runs on its own and the dialog says how far it has got
    #[serde(default)]
    pub running: bool,
    /// Which step is running, counting from zero, while it is running
    #[serde(default)]
    pub at_step: usize,
}

/// Nothing put away, which is nearly always, and then the number is not sent.
fn none_hidden(n: &usize) -> bool {
    *n == 0
}

/// A project already on this machine, offered as the one a folder belongs to.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct Project {
    /// Its remote, with no credentials in it. This is what gets written down:
    /// it is the same on every machine, and a path is not
    pub origin: String,
    /// Where it is here, so the list reads as places rather than as URLs
    pub at: String,
    /// What to call it in the list
    pub name: String,
}

/// A project's name read off the git folder its checkouts share: the folder
/// the repository is checked out in (`D:\orion\.git` is orion), or a bare
/// repository's own name without its `.git`
/// The household of a project's folders on another machine: the git folder of
/// its checkout there, named with the machine so that two machines holding
/// the same path are two households. Read the same way as a git folder here
/// ([`project_by_family`] names it after the checkout), and never a path on
/// this PC
pub fn far_family(host: &str, checkout: &str) -> String {
    format!("{host}:{}/.git", checkout.trim_end_matches('/'))
}

pub(crate) fn project_by_family(family: &str) -> Option<String> {
    let p = std::path::Path::new(family.trim_end_matches(['\\', '/']));
    let name = if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(".git")) {
        p.parent()?.file_name()?
    } else {
        p.file_name()?
    };
    let name = name.to_string_lossy();
    let name = name.strip_suffix(".git").unwrap_or(&name);
    (!name.is_empty()).then(|| name.to_string())
}

/// Whether two paths name one folder, the way Windows sees it: case does
/// not tell them apart, and neither does a trailing separator
/// Where a branch folder's family would be, read off nothing but its path.
///
/// The shape this app gives a branch's folder is
/// `<parent>/<name>.worktrees/<branch as folders>`, and the project's own git
/// folder is then `<parent>/<name>/.git`. That is a guess about a folder
/// nothing is running in yet, made without touching the disk; the moment a
/// tab starts there, what git actually says takes over
pub(crate) fn family_by_path(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut at = cwd;
    loop {
        let parent = at.parent()?;
        let leaf = parent.file_name()?.to_string_lossy().to_string();
        if let Some(name) = leaf.strip_suffix(".worktrees") {
            return Some(parent.parent()?.join(name).join(".git"));
        }
        at = parent;
    }
}

/// Gives an empty checkout its household.
///
/// A project folder with nothing running in it has no family of its own --
/// nothing has read its git folder -- but its branches have, and each of them
/// names the git folder that sits inside the checkout. A folder standing
/// exactly there is the checkout, and is drawn as the head of that household
/// rather than as a stranger next to it
fn adopt_checkouts(list: &mut [(std::path::PathBuf, GroupState)]) {
    let known: Vec<String> = list
        .iter()
        .filter_map(|(_, g)| g.family.clone())
        .collect();
    for (cwd, g) in list.iter_mut() {
        if g.family.is_some() || !g.empty {
            continue;
        }
        let mine = known.iter().find(|f| {
            std::path::Path::new(f)
                .parent()
                .is_some_and(|checkout| same_folder(checkout, cwd))
        });
        if let Some(f) = mine {
            g.family = Some(f.clone());
            g.linked = false;
        }
    }
}

/// The list in the order it is drawn: each household together, its checkout
/// first and its branches after, at the place the household first appeared.
///
/// Within a household the settings' order is kept, and folders belonging to
/// no household stay exactly where they were. Tabs keep their numbers
/// whatever the order here -- the number is on the row -- so the only thing
/// that moves is which heading stands under which
pub(crate) fn by_family(list: Vec<(std::path::PathBuf, GroupState)>) -> Vec<(std::path::PathBuf, GroupState)> {
    let mut out = Vec::with_capacity(list.len());
    let mut placed = vec![false; list.len()];
    for i in 0..list.len() {
        if placed[i] {
            continue;
        }
        let Some(fam) = list[i].1.family.clone() else {
            out.push(list[i].clone());
            placed[i] = true;
            continue;
        };
        let kin: Vec<usize> = (i..list.len())
            .filter(|&j| {
                !placed[j]
                    && list[j].1.family.as_deref().is_some_and(|f| {
                        same_folder(std::path::Path::new(f), std::path::Path::new(&fam))
                    })
            })
            .collect();
        // Spelled the one way for the whole household, so whoever draws it
        // can tell kin apart by the string alone
        let mut take = |j: usize| {
            let mut g = list[j].clone();
            g.1.family = Some(fam.clone());
            out.push(g);
            placed[j] = true;
        };
        for &j in kin.iter().filter(|&&j| !list[j].1.linked) {
            take(j);
        }
        for &j in kin.iter().filter(|&&j| list[j].1.linked) {
            take(j);
        }
    }
    out
}

/// Whether two spellings name one folder. On Windows the case of the letters
/// and the direction of the slashes do not make a different folder: git writes
/// its notes with forward slashes, the settings keep whatever was typed, and a
/// worktree compared only by case read as a stranger to itself
pub fn same_folder(a: &std::path::Path, b: &std::path::Path) -> bool {
    let key = |p: &std::path::Path| {
        let s = p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
        if cfg!(windows) { s.replace('/', "\\") } else { s }
    };
    key(a) == key(b)
}

/// Folders to choose from, when somewhere new is being opened.
///
/// The list is made here rather than by a dialog the operating system draws,
/// because half the people using this are holding a phone and there is no
/// dialog to draw for them. One list, walked the same way from either.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct BrowseState {
    /// Where the list is standing. Empty means the top, where the drives are
    pub at: String,
    /// The folder above, when there is one
    #[serde(default)]
    pub up: Option<String>,
    /// What is inside, folders only -- files are not somewhere to work
    pub dirs: Vec<String>,
    /// The files inside as well, when the walk was asked for them. Somewhere
    /// to work is a folder, so the sidebar never asks; a setting that names a
    /// file (a key, a secrets file) does
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Why nothing is listed, when nothing is
    #[serde(default)]
    pub error: Option<String>,
    /// When each folder in `dirs` last changed, in the same order, as seconds
    /// since 1970. Beside the names rather than inside them so that the settings
    /// page, which reads `dirs` as plain paths, reads exactly what it always has.
    /// `None` where the system would not say
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified: Vec<Option<i64>>,
    /// Where a walk can start: home, the desktop, the projects this app already
    /// knows, the drives. The same places whichever folder is being looked at,
    /// so the column they sit in does not change under somebody's pointer
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<BrowsePlace>,
    /// What happened to the folder somebody last asked to make here: its path
    /// when it was made, so the list can point at it, or why it was not. Kept
    /// apart from `error`, which is about the listing -- a name that could not be
    /// used is no reason to empty the list the person is looking at
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_error: Option<String>,
    /// Whether the folder being looked at is in a git repository
    #[serde(default)]
    pub at_git: bool,
    /// Whether each folder in `dirs` is, in the same order
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub git: Vec<bool>,
}

/// One place a walk can start from.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct BrowsePlace {
    /// `home`, `desktop`, `project` or `drive` -- what it is, so the page can
    /// draw it the way that kind of place is drawn and say it in its own words
    pub kind: String,
    /// What it is called. Empty for home and the desktop, whose names are words
    /// the page already has; the project's name, or the drive's letter
    pub name: String,
    pub path: String,
}

impl BrowseState {
    /// Where the walk starts: the person's own folder, then the drives.
    ///
    /// Their own first, because that is where work is, and a list that opens
    /// on `A:` makes everyone scroll past floppy disks to reach it
    fn top() -> Vec<String> {
        let mut out = Vec::new();
        // Where this person's own things are, whichever way the system spells it
        for var in ["USERPROFILE", "HOME"] {
            if let Ok(home) = std::env::var(var)
                && !home.is_empty() && std::path::Path::new(&home).is_dir() {
                    out.push(home);
                    break;
                }
        }
        // Then everything there is. Windows has one tree per drive and a name
        // for each; every other system has one tree, and `/` is its name
        #[cfg(windows)]
        for letter in 'A'..='Z' {
            let root = format!("{letter}:\\");
            if std::path::Path::new(&root).is_dir() {
                out.push(root);
            }
        }
        #[cfg(not(windows))]
        out.push("/".to_string());
        out
    }

    /// What is inside a folder, or the drives when nothing is named.
    ///
    /// Folders only, in the order a person reads them, and capped: a folder
    /// with fifty thousand entries in it is not a list anyone scrolls, and
    /// building it would stall the frame it was asked in
    pub fn of(path: &str) -> Self {
        Self::walk(path, false)
    }

    /// The same walk, with the files in each folder listed too.
    ///
    /// For choosing a file rather than a folder. Files are listed, never
    /// entered, and the same cap applies to them
    pub fn with_files(path: &str) -> Self {
        Self::walk(path, true)
    }

    fn walk(path: &str, want_files: bool) -> Self {
        let at = path.trim().to_string();
        if at.is_empty() {
            return Self { at, dirs: Self::top(), ..Default::default() };
        }
        // A file, named or pasted, is looked at from the folder it is in
        let at = match std::path::Path::new(&at).is_file() {
            true => std::path::Path::new(&at).parent().map(|p| p.display().to_string()).unwrap_or(at),
            false => at,
        };
        let here = std::path::Path::new(&at);
        // A drive has no folder above it, but there is still somewhere to go
        // back to -- the list of drives itself. Without this, stepping into
        // one is a door that only opens inwards
        let up = Some(here.parent().map(|p| p.display().to_string()).unwrap_or_default());
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        let mut error = None;
        match std::fs::read_dir(here) {
            Ok(entries) => {
                for e in entries.flatten().take(4000) {
                    let p = e.path();
                    // Skip what the person cannot open anyway, and the places
                    // tools keep their own things
                    let hidden = p
                        .file_name()
                        .map(|n| {
                            let n = n.to_string_lossy();
                            n.starts_with('.') || n.starts_with('$')
                        })
                        .unwrap_or(false);
                    if hidden || kept_by_the_system(&e) {
                        continue;
                    }
                    if p.is_dir() {
                        if dirs.len() < 400 {
                            // From the entry the listing already read, not a
                            // second trip to the disk per folder
                            let when = e
                                .metadata()
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                .map(|d| d.as_secs() as i64);
                            dirs.push((p.display().to_string(), when));
                        }
                    } else if want_files && files.len() < 400 {
                        files.push(p.display().to_string());
                    }
                    if dirs.len() >= 400 && (!want_files || files.len() >= 400) {
                        break;
                    }
                }
                dirs.sort_by_key(|d| d.0.to_lowercase());
                files.sort_by_key(|f| f.to_lowercase());
            }
            Err(e) => error = Some(e.to_string()),
        }
        let (dirs, modified): (Vec<String>, _) = dirs.into_iter().unzip();
        // Whether choosing each would add a git repository, which decides what
        // happens next: a repository goes on to its first worktree, anything
        // else is asked about first. Inside a repository every folder is in
        // it; elsewhere a folder is a repository when it holds a `.git` of its
        // own -- one look per folder, not a walk up from each
        let at_git = crate::repo::family_of(here).is_some();
        let git = match at_git {
            true => vec![true; dirs.len()],
            false => dirs.iter().map(|d| std::path::Path::new(d).join(".git").exists()).collect(),
        };
        Self { at, up, dirs, files, error, modified, at_git, git, ..Default::default() }
    }

    /// The same walk, with the places to start from filled in.
    ///
    /// `projects` are the checkouts this app already knows, by name and path.
    /// The folder picker asks for this; the settings page, which walks to find
    /// a key file, does not need a column of shortcuts and never asks
    pub fn with_places(mut self, projects: &[(String, String)]) -> Self {
        let mut out = Vec::new();
        for var in ["USERPROFILE", "HOME"] {
            if let Ok(home) = std::env::var(var)
                && !home.is_empty()
                && std::path::Path::new(&home).is_dir()
            {
                out.push(BrowsePlace { kind: "home".into(), name: String::new(), path: home });
                break;
            }
        }
        if let Some(desk) = desktop_dir() {
            out.push(BrowsePlace { kind: "desktop".into(), name: String::new(), path: desk });
        }
        let mut seen = std::collections::HashSet::new();
        for (name, path) in projects {
            if std::path::Path::new(path).is_dir() && seen.insert(path.to_lowercase()) {
                out.push(BrowsePlace { kind: "project".into(), name: name.clone(), path: path.clone() });
            }
        }
        #[cfg(windows)]
        for letter in 'A'..='Z' {
            let root = format!("{letter}:\\");
            if std::path::Path::new(&root).is_dir() {
                out.push(BrowsePlace { kind: "drive".into(), name: format!("{letter}:"), path: root });
            }
        }
        #[cfg(not(windows))]
        out.push(BrowsePlace { kind: "drive".into(), name: "/".into(), path: "/".into() });
        self.places = out;
        self
    }
}

/// Whether a folder is one Windows keeps for itself: hidden and system both.
///
/// A home folder holds a dozen of these -- `My Documents`, `NetHood`, `SendTo`,
/// `Local Settings` -- left behind for programs from twenty years ago. Every one
/// refuses to open, and Explorer does not show them. A list that does is a list
/// where a third of the rows are doors that are locked. Both attributes, not
/// either: a folder somebody hid on purpose is still theirs to walk into
#[cfg(windows)]
fn kept_by_the_system(e: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    e.metadata().is_ok_and(|m| m.file_attributes() & (HIDDEN | SYSTEM) == (HIDDEN | SYSTEM))
}
#[cfg(not(windows))]
fn kept_by_the_system(_: &std::fs::DirEntry) -> bool {
    false
}

/// The desktop, where the system keeps it.
///
/// Not `%USERPROFILE%\\Desktop`: with OneDrive backing it up, the desktop is
/// somewhere under OneDrive and is named in the system's own language -- on the
/// machine this was written on, it is not called Desktop at all. A guessed path would
/// open a folder that is not the one on the screen behind the window
#[cfg(windows)]
fn desktop_dir() -> Option<String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: the returned buffer is ours to free, and is freed on every path
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        s.filter(|d| std::path::Path::new(d).is_dir())
    }
}
#[cfg(not(windows))]
fn desktop_dir() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let d = std::path::Path::new(&home).join("Desktop");
    d.is_dir().then(|| d.display().to_string())
}

/// Makes a folder somebody named in the picker, inside the folder being looked
/// at. The new folder's path, or why it could not be made.
///
/// One level, never a chain: a name with a separator in it is somebody trying
/// to reach somewhere else from a box that is only for naming, so it is
/// refused rather than followed. Reserved device names are refused for the
/// same reason Windows refuses them -- a folder called `CON` cannot be opened
/// afterwards
pub fn make_folder(inside: &str, name: &str) -> Result<String, String> {
    let name = name.trim();
    if inside.trim().is_empty() {
        return Err(crate::i18n::t("tui.browse.make.nowhere"));
    }
    let bad = name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with('.')
        || name.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control());
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if bad || reserved {
        return Err(crate::i18n::t("tui.browse.make.badname"));
    }
    let at = std::path::Path::new(inside.trim()).join(name);
    if at.exists() {
        return Err(crate::i18n::t("tui.browse.make.exists"));
    }
    std::fs::create_dir(&at)
        .map(|_| at.display().to_string())
        .map_err(|e| crate::i18n::tp("tui.browse.make.failed", &[("e", &e.to_string())]))
}

#[cfg(test)]
mod browse_tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir()
            .join(format!("shikisha-browse-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Every folder listed has a time beside it, in the same place in the
    /// list. They are two lists because the settings page reads `dirs` as bare
    /// paths; two lists that fall out of step would put yesterday next to the
    /// wrong folder, which is worse than no date at all
    #[test]
    fn each_folder_is_listed_with_when_it_changed() {
        let d = scratch("dates");
        for n in ["b", "a", "C"] {
            std::fs::create_dir(d.join(n)).unwrap();
        }
        let st = BrowseState::of(&d.display().to_string());
        assert_eq!(st.dirs.len(), 3);
        assert_eq!(st.modified.len(), st.dirs.len(), "the number of dates and names does not match");
        assert!(st.modified.iter().all(Option::is_some), "the dates were not read");
        let leaves: Vec<String> = st
            .dirs
            .iter()
            .map(|p| std::path::Path::new(p).file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(leaves, ["a", "b", "C"], "they are not in the order a person reads");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The folders Windows keeps for itself are left out; one somebody only hid
    /// is not. Explorer draws the same line
    #[cfg(windows)]
    #[test]
    fn folders_the_system_keeps_for_itself_are_not_listed() {
        let d = scratch("system");
        for n in ["mine", "hidden", "kept"] {
            std::fs::create_dir(d.join(n)).unwrap();
        }
        let attrib = |path: &std::path::Path, flags: &[&str]| {
            let ok = std::process::Command::new("attrib")
                .args(flags)
                .arg(path)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            assert!(ok, "attrib cannot be used");
        };
        attrib(&d.join("hidden"), &["+h"]);
        attrib(&d.join("kept"), &["+h", "+s"]);
        let st = BrowseState::of(&d.display().to_string());
        let leaves: Vec<String> = st
            .dirs
            .iter()
            .map(|p| std::path::Path::new(p).file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(leaves, ["hidden", "mine"], "system items show, or items that were only hidden disappeared");
        attrib(&d.join("kept"), &["-h", "-s"]);
        attrib(&d.join("hidden"), &["-h"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_is_made_one_level_down_and_nowhere_else() {
        let d = scratch("make");
        let inside = d.display().to_string();
        let made = make_folder(&inside, "  shinkoku ").expect("it should be possible to make it");
        assert!(std::path::Path::new(&made).is_dir());
        assert!(made.ends_with("shinkoku"), "surrounding spaces stayed in the name");
        // The same name again is refused, not silently reused
        assert!(make_folder(&inside, "shinkoku").is_err());
        // A box for a name is not a way to reach somewhere else
        for bad in ["..", "a/b", "a\\b", "c:x", "", "   ", "end.", "CON", "com1", "LPT9.txt"] {
            assert!(make_folder(&inside, bad).is_err(), "{bad:?} got through");
        }
        assert!(!d.join("a").exists(), "a name containing a separator created intermediate folders");
        // And nothing is made at the top, where there is no folder to make it in
        assert!(make_folder("", "x").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn places_start_with_home_and_end_with_the_drives() {
        let d = scratch("places");
        let st = BrowseState::of("").with_places(&[
            ("tools".into(), d.display().to_string()),
            ("again".into(), d.display().to_string()),
            ("gone".into(), d.join("not-here").display().to_string()),
        ]);
        let kinds: Vec<&str> = st.places.iter().map(|p| p.kind.as_str()).collect();
        assert_eq!(kinds.first(), Some(&"home"), "home is not first");
        assert_eq!(kinds.last(), Some(&"drive"), "the drives are not last");
        let projects: Vec<&str> = st
            .places
            .iter()
            .filter(|p| p.kind == "project")
            .map(|p| p.name.as_str())
            .collect();
        // One entry per checkout, and none for a checkout that is not here
        assert_eq!(projects, ["tools"], "the same place twice, or a place that does not exist, is shown");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// What the screen shows about a run that ended badly
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct LastExit {
    /// As the machine spells it (`0xc0000409`). Empty when it kept no record
    #[serde(default)]
    pub code: String,
    /// When it happened, in the machine's own words. Empty when unknown
    #[serde(default)]
    pub when: String,
    /// The explanation, once somebody has been asked for one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Whether the asking is under way, so the button can say so instead of
    /// looking like it did nothing
    #[serde(default)]
    pub asking: bool,
    /// The assistant AI that would be asked, by the name it goes by on
    /// screen. Empty when there is none installed, which is why the button
    /// cannot be pressed -- said rather than left to be discovered
    #[serde(default)]
    pub by: String,
    /// How many conversations came back with the tabs, and how many did not.
    /// The question somebody has after their terminal disappears
    #[serde(default)]
    pub carried: u32,
    #[serde(default)]
    pub lost: u32,
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
    /// The second reload, which throws away what is held first
    #[serde(default)]
    pub reload_hard: bool,
    /// URL field (how a person navigates to an arbitrary page)
    pub edit: bool,
    /// The switch for how a press on a relayed page is meant. Only a phone
    /// watching the page draws it; the window has a mouse and nothing to choose
    #[serde(default)]
    pub point: bool,
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
    pub desk: String,
    /// The desk on screen, by the id its settings and secrets are filed under.
    /// What a tool asks with when an answer belongs to the desk -- a name on
    /// screen can be shared by two
    #[serde(default)]
    pub desk_id: String,
    /// The keys that open the tools from anywhere, by what they open, for the
    /// ones that are registered and work. Shown beside what they open
    #[serde(default)]
    pub hotkeys: std::collections::BTreeMap<String, String>,
    /// The keys set to work with no prefix, for the page to hand on when they
    /// are pressed in one of its own boxes (see `keys::direct_now`)
    #[serde(default)]
    pub direct_keys: Vec<crate::keys::DirectKey>,
    /// The quick commands, laid out and with their drawings, for the launcher.
    /// Shared rather than copied: it changes only when the settings are saved,
    /// and the state is put together many times a second
    #[serde(default)]
    pub quick: std::sync::Arc<crate::quick::QuickView>,
    /// Where each kind of quick command would go right now, by
    /// `quick::dest_key`: to which tab, into a new one in which folder, or
    /// nowhere and why
    #[serde(default)]
    pub quick_to: std::collections::BTreeMap<String, crate::quick::QuickDest>,
    /// The folders this desk's tabs are working in. One means nothing is
    /// drawn: the heading only exists to tell folders apart
    #[serde(default)]
    pub groups: Vec<GroupState>,
    /// The answer to "what would happen if I made this branch"
    #[serde(default)]
    pub branch: Option<BranchPlan>,
    /// The answer to "what would it take to have this folder here"
    #[serde(default)]
    pub repair: Option<RepairPlan>,
    /// Folders to choose from, while somewhere new is being opened
    #[serde(default)]
    pub browse: Option<BrowseState>,
    pub desks: Vec<String>,
    pub desk_index: usize,
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
    /// Whether the settings form is the add-a-tab dialog: a rectangle over the
    /// board, which stays drawn, dimmed, behind it
    #[serde(default)]
    pub settings_float: bool,
    pub auto_enabled: bool,
    /// How the content area is divided right now.
    ///
    /// Here, and not in a message of its own, because it is part of the same
    /// moment as everything else in this struct. Sent separately, the page
    /// learned it a beat late and drew one frame with the new answer to
    /// "which row am I on" and the old rectangles (see `view::PanesState`)
    #[serde(default)]
    pub panes: crate::view::PanesState,
    /// The split row whose arrangement is on screen, by the name automation
    /// calls it. Absent when what is in front is one row, undivided.
    ///
    /// The list needs it because `active` cannot say: inside a split, `active`
    /// is the row in the focused pane, which is where the keyboard is and not
    /// what is in front. Without this the split row is the one row that is
    /// never drawn as the one being looked at
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_open: Option<String>,
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
    /// What the focused tab is aimed at, as a screen number, when it has been
    /// aimed at anything. The picker on screen is the only place an aim is
    /// chosen and it is written down against that tab, so this is how it comes
    /// back after a restart -- not a second setting to keep in step with.
    #[serde(default)]
    pub aim: Option<usize>,
    /// First launch, before any settings exist yet
    pub first_run: bool,
    /// How many times the settings have been read in since the start. A
    /// dialog showing an answer worked out from the settings asks again when
    /// this moves, so a rule saved while it is open is the rule it shows
    #[serde(default)]
    pub settings_gen: u64,
    /// The AIs this machine can start in a folder just made
    #[serde(default)]
    pub ais: Vec<AiChoice>,
    /// What each AI's subscription has left, by the tab's AI kind ("claude",
    /// "codex"), for those that are known. The page shows the one belonging
    /// to the tab in view, and nothing over a tab of any other kind
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub usage: std::collections::BTreeMap<String, UsageState>,
    /// The first-run pointer that is up: 1 = add a folder, 2 = press its +
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coach: Option<u8>,
    /// A worktree is deleted from the list without asking first
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub discard_unasked: bool,
    /// How many folders are put out of sight until the next launch. A number
    /// rather than the list: one line brings all of them back, and a line for
    /// each would take the width the list is drawn in
    #[serde(default, skip_serializing_if = "none_hidden")]
    pub hidden: usize,
    /// The first-start setup, while it has not been answered
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SetupState>,
    /// A project being cloned or made new, from the add-a-project dialog
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add_project: Option<AddProjectState>,
    /// Worktrees of this desk's projects that the desk does not list
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovered: Vec<DiscoveredState>,
    /// Worktrees being made, or that failed to be and are still said
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub making: Vec<MakingState>,
    /// The machines a project can be added on, besides this PC
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<HostChoice>,
    /// The aliases of `~/.ssh/config`, which a new machine can be filled in from
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_aliases: Vec<crate::discover::SshAlias>,
    /// A folder on another machine, listed for the add-a-project dialog
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_list: Option<RemoteListState>,
    /// The public addresses of a folder on a MicroVM, last asked for
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub far_ports: Option<FarPortsState>,
    /// The sign-in step of a project just cloned onto a MicroVM, while it
    /// is open
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_step: Option<LoginStepState>,
    /// The AIs a MicroVM can be given, by command and name
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machine_ais: Vec<MachineAiChoice>,
    /// Where a cloned or new project goes until somebody picks elsewhere
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_home: String,
    /// The project whose worktree rules were just settled on the settings
    /// page, which asked for its first worktree next. A count with it, so the
    /// board opens the dialog once for each ask and never again on a reload
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_next: Option<BranchNext>,
    /// The AI chosen under Basic > Assistant AI (its command), which a new
    /// worktree starts with when this PC has it
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub assistant: String,
    /// The app's own git accounts, for a dialog that asks which one a
    /// machine signs in as
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub git_accounts: Vec<GitAccountChoice>,
    /// The thanks card, when it is up: `github` or `store`, which is where
    /// its button leads
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thanks: Option<String>,
    /// The newer version the update card asks about, when it is up. Answered
    /// once, either way, and not shown again for that version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<crate::update::Offer>,
    /// Whether the settings name a phone as somewhere answers go. Only the
    /// browser holding the page can know whether it is that phone yet, so
    /// the app says just that one is wanted, and the phone's board offers
    /// to become it (src/shell.rs, drawPushBar).
    #[serde(default)]
    pub push_wanted: bool,
    /// How the run before this one ended, when it did not end properly.
    ///
    /// A program that disappears without a word leaves the person with
    /// nothing to go on, so the next start says what the machine recorded --
    /// and offers to have it explained (src/shell.rs, drawCrashBar)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<LastExit>,
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
    /// What was said before in one tab's folder, while that overlay is open
    #[serde(default)]
    pub past: Option<PastState>,
    /// The help itself: the keys in force, paired with the dictionary key for
    /// the line describing each. Built from the same table the window
    /// dispatches on, so a rebound key cannot leave the help telling people to
    /// press something that no longer does anything
    #[serde(default)]
    pub help_rows: Vec<(String, String)>,
    /// Whether the desk picker is being shown
    pub desk_open: bool,
    /// If a QR for phone pairing is being shown, the destination it encodes
    pub qr: Option<String>,
    /// The QR image itself (inline SVG). Making it a separate image request
    /// meant that of the two servers serving the same screen (window and
    /// phone), only one could actually render the image — opening it from
    /// the phone produced a broken link. Carrying it in `state` lets either
    /// side draw the same thing, and no second request is needed
    #[serde(default)]
    pub qr_svg: Option<String>,
    /// Which network that destination is on ("tailscale" / "lan" / "local" /
    /// "public"), worked out by `netaddr::url_kind`. The screen colours a badge
    /// with it: whether the link can be handed around is the one thing about an
    /// address a person has to know, and it is not readable from the digits
    #[serde(default)]
    pub qr_kind: Option<String>,
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
    /// If the current desk is an AI-vs-AI discussion, the session number
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
    /// A tab's ✕ waiting for an answer, while there is one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_ask: Option<CloseAskState>,
    /// This desk's closed tabs that can be opened again, newest first
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<ClosedState>,
}

/// Whether this tab's conversation can be read back as flowing text.
///
/// True when the CLI keeps a record we know how to find and we know which
/// conversation is this tab's — claude and codex today. It rides in the tab
/// state rather than in the phone's own snapshot because the reader is a
/// property of the tab, and both surfaces ask the same question of it
fn readable(t: &crate::tab::Tab) -> bool {
    t.session.is_some() && t.resume.as_ref().is_some_and(|r| r.verify.is_some())
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
            since: t.state_since.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs()),
            profile: t.profile_name().to_string(),
            locked: t.locked,
            depth: t.chain_depth,
            activity: t.activity().to_vec(),
            group: None,
            kind: "pty".into(),
            words_unset: false,
            words_fast: false,
            restartable: true,
            past: t.past_here && (!t.spoke() || t.lost),
            lost: t.lost,
            model: t.is_model(),
            busy: t.is_generating(),
            settings: false,
            ai: t.ai_kind(),
            limit: t.limit_note().map(str::to_string),
            auto: t.auto_runs(),
            status: t.status_line(),
            progress: t.progress.as_ref().map(|(p, _)| *p),
            // Filled in by `view::ui_state_of`: what is being typed into a tab
            // is the runtime's business, not the tab's
            sending: None,
            place: (t.place != crate::repo::Place::default()).then(|| PlaceState {
                branch: t.place.branch.clone(),
                pr: t.place.pr.clone(),
                ports: t.place.ports.clone(),
                repo: t.place.repo.clone(),
            }),
            cost: t.usage.line(),
            readable: readable(t),
            // A session is not a page; nothing asks the person about it here
            ask: None,
            // ...and a session is not showing a file
            file: None,
            file_stamp: None,
            file_diff: None,
            // ...and a session is drawn wherever its terminal is, which is here
            away: None,
            failed: None,
            hold: t.held().map(|h| HoldState {
                head: h.header(),
                say: h.say(),
                folder: h.folder().map(|f| f.display().to_string()),
            }),
            draft: None,
            // Filled in by `view::ui_state_of`, which knows the rows
            key: String::new(),
            // Filled in by `view::ui_state_of` too: which server a tab is on
            // is the tab's, but what the person named it is the settings'
            mark: None,
        }
    }

    /// Build from a browser placed inside the window.
    ///
    /// It isn't a session, so it has no state and no output volume.
    /// Rather than pad it out to look similar, it's more readable left as-is
    /// The git panel. It has no process and no page of its own -- the board
    /// draws it -- so most of what a tab carries is simply absent
    /// The file panel. Two lists of files and no process, like the git panel
    pub fn sftp(index: usize, key: &str, name: &str, group: Option<usize>) -> Self {
        Self {
            kind: "sftp".into(),
            state: "SFTP".into(),
            state_label: crate::i18n::t("tui.state.sftp"),
            restartable: false,
            ..Self::browser(index, key, name, group)
        }
    }

    /// The editor. No process and no page either -- the board draws it, and
    /// what it is showing is picked while the program runs
    pub fn editor(index: usize, key: &str, name: &str, group: Option<usize>) -> Self {
        Self {
            kind: "editor".into(),
            state: "EDIT".into(),
            state_label: crate::i18n::t("tui.state.editor"),
            restartable: false,
            ..Self::browser(index, key, name, group)
        }
    }

    /// A split: several rows shown at once, divided. It runs nothing, so it
    /// has nothing to restart and nothing to stop -- what it holds are other
    /// rows, and each of those keeps its own ✕
    pub fn split(index: usize, key: &str, name: &str, group: Option<usize>) -> Self {
        Self {
            kind: "split".into(),
            state: "SPLIT".into(),
            state_label: crate::i18n::t("tui.state.split"),
            restartable: false,
            ..Self::browser(index, key, name, group)
        }
    }

    /// The Issue tab: the desk's issues and pull requests, drawn by the board
    pub fn issues(index: usize, key: &str) -> Self {
        Self {
            kind: "issues".into(),
            state: "ISSUES".into(),
            state_label: crate::i18n::t("tui.state.issues"),
            restartable: false,
            ..Self::browser(index, key, &crate::i18n::t("tui.issues.tab"), None)
        }
    }

    /// A tab that could not be started. No process and nothing to show but
    /// why, so the rest of what a tab carries is absent
    pub fn failed(index: usize, key: &str, name: &str, group: Option<usize>) -> Self {
        Self {
            kind: "failed".into(),
            state: "FAILED".into(),
            state_label: crate::i18n::t("tui.state.failed"),
            // Starting it again is exactly what the person will want once the
            // program is installed
            restartable: true,
            ..Self::browser(index, key, name, group)
        }
    }

    pub fn git(index: usize, key: &str, name: &str, group: Option<usize>) -> Self {
        Self {
            kind: "git".into(),
            state: "GIT".into(),
            state_label: crate::i18n::t("tui.state.git"),
            // Opening and closing it is the tab list's business, not a restart
            restartable: false,
            ..Self::browser(index, key, name, group)
        }
    }

    pub fn browser(index: usize, key: &str, name: &str, group: Option<usize>) -> Self {
        Self {
            index,
            name: name.to_string(),
            id: Some(key.to_string()),
            state: "WEB".into(),
            state_label: crate::i18n::t("tui.state.web"),
            since: None,
            profile: String::new(),
            limit: None,
            locked: false,
            depth: 0,
            activity: Vec::new(),
            group,
            kind: "browser".into(),
            words_unset: false,
            words_fast: false,
            // A page has no conversation to have been having
            past: false,
            lost: false,
            // The same two keys `main::restartable_page` refuses, and for the
            // same reason: they are opened and closed by the app, so "open it
            // again" is not a thing a person can want from them
            restartable: key != "settings" && key != "result" && key != "guide",
            status: None,
            progress: None,
            // Nothing is typed into a page by this road
            sending: None,
            // A browser is not in a folder and starts nothing, so it has
            // nowhere to be, and nothing of its own to cost
            place: None,
            cost: None,
            model: false,
            busy: false,
            // Kept off the list on the left: the app opens and closes these
            // itself, and a row for something nobody can switch to is a row
            // that does nothing when it is pressed
            settings: key == "settings" || key == "guide",
            ai: None,
            auto: false,
            // Nothing was said here to read back
            readable: false,
            ask: None,
            file: None,
            file_stamp: None,
            file_diff: None,
            // Where it is drawn is known to the runtime, not to this; filled
            // in by `view::ui_state_of` along with everything else
            away: None,
            failed: None,
            hold: None,
            draft: None,
            key: String::new(),
            // Filled in by `view::ui_state_of` too: which server a tab is on
            // is the tab's, but what the person named it is the settings'
            mark: None,
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
        let a = TabState::browser(3, "shop", "通販サイト", None);
        let b = TabState::browser(4, "mail", "メール", None);
        assert_eq!((a.index, b.index), (3, 4));
        assert_eq!(a.kind, "browser", "it is shown the same way as a session");
        assert_eq!(a.id.as_deref(), Some("shop"), "the name automation uses is wrong");
        // The human-readable name and the name automation refers to are different things
        assert_eq!(a.name, "通販サイト", "the display name that was set is not shown");
        // It isn't a session, so don't pad it out to look like one
        assert!(a.activity.is_empty() && a.profile.is_empty() && a.depth == 0);
        assert!(!a.locked);
    }


    /// A tab that is really running, in a folder of its own.
    fn in_folder(folder: &str, named: Option<&str>) -> crate::tab::Tab {
        let dir = std::env::temp_dir().join(format!("shikisha-group-{folder}"));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = crate::tab::TabOptions {
            cwd: Some(dir),
            group: named.map(str::to_string),
            ..Default::default()
        };
        crate::tab::Tab::spawn("t".into(), &[crate::test_shell()], None, 6, 40, opts).unwrap()
    }

    #[test]
    fn the_folders_are_the_ones_the_tabs_are_actually_in() {
        // Two tabs in one folder are one heading, not two: what groups them is
        // the folder, so nothing has to be declared for them to be together
        let mut tabs = vec![in_folder("one", None), in_folder("one", None), in_folder("two", None)];
        let found = GroupState::all(&tabs, &Default::default(), &[]);
        for t in tabs.iter_mut() {
            t.kill();
        }
        assert_eq!(found.len(), 2, "as many as there are folders");
        assert!(found[0].0.ends_with("shikisha-group-one"));
        assert_eq!(found[0].1.name, "shikisha-group-one", "with no name, the folder's own name");
        // Not in a repository, so it belongs to no family and has no colour
        assert_eq!(found[0].1.color, None);
        assert!(!found[0].1.linked);
    }

    /// Where a tab is says which repository its folder pushes to.
    ///
    /// The screen asks that before it asks GitHub anything: with it missing,
    /// the git column could not tell a folder whose server has pull requests
    /// and CI from one whose server has neither, so it asked for neither and
    /// showed neither -- while the pull requests and the CI were there
    #[test]
    fn where_a_tab_is_says_which_repository_it_pushes_to() {
        let place = crate::repo::Place {
            branch: Some("feature".into()),
            repo: Some("owner/name".into()),
            ..Default::default()
        };
        let sent = PlaceState { branch: place.branch.clone(), pr: None, ports: vec![], repo: place.repo.clone() };
        let js = serde_json::to_value(&sent).expect("it cannot be sent to the screen");
        assert_eq!(js["repo"], "owner/name", "the screen is not told the repository: {js}");
        // And nothing is added for a folder that pushes nowhere
        let none = serde_json::to_value(PlaceState::default()).unwrap();
        assert!(none.get("repo").is_none(), "an answer nobody has is sent as one: {none}");
    }

    /// A project's own checkout is headed by its folder and a worktree by its
    /// branch, and both say which project they are in by the project's name.
    ///
    /// Headed by its branch, every checkout standing on main read "main" --
    /// two projects side by side were two identical rows -- and the pill on a
    /// worktree, read off that heading, named the branch instead of the project
    #[test]
    fn a_checkout_is_headed_by_its_folder_and_a_worktree_by_its_branch() {
        let family = std::env::temp_dir().join("shikisha-group-orion").join(".git");
        let place = |branch: &str, linked: bool| crate::repo::Place {
            branch: Some(branch.to_string()),
            family: Some(family.clone()),
            linked,
            ..Default::default()
        };
        let mut tabs = vec![in_folder("orion", None), in_folder("feature-x", None), in_folder("plain", None)];
        tabs[0].place = place("main", false);
        tabs[1].place = place("feature-x-branch", true);
        let found = GroupState::all(&tabs, &Default::default(), &[]);
        for t in tabs.iter_mut() {
            t.kill();
        }
        let by = |end: &str| &found.iter().find(|(k, _)| k.ends_with(end)).unwrap().1;
        let (head, cut, plain) = (by("shikisha-group-orion"), by("shikisha-group-feature-x"), by("shikisha-group-plain"));
        assert_eq!(head.name, "shikisha-group-orion", "the original folder was called by the branch name");
        assert_eq!(head.branch.as_deref(), Some("main"));
        assert_eq!(cut.name, "feature-x-branch", "a worktree is called by its branch name");
        assert_eq!(head.project.as_deref(), Some("shikisha-group-orion"));
        assert_eq!(cut.project.as_deref(), Some("shikisha-group-orion"), "the worktree's pill is not the project's name");
        assert_eq!(plain.project, None, "a project name was given outside a repository");

        // A name the settings give one folder of the household names all of it
        let mut named = found.clone();
        let at = named.iter().find(|(k, _)| k.ends_with("shikisha-group-orion")).unwrap().0.clone();
        GroupState::name_projects(&mut named, &[(at, "Orion API".into())]);
        let projects: Vec<Option<&str>> = named.iter().map(|(_, g)| g.project.as_deref()).collect();
        assert_eq!(projects.iter().filter(|p| **p == Some("Orion API")).count(), 2, "{projects:?}");
    }

    #[test]
    fn a_project_is_named_after_the_folder_its_repository_is_checked_out_in() {
        assert_eq!(project_by_family(&crate::local_path(r"D:\work\orion\.git")).as_deref(), Some("orion"));
        assert_eq!(project_by_family(&crate::local_path(r"D:\work\orion\.git\")).as_deref(), Some("orion"));
        assert_eq!(project_by_family("/srv/repos/orion.git").as_deref(), Some("orion"));
        assert_eq!(project_by_family(""), None);
    }

    #[test]
    fn a_folder_someone_named_is_called_that() {
        let mut tabs = vec![in_folder("named", Some("feature/login"))];
        let found = GroupState::all(&tabs, &Default::default(), &[]);
        tabs[0].kill();
        assert_eq!(found[0].1.name, "feature/login");
    }

    /// A folder the settings name and no tab is in still gets a heading --
    /// marked empty, after the ones with tabs -- and a folder a tab IS in is
    /// not listed twice because the settings spell it differently.
    #[test]
    fn a_folder_with_nothing_in_it_is_still_on_the_list() {
        let mut tabs = vec![in_folder("Work", None)];
        let work = tabs[0].cwd().unwrap().to_path_buf();
        let fresh = std::env::temp_dir().join("shikisha-group-fresh");
        let spelled = std::path::PathBuf::from(work.display().to_string().to_lowercase() + "\\");
        let found = GroupState::all(
            &tabs,
            &Default::default(),
            &[(spelled, String::new()), (fresh.clone(), "New one".into())],
        );
        // Without a name it is called by its folder
        let unnamed = GroupState::all(&tabs, &Default::default(), &[(fresh.clone(), String::new())]);
        for t in tabs.iter_mut() {
            t.kill();
        }
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(!found[0].1.empty, "a folder with tabs is treated as empty");
        assert_eq!(found[1].0, fresh);
        assert!(found[1].1.empty, "a folder with no tabs is not recognized as empty");
        assert_eq!(found[1].1.name, "New one");
        assert_eq!(unnamed[1].1.name, "shikisha-group-fresh");
    }

    /// Each window comes as a number for the bar and words for beside it,
    /// with the time to reset in days and hours or hours and minutes.
    #[test]
    fn the_usage_reading_is_worded_for_the_status_line() {
        use crate::limits::{Limits, Window};
        let l = Limits {
            five_hour: Some(Window { pct: 19, resets_at: Some(1_000 + 3 * 3600 + 45 * 60) }),
            seven_day: Some(Window { pct: 83, resets_at: Some(1_000 + 4 * 86_400 + 10 * 3600) }),
            as_of: None,
        };
        let u = UsageState::of("Claude", &l, 1_000);
        let five = u.five.as_ref().unwrap();
        assert_eq!(five.pct, 19);
        assert!(five.used.contains("19"), "{}", five.used);
        // Beside the bar the span stands bare; "resets in" is for the hover
        assert_eq!(five.resets.as_deref(), Some("3h 45m"));
        let week = u.week.as_ref().unwrap();
        assert_eq!(week.pct, 83);
        assert_eq!(week.resets.as_deref(), Some("4d 10h"));
        assert!(u.title.contains("19") && u.title.contains("83"), "{}", u.title);
        assert!(u.title.contains("resets in 3h 45m"), "{}", u.title);
        // A window the service withheld is absent, and a reset already past
        // never goes negative
        let l = Limits { five_hour: None, seven_day: Some(Window { pct: 2, resets_at: Some(0) }), as_of: None };
        let u = UsageState::of("Claude", &l, 5_000);
        assert!(u.five.is_none());
        assert_eq!(u.week.as_ref().unwrap().resets.as_deref(), Some("0m"));
        // No time at all: the words say how much, and nothing about when
        let l = Limits { five_hour: Some(Window { pct: 7, resets_at: None }), seven_day: None, as_of: None };
        assert_eq!(UsageState::of("Claude", &l, 0).five.unwrap().resets, None);
    }

    /// No leading zero in a span: the row is paid for by the character, and
    /// STOP sits at the end of it
    #[test]
    fn a_span_never_starts_with_a_zero() {
        assert_eq!(until(9 * 60), "9m");
        assert_eq!(until(3 * 3600 + 45 * 60), "3h 45m");
        assert_eq!(until(2 * 86_400), "2d");
        assert_eq!(until(2 * 86_400 + 23 * 3600), "2d 23h");
        assert_eq!(until(-5), "0m");
    }

    /// One household is drawn together, checkout first, wherever its members
    /// were in the settings; folders with no household stay put.
    #[test]
    fn a_household_stands_together_with_the_checkout_first() {
        let g = |folder: &str, family: Option<&str>, linked: bool| {
            (
                std::path::PathBuf::from(folder),
                GroupState {
                    name: folder.rsplit('\\').next().unwrap().to_string(),
                    folder: folder.to_string(),
                    family: family.map(str::to_string),
                    linked,
                    ..Default::default()
                },
            )
        };
        let f = Some(r"D:\proj\.git");
        let list = vec![
            g(r"D:\proj.worktrees\a", f, true),
            g(r"D:\other", None, false),
            g(r"D:\proj", f, false),
            g(r"D:\proj.worktrees\b", f, true),
            g(r"D:\lone.worktrees\x", Some(r"D:\lone\.git"), true),
        ];
        let names: Vec<String> = by_family(list).into_iter().map(|(_, g)| g.name).collect();
        assert_eq!(names, ["proj", "a", "b", "other", "x"], "not in the order original, branches, others");
        // Spelled differently, still one family
        let list = vec![
            g(r"D:\proj.worktrees\a", Some(r"d:\PROJ\.git\"), true),
            g(r"D:\proj", f, false),
        ];
        let out = by_family(list);
        let names: Vec<&str> = out.iter().map(|(_, g)| g.name.as_str()).collect();
        assert_eq!(names, ["proj", "a"], "a difference only in spelling put it in a different family");
        assert_eq!(out[0].1.family, out[1].1.family, "the family's spelling is not consistent");
    }

    /// A folder nothing runs in is placed by its path: a branch folder under
    /// `<name>.worktrees` belongs to `<name>`, and a folder that IS `<name>`
    /// heads the household its branches already named.
    #[test]
    fn an_empty_folder_finds_its_household_without_touching_the_disk() {
        let at = |p: &str| std::path::PathBuf::from(crate::local_path(p));
        let git = crate::local_path(r"D:\proj\.git");
        assert_eq!(
            family_by_path(&at(r"D:\proj.worktrees\feature\login")),
            Some(std::path::PathBuf::from(&git))
        );
        assert_eq!(family_by_path(&at(r"D:\plain\folder")), None);

        let mut list = vec![
            (
                at(r"D:\proj.worktrees\a"),
                GroupState { family: Some(git.clone()), linked: true, ..Default::default() },
            ),
            (at(r"D:\proj"), GroupState { empty: true, ..Default::default() }),
            (at(r"D:\elsewhere"), GroupState { empty: true, ..Default::default() }),
        ];
        adopt_checkouts(&mut list);
        assert_eq!(list[1].1.family.as_deref(), Some(git.as_str()), "an empty original folder is not included in the family");
        assert!(!list[1].1.linked, "the original is treated as a branch");
        assert_eq!(list[2].1.family, None, "an unrelated folder was put into the family");
    }

    #[test]
    fn folders_can_be_walked_into_and_back_out_of() {
        let root = std::env::temp_dir().join(format!("shikisha-browse-{}", crate::random_hex(6)));
        std::fs::create_dir_all(root.join("work").join("inner")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::write(root.join("notes.txt"), "x").unwrap();

        let at = BrowseState::of(&root.display().to_string());
        // Folders only: a file is not somewhere to work, and the places tools
        // keep their own things are not either
        assert_eq!(at.dirs.len(), 1, "{:?}", at.dirs);
        assert!(at.dirs[0].ends_with("work"));
        assert!(at.files.is_empty(), "files got mixed into the list for choosing where to work: {:?}", at.files);
        assert_eq!(at.up.as_deref(), Some(root.parent().unwrap().display().to_string().as_str()));
        // Asked for the files as well -- choosing a key or a secrets file --
        // the same walk lists them, still leaving out what tools keep for
        // themselves
        let with = BrowseState::with_files(&root.display().to_string());
        assert_eq!(with.dirs, at.dirs, "adding a file does not change the folder list");
        assert_eq!(with.files.len(), 1, "{:?}", with.files);
        assert!(with.files[0].ends_with("notes.txt"));

        // The top is the drives, and every drive can get back to it
        let top = BrowseState::of("");
        assert!(top.up.is_none(), "the top has nowhere to go back to");
        assert!(!top.dirs.is_empty(), "the drives are shown");
        let drive = BrowseState::of(&top.dirs.last().cloned().unwrap());
        assert!(drive.up.is_some(), "a drive can go back to the list");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn one_project_gets_one_colour_and_two_get_two() {
        // What the list draws branches of one repository with. The answer has
        // to be the same every time it is asked, or the colours would move
        // around as tabs open and close
        let none = Default::default();
        let a = std::path::Path::new("D:/work/myproject/.git");
        let b = std::path::Path::new("D:/work/other/.git");
        assert_eq!(GroupState::color_of(a, &none), GroupState::color_of(a, &none));
        assert!(PALETTE.contains(&GroupState::color_of(a, &none).as_str()));
        assert_ne!(GroupState::color_of(a, &none), GroupState::color_of(b, &none));

        // Someone said which colour they wanted, and that is the answer for
        // every branch of that project -- the key is the folder they share
        let mut chosen = std::collections::HashMap::new();
        chosen.insert(a.display().to_string(), "#123456".to_string());
        assert_eq!(GroupState::color_of(a, &chosen), "#123456");
        assert_ne!(GroupState::color_of(b, &chosen), "#123456", "it does not change another place's color");
    }

    /// A server wears a mark only once somebody named it. The colour is what
    /// was chosen when it is plainly a colour, and one worked out from the name
    /// otherwise -- and the settings are read however the address was
    /// capitalised by a hand editing the file
    #[test]
    fn a_server_is_marked_only_with_a_name() {
        use crate::config::ServerMark;
        let marks = |name: &str, color: Option<&str>| {
            std::collections::HashMap::from([(
                "Prod.Example.com:22".to_string(),
                ServerMark { name: name.into(), color: color.map(str::to_string), careful: true },
            )])
        };
        let found = MarkState::of("prod.example.com:22", &marks(" Production ", Some("#E5644D")))
            .expect("a named server is not marked");
        assert_eq!(found.name, "Production");
        assert_eq!(found.color, "#E5644D");
        assert!(found.careful);
        assert_eq!(found.machine, "prod.example.com:22");

        assert_eq!(MarkState::of("prod.example.com:22", &marks("  ", Some("#e5644d"))), None,
            "a colour with no name was worn as a mark");
        assert_eq!(MarkState::of("staging.example.com:22", &marks("Production", None)), None);

        // Not plainly a colour, so not handed to the page's styles
        let odd = MarkState::of("prod.example.com:22", &marks("Production", Some("red;background:url(x)"))).unwrap();
        assert!(PALETTE.contains(&odd.color.as_str()), "{}", odd.color);
        assert_eq!(odd.color, MarkState::of("prod.example.com:22", &marks("production", None)).unwrap().color,
            "the same name came out two colours");
    }

    fn tab(index: usize, name: &str) -> TabState {
        TabState {
            index,
            name: name.into(),
            id: None,
            state: "WAIT".into(),
            state_label: "WAIT".into(),
            since: None,
            profile: "GENERIC".into(),
            past: false,
            lost: false,
            locked: false,
            depth: 0,
            activity: vec![0; 4],
            group: None,
            kind: "pty".into(),
            words_unset: false,
            words_fast: false,
            restartable: true,
            model: false,
            busy: false,
            settings: false,
            ai: None,
            limit: None,
            auto: false,
            status: None,
            progress: None,
            sending: None,
            place: None,
            cost: None,
            readable: false,
            ask: None,
            away: None,
            failed: None,
            hold: None,
            draft: None,
            file: None,
            file_stamp: None,
            file_diff: None,
            key: format!("tab:{index}"),
            mark: None,
        }
    }

    /// Confirms appearance never leaks into state.
    ///
    /// Putting a color or symbol here would lock the window, TUI, and phone
    /// all into the same look. Keeping them separate lets each pick its own
    #[test]
    fn the_state_carries_no_appearance() {
        let s = UiState {
            desk: "検証".into(),
            tabs: vec![tab(1, "実装")],
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        for looky in ["color", "#", "rgb", "▁", "●", "width", "px"] {
            assert!(
                !json.contains(looky),
                "the look is mixed in ({looky}): {json}"
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
        assert_ne!(a, b, "the state changed but was judged the same");
    }
}

