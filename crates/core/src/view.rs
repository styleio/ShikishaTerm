//! Assembling the picture a shell draws.
//!
//! `UiState` is what every shell is handed -- the window on this machine and a
//! phone see the same thing -- and this is the one place it is built. `Ui` is
//! what the runtime knows that the tabs themselves do not: which one is in
//! front, whether a screen is covering them, what the desk is called.

use crate::tab::Tab;
use crate::{ball, config, folders, i18n, ssh, uistate};
use std::time::Duration;

/// The pane tree in the form the page draws it: one rectangle per pane, in
/// fractions of the content area.
///
/// Only geometry and identity travel here. What a pane *shows* — the name, the
/// state dot, whether it is a browser — the page already has from `__state`,
/// looked up by surface number. Sending it twice would let the two copies
/// disagree, and the pane would caption itself with a stale name.
pub fn panes_json(l: &crate::layout::Layout) -> String {
    #[derive(serde::Serialize)]
    struct Pane {
        id: crate::layout::PaneId,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        surface: usize,
        focused: bool,
    }
    /// One divider, in the same fractions. `i` is its position in
    /// `Layout::dividers()`, which is how a drag names it coming back
    #[derive(serde::Serialize)]
    struct Divider {
        i: usize,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        /// The first half's share, so the page can draw the handle on the line
        ratio: f32,
        /// true = the halves are stacked, so the divider lies across
        down: bool,
    }
    let focus = l.focus();
    let surfaces: std::collections::HashMap<_, _> = l.leaves().into_iter().collect();
    let panes: Vec<Pane> = l
        .rects()
        .into_iter()
        .map(|(id, r)| Pane {
            id,
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            surface: surfaces.get(&id).copied().unwrap_or(0),
            focused: id == focus,
        })
        .collect();
    let dividers: Vec<Divider> = l
        .dividers()
        .into_iter()
        .enumerate()
        .map(|(i, (r, dir, ratio))| Divider {
            i,
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            ratio,
            down: dir == crate::layout::Dir::Col,
        })
        .collect();
    serde_json::json!({
        "single": l.is_single(),
        "focus": focus,
        "panes": panes,
        "dividers": dividers,
    })
    .to_string()
}

/// What the page has to be told about a screen that has just been rendered.
#[derive(Debug, PartialEq, Eq)]
pub enum ScreenPush {
    /// It already shows exactly this
    Nothing,
    /// These rows, and nothing else, moved
    Rows(Vec<usize>),
    /// Hand it the whole grid again
    Whole,
}

/// Decides which of the three a frame calls for.
///
/// The whole grid goes out when the screen changed shape (the page's rows no
/// longer line up with ours, so a row number would land somewhere else), and
/// when enough of it moved that dozens of separate repairs cost more than one
/// parse of the lot. Everything in between -- which is nearly every frame an
/// AI at work produces -- is a handful of rows.
pub fn screen_push(had: &[String], now: &[String]) -> ScreenPush {
    if had.len() != now.len() {
        return ScreenPush::Whole;
    }
    let moved: Vec<usize> = (0..now.len()).filter(|&i| now[i] != had[i]).collect();
    match moved.len() {
        0 => ScreenPush::Nothing,
        n if n * 3 > now.len() => ScreenPush::Whole,
        _ => ScreenPush::Rows(moved),
    }
}

/// How long to wait before pushing the screen to a viewer again.
///
/// The window repairs rows sixty times a second. A phone used to get one whole
/// screen every 140ms, which is fine while an AI types into the last line and
/// ruinous while somebody scrolls -- every row moves, so seven whole frames a
/// second is what the reader sees, and it reads as the page stuttering rather
/// than as a terminal moving.
///
/// So the pace follows the viewer's line instead of a fixed number. `pending`
/// is what that line has been handed and not yet written: nothing waiting means
/// it is keeping up and can have frames as fast as the loop makes them, and a
/// backlog means the socket is the narrow part, where sending more only queues
/// pictures that will arrive too late to be worth drawing.
pub fn remote_floor(pending: usize) -> Duration {
    match pending {
        0 => Duration::from_millis(33),
        1..=2 => Duration::from_millis(70),
        _ => Duration::from_millis(200),
    }
}

#[cfg(test)]
mod remote_floor_tests {
    use super::{ScreenPush, remote_floor, screen_push};

    /// Scrolling moves every row at once, so the row diff cannot help there --
    /// the whole grid still goes out, exactly as before. What changed for
    /// scrolling is the pace, and this is the honest size of that change
    #[test]
    fn scrolling_still_sends_the_whole_grid_but_four_times_as_often() {
        let had: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        let now: Vec<String> = (1..41).map(|i| format!("line {i}")).collect();
        assert_eq!(screen_push(&had, &now), ScreenPush::Whole);
        let (was, is) = (140.0, remote_floor(0).as_millis() as f64);
        assert!(was / is >= 4.0, "not at least four times the old 140ms: {is}ms");
    }

    /// An AI at work is the other shape: a spinner and a line of output, which
    /// is a handful of rows out of fifty. That is where the diff earns its keep
    #[test]
    fn an_ai_typing_sends_only_the_rows_that_moved() {
        let had: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        let mut now = had.clone();
        now[38] = "spinner".into();
        now[39] = "one more line".into();
        assert_eq!(screen_push(&had, &now), ScreenPush::Rows(vec![38, 39]));
    }

    #[test]
    fn a_line_that_is_keeping_up_gets_frames_at_thirty_a_second() {
        assert_eq!(remote_floor(0).as_millis(), 33);
    }

    /// The page reads `list[i][0]` as the row number and `list[i][1]` as its
    /// html (`window.__rows`), so the pair has to stay a pair on the wire
    #[test]
    fn a_row_repair_goes_out_as_number_and_html() {
        let list: Vec<(usize, &str)> = vec![(0, "a"), (3, "<span>b</span>")];
        assert_eq!(
            serde_json::to_string(&list).unwrap(),
            r#"[[0,"a"],[3,"<span>b</span>"]]"#
        );
    }

    #[test]
    fn a_backlog_slows_the_pace_instead_of_deepening_it() {
        let (none, some, lots) = (remote_floor(0), remote_floor(2), remote_floor(8));
        assert!(none < some, "a waiting frame should slow the pace");
        assert!(some < lots, "a deeper backlog should slow it further");
        // Never faster than the old fixed pace once the line is truly behind:
        // that number was chosen so a burst can't saturate a slow link
        assert!(lots.as_millis() >= 140);
    }
}

#[cfg(test)]
mod screen_push_tests {
    use super::{ScreenPush, screen_push};

    fn rows(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("row {i}")).collect()
    }

    /// An AI at work redraws its spinner and nothing else. That must reach the
    /// page as that one row: rewriting the grid makes the browser build every
    /// element on screen again, and the next keystroke in the composer waits
    /// behind that layout.
    #[test]
    fn a_spinner_turning_over_sends_one_row() {
        let had = rows(40);
        let mut now = had.clone();
        now[39] = "* Flambeing... (13s)".into();
        assert_eq!(screen_push(&had, &now), ScreenPush::Rows(vec![39]));
    }

    /// A screen that did not move is not worth a word.
    #[test]
    fn an_unchanged_screen_says_nothing() {
        let had = rows(40);
        assert_eq!(screen_push(&had, &had.clone()), ScreenPush::Nothing);
    }

    /// Output scrolling past moves nearly every row. Dozens of separate
    /// repairs cost more than handing over one string, so hand it over.
    #[test]
    fn a_scrolling_screen_goes_over_whole() {
        let had = rows(40);
        let now: Vec<String> = (0..40).map(|i| format!("row {}", i + 1)).collect();
        assert_eq!(screen_push(&had, &now), ScreenPush::Whole);
    }

    /// A resize renumbers everything. A row number sent now would land on a
    /// different line over there, so nothing may be said row by row.
    #[test]
    fn a_resized_screen_goes_over_whole() {
        assert_eq!(screen_push(&rows(40), &rows(50)), ScreenPush::Whole);
        // Including the first frame, when the page has nothing at all
        assert_eq!(screen_push(&[], &rows(40)), ScreenPush::Whole);
    }
}

/// The worktrees git knows each project has and the desk does not list, by
/// project. `on_desk` is every folder the list holds; `kept` the projects whose
/// found worktrees somebody chose to keep hidden, which are still said -- as
/// kept -- so the project's heading can offer them back
fn discovered_of(
    cuts: &std::collections::HashMap<std::path::PathBuf, Vec<(std::path::PathBuf, Option<String>)>>,
    on_desk: &[std::path::PathBuf],
    kept: &std::collections::BTreeSet<String>,
) -> Vec<crate::uistate::DiscoveredState> {
    let mut out: Vec<crate::uistate::DiscoveredState> = cuts
        .iter()
        .filter_map(|(family, found)| {
            let away: Vec<crate::uistate::FoundWorktree> = found
                .iter()
                .filter(|(f, _)| !on_desk.iter().any(|d| crate::uistate::same_folder(d, f)))
                .map(|(f, b)| crate::uistate::FoundWorktree { folder: f.display().to_string(), branch: b.clone() })
                .collect();
            let family = family.display().to_string();
            (!away.is_empty()).then(|| crate::uistate::DiscoveredState { kept: kept.contains(&family), family, found: away })
        })
        .collect();
    out.sort_by(|a, b| a.family.cmp(&b.family));
    out
}

pub fn ui_state_of(tabs: &[Tab], ui: &Ui, flash: Option<&str>) -> crate::uistate::UiState {
    // The folders these tabs are actually in. Worked out here, once, so the
    // window and the phone are looking at the same list
    let mut groups = crate::uistate::GroupState::all(tabs, &ui.folder_colors, &ui.folders);
    crate::uistate::GroupState::name_projects(&mut groups, &ui.folder_projects);
    crate::uistate::GroupState::name_work_items(&mut groups, &ui.folder_items);
    crate::uistate::GroupState::describe(&mut groups, &ui.folder_labels);
    // And whether each of them is on this machine. Asked here because this is
    // the one place the list is built, and answered from a table kept up to
    // date on its own threads -- a drive that has stopped answering must not
    // stop the drawing
    // Only the ones that are meant to be here. Asking this machine about a
    // folder on a server answers "missing", which is true and is not a fault:
    // it was never supposed to be here, and saying so on every one of them
    // buries the folders that really are missing
    let elsewhere: std::collections::HashSet<&std::path::PathBuf> =
        ui.folders_elsewhere.iter().collect();
    let health = folders::watch().look(
        &groups
            .iter()
            .map(|(k, _)| k.clone())
            .filter(|k| !elsewhere.contains(k))
            .collect::<Vec<_>>(),
    );
    // And how far each has drifted from the remote. Read off the same disk,
    // never over the network -- what is on it says "three behind what you last
    // fetched", which is the thing worth seeing without asking
    let drift = folders::drifts().look(
        &groups.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
    );
    // Which repository a folder with nothing running in it belongs to, as the
    // background look found it. Its household was only guessed from the path,
    // and without a colour it had no + to cut a worktree with: a project added
    // a moment ago could not be given its first worktree until a tab ran there
    let repos = folders::watch().repos(&groups.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>());
    // The worktrees each project has that this desk does not list, as the same
    // background look found them
    let cuts = folders::watch().cuts(&groups.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>());
    // A worktree being made is git's before it is the desk's; its own row says
    // it, and it is not a stranger found for that moment in between
    let listed: Vec<std::path::PathBuf> = groups
        .iter()
        .map(|(k, _)| k.clone())
        .chain(ui.making.iter().map(|m| std::path::PathBuf::from(&m.folder)))
        .collect();
    let discovered = discovered_of(&cuts, &listed, &ui.worktrees_kept);
    for (at, g) in groups.iter_mut() {
        g.host = ui.folder_hosts.iter().find(|(k, _)| k == at).map(|(_, h)| h.clone());
        g.mark = ui
            .folder_machines
            .iter()
            .find(|(k, _)| k == at)
            .and_then(|(_, m)| crate::uistate::MarkState::of(m, &ui.server_marks));
        if g.empty
            && g.color.is_none()
            && let Some((family, linked)) = repos.get(at)
        {
            g.color = Some(crate::uistate::GroupState::color_of(family, &ui.folder_colors));
            g.linked = *linked;
            g.family = Some(family.display().to_string());
        }
        g.health = health.get(at).cloned().unwrap_or_default();
        g.drift = drift.get(at).cloned().unwrap_or_default();
        // A folder with a panel in it is not empty. The list is built from
        // running tabs, and a panel is not one -- it has no process. Left
        // marked empty, the sidebar draws the heading as a folder with nothing
        // in it and never draws the rows underneath, so the panel simply had
        // no line to press
        if g.empty
            && ui.surfaces.iter().any(|s| match s {
                Surface::Git { dir: Some(d), .. }
                | Surface::Sftp { dir: Some(d), .. }
                | Surface::Editor { dir: Some(d), .. } => {
                    crate::uistate::same_folder(d, at)
                }
                _ => false,
            })
        {
            g.empty = false;
        }
    }
    // The pairing link as it should be shown, with the network it leads to
    let shown = ui.qr.as_deref().map(crate::netaddr::shown_link);
    crate::uistate::UiState {
        groups: groups.iter().map(|(_, g)| g.clone()).collect(),
        branch: ui.branch.clone(),
        repair: ui.repair.clone(),
        browse: ui.browse.clone(),
        desk: ui
            .desk_names
            .get(ui.desk_index)
            .cloned()
            .unwrap_or_default(),
        desk_id: ui.desk_ids.get(ui.desk_index).cloned().unwrap_or_default(),
        hotkeys: crate::hotkeys::working(),
        direct_keys: crate::keys::direct_now(),
        quick: ui.quick.clone(),
        quick_to: ui.quick_to.clone(),
        desks: ui.desk_names.clone(),
        desk_index: ui.desk_index,
        active: ui.active,
        board: ui.board,
        settings_open: ui.settings,
        settings_float: ui.settings && ui.settings_float,
        auto_enabled: ui.auto.unwrap_or(true),
        remote_on: ui.remote_on,
        remote_conn: ui.remote_conn,
        remote_sticky: ui.remote_sticky,
        aim: ui.aim,
        first_run: ui.first_run,
        push_wanted: ui.push_wanted,
        ais: ui.ais.clone(),
        coach: ui.coach,
        discard_unasked: ui.discard_unasked,
        setup: ui.setup.clone(),
        add_project: ui.add_project.clone(),
        discovered,
        making: ui.making.clone(),
        hosts: ui.hosts.clone(),
        ssh_aliases: ui.ssh_aliases.clone(),
        remote_list: ui.remote_list.clone(),
        project_home: ui.project_home.clone(),
        assistant: ui.assistant.clone(),
        thanks: ui.thanks.clone(),
        update: ui.update.clone(),
        usage: ui.usage.clone(),
        // Keep the order exactly as written in the config.
        // Listing sessions and browsers separately would push the browser
        // written first to the back.
        tabs: ui
            .surfaces
            .iter()
            .enumerate()
            .filter_map(|(i, p)| Some((p, match p {
                Surface::Session(s) => tabs.get(*s).map(|t| {
                    let mut ts = crate::uistate::TabState::of(i + 1, t);
                    ts.draft = t
                        .cwd()
                        .filter(|_| t.is_ai())
                        .and_then(|c| ui.drafts.iter().find(|(k, _)| crate::uistate::same_folder(k, c)))
                        .map(|(_, d)| d.clone());
                    ts.group = t.cwd().and_then(|c| {
                        groups.iter().position(|(k, _)| crate::uistate::same_folder(k, c))
                    });
                    // A terminal on a server wears that server's name. A tab
                    // in a folder on a server is on one too: its terminal is
                    // opened there, with the same address the folder has
                    ts.mark = t
                        .remote()
                        .and_then(|spec| crate::uistate::MarkState::of(&spec.machine(), &ui.server_marks));
                    // Beside a folder in a repository, the git column signs in
                    // as the project's account
                    ts.git_acct = t.place.family.is_some().then(|| {
                        crate::uistate::GitAcctState::of(
                            &ui.git_accounts,
                            &ui.pc_accounts,
                            &t.git_use,
                            t.place.repo.as_deref(),
                            "project",
                            t.git_project.clone(),
                        )
                    });
                    ts
                }),
                Surface::Browser { key, name } => {
                    let mut t = crate::uistate::TabState::browser(i + 1, key, name);
                    // What a script is asking the person about this page, if
                    // anything. The board draws the bar under the page from it
                    t.ask = ui.asks.iter().find(|(k, _)| k == key).map(|(_, a)| a.clone());
                    // And, when the page is not drawn here at all, the device
                    // it is drawn on. Filled in here with everything else about
                    // the tab, so no second pass can disagree about it
                    t.away = ui.away.iter().find(|(k, _)| k == key).map(|(_, who)| who.clone());
                    Some(t)
                }
                Surface::Sftp { key, name, dir, at, .. } => {
                    let group = dir.as_deref().and_then(|d| {
                        groups.iter().position(|(k, _)| crate::uistate::same_folder(k, d))
                    });
                    let mut t = crate::uistate::TabState::sftp(i + 1, key, name, group);
                    // The panel's server, which is the one its questions are
                    // about -- so the page reads the mark off the tab, and the
                    // tab row and the question cannot disagree about it
                    t.mark = at.as_ref().and_then(|m| crate::uistate::MarkState::of_place(m, &ui.server_marks));
                    Some(t)
                }
                Surface::Editor { key, name, dir } => {
                    let showing = ui
                        .editors
                        .iter()
                        .find(|e| &e.key == key)
                        .and_then(|e| e.showing.clone());
                    // It works in a folder, so it stands under that folder's
                    // heading and is folded away with it -- the same as the
                    // panels beside it
                    let group = dir.as_deref().and_then(|d| {
                        groups.iter().position(|(k, _)| crate::uistate::same_folder(k, d))
                    });
                    let mut t = crate::uistate::TabState::editor(i + 1, key, name, group);
                    t.file = showing;
                    t.file_stamp =
                        ui.editors.iter().find(|e| &e.key == key).and_then(|e| e.stamp.clone());
                    t.file_diff =
                        ui.editors.iter().find(|e| &e.key == key).and_then(|e| e.diff.clone());
                    Some(t)
                }
                Surface::Issues { key } => Some(crate::uistate::TabState::issues(i + 1, key)),
                Surface::Failed { key, name, dir, why, install_url, machine } => {
                    let group = dir.as_deref().and_then(|d| {
                        groups.iter().position(|(k, _)| crate::uistate::same_folder(k, d))
                    });
                    let mut t = crate::uistate::TabState::failed(i + 1, key, name, group);
                    t.failed = Some(crate::uistate::FailedState { why: why.clone(), install_url: install_url.clone() });
                    t.mark = machine.as_deref().and_then(|m| crate::uistate::MarkState::of(m, &ui.server_marks));
                    Some(t)
                }
                Surface::Git { key, name, dir, git, .. } => {
                    // The panel reports on a folder, so it stands under that
                    // folder's heading and is put away with it. Worked out from
                    // where it actually points, exactly as a tab's is -- carried
                    // as a number decided elsewhere, it was never filled in, and
                    // a panel belonging to nothing sat on outside a folded folder
                    let group = dir.as_deref().and_then(|d| {
                        groups.iter().position(|(k, _)| crate::uistate::same_folder(k, d))
                    });
                    let mut t = crate::uistate::TabState::git(i + 1, key, name, group);
                    // A git tab signs in as its own account
                    let repo = dir.as_deref().and_then(|d| {
                        ui.git_repos
                            .iter()
                            .find(|(k, _)| crate::uistate::same_folder(k, d))
                            .map(|(_, r)| r.as_str())
                    });
                    t.git_acct =
                        Some(crate::uistate::GitAcctState::of(&ui.git_accounts, &ui.pc_accounts, git, repo, "tab", None));
                    Some(t)
                }
            }?)))
            .map(|(p, mut t)| {
                t.key = surface_key(p, tabs);
                t
            })
            .collect(),
        close_ask: ui.close_ask.clone(),
        closed: ui.closed.clone(),
        // The ball moves by session number; what we display is the screen number
        ball: crate::uistate::BallState::of(&ui.ball, ui.max_chain, ui.now_ms),
        flash: flash.map(str::to_string),
        help_open: ui.help_open,
        help_rows: ui.help_rows.clone(),
        vault: ui.vault.clone(),
        self_cost: ui.self_cost.clone(),
        desk_open: ui.desk_open,
        // The link, its picture and the badge under it are decided together, in
        // this one place: a QR that says one thing while the badge beside it
        // says another is worse than either alone.
        //
        // Build the image just once here. Both the window and the phone read the
        // same state, so the same QR shows up regardless of origin (this ends the
        // link-rot we used to get back when it was served as a separate image).
        qr: shown.as_ref().map(|(url, _)| url.clone()),
        qr_svg: shown.as_ref().map(|(url, _)| crate::netaddr::qr_svg(url, 6)),
        qr_kind: shown.as_ref().map(|(_, kind)| kind.to_string()),
        nav: ui.nav.clone(),
        scrolled: ui.scrolled,
        build: format!("build {}  ({})", env!("BUILD_TIME"), env!("BUILD_REV")),
        restartable: ui.restartable,
        discuss_start: ui.discuss_start,
        discuss_start_name: ui.discuss_start_name.clone(),
        // "At rest" = a discussion desk where every participant's screen
        // has gone quiet and the automation ring has settled (Idle). We gauge
        // "quiet" from how long the screen has been unchanged rather than the
        // BUSY verdict, because some CLIs (Claude Code) leave a static status
        // footer that keeps the busy-pattern matcher latched — a screen that
        // hasn't changed in a couple of seconds is genuinely done regardless.
        // Requiring the ring to be idle too covers the brief hand-off gap
        // between turns, when the outgoing speaker has stopped but the ring is
        // still in flight — without it the banner would flicker mid-round.
        discuss_idle: ui.discuss_start.is_some() && {
            const QUIET_MS: u64 = 2000;
            let anyone_active = ui.surfaces.iter().any(|p| match p {
                Surface::Session(s) => tabs
                    .get(*s)
                    .map(|t| t.ms_since_change(ui.now_ms) < QUIET_MS)
                    .unwrap_or(false),
                // Neither a page nor a panel is doing anything on its own
                Surface::Browser { .. }
                | Surface::Git { .. }
                | Surface::Sftp { .. }
                | Surface::Editor { .. }
                | Surface::Failed { .. }
                | Surface::Issues { .. } => false,
            });
            let ring_idle = matches!(ui.ball.phase(ui.now_ms), crate::ball::Phase::Idle);
            !anyone_active && ring_idle
        },
    }
}

#[cfg(test)]
mod file_panel_tests {
    use super::*;
    use crate::elsewhere::Elsewhere;

    fn panel_of(json: &str) -> Option<Elsewhere> {
        let cfg: config::Config = serde_json::from_str(json).expect("the settings cannot be read");
        let (desks, errs) = cfg.resolve_desks();
        assert!(errs.is_empty(), "{errs:?}");
        let desk = desks.first().expect("there is no desk");
        surfaces_of(Some(desk), &[], &[], &[], false).into_iter().find_map(|s| match s {
            Surface::Sftp { at, .. } => Some(at),
            _ => None,
        })?
    }

    /// A panel on a tab that names a server reaches that server. Nothing about
    /// the folder it happens to sit in changes that: somebody wrote the
    /// address on that tab, and a tab's settings belong to the tab
    #[test]
    fn an_address_written_on_the_tab_is_the_one_used() {
        let at = panel_of(
            r#"{
              "hosts": [ {"name":"cloud","at":"","kind":"e2b"} ],
              "desks": [ { "name":"w", "id": "w",
                "folders": [ {"name":"out there","cwd":"/home/user/p","host":"cloud"} ],
                "tabs": [ {"name":"files","id":"files",
                           "command":"sftp://someone@example.com:2222","group":0} ] } ]
            }"#,
        )
        .expect("the panel has no machine");
        match at {
            Elsewhere::Ssh(spec) => {
                assert_eq!(spec.host, "example.com");
                assert_eq!(spec.port, 2222);
                assert_eq!(spec.user, "someone");
            }
            Elsewhere::Cloud(_) => panic!("the folder overrode the address written on the tab"),
        }
    }

    /// A panel with nothing written on it, in a folder that lives somewhere
    /// else, reaches that somewhere else.
    ///
    /// This is the only thing a cloud sandbox can be: it has no address for
    /// anybody to have written, so without this the panel has nothing to reach
    /// and says so on screen forever
    #[test]
    fn a_panel_with_no_address_reaches_the_folders_machine() {
        let at = panel_of(
            r#"{
              "hosts": [ {"name":"cloud","at":"","kind":"e2b"} ],
              "desks": [ { "name":"w", "id": "w",
                "folders": [ {"name":"out there","cwd":"/home/user/p","host":"cloud"} ],
                "tabs": [ {"name":"files","id":"files","command":"sftp://","group":0} ] } ]
            }"#,
        )
        .expect("the panel does not reach the folder's machine");
        match at {
            Elsewhere::Cloud(host) => assert_eq!(host.name, "cloud"),
            Elsewhere::Ssh(spec) => panic!("it went to SSH though there is no address: {}", spec.host),
        }
    }

    /// A panel with nothing written on it, in a folder that is on this
    /// machine, has no far end at all -- and says so, rather than guessing one
    #[test]
    fn a_panel_in_a_folder_here_has_nowhere_to_reach() {
        let at = panel_of(
            r#"{
              "desks": [ { "name":"w", "id": "w",
                "folders": [ {"name":"here","cwd":"."} ],
                "tabs": [ {"name":"files","id":"files","command":"sftp://","group":0} ] } ]
            }"#,
        );
        assert!(at.is_none(), "it made a destination though there is none");
    }

    /// A name given to a server from the settings is found by the file panel
    /// that reaches it, and by a terminal that reaches it through the same
    /// bastion -- because the settings file it under the spelling a launch
    /// builds, and nothing else spells it.
    #[test]
    fn a_servers_name_is_filed_the_way_a_launch_reaches_it() {
        let json = r#"{
          "desks": [ { "name":"w", "id": "w",
            "folders": [ {"name":"here","cwd":"."} ],
            "tabs": [
              {"name":"files","id":"files","group":0,
               "command":"sftp://deploy@10.0.0.5:22",
               "server":{"jump":{"host":"GW-Prod.example.com","user":"me"}}},
              {"name":"shell","id":"shell","group":0,
               "command":"ssh://root@10.0.0.5",
               "server":{"jump":{"host":"gw-prod.example.com","port":22,"user":"other"}}},
              {"name":"staging","id":"staging","group":0,
               "command":"ssh://root@10.0.0.5",
               "server":{"jump":{"host":"gw-staging.example.com","user":"me"}}}
            ] } ]
        }"#;
        let cfg: config::Config = serde_json::from_str(json).expect("the settings cannot be read");
        let (desks, errs) = cfg.resolve_desks();
        assert!(errs.is_empty(), "{errs:?}");
        let desk = desks.first().expect("there is no desk");
        let tab = |id: &str| {
            desk.tabs.iter().find(|t| t.cfg.id.as_deref() == Some(id)).expect("no such tab").cfg.clone()
        };
        // What the settings screen is told, for the panel it names from
        let filed = machine_of(&tab("files").command.argv(), tab("files").server.as_ref())
            .expect("the panel's address reaches no server");
        assert_eq!(filed, "gw-prod.example.com:22>10.0.0.5:22");

        // What a terminal is launched with, through a different person on the
        // same bastion
        let launched = |id: &str| {
            let cfg = tab(id);
            let mut opts = crate::tab::TabOptions::default();
            crate::desk::resolve_launch(cfg.command.argv(), &mut opts, Some(desk), &cfg);
            opts.remote.expect("the terminal reaches no server").machine()
        };
        assert_eq!(launched("shell"), filed, "the terminal is filed under another spelling");
        assert_ne!(launched("staging"), filed, "staging behind its own bastion took production's name");

        // And the panel on screen wears what was filed
        let marks = std::collections::HashMap::from([(
            filed.clone(),
            config::ServerMark { name: "Production".into(), color: Some("#e5644d".into()), careful: true },
        )]);
        let ui = Ui {
            active: 1,
            surfaces: surfaces_of(Some(desk), &[], &[], &[], false),
            server_marks: marks,
            ..Default::default()
        };
        let state = ui_state_of(&[], &ui, None);
        let panel = state.tabs.iter().find(|t| t.kind == "sftp").expect("the panel is not on screen");
        let mark = panel.mark.as_ref().expect("the panel does not wear its server's name");
        assert_eq!((mark.name.as_str(), mark.color.as_str(), mark.careful), ("Production", "#e5644d", true));
        assert_eq!(mark.machine, filed);
    }

    /// Nothing reaches a server until its address is written out: no name can
    /// be filed for a half-written one, and a command that is a program is
    /// not an address at all
    #[test]
    fn no_server_is_named_before_there_is_one() {
        let argv = |c: &str| config::CommandSpec::Line(c.into()).argv();
        assert_eq!(machine_of(&argv("sftp://"), None), None);
        assert_eq!(machine_of(&argv("ssh://@example.com"), None), None);
        assert_eq!(machine_of(&argv("ssh deploy@example.com"), None), None);
        assert_eq!(machine_of(&argv("ssh://deploy@Example.com:2222"), None).as_deref(), Some("example.com:2222"));
    }
}

#[cfg(test)]
mod drawn_away_tests {
    use super::{Surface, Ui, ui_state_of};

    /// Where a page is drawn reaches the screen with the page, and only with
    /// that page.
    ///
    /// Built here, once, along with everything else about the tab. The state was
    /// assembled in two places once before and the two drifted; a note attached
    /// to the wrong page would be worse than none, because it would be read.
    #[test]
    fn the_page_drawn_elsewhere_is_the_only_one_marked() {
        let ui = Ui {
            active: 1,
            surfaces: vec![
                Surface::Browser { key: "probe".into(), name: "試し".into() },
                Surface::Browser { key: "here".into(), name: "こちら".into() },
            ],
            away: vec![("probe".to_string(), "台所のノート".to_string())],
            ..Default::default()
        };
        let state = ui_state_of(&[], &ui, None);
        assert_eq!(
            state.tabs[0].away.as_deref(),
            Some("台所のノート"),
            "a page drawn over there does not say which device"
        );
        assert_eq!(state.tabs[1].away, None, "even a page here is treated as over there");
    }

    /// Nothing is said about a page drawn here, which is nearly every page.
    /// A project's worktrees are offered only when the desk does not list them,
    /// and a project whose were kept hidden still says so -- as kept -- so its
    /// heading can offer them back. A project with none to offer says nothing
    #[test]
    fn found_worktrees_are_the_ones_the_desk_does_not_list() {
        use std::path::PathBuf;
        let family = PathBuf::from(r"C:\work\app\.git");
        let quiet = PathBuf::from(r"C:\work\other\.git");
        let cuts = std::collections::HashMap::from([
            (family.clone(), vec![
                (PathBuf::from(r"C:\wt\app-login"), Some("login".to_string())),
                (PathBuf::from(r"C:\wt\app-fix"), None),
            ]),
            (quiet.clone(), vec![(PathBuf::from(r"C:\wt\other-a"), None)]),
        ]);
        // One of app's is on the desk already -- spelled differently where
        // Windows spells one folder more than one way -- and other's only one is
        let login = if cfg!(windows) { r"c:/wt/APP-LOGIN" } else { r"C:\wt\app-login" };
        let on_desk = vec![PathBuf::from(login), PathBuf::from(r"C:\wt\other-a")];
        let none_kept = std::collections::BTreeSet::new();
        let found = super::discovered_of(&cuts, &on_desk, &none_kept);
        assert_eq!(found.len(), 1, "a project with nothing to offer is offered: {found:?}");
        assert_eq!(found[0].found.len(), 1);
        assert_eq!(found[0].found[0].folder, r"C:\wt\app-fix");
        assert!(!found[0].kept);
        let kept = std::collections::BTreeSet::from([family.display().to_string()]);
        let again = super::discovered_of(&cuts, &on_desk, &kept);
        assert!(again[0].kept, "a kept project is not said to be kept");
    }

    #[test]
    fn a_page_of_this_machines_own_says_nothing_about_where_it_is() {
        let ui = Ui {
            active: 1,
            surfaces: vec![Surface::Browser { key: "probe".into(), name: "試し".into() }],
            ..Default::default()
        };
        let state = ui_state_of(&[], &ui, None);
        assert_eq!(state.tabs[0].away, None);
        // ...and it is left out of the state altogether rather than sent as a
        // null on every push
        let json = serde_json::to_string(&state).unwrap_or_default();
        assert!(!json.contains("\"away\""), "it sends every time though there is nothing to say");
    }
}

/// Builds what's laid out on screen, in the order written in config.
///
/// Things not in config (a browser automation opened later, a tab launched
/// via arguments) get appended at the end. There's no way to decide a
/// position for something that was never written down.
/// An editor as it stands right now: which folder it works in, and which file
/// it is showing. Held by the loop rather than the settings, because the file
/// somebody opened this afternoon is not a setting.
///
/// `scratch` marks the throwaway one the file list opens when there is no
/// editor to put a file in. It is not written anywhere: it exists while it is
/// open and is gone when it is closed.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EditorOpen {
    pub key: String,
    pub dir: Option<std::path::PathBuf>,
    /// The file it is showing, as it should read on the tab
    pub showing: Option<String>,
    /// What the disk says about that file right now. The page compares it with
    /// what it was given when it read: the same means nobody has touched it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<String>,
    pub scratch: bool,
    /// Which change of the file it is showing instead of the file itself --
    /// `work`, `staged` or `commit:<hash>` -- when a list of changes opened it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

pub fn surfaces_of(
    desk: Option<&config::Desk>,
    titles: &[&str],
    hosted: &[String],
    editors: &[EditorOpen],
    issues: bool,
) -> Vec<Surface> {
    surfaces_written(desk, titles, hosted, editors, issues).into_iter().map(|(s, _)| s).collect()
}

/// What the Issue tab is called by automation and by the board
pub const ISSUES_KEY: &str = "issues";

/// The same list, each row with where it is written in the settings: its
/// position in the desk's tabs, or nothing for a row the settings never
/// mentioned. What taking one out of the settings needs to know, worked out by
/// the one walk that decides the rows -- a second walk would one day pair a row
/// with somebody else's line
pub fn surfaces_written(
    desk: Option<&config::Desk>,
    titles: &[&str],
    hosted: &[String],
    editors: &[EditorOpen],
    issues: bool,
) -> Vec<(Surface, Option<usize>)> {
    let mut out: Vec<(Surface, Option<usize>)> = Vec::new();
    let mut used_tabs = vec![false; titles.len()];
    let mut used_web: Vec<&str> = Vec::new();
    if let Some(desk) = desk {
        for (written, ft) in desk.tabs.iter().enumerate() {
            let argv = ft.cfg.command.argv();
            if argv.is_empty() {
                continue;
            }
            if config::is_editor_panel(&argv) {
                let key = ft
                    .cfg
                    .id
                    .clone()
                    .or_else(|| ft.cfg.name.clone())
                    .unwrap_or_else(|| "editor".into());
                let name = ft.cfg.name.clone().unwrap_or_else(|| key.clone());
                out.push((Surface::Editor { key, name, dir: desk.cwd_of(ft) }, Some(written)));
                continue;
            }
            if config::is_sftp_panel(&argv) {
                let key = ft
                    .cfg
                    .id
                    .clone()
                    .or_else(|| ft.cfg.name.clone())
                    .unwrap_or_else(|| "sftp".into());
                let name = ft.cfg.name.clone().unwrap_or_else(|| key.clone());
                // Its credentials are filed under the desk and this tab,
                // exactly as a terminal's are, so the two are named the same
                // way and neither is written into the settings
                let under = |what: &str| {
                    let t = ft.cfg.id.as_deref()?;
                    Some(format!("ssh/{}/{}/{}", desk.id, t, what))
                };
                // The tab's own address wins: somebody wrote it on this
                // tab, and the folder it happens to sit in does not overrule
                // that. The folder's machine is what answers when nothing was
                // written -- which is the only thing a cloud sandbox can be,
                // having no address to write
                let at = config::sftp_endpoint(&argv)
                    .map(|(host, port, user)| {
                        crate::elsewhere::Elsewhere::Ssh(server_spec(
                            &host,
                            port,
                            &user,
                            ft.cfg.server.as_ref(),
                            &under,
                        ))
                    })
                    .or_else(|| {
                        desk.folder_of(ft)
                            .and_then(|f| f.host.as_ref())
                            .and_then(|h| crate::elsewhere::Elsewhere::of(h).ok())
                    });
                let remote_dir = ft
                    .cfg
                    .server
                    .as_ref()
                    .and_then(|sp| sp.remote_dir.clone())
                    .unwrap_or_default();
                out.push((Surface::Sftp { key, name, dir: desk.cwd_of(ft), at, remote_dir }, Some(written)));
                continue;
            }
            if config::is_git_panel(&argv) {
                let key = ft
                    .cfg
                    .id
                    .clone()
                    .or_else(|| ft.cfg.name.clone())
                    .unwrap_or_else(|| "git".into());
                let name = ft.cfg.name.clone().unwrap_or_else(|| key.clone());
                out.push((
                    Surface::Git {
                        dir: desk.cwd_of(ft),
                        protect: desk.folder_of(ft).map(|f| f.protect.clone()).unwrap_or_default(),
                        // Its own choice when it made one, else its folder's
                        // project's -- the one the column beside that folder uses
                        git: desk.git_use_here(ft.cfg.git_account.as_deref(), desk.cwd_of(ft).as_deref()),
                        key,
                        name,
                    },
                    Some(written),
                ));
                continue;
            }
            if config::browser_url_of(&argv).is_some() {
            let key = ft
                    .cfg
                    .id
                    .clone()
                    .or_else(|| ft.cfg.name.clone())
                    .unwrap_or_else(|| "browser".into());
                // Keeps a position even if it isn't open. If numbering shifted
                // based on open order, whatever a script points to would change
                // on every run.
                if let Some(h) = hosted.iter().find(|h| **h == key) {
                    used_web.push(h);
                }
                let name = ft.cfg.name.clone().unwrap_or_else(|| key.clone());
                out.push((Surface::Browser { key, name }, Some(written)));
                continue;
            }
            let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
            // Even with duplicate names, match them one-to-one in written order
            let found = titles
                .iter()
                .enumerate()
                .find(|(i, t)| **t == title && !used_tabs[*i])
                .map(|(i, _)| i);
            if let Some(i) = found {
                used_tabs[i] = true;
                out.push((Surface::Session(i), Some(written)));
            } else if let Some(failed) = crate::desk::launch_failure(&desk.name, &title) {
                // Written in the settings and not running because it could not
                // start. It keeps its place, saying why, rather than not being
                // there at all
                out.push((
                    Surface::Failed {
                        key: ft.cfg.id.clone().unwrap_or_else(|| title.clone()),
                        name: title,
                        dir: desk.cwd_of(ft),
                        why: failed.why,
                        install_url: failed.install_url,
                        // Worked out the way a launch reaches it: the address
                        // on the tab, else the machine its folder is on
                        machine: machine_of(&argv, ft.cfg.server.as_ref()).or_else(|| {
                            let host = desk.folder_of(ft).and_then(|f| f.host.as_ref())?;
                            match crate::elsewhere::Elsewhere::of(host).ok()? {
                                crate::elsewhere::Elsewhere::Ssh(spec) => Some(spec.machine()),
                                crate::elsewhere::Elsewhere::Cloud(_) => None,
                            }
                        }),
                    },
                    Some(written),
                ));
            }
        }
    }
    // Things not written in config
    for (i, used) in used_tabs.iter().enumerate() {
        if !used {
            out.push((Surface::Session(i), None));
        }
    }
    // The throwaway editor, if one is open. Same standing as a page placed
    // while the program runs: not in the settings, here because somebody
    // opened it, gone when they close it
    for e in editors.iter().filter(|e| e.scratch) {
        if !out.iter().any(|(s, _)| matches!(s, Surface::Editor { key, .. } if key == &e.key)) {
            out.push((
                Surface::Editor {
                    key: e.key.clone(),
                    name: e
                        .showing
                        .as_deref()
                        .map(leaf_of)
                        .unwrap_or_else(|| i18n::t("tui.state.editor")),
                    dir: e.dir.clone(),
                },
                None,
            ));
        }
    }
    // The Issue tab, once it has been opened. Written nowhere: opened from its
    // row in the list, and not closed
    if issues {
        out.push((Surface::Issues { key: ISSUES_KEY.to_string() }, None));
    }
    // Things not in config (opened later by automation, the settings screen, etc.) — the name is all there is
    for h in hosted {
        if !used_web.iter().any(|u| u == h) {
            // The result view gets a friendly, localized tab label; every other
            // ad-hoc page is addressed by (and labeled with) its own name.
            let name = if h == RESULT_TAB {
                i18n::t("tui.result.tab")
            } else {
                h.clone()
            };
            out.push((Surface::Browser { key: h.clone(), name }, None));
        }
    }
    // An editor's tab says which file it is showing rather than what it was
    // called -- that is the one thing about it worth reading from across the
    // window. Done here so a configured editor and a throwaway one read alike
    for (s, _) in out.iter_mut() {
        if let Surface::Editor { key, name, .. } = s
            && let Some(showing) =
                editors.iter().find(|e| &e.key == key).and_then(|e| e.showing.as_deref())
            {
                *name = leaf_of(showing);
            }
    }
    out
}

/// UI state needed for drawing.
///
/// `Default` is what a runtime with no shell reports: nothing is covering the
/// screen, no overlay is open, no window has been resized. A shell fills in
/// what it knows on top of that.
#[derive(Default)]
pub struct Ui {
    /// First-ever run, before config exists (shows onboarding on INDEX)
    pub first_run: bool,
    /// Whether the settings name a phone as somewhere answers go (see
    /// UiState::push_wanted)
    pub push_wanted: bool,
    /// Whether what's in view can be put back the way it started (see
    /// `restartable_page`). Drives the restart button beside the stop button
    pub restartable: bool,
    pub active: usize,
    /// Whether INDEX is covering the window. Not a surface: the panes wait
    /// underneath and come back the moment a running thing is picked
    pub board: bool,
    /// Whether the settings form is covering the window. A screen the same way
    pub settings: bool,
    /// Whether that form is the add-a-tab dialog rather than the page proper:
    /// it covers a rectangle, and the board stays drawn and dimmed around it
    pub settings_float: bool,
    pub auto: Option<bool>,
    pub desk_names: Vec<String>,
    /// What each of those desks is called by everything that is not a
    /// person, in the same order
    pub desk_ids: Vec<String>,
    pub desk_index: usize,
    pub desk_open: bool,
    pub help_open: bool,
    /// The keys in force, for the help screen to show
    pub help_rows: Vec<(String, String)>,
    /// The Vault's current search, when its overlay is open
    pub vault: Option<uistate::VaultState>,
    /// What this whole app is costing the machine, for the board header
    pub self_cost: Option<String>,
    /// The connection URL, if the QR code is being shown
    pub qr: Option<String>,
    /// Whether the remote UI is listening (shown at all times so it's never a mystery)
    pub remote_on: bool,
    /// Whether a phone/browser is connected over the remote link right now
    pub remote_conn: bool,
    /// Whether the pairing token is the fixed one from settings (it decides
    /// what the disconnect button promises, not what it does)
    pub remote_sticky: bool,
    /// What the focused tab is aimed at (🎯), as a screen number
    pub aim: Option<usize>,
    /// Where the auto-chain currently is (the invisible ball, made visible)
    pub ball: ball::Ball,
    /// The chain cap. Represents how close the ball's color is to that cap.
    pub max_chain: u32,
    /// Draw timestamp (relative ms). Used to drive the ball's animation.
    pub now_ms: u64,
    /// The surfaces on screen (one per tab-bar row), in the order written in config
    pub surfaces: Vec<Surface>,
    /// Which file each editor is showing, by the editor's own name. The page
    /// asks for the contents itself, so it has to be told this -- and it has to
    /// survive a reload, which is why it travels in the state rather than
    /// living in the page
    pub editors: Vec<crate::view::EditorOpen>,
    /// How the content area is divided, and which pane the keyboard is aimed at.
    /// `active` is always the surface in the focused pane
    pub layout: crate::layout::Layout,
    /// If the current desk is a discussion, the opening speaker's session
    /// number (1-based) and display name — for the dashboard's "start" card
    pub discuss_start: Option<usize>,
    pub discuss_start_name: Option<String>,
    /// What making a branch would do, while someone is naming one
    pub branch: Option<crate::uistate::BranchPlan>,
    /// What it would take to put a working folder back on this machine, while
    /// somebody is looking at the one that is missing
    pub repair: Option<crate::uistate::RepairPlan>,
    /// The folders being looked through, while somewhere new is being chosen
    pub browse: Option<crate::uistate::BrowseState>,
    /// The colours chosen for projects, by the folder git shares
    pub folder_colors: std::collections::HashMap<String, String>,
    /// The current desk's folders as the settings have them, so one with
    /// no tab in it is still on the list (uistate::GroupState::all)
    pub folders: Vec<(std::path::PathBuf, String)>,
    /// Of those, the ones the settings say which project they are in: (the
    /// folder, the project's name)
    pub folder_projects: Vec<(std::path::PathBuf, String)>,
    /// Of those, the ones made for an issue or a pull request: (the folder, what for)
    pub folder_items: Vec<(std::path::PathBuf, String)>,
    /// Of those, what each says is being done in it, and whether that is
    /// written for it from what its AIs are asked
    pub folder_labels: Vec<crate::uistate::FolderLabel>,
    /// Words waiting for the input bar of the AI tabs in a folder: (the folder,
    /// the words). The address of the issue a worktree was just made for
    pub drafts: Vec<(std::path::PathBuf, String)>,
    /// Of those, the ones that live on another machine. This machine has no
    /// opinion worth having about them: it is asked whether every folder is
    /// here, and for these the answer is "no" and is not a fault
    pub folders_elsewhere: Vec<std::path::PathBuf>,
    /// Those same folders, each with the name of the machine it is on
    pub folder_hosts: Vec<(std::path::PathBuf, String)>,
    /// And each with the server that machine is, for the name a person gave it
    /// ([`crate::ssh::Spec::machine`]). Absent for a sandbox, which is no
    /// lasting machine to name
    pub folder_machines: Vec<(std::path::PathBuf, String)>,
    /// What each server is called, as the settings have it
    pub server_marks: std::collections::HashMap<String, crate::config::ServerMark>,
    /// The controls shown over the browser being viewed (None = don't show)
    pub nav: Option<crate::uistate::NavState>,
    /// What each page of this desk is asking the person, by the name
    /// automation gives it. Drawn as a bar under that page
    pub asks: Vec<(String, crate::uistate::AskState)>,
    /// Which of this desk's pages are drawn on the connected device
    /// rather than here, by the same name, each with what that device is
    /// called (`caps::drawn_away`)
    pub away: Vec<(String, String)>,
    /// How many lines back from the current screen we're scrolled (0 = live)
    pub scrolled: usize,
    /// The AIs this machine can start, for the dialog that makes a folder
    /// and starts one in it
    pub ais: Vec<crate::uistate::AiChoice>,
    /// Which first-run pointer is up, if one is (see `coach_step`)
    pub coach: Option<u8>,
    /// A worktree is deleted from the list without asking first (Basic >
    /// Ask before deleting a worktree, turned off)
    pub discard_unasked: bool,
    /// The first-start setup, while it has not been answered
    pub setup: Option<crate::uistate::SetupState>,
    /// A project being cloned or made new
    pub add_project: Option<crate::uistate::AddProjectState>,
    /// The projects whose found worktrees somebody chose to keep hidden, by
    /// shared git folder
    pub worktrees_kept: std::collections::BTreeSet<String>,
    /// Worktrees being made, as their rows say
    pub making: Vec<crate::uistate::MakingState>,
    /// The machines a project can be added on
    pub hosts: Vec<crate::uistate::HostChoice>,
    /// The aliases of `~/.ssh/config`
    pub ssh_aliases: Vec<crate::discover::SshAlias>,
    /// A folder on another machine being walked
    pub remote_list: Option<crate::uistate::RemoteListState>,
    /// Where a cloned or new project goes by default
    pub project_home: String,
    /// The Assistant AI setting, as its command
    pub assistant: String,
    /// What Claude's subscription has left, when known
    pub usage: Option<crate::uistate::UsageState>,
    /// The thanks card, when it is up: which page it would open
    pub thanks: Option<String>,
    /// The newer version the update card asks about, when it is up
    pub update: Option<crate::update::Offer>,
    /// The current desk's git accounts, for the account menu on the git column
    pub git_accounts: Vec<config::GitAccountSpec>,
    /// The GitHub accounts git on this PC holds, for the same menu: with two,
    /// "this PC's git" has to be told which
    pub pc_accounts: Vec<String>,
    /// Where each git tab's folder pushes to on GitHub (`owner/name`), looked
    /// up every couple of seconds with the tabs' places rather than per frame
    pub git_repos: Vec<(std::path::PathBuf, String)>,
    /// A tab's ✕ waiting for an answer (see `closed::close`)
    pub close_ask: Option<crate::uistate::CloseAskState>,
    /// This desk's closed tabs that can be opened again
    pub closed: Vec<crate::uistate::ClosedState>,
    /// The quick commands as the launcher draws them (see `quick::view`)
    pub quick: std::sync::Arc<crate::quick::QuickView>,
    /// Where each kind of quick command would go right now
    pub quick_to: std::collections::BTreeMap<String, crate::quick::QuickDest>,
}

/// The name used when placing the result view (finished discussion / review /
/// rally, rendered as a chat) inside the window. Unlike settings it *does* show
/// in the tab strip, and it is reused (re-pointed) on each new result rather
/// than piling up copies.
pub const RESULT_TAB: &str = "result";

/// What's laid out on screen, in exactly the order written in config.
///
/// Keeping sessions and browsers as separate variants is purely an internal
/// concern; it has nothing to do with whoever wrote the config.
#[derive(Clone, Debug, PartialEq)]
pub enum Surface {
    /// Which index into `tabs` (0-based)
    Session(usize),
    /// A page placed inside the window
    Browser {
        /// The name automation addresses it by (ID, or display name if none). Also the name used to place it in the window.
        key: String,
        /// The human-readable name
        name: String,
    },
    /// The git panel: no process, no page, drawn by the board itself.
    ///
    /// It carries the folder it reports on because it has no tab of its own to
    /// borrow one from -- and that folder is what the primitives are called
    /// against, by this surface's own name
    Git {
        key: String,
        name: String,
        dir: Option<std::path::PathBuf>,
        /// The branches its folder will not take a direct commit onto. The
        /// panel has no tab of its own to borrow the answer from, so it carries
        /// the folder's own
        protect: Vec<String>,
        /// The git account this tab was set to use
        git: config::GitUse,
    },
    /// A tab the settings name that could not be started: the program is not
    /// on this PC, or its folder is not. Drawn by the board as what went wrong
    /// and what to do, in the place the tab would have been
    Failed {
        key: String,
        name: String,
        dir: Option<std::path::PathBuf>,
        why: String,
        install_url: Option<String>,
        /// The server it was to reach ([`crate::ssh::Spec::machine`]), for a
        /// terminal that could not get there. A tab that failed to reach
        /// production is still a production tab, and which server did not
        /// answer is the first thing to know about it
        machine: Option<String>,
    },
    /// The issues and pull requests of the desk's projects, drawn by the board.
    /// One per desk, opened from the list's own row and put away like a tab
    Issues { key: String },
    /// The editor: one text file of this tab's folder, drawn by the board.
    ///
    /// Like the git panel it has no process and no page, and it carries the
    /// folder it works in because it has no tab of its own to borrow one from.
    /// Which file it is showing is not written down here -- that is chosen
    /// while the program runs, and a file chosen yesterday is not a setting
    Editor {
        key: String,
        name: String,
        dir: Option<std::path::PathBuf>,
    },
    /// The file panel: two lists of files, one on this machine and one on a
    /// server, drawn by the board.
    ///
    /// It usually carries its own connection, written on its own command
    /// line, because the settings for a tab belong on that tab. `at` is absent
    /// while the address is still half-written -- a state the panel has to
    /// have, and says so on screen. `dir` is its folder on this machine, which
    /// is the folder its group is in.
    ///
    /// A folder that lives on another machine gives its panel that machine
    /// instead, since there is no address for somebody to have written: a
    /// sandbox in the cloud has a name in the settings and nothing else
    Sftp {
        key: String,
        name: String,
        dir: Option<std::path::PathBuf>,
        at: Option<crate::elsewhere::Elsewhere>,
        /// Where the far side's list opens. Empty starts wherever signing in
        /// puts you
        remote_dir: String,
    },
}

/// What a row is, for as long as it is on screen, whatever number it has.
///
/// A row's number is only where it stands, and it changes whenever something
/// before it goes. This is what stays the same, so a press aimed at one row
/// can be checked against the row it lands on, and a pane can be kept on the
/// thing it was showing when the rows move. A running tab is its serial rather
/// than its name: copies of a folder's tabs share their names. Everything else
/// is its own name, which is unique by the time it is a row
pub fn surface_key(s: &Surface, tabs: &[Tab]) -> String {
    match s {
        Surface::Session(i) => format!("tab:{}", tabs.get(*i).map(Tab::serial).unwrap_or_default()),
        Surface::Browser { key, .. } => format!("page:{key}"),
        Surface::Git { key, .. } => format!("git:{key}"),
        Surface::Sftp { key, .. } => format!("sftp:{key}"),
        Surface::Editor { key, .. } => format!("editor:{key}"),
        Surface::Failed { key, .. } => format!("failed:{key}"),
        Surface::Issues { key } => format!("issues:{key}"),
    }
}

/// Where each row went, after the rows changed: `moves[n - 1]` is the new
/// number of what was row `n`, or nothing when it is gone (see
/// `Layout::follow`).
///
/// A row whose key is not there any more, in a list of the same length whose
/// row at that number is new, is taken to be the same row under a new key --
/// something replaced in place (a tab that could not start, started), not
/// something closed while something else happened to open
pub fn surface_moves(before: &[String], now: &[String]) -> Vec<Option<usize>> {
    before
        .iter()
        .enumerate()
        .map(|(i, k)| {
            now.iter().position(|n| n == k).map(|p| p + 1).or_else(|| {
                (before.len() == now.len() && !before.contains(&now[i])).then_some(i + 1)
            })
        })
        .collect()
}

/// The last part of a path, which is what a row has room for.
fn leaf_of(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

pub fn title_of(argv: &[String]) -> String {
    argv.first()
        .map(|c| {
            std::path::Path::new(c)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(c)
                .to_uppercase()
        })
        .unwrap_or_else(|| "SHELL".into())
}

/// Everything needed to reach a server, from what the settings say about it.
///
/// The address comes from the command line, where a person can read it; the
/// rest comes from the tab's `server` block. Credentials are named, never
/// carried: `under` works out what each one is filed under from the desk
/// and the tab, so nothing here is a secret and the same name is not written
/// down twice.
///
/// A bastion is built by the same rules, one hop out, so that a server behind
/// two of them needs no new idea -- only another `jump`.
pub fn server_spec(
    host: &str,
    port: u16,
    user: &str,
    server: Option<&config::ServerSpec>,
    under: &dyn Fn(&str) -> Option<String>,
) -> ssh::Spec {
    let key = |k: &Option<String>| k.clone().filter(|s| !s.trim().is_empty());
    ssh::Spec {
        host: host.to_string(),
        port,
        user: user.to_string(),
        password_key: under("password"),
        key: server.and_then(|s| key(&s.key)),
        passphrase_key: under("passphrase"),
        jump: server.and_then(|s| s.jump.as_ref()).filter(|j| !j.host.trim().is_empty()).map(
            |j| {
                Box::new(ssh::Spec {
                    host: j.host.clone(),
                    port: j.port.unwrap_or(22),
                    user: j.user.clone(),
                    password_key: under("jump_password"),
                    key: key(&j.key),
                    passphrase_key: under("jump_passphrase"),
                    jump: None,
                    keepalive: None,
                    file_command: None,
                })
            },
        ),
        keepalive: server.and_then(|s| s.keepalive).filter(|n| *n > 0),
        file_command: server.and_then(|s| key(&s.file_command)),
    }
}

/// Which server a tab's command reaches, for the name a person gave it
/// ([`crate::ssh::Spec::machine`]).
///
/// Worked out from the same spec a launch builds, so the settings screen that
/// files a name and the tab row that looks it up cannot spell the server two
/// ways. None for a command that reaches no server, or whose address is not
/// written out far enough to be one yet
pub fn machine_of(argv: &[String], server: Option<&config::ServerSpec>) -> Option<String> {
    let (host, port, user) = config::ssh_endpoint(argv).or_else(|| config::sftp_endpoint(argv))?;
    Some(server_spec(&host, port, &user, server, &|_| None).machine())
}

/// Screen size. Only width and height are needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u16,
    pub height: u16,
}

/// The terminal size (rows, cols) passed to the PTY.
///
/// `size` already IS the content area: the page measures `#main` — the region
/// to the right of the tab bar and above the status bar — and reports the
/// rows/columns that fit there directly (see the shell's `report()`), which is
/// what `surface.size()` and the resize event carry. So this only guards the
/// floor; it must NOT subtract the tab bar or status bar again.
///
/// It used to. Back when `size` was the whole window in character cells, the
/// app drew its own tab bar and status bar, so it carved them out here. Once the
/// WebView took over that chrome and started measuring the content area itself,
/// the subtraction became a *second* one: every AI was handed the tab bar's
/// width in columns fewer than it had, rendering into only part of the width
/// with a wide blank margin on the right — and on a phone-narrow screen, where
/// the total column count is barely above it, it collapsed almost to nothing.
///
/// The tab bar's width was still being carried in here long after that, unread
/// behind an underscore, and a whole config field was computed for the sole
/// purpose of feeding it. Both are gone: the width is the window's business,
/// measured in pixels, and it is measured where it is drawn.
pub fn pty_dims(size: Size) -> (u16, u16) {
    (size.height.max(3), size.width.max(10))
}

/// The shape every terminal is cut to: **a phone that is watching decides it,
/// and the window decides it when none is.**
///
/// The two viewers see the same terminals at wildly different widths, and only
/// one number can be handed to a program. Both of them re-measure and re-report
/// freely -- the pane tree is redrawn on a tab switch and re-reports as part of
/// that -- so "whoever spoke last wins" was never a rule at all: the window
/// spoke on every repaint and took the size back within a frame of the phone
/// getting it. A phone opened onto a tab fitted its screen, then jumped to the
/// window's width the first time a tab was switched, and Claude Code -- which
/// rules a line clean across the terminal -- hung two thirds of itself off the
/// right edge with only a sideways scroll to read it by.
///
/// So the choice is made in one place, from who is looking rather than from who
/// spoke most recently, and the reports themselves become harmless. Watching
/// means a live state socket or a viewer still polling for the state; the
/// heartbeat sent along that socket is what makes a phone that walks away
/// noticed within a few seconds, and the window then has its own shape back
/// without anybody having to ask for it.
pub fn terminal_size(window: (u16, u16), phone: Option<(u16, u16)>, watched: bool) -> Size {
    match phone {
        Some((rows, cols)) if watched => Size { width: cols, height: rows },
        _ => Size { width: window.1, height: window.0 },
    }
}

/// Turn text a human typed into a destination we're allowed to open.
///
/// Works like a browser's combined address/search box: text that reads as a
/// web address goes there (`example.com` -> `https://example.com`), and
/// anything else — words with spaces, Japanese text, a lone word — becomes a
/// Google search. `file:` can read local files and `javascript:` can hijack
/// the current page, so neither passes through an address bar — a "gateway
/// to anywhere"; they too fall through to search, which is inert.
pub fn openable(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((scheme, rest)) = s.split_once("://") {
        // An explicit scheme means the writer wanted a URL, not a search.
        // Normalize its case so a pasted HTTPS:// still opens.
        if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
            return Some(format!("{}://{rest}", scheme.to_ascii_lowercase()));
        }
        // file:// and friends never open here — hand them to search instead
        return Some(search_url(s));
    }
    // Scheme-less: a single token whose host part has a dot (example.com,
    // 127.0.0.1) or is localhost reads as an address; everything else —
    // including `javascript:alert(1)`, which has no dot — reads as words
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    let address_like = !s.chars().any(char::is_whitespace)
        && (host.contains('.') || host == "localhost" || host.starts_with("localhost:"));
    if address_like {
        Some(format!("https://{s}"))
    } else {
        Some(search_url(s))
    }
}

/// A Google search for the given words, with every byte outside the URL-safe
/// set percent-encoded (UTF-8), so Japanese and symbols survive the trip
fn search_url(words: &str) -> String {
    use std::fmt::Write as _;
    let mut u = String::from("https://www.google.com/search?q=");
    for b in words.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                u.push(*b as char)
            }
            b' ' => u.push('+'),
            _ => {
                let _ = write!(u, "%{b:02X}");
            }
        }
    }
    u
}
