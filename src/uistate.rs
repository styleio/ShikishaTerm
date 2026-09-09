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
}

/// What a script is asking the person about a page, for the bar the board
/// draws under it: the words on the left, the button on the right
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct AskState {
    pub text: String,
    pub label: String,
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

/// The colours a project is given when nobody has chosen one. Eight, because
/// past that they stop being telling apart and start being decoration
pub const PALETTE: [&str; 8] = [
    "#d97757", "#19c37d", "#4285f4", "#a06bff",
    "#e0a80a", "#12b3a8", "#e5644d", "#7f8cff",
];

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
    /// The heading: what someone named it, else the branch, else the folder
    pub name: String,
    /// The whole path, for the tooltip
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
}

impl GroupState {
    /// The folders these tabs are in, in the order they first appear, each
    /// paired with the folder itself so a tab can find its own.
    ///
    /// Tabs that are in no folder at all -- a browser is in none -- get no
    /// heading and belong to nothing, which is why the answer is looked up by
    /// path rather than handed out by position
    ///
    /// `configured` is what the settings say the workspace's folders are, by
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
            // What to call it: what someone typed, else the branch it is on,
            // else the folder's own name. A branch first because with several
            // of them open, the branch is the thing that tells them apart
            let name = t
                .group_name()
                .map(str::to_string)
                .filter(|n| !n.trim().is_empty())
                .or_else(|| t.place.branch.clone())
                .or_else(|| cwd.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_default();
            out.push((
                cwd.to_path_buf(),
                GroupState {
                    name,
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
                },
            ));
        }
        adopt_checkouts(&mut out);
        by_family(out)
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
        if let Some(c) = chosen.get(&key).or_else(|| chosen.get(&key.to_lowercase())) {
            if !c.trim().is_empty() {
                return c.trim().to_string();
            }
        }
        let mut h: u32 = 2166136261;
        for b in key.to_lowercase().bytes() {
            h = (h ^ b as u32).wrapping_mul(16777619);
        }
        PALETTE[(h % PALETTE.len() as u32) as usize].to_string()
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
}

/// What Claude's subscription has left, as the status line draws it.
///
/// Numbers and words apart, so the page can draw a bar for the number and
/// put the words beside it: "20% used · resets in 3h 45m" reads; "5h 20%"
/// does not. Each window is absent when the service withheld it
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct UsageState {
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
    /// "5時間枠 24% 使用 · 0時間9分後に回復" ran the reading to twice the
    /// width of "5h 24% used 9m", pushing the button toward the edge. A
    /// person's decision to translate these keys anyway is honoured, but it
    /// is a decision to make the row wider
    pub fn of(l: &crate::limits::Limits, now: i64) -> Self {
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
            title: crate::i18n::tp("tui.usage.title", &[("five", &say(&five)), ("week", &say(&week))]),
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

/// Whether two paths name one folder, the way Windows sees it: case does
/// not tell them apart, and neither does a trailing separator
/// Where a branch folder's family would be, read off nothing but its path.
///
/// The shape this app gives a branch's folder is
/// `<parent>/<name>.worktrees/<branch as folders>`, and the project's own git
/// folder is then `<parent>/<name>/.git`. That is a guess about a folder
/// nothing is running in yet, made without touching the disk; the moment a
/// tab starts there, what git actually says takes over
fn family_by_path(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
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
fn by_family(list: Vec<(std::path::PathBuf, GroupState)>) -> Vec<(std::path::PathBuf, GroupState)> {
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

fn same_folder(a: &std::path::Path, b: &std::path::Path) -> bool {
    let key = |p: &std::path::Path| {
        p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
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
}

impl BrowseState {
    /// Where the walk starts: the person's own folder, then the drives.
    ///
    /// Their own first, because that is where work is, and a list that opens
    /// on `A:` makes everyone scroll past floppy disks to reach it
    fn top() -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(home) = std::env::var("USERPROFILE") {
            if !home.is_empty() {
                out.push(home);
            }
        }
        for letter in 'A'..='Z' {
            let root = format!("{letter}:\\");
            if std::path::Path::new(&root).is_dir() {
                out.push(root);
            }
        }
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
            return Self { at, up: None, dirs: Self::top(), files: Vec::new(), error: None };
        }
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
                    if hidden {
                        continue;
                    }
                    if p.is_dir() {
                        if dirs.len() < 400 {
                            dirs.push(p.display().to_string());
                        }
                    } else if want_files && files.len() < 400 {
                        files.push(p.display().to_string());
                    }
                    if dirs.len() >= 400 && (!want_files || files.len() >= 400) {
                        break;
                    }
                }
                dirs.sort_by_key(|d| d.to_lowercase());
                files.sort_by_key(|f| f.to_lowercase());
            }
            Err(e) => error = Some(e.to_string()),
        }
        Self { at, up, dirs, files, error }
    }
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
    /// The folders this workspace's tabs are working in. One means nothing is
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
    /// What the focused tab is aimed at, as a screen number, when it has been
    /// aimed at anything. The picker on screen is the only place an aim is
    /// chosen and it is written down against that tab, so this is how it comes
    /// back after a restart -- not a second setting to keep in step with.
    #[serde(default)]
    pub aim: Option<usize>,
    /// First launch, before any settings exist yet
    pub first_run: bool,
    /// The AIs this machine can start in a folder just made
    #[serde(default)]
    pub ais: Vec<AiChoice>,
    /// What Claude's subscription has left, when it is known. The page shows
    /// it only while a Claude tab is in view
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageState>,
    /// The first-run pointer that is up: 1 = add a folder, 2 = press its +
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coach: Option<u8>,
    /// The thanks card, when it is up: `github` or `store`, which is where
    /// its button leads
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thanks: Option<String>,
    /// The version the update card asks about, when it is up. Answered
    /// once, either way, and not shown again for that version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
    /// Whether the settings name a phone as somewhere answers go. Only the
    /// browser holding the page can know whether it is that phone yet, so
    /// the app says just that one is wanted, and the phone's board offers
    /// to become it (src/shell.rs, drawPushBar).
    #[serde(default)]
    pub push_wanted: bool,
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
            profile: t.profile_name().to_string(),
            locked: t.locked,
            depth: t.chain_depth,
            activity: t.activity().to_vec(),
            group: None,
            kind: "pty".into(),
            restartable: true,
            model: t.is_model(),
            busy: t.is_generating(),
            settings: false,
            ai: t.ai_kind(),
            limit: t.limit_note().map(str::to_string),
            auto: t.auto_runs(),
            status: t.status_line(),
            progress: t.progress.as_ref().map(|(p, _)| *p),
            place: (t.place != crate::repo::Place::default()).then(|| PlaceState {
                branch: t.place.branch.clone(),
                pr: t.place.pr.clone(),
                ports: t.place.ports.clone(),
            }),
            cost: t.usage.line(),
            readable: readable(t),
            // A session is not a page; nothing asks the person about it here
            ask: None,
        }
    }

    /// Build from a browser placed inside the window.
    ///
    /// It isn't a session, so it has no state and no output volume.
    /// Rather than pad it out to look similar, it's more readable left as-is
    /// The git panel. It has no process and no page of its own -- the board
    /// draws it -- so most of what a tab carries is simply absent
    pub fn git(index: usize, key: &str, name: &str, group: Option<usize>) -> Self {
        Self {
            kind: "git".into(),
            state: "GIT".into(),
            state_label: crate::i18n::t("tui.state.git"),
            group,
            // Opening and closing it is the tab list's business, not a restart
            restartable: false,
            ..Self::browser(index, key, name)
        }
    }

    pub fn browser(index: usize, key: &str, name: &str) -> Self {
        Self {
            index,
            name: name.to_string(),
            id: Some(key.to_string()),
            state: "WEB".into(),
            state_label: crate::i18n::t("tui.state.web"),
            profile: String::new(),
            limit: None,
            locked: false,
            depth: 0,
            activity: Vec::new(),
            group: None,
            kind: "browser".into(),
            // The same two keys `main::restartable_page` refuses, and for the
            // same reason: they are opened and closed by the app, so "open it
            // again" is not a thing a person can want from them
            restartable: key != "settings" && key != "result",
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
            // Nothing was said here to read back
            readable: false,
            ask: None,
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


    /// A tab that is really running, in a folder of its own.
    fn in_folder(folder: &str, named: Option<&str>) -> crate::tab::Tab {
        let dir = std::env::temp_dir().join(format!("shikisha-group-{folder}"));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = crate::tab::TabOptions {
            cwd: Some(dir),
            group: named.map(str::to_string),
            ..Default::default()
        };
        crate::tab::Tab::spawn("t".into(), &["cmd.exe".to_string()], None, 6, 40, opts).unwrap()
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
        assert_eq!(found.len(), 2, "フォルダの数だけ");
        assert!(found[0].0.ends_with("shikisha-group-one"));
        assert_eq!(found[0].1.name, "shikisha-group-one", "名前が無ければフォルダ自身の名前");
        // Not in a repository, so it belongs to no family and has no colour
        assert_eq!(found[0].1.color, None);
        assert!(!found[0].1.linked);
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
        assert!(!found[0].1.empty, "タブのあるフォルダが空扱い");
        assert_eq!(found[1].0, fresh);
        assert!(found[1].1.empty, "タブの無いフォルダが空と分からない");
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
        };
        let u = UsageState::of(&l, 1_000);
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
        let l = Limits { five_hour: None, seven_day: Some(Window { pct: 2, resets_at: Some(0) }) };
        let u = UsageState::of(&l, 5_000);
        assert!(u.five.is_none());
        assert_eq!(u.week.as_ref().unwrap().resets.as_deref(), Some("0m"));
        // No time at all: the words say how much, and nothing about when
        let l = Limits { five_hour: Some(Window { pct: 7, resets_at: None }), seven_day: None };
        assert_eq!(UsageState::of(&l, 0).five.unwrap().resets, None);
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
        assert_eq!(names, ["proj", "a", "b", "other", "x"], "元→枝→その他、の順になっていない");
        // Spelled differently, still one family
        let list = vec![
            g(r"D:\proj.worktrees\a", Some(r"d:\PROJ\.git\"), true),
            g(r"D:\proj", f, false),
        ];
        let out = by_family(list);
        let names: Vec<&str> = out.iter().map(|(_, g)| g.name.as_str()).collect();
        assert_eq!(names, ["proj", "a"], "綴りが違うだけで別の家族にされた");
        assert_eq!(out[0].1.family, out[1].1.family, "家族の綴りが揃っていない");
    }

    /// A folder nothing runs in is placed by its path: a branch folder under
    /// `<name>.worktrees` belongs to `<name>`, and a folder that IS `<name>`
    /// heads the household its branches already named.
    #[test]
    fn an_empty_folder_finds_its_household_without_touching_the_disk() {
        assert_eq!(
            family_by_path(std::path::Path::new(r"D:\proj.worktrees\feature\login")),
            Some(std::path::PathBuf::from(r"D:\proj\.git"))
        );
        assert_eq!(family_by_path(std::path::Path::new(r"D:\plain\folder")), None);

        let mut list = vec![
            (
                std::path::PathBuf::from(r"D:\proj.worktrees\a"),
                GroupState { family: Some(r"D:\proj\.git".into()), linked: true, ..Default::default() },
            ),
            (
                std::path::PathBuf::from(r"D:\proj"),
                GroupState { empty: true, ..Default::default() },
            ),
            (
                std::path::PathBuf::from(r"D:\elsewhere"),
                GroupState { empty: true, ..Default::default() },
            ),
        ];
        adopt_checkouts(&mut list);
        assert_eq!(list[1].1.family.as_deref(), Some(r"D:\proj\.git"), "空の元フォルダが家族に入らない");
        assert!(!list[1].1.linked, "元が枝扱い");
        assert_eq!(list[2].1.family, None, "無関係のフォルダが家族にされた");
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
        assert!(at.files.is_empty(), "作業場所を選ぶ一覧にファイルが混ざった: {:?}", at.files);
        assert_eq!(at.up.as_deref(), Some(root.parent().unwrap().display().to_string().as_str()));
        // Asked for the files as well -- choosing a key or a secrets file --
        // the same walk lists them, still leaving out what tools keep for
        // themselves
        let with = BrowseState::with_files(&root.display().to_string());
        assert_eq!(with.dirs, at.dirs, "ファイルを足しても、フォルダの一覧は変わらない");
        assert_eq!(with.files.len(), 1, "{:?}", with.files);
        assert!(with.files[0].ends_with("notes.txt"));

        // The top is the drives, and every drive can get back to it
        let top = BrowseState::of("");
        assert!(top.up.is_none(), "一番上には戻る先が無い");
        assert!(!top.dirs.is_empty(), "ドライブが出ている");
        let drive = BrowseState::of(&top.dirs.last().cloned().unwrap());
        assert!(drive.up.is_some(), "ドライブから一覧へ戻れる");

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
        assert_ne!(GroupState::color_of(b, &chosen), "#123456", "他所の色まで変えない");
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
            group: None,
            kind: "pty".into(),
            restartable: true,
            model: false,
            busy: false,
            settings: false,
            ai: None,
            limit: None,
            auto: false,
            status: None,
            progress: None,
            place: None,
            cost: None,
            readable: false,
            ask: None,
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
