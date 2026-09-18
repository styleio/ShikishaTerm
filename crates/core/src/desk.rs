//! Opening a desk: the tabs it names, the automation it carries, and
//! moving between them.
//!
//! What a desk *is* -- which tabs exist, what each one runs, which folder
//! it works in, which automation is loaded for it -- is the runtime's to know.
//! A shell only ever asks for the result.

use crate::tab::Tab;
use crate::view::Surface;
use crate::hooks::HookEngine;
use crate::view::{server_spec, title_of};
use crate::{append_hook_log, resume_plan_of};
use crate::{bridge, config, hooks, i18n, tab};

/// The folders the git panels report on, keyed by their own names.
///
/// Appended after the tabs so that naming a panel resolves to its folder. A
/// panel is not a tab and has no process, but it is the thing a person clicked
/// on, so it has to be nameable the same way
pub fn panel_places(surfaces: &[Surface]) -> Vec<hooks::TabPlace> {
    surfaces
        .iter()
        .filter_map(|s| match s {
            Surface::Git { key, dir: Some(d), protect, git, .. } => Some(hooks::TabPlace {
                key: hooks::TabKey { id: Some(key.clone()) },
                dir: d.clone(),
                // A git panel reports on a folder on this machine, and is not a
                // place files can be sent to
                remote: None,
                remote_dir: String::new(),
                protect: protect.clone(),
                git: git.clone(),
            }),
            // The editor works in a folder too, and is named the same way, so
            // reading and writing the file it is showing goes through the same
            // fence as everything else
            Surface::Editor { key, dir: Some(d), .. } => Some(hooks::TabPlace {
                key: hooks::TabKey { id: Some(key.clone()) },
                dir: d.clone(),
                remote: None,
                remote_dir: String::new(),
                protect: Vec::new(),
                git: Default::default(),
            }),
            // A file panel is. `sftp_put("that name", …)` reaches the same
            // server the screen is showing, which is the whole point of the
            // panel being a tab rather than a window of its own
            Surface::Sftp { key, dir, at, remote_dir, .. } => Some(hooks::TabPlace {
                key: hooks::TabKey { id: Some(key.clone()) },
                dir: dir.clone().unwrap_or_default(),
                remote: at.clone(),
                remote_dir: remote_dir.clone(),
                protect: Vec::new(),
                git: Default::default(),
            }),
            _ => None,
        })
        .collect()
}

pub fn build_engine(
    cfg: Option<&config::Config>,
    desk: Option<&config::Desk>,
    errors: &mut Vec<String>,
    caps: &hooks::Caps,
) -> Option<HookEngine> {
    let base = cfg.and_then(|c| c.automation_path());
    let desk_lua = desk.and_then(|w| w.automation.clone());
    let tab_luas: Vec<(usize, TabAuto)> = desk.map(automation_by_pane).unwrap_or_default();
    let has_discuss = desk
        .and_then(|w| w.discuss.as_ref())
        .is_some_and(|d| d.agents.len() >= 2);
    // Keep the engine even with no Lua hooks when a tab wants a completion
    // notification: the on_done detection loop only runs when an engine exists,
    // so without this a notify-only desk would never fire on_done.
    let wants_notify = desk
        .map(|w| w.tabs.iter().any(|t| t.cfg.notify_on_done.is_some()))
        .unwrap_or(false);
    // A Lua quick-action needs an engine to run in, even when nothing else does.
    let has_lua_actions = cfg
        .map(|c| c.actions.iter().any(|a| a.lua))
        .unwrap_or(false);
    if base.is_none()
        && desk_lua.is_none()
        && tab_luas.is_empty()
        && !has_discuss
        && !wants_notify
        && !has_lua_actions
    {
        return None;
    }

    let mut engine = match HookEngine::with_caps(hooks::Caps::clone(caps)) {
        Ok(e) => e,
        Err(e) => {
            errors.push(format!("Lua: {e:#}"));
            return None;
        }
    };
    let load = |engine: &mut HookEngine, path: &str, errors: &mut Vec<String>| -> Option<usize> {
        // One lookup, shared with the settings screen and the config loader, so
        // "where the program runs it from" and "where the screen reads and
        // writes it" cannot drift apart. A second copy used to live here, without
        // the layout root, which is how the Store copy came to run none of the
        // scripts a person wrote
        match engine.load_path(&config::resolve_data_path(path)) {
            Ok(id) => Some(id),
            Err(e) => {
                errors.push(format!("Lua({path}): {e:#}"));
                None
            }
        }
    };
    if let Some(p) = &base
        && let Some(id) = load(&mut engine, p, errors) {
            engine.set_base(id);
        }
    if let Some(p) = &desk_lua
        && let Some(id) = load(&mut engine, p, errors) {
            engine.set_desk(id);
        }
    // The referee (stop conditions) is per-desk. Passed to the built-in commander as a Lua table.
    let stops_lua = desk
        .map(|w| config::stops_to_lua(&w.stops))
        .unwrap_or_else(|| "{}".to_string());
    for (idx, auto) in &tab_luas {
        let id = match auto {
            TabAuto::Path(p) => load(&mut engine, p, errors),
        };
        if let Some(id) = id {
            engine.set_tab(*idx, id);
        }
    }
    // AI-vs-AI discussion: if the desk has `discuss`, load the built-in discussion commander into each participant tab
    if let Some(w) = desk
        && let Some(d) = &w.discuss {
            let agents: Vec<String> = d
                .agents
                .iter()
                .filter(|s| !s.trim().is_empty())
                .cloned()
                .collect();
            let n = agents.len();
            if n >= 2 {
                let max_turns = (d.max_rounds.max(1) as usize) * n;
                // Turn the participant list into a Lua list literal to pass to the commander (used by group stops)
                let agents_lua = format!(
                    "{{{}}}",
                    agents
                        .iter()
                        .map(|a| format!("{a:?}"))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                // id -> display name, so the discussion (statements, transcript,
                // hand-offs) refers to participants by their display name while
                // routing still uses the stable id.
                let names_lua = {
                    let mut s = String::from("{");
                    for t in &w.tabs {
                        if t.cfg.command.argv().is_empty() {
                            continue;
                        }
                        let key = t
                            .cfg
                            .id
                            .as_deref()
                            .filter(|x| !x.is_empty())
                            .or_else(|| t.cfg.name.as_deref().filter(|x| !x.is_empty()));
                        let Some(key) = key else { continue };
                        let disp = t
                            .cfg
                            .name
                            .as_deref()
                            .filter(|x| !x.is_empty())
                            .unwrap_or(key);
                        s.push_str(&format!("[{key:?}]={disp:?},"));
                    }
                    s.push('}');
                    s
                };
                let moderator = d.moderator.as_deref().filter(|s| !s.trim().is_empty());
                for (i, id) in agents.iter().enumerate() {
                    let Some(pane) = surface_of_id(w, id) else {
                        errors.push(crate::i18n::tp(
                            "err.desk.discuss_tab_missing",
                            &[("id", id)],
                        ));
                        continue;
                    };
                    let next = &agents[(i + 1) % n];
                    let persona = d.personas.get(id).map(String::as_str).unwrap_or("");
                    match engine.load_discuss_agent(
                        id,
                        next,
                        i == 0,
                        false,
                        d.judge.as_deref(),
                        max_turns,
                        &agents_lua,
                        &names_lua,
                        &stops_lua,
                        &d.verdict,
                        &d.order,
                        moderator,
                        false,
                        persona,
                    ) {
                        Ok(sid) => engine.set_tab(pane, sid),
                        Err(e) => errors.push(crate::i18n::tp(
                            "err.desk.discuss_agent_failed",
                            &[("id", id), ("e", &format!("{e:#}"))],
                        )),
                    }
                }
                if let Some(j) = d.judge.as_deref().filter(|s| !s.trim().is_empty()) {
                    let persona = d.personas.get(j).map(String::as_str).unwrap_or("");
                    match surface_of_id(w, j) {
                        Some(pane) => match engine.load_discuss_agent(
                            j, j, false, true, None, max_turns, &agents_lua, &names_lua,
                            &stops_lua,
                            &d.verdict, &d.order, moderator, false, persona,
                        ) {
                            Ok(sid) => engine.set_tab(pane, sid),
                            Err(e) => errors.push(crate::i18n::tp(
                                "err.desk.discuss_judge_failed",
                                &[("j", j), ("e", &format!("{e:#}"))],
                            )),
                        },
                        None => errors.push(crate::i18n::tp(
                            "err.desk.discuss_judge_missing",
                            &[("j", j)],
                        )),
                    }
                }
                // The moderator tab: nominates the next speaker when order="moderated"
                if let Some(m) = moderator {
                    let persona = d.personas.get(m).map(String::as_str).unwrap_or("");
                    match surface_of_id(w, m) {
                        Some(pane) => match engine.load_discuss_agent(
                            m, m, false, false, d.judge.as_deref(), max_turns, &agents_lua,
                            &names_lua, &stops_lua, &d.verdict, &d.order, moderator, true, persona,
                        ) {
                            Ok(sid) => engine.set_tab(pane, sid),
                            Err(e) => errors.push(crate::i18n::tp(
                                "err.desk.discuss_moderator_failed",
                                &[("m", m), ("e", &format!("{e:#}"))],
                            )),
                        },
                        None => errors.push(crate::i18n::tp(
                            "err.desk.discuss_moderator_missing",
                            &[("m", m)],
                        )),
                    }
                }
            } else if !d.agents.is_empty() {
                errors.push(crate::i18n::t("err.desk.discuss_needs_two"));
            }
        }
    // Keep the engine even with no Lua hooks when a tab wants a completion
    // notification (the on_done detection loop lives behind `Some(engine)`), or
    // when there are Lua quick-actions to run in it.
    (!engine.is_empty() || wants_notify || has_lua_actions).then_some(engine)
}

/// Applies a config change to the running set of tabs.
/// Whatever can take effect immediately does; whatever needs the session
/// rebuilt is deferred and flagged instead (so a running AI doesn't get cut
/// off without asking). The return value is the message reported to the user.
///
/// `resume` holds conversations to hand the tabs this starts, by automation
/// name: a tab opened again from the tab bar comes back into the conversation
/// it was having. Taken out as they are used, and preferred over a `resume`
/// written in the settings, which is older news
///
/// `carry` is what was on screen when the app last closed, for the tabs this
/// starts that nothing else speaks for. A settings change starts tabs by the
/// same rules a launch does, so a folder added back is the folder it was, with
/// the conversation it was having -- and, just as importantly, a tab started
/// here holds on to what it was having, so saving the settings cannot quietly
/// replace a real conversation with an empty one nobody has spoken in
pub fn apply_ws_config(
    tabs: &mut Vec<Tab>,
    desk: &config::Desk,
    rows: u16,
    cols: u16,
    errors: &mut Vec<String>,
    resume: &mut std::collections::HashMap<String, tab::Session>,
    carry: Option<&crate::lastsession::Saved>,
) -> String {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut staged = 0usize;

    // Close tabs no longer in config (removed via the GUI = an explicit instruction)
    let wanted: Vec<String> = desk
        .tabs
        .iter()
        .map(|f| {
            f.cfg
                .name
                .clone()
                .unwrap_or_else(|| title_of(&f.cfg.command.argv()))
        })
        .collect();
    // A tab taken out of the settings takes its failure with it
    failures().retain(|f| f.desk != desk.name || wanted.contains(&f.title));

    // Every line of the settings that launches a process, worked out once:
    // matching needs where each one stands before anything is started
    let launches: Vec<_> = desk
        .tabs
        .iter()
        .filter_map(|ft| {
            let argv = ft.cfg.command.argv();
            // Browsers aren't child processes, so don't launch them here
            // (open_declared_browsers opens the window)
            if argv.is_empty() || config::is_app_panel(&argv) {
                return None;
            }
            let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
            let mut opts = tab_options(&ft.cfg, desk.folder_of(ft));
            let argv = resolve_launch(argv, &mut opts, Some(desk), &ft.cfg);
            Some((ft, title, argv, opts))
        })
        .collect();
    let places: Vec<(&String, &tab::TabOptions)> = launches.iter().map(|(_, title, _, opts)| (title, opts)).collect();
    let claims = claim_running(tabs, &places);

    // Update existing tabs and add new ones
    let mut running: Vec<Option<Tab>> = std::mem::take(tabs).into_iter().map(Some).collect();
    let mut ordered: Vec<Tab> = Vec::with_capacity(launches.len());
    for ((ft, title, argv, opts), claim) in launches.into_iter().zip(claims) {
        // Kept for the message, because the options themselves are moved into
        // the tab and the message is only wanted when that did not happen
        let said = opts.clone();
        match claim.and_then(|i| running[i].take()) {
            Some(mut t) => {
                forget_failure(&desk.name, &title);
                t.apply_live_config(
                    ft.cfg.profile.clone(),
                    ft.cfg.locked,
                    ft.cfg.auto_restart,
                    ft.depth,
                    ft.cfg.notify_on_done.clone(),
                    &opts,
                );
                // Changes to command, encoding, or line count require a rebuild
                if t.signature() != tab::signature_of(&argv, &opts) {
                    t.stage_restart_config(argv.clone(), opts);
                    staged += 1;
                }
                ordered.push(t);
            }
            None => {
                // Worked out before the tab is built, because how it is
                // starting is part of what it comes up as: a tab that lost the
                // conversation it was having keeps the offer of the way back
                let carried = match ft.cfg.id.as_deref().and_then(|id| resume.remove(id)) {
                    Some(s) if tab::resumable(&argv, &ft.cfg.profile, &s.id) => {
                        Carried { plan: tab::Resume::Id(s), lost: false }
                    }
                    _ => launch_plan(carry, desk, &argv, &ft.cfg, &said.cwd, &title),
                };
                let mut opts = opts;
                opts.lost = carried.lost;
                match Tab::spawn_as(
                    title.clone(),
                    &argv,
                    ft.cfg.profile.clone(),
                    rows,
                    cols,
                    opts,
                    carried.plan,
                ) {
                    Ok(mut t) => {
                        forget_failure(&desk.name, &title);
                        t.locked = ft.cfg.locked;
                        t.auto_restart = ft.cfg.auto_restart;
                        t.depth = ft.depth;
                        t.id = ft.cfg.id.clone();
                        t.notify_on_done = ft.cfg.notify_on_done.clone();
                        t.notify_reply = ft.cfg.notify_reply;
                        // What this tab was having, whether or not it was carried:
                        // the key that means "carry the conversation over" reaches
                        // for it, and it is what the app goes on remembering for a
                        // tab nobody has spoken to yet
                        t.previous = carry.and_then(|last| last.conversation_for(desk, &t));
                        ordered.push(t);
                        added += 1;
                    }
                    Err(e) => {
                        let prog = argv.first().map(String::as_str).unwrap_or("");
                        let why = tab::launch_problem_for(&title, prog, &said, &e.to_string());
                        remember_failure(&desk.name, &title, prog, &why);
                        errors.push(why);
                    }
                }
            }
        }
    }
    // Close whatever's left that isn't in config
    for mut t in running.into_iter().flatten() {
        t.kill();
        removed += 1;
    }
    *tabs = ordered;

    let mut parts = vec![i18n::t("msg.config_reloaded")];
    if added > 0 {
        parts.push(i18n::tp("msg.config_added", &[("n", &added.to_string())]));
    }
    if removed > 0 {
        parts.push(i18n::tp("msg.config_removed", &[("n", &removed.to_string())]));
    }
    if staged > 0 {
        parts.push(i18n::tp("msg.config_needs_restart", &[("n", &staged.to_string())]));
    }
    parts.join(" / ")
}

/// Which running tab each line of the settings is, by position in `tabs`.
///
/// `wanted` is each line's title and where it would launch, in the order of
/// the settings. A running tab is only ever one line's, and a line takes the
/// tab that is plainly the same one: the same title in the same place. Two
/// tabs of one name in one folder are told apart by their order.
///
/// Only then may a line reach for a tab of its name in another place, and
/// only for a tab whose place no line stands in any more -- a folder moved to
/// a new path in the settings. That tab is kept and restarted into the new
/// place once it is idle, which is what a changed launch has always done. A
/// tab whose folder is still listed is never taken this way: it belongs to that
/// folder's line. Taking it anyway is how deleting one worktree stopped the AI
/// at work in another, and started that one again in a conversation of nobody's
fn claim_running(tabs: &[Tab], wanted: &[(&String, &tab::TabOptions)]) -> Vec<Option<usize>> {
    let mut taken = vec![false; tabs.len()];
    let mut claims: Vec<Option<usize>> = vec![None; wanted.len()];
    for (j, (title, opts)) in wanted.iter().enumerate() {
        let found = tabs
            .iter()
            .enumerate()
            .position(|(i, t)| !taken[i] && &t.title == *title && t.stands_at(opts));
        if let Some(i) = found {
            taken[i] = true;
            claims[j] = Some(i);
        }
    }
    for (j, (title, _)) in wanted.iter().enumerate() {
        if claims[j].is_some() {
            continue;
        }
        let found = tabs.iter().enumerate().position(|(i, t)| {
            !taken[i]
                && &t.title == *title
                && !wanted.iter().any(|(_, o)| t.stands_at(o))
        });
        if let Some(i) = found {
            taken[i] = true;
            claims[j] = Some(i);
        }
    }
    claims
}

/// Redraws a page's top bar and bottom band to match config.
///
/// Runs not just on open, but also whenever config is reloaded.
/// Without going through here, checking a box wouldn't show up until a restart.
pub fn apply_browser_chrome(desk: &config::Desk, caps: &hooks::Caps) {
    // Close browsers that dropped out of config. Leaving them in place would
    // make them reappear at the back of the list as a "page not in config".
    let declared: Vec<String> = desk
        .browsers
        .iter()
        .map(|b| b.id.clone())
        .chain(desk.tabs.iter().filter_map(|ft| {
            let argv = ft.cfg.command.argv();
            config::browser_url_of(&argv)?;
            Some(
                ft.cfg
                    .id
                    .clone()
                    .or_else(|| ft.cfg.name.clone())
                    .unwrap_or_else(|| "browser".into()),
            )
        }))
        .collect();
    for gone in caps.keep_only_declared(&declared) {
        append_hook_log(&format!("Closed because it dropped out of config: {gone}"));
    }

    for ft in &desk.tabs {
        let argv = ft.cfg.command.argv();
        if config::browser_url_of(&argv).is_none() {
            continue;
        }
        let name = ft
            .cfg
            .id
            .clone()
            .or_else(|| ft.cfg.name.clone())
            .unwrap_or_else(|| "browser".into());
        // Clear it if it was removed. Leaving it around even after reverting
        // config would make it un-fixable. This also brings anything Lua set
        // into line with whatever config specifies at save time.
        match ft.cfg.nav {
            Some(nav) => {
                let _ = caps.browser_nav(&name, nav);
            }
            None => {
                let _ = caps.browser_unnav(&name);
            }
        }
        match &ft.cfg.ask {
            Some(ask) => {
                let label = if ask.label.trim().is_empty() {
                    i18n::t("tui.ask.label")
                } else {
                    ask.label.clone()
                };
                let _ = caps.browser_ask(&name, &ask.text, &label);
            }
            None => {
                let _ = caps.browser_unask(&name);
            }
        }
    }
}

/// Launch every tab a desk declares.
///
/// `carry` is what was on screen when the app last closed; whether a given tab
/// actually comes back to it is that tab's own setting. A tab the Vault
/// reopened names its own conversation and outranks both: that id was chosen
/// deliberately, a moment ago, and "what this tab was saying last time" is not
/// an answer to it
pub fn spawn_desk(
    desk: &config::Desk,
    rows: u16,
    cols: u16,
    tabs: &mut Vec<Tab>,
    errors: &mut Vec<String>,
    carry: Option<&crate::lastsession::Saved>,
) {
    for ft in &desk.tabs {
        let argv = ft.cfg.command.argv();
        if argv.is_empty() {
            continue;
        }
        // Browsers aren't child processes; they're just pages placed inside the
        // window. Trying to launch one here would produce a baffling "no
        // executable named browser" failure every time, out of nowhere.
        // (open_declared_browsers opens them)
        if config::is_app_panel(&argv) {
            continue;
        }
        let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
        let mut opts = tab_options(&ft.cfg, desk.folder_of(ft));
        let argv = resolve_launch(argv, &mut opts, Some(desk), &ft.cfg);
        let cwd = opts.cwd.clone();
        // Kept for the message, because the options are moved into the tab and
        // the message is only wanted when that did not happen
        let said = opts.clone();
        let carried = launch_plan(carry, desk, &argv, &ft.cfg, &cwd, &title);
        opts.lost = carried.lost;
        match Tab::spawn_as(
            title.clone(),
            &argv,
            ft.cfg.profile.clone(),
            rows,
            cols,
            opts,
            carried.plan,
        ) {
            Ok(mut tab) => {
                forget_failure(&desk.name, &title);
                tab.locked = ft.cfg.locked;
                tab.auto_restart = ft.cfg.auto_restart;
                tab.depth = ft.depth;
                tab.notify_on_done = ft.cfg.notify_on_done.clone();
                tab.notify_reply = ft.cfg.notify_reply;
                tabs.push(tab);
            }
            Err(e) => {
                let prog = argv.first().map(String::as_str).unwrap_or("");
                let why = tab::launch_problem_for(&title, prog, &said, &e.to_string());
                remember_failure(&desk.name, &title, prog, &why);
                errors.push(why);
            }
        }
    }
}

/// A tab the settings name that could not be started, and why.
///
/// Kept so the tab stays on the screen. It used to be a line in a log and a
/// toast that faded in seconds -- or, after a settings save, not even that --
/// while the tab itself was simply not there: somebody who had just added one
/// saw nothing happen, with nothing to say why
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchFailure {
    pub desk: String,
    /// The tab's title, which is how a running tab is matched to its settings
    pub title: String,
    /// The program that was not started
    pub prog: String,
    /// What went wrong and what to do, in words
    pub why: String,
    /// Where the maker says how to install it, when a profile knows
    pub install_url: Option<String>,
}

fn failures() -> std::sync::MutexGuard<'static, Vec<LaunchFailure>> {
    static FAILED: std::sync::Mutex<Vec<LaunchFailure>> = std::sync::Mutex::new(Vec::new());
    FAILED.lock().unwrap_or_else(|e| e.into_inner())
}

fn remember_failure(desk: &str, title: &str, prog: &str, why: &str) {
    append_hook_log(&format!("could not start \"{title}\" in \"{desk}\": {why}"));
    let mut list = failures();
    list.retain(|f| !(f.desk == desk && f.title == title));
    list.push(LaunchFailure {
        desk: desk.to_string(),
        title: title.to_string(),
        prog: prog.to_string(),
        why: why.to_string(),
        install_url: crate::profile::install_url_for(prog),
    });
}

fn forget_failure(desk: &str, title: &str) {
    failures().retain(|f| !(f.desk == desk && f.title == title));
}

/// Why this tab of this desk could not be started, if it could not
pub fn launch_failure(desk: &str, title: &str) -> Option<LaunchFailure> {
    failures().iter().find(|f| f.desk == desk && f.title == title).cloned()
}

/// Everything that belongs to the desk now on screen, handed over at once.
///
/// Five things belong to a desk alone -- its notification destinations, its
/// model connections, what doors its automation has, who may use them, and
/// the tokens of its git accounts -- and every one of them has to
/// change at the same moment as the screen does. In one place because the
/// failure otherwise is silent and one-sided: the half nobody remembered to
/// swap keeps answering for the desk that was on screen a moment ago.
///
/// Nothing here decides anything: each value is the desk's own, whole.
pub fn hand_over(
    desk: &config::Desk,
    caps: &hooks::Caps,
    notifier: &crate::notify::Notifier,
    prs: &crate::pr::Watch,
) {
    // A value written `@name` is read from the store here, through the
    // program's own door, which already holds whatever the password unlocked
    let look = |k: &str| caps.secret_value(k).ok();
    notifier.use_desk(config::desk_notify(desk, &look), desk.primary_notify.clone());
    crate::bridge::use_desk(config::desk_providers(desk, &look));
    caps.set_capabilities(desk.capabilities.clone());
    caps.set_grants(desk.automation_permissions.clone());
    // A script's `token` means this desk's, and no other's
    caps.set_desk_id(&desk.id);
    // The tokens pull request numbers are read with, one per git account of
    // this desk that has one. Which of them a row asks with is its project's
    // choice. The program reaches for the values itself here -- a script never
    // sees them
    prs.use_tokens(
        desk.git_accounts
            .iter()
            .filter_map(|a| Some((a.name.clone(), a.token(&desk.id, &look)?)))
            .collect(),
    );
}

/// Switches desks (virtual-desktop model).
/// Switching means hiding, not stopping — tabs that go into the background keep running.
/// An unlaunched desk gets its first launch right here.
#[allow(clippy::too_many_arguments)]
pub fn switch_desk(
    to: usize,
    desk_index: &mut usize,
    tabs: &mut Vec<Tab>,
    desk_tabs: &mut [Vec<Tab>],
    desks: &[config::Desk],
    active: &mut usize,
    panes: &mut crate::layout::Layout,
    desk_panes: &mut [crate::layout::Layout],
    rows: u16,
    cols: u16,
    errors: &mut Vec<String>,
    started_fired: &mut Vec<bool>,
    cfg: Option<&config::Config>,
    engine: &mut Option<HookEngine>,
    engines: &mut [Option<HookEngine>],
    caps: &hooks::Caps,
    notifier: &crate::notify::Notifier,
    prs: &crate::pr::Watch,
    last: &crate::lastsession::Saved,
) {
    // Guard against every backing array, not just `desks`: the per-desk
    // `engines`/`desk_tabs` caches are resized on config reload, and a mismatch must
    // never index out of bounds (that would crash the whole app on switch).
    if to == *desk_index
        || to >= desks.len()
        || to >= engines.len()
        || to >= desk_tabs.len()
        || to >= desk_panes.len()
        || *desk_index >= desk_tabs.len()
        || *desk_index >= desk_panes.len()
    {
        return;
    }
    desk_tabs[*desk_index] = std::mem::take(tabs);
    // How a desk is divided belongs to that desk. Carrying one
    // layout across the switch would leave a project split into panes that
    // point at another project's tab numbers — the screen would look
    // deliberate and mean nothing.
    desk_panes[*desk_index] = panes.clone();
    // The Lua environment is kept per desk (so shared variables survive switching)
    engines[*desk_index] = engine.take();
    *desk_index = to;
    // Ids only mean something within their own desk.
    // Placed pages also only appear in the tab list for whichever one is currently viewed.
    caps.set_desk(to);
    // Everything that is this desk's rather than the app's, in one act and
    // before its tabs are launched below: a tab opening for the first time is
    // held to the same answers as one that was already running
    hand_over(&desks[to], caps, notifier, prs);
    config::save_last_desk(&desks[to].id);
    *tabs = std::mem::take(&mut desk_tabs[to]);
    if tabs.is_empty() {
        // First visit this run, so these tabs are being launched for the first
        // time and the same question applies as at startup: come back to what
        // this desk was saying, or start it clean
        spawn_desk(&desks[to], rows, cols, tabs, errors, Some(last));
        // Whether or not it was carried, the way back is worth holding on to:
        // this is what Ctrl+B r reaches for on a tab nobody has spoken to yet
        for t in tabs.iter_mut() {
            t.previous = last.conversation_for(&desks[to], t);
        }
        open_declared_browsers(&desks[to], caps, errors);
    }
    *engine = match engines[to].take() {
        Some(e) => Some(e),
        None => build_engine(cfg, desks.get(to), errors, caps),
    };
    started_fired.clear();
    started_fired.resize(tabs.len(), false);
    *active = if tabs.is_empty() { 0 } else { 1 };
    *panes = std::mem::replace(&mut desk_panes[to], crate::layout::Layout::single(*active));
    panes.show(*active);
}

/// Carries every desk's running tabs over to the settings just read.
///
/// `tabs` are the tabs of the desk on screen (`viewing` in `before`), and
/// `parked` the others', by position in `before`; afterwards `parked` is by
/// position in `after`. Each desk is found again by `config::pair_desks` --
/// its id, which renaming leaves alone -- and its tabs go with it, whatever it
/// is called now and wherever it stands. A desk deleted in the settings takes
/// its tabs with it.
///
/// Returns where the desk on screen stands now. `None` means it was deleted:
/// its tabs are stopped, and `tabs` holds the running tabs of the first desk,
/// which takes the screen (empty when it has not been opened yet). Those tabs
/// are that desk's own. Handing the deleted desk's tabs to the first desk's
/// settings instead took whichever of them shared a name, started the rest
/// from nothing, and left the first desk's real tabs parked, to be thrown away
/// on the next switch: an AI at work there stopped mid-task and came back as a
/// new conversation. A desk only renamed used to be taken for a deleted one,
/// with the same result
pub fn reseat_desks(
    before: &[config::Desk],
    after: &[config::Desk],
    viewing: usize,
    tabs: &mut Vec<Tab>,
    parked: &mut Vec<Vec<Tab>>,
) -> Option<usize> {
    let paired = config::pair_desks(before, after);
    let mut reseated: Vec<Vec<Tab>> = after.iter().map(|_| Vec::new()).collect();
    for (i, slot) in parked.iter_mut().enumerate() {
        // The viewed desk's tabs live in `tabs`, so its slot stays empty
        if i == viewing {
            continue;
        }
        let mut cached = std::mem::take(slot);
        match paired.get(i).copied().flatten().and_then(|j| reseated.get_mut(j)) {
            Some(home) => *home = cached,
            None => cached.iter_mut().for_each(Tab::kill),
        }
    }
    *parked = reseated;
    let viewed = paired.get(viewing).copied().flatten();
    if viewed.is_none() {
        tabs.iter_mut().for_each(Tab::kill);
        *tabs = parked.first_mut().map(std::mem::take).unwrap_or_default();
    }
    viewed
}

/// Pull the survey's output block off a screen. The drafted command itself
/// echoes on screen too (with both markers inside one command line), so the
/// capture insists on the shape only real output has: the start marker ALONE
/// on its line, with content lines underneath, ending at the bare end marker
pub fn extract_env_block(screen: &str) -> Option<String> {
    // Walk marker lines, not raw indices: the echoed command line contains
    // the marker mid-line and must never match
    let mut start_line: Option<usize> = None;
    let lines: Vec<&str> = screen.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if l.trim() == "===SHIKISHA ENV===" {
            start_line = Some(i);
        }
    }
    let start = start_line?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim().starts_with("===ENV END==="))?
        + start
        + 1;
    let body = lines[start..end].join("\n");
    (end > start + 1).then(|| body.trim().chars().take(1500).collect())
}

/// Loads Lua hooks across 3 tiers (base > desk > tab).
/// Hook resolution favors "the more specific one wins", so only hooks a tab's
/// script doesn't define fall back to desk, then base.
/// Lines up per-tab automation by screen number.
///
/// The number matches whatever's shown on screen. If the number a human
/// presses, the number a script points at, and the number the ball flies to
/// were all different, nobody could keep track of any of it.
///
/// Reordering still works, because it's reassigned every time config is reloaded.
/// Nothing is remembered anywhere, so it can never drift out of sync.
/// The automation assignment for a tab: a file or directory of Lua.
///
/// Being aimed at another tab (ð¯) is NOT one of these. An aim is picked on
/// screen and attached when there is a goal, so a tab keeps the automation it
/// was given either way -- the two used to fight, and the aim won silently.
#[derive(Debug, PartialEq)]
pub enum TabAuto {
    /// An automation path (a directory or .lua file)
    Path(String),
}

/// Returns the screen number (1-based) of the tab in a desk whose id
/// (or name, if no id) matches. Used to resolve discussion participants/referee
/// from a tab id to a screen number.
pub fn surface_of_id(desk: &config::Desk, id: &str) -> Option<usize> {
    let mut pane = 0;
    for t in &desk.tabs {
        if t.cfg.command.argv().is_empty() {
            continue;
        }
        pane += 1;
        // The id, and not the name on screen: see hooks::TabKey
        if t.cfg.id.as_deref() == Some(id) {
            return Some(pane);
        }
    }
    None
}

pub fn automation_by_pane(desk: &config::Desk) -> Vec<(usize, TabAuto)> {
    let mut pane = 0;
    let mut out = Vec::new();
    for t in &desk.tabs {
        // A row with an empty command doesn't show up on screen either
        if t.cfg.command.argv().is_empty() {
            continue;
        }
        pane += 1;
        if let Some(p) = t.cfg.automation_path() {
            out.push((pane, TabAuto::Path(p)));
        }
    }
    out
}

/// Converts a rebuilt tab config into TabOptions.
/// For a `model <provider>/<model>` tab, loads the resolved connection info into opts.
/// A discussion participant also gets its persona attached (so the stateless
/// bridge doesn't forget its stance). argv is left as-is for identification
/// (the spawn side swaps it for the waiting process). A regular tab passes through untouched.
pub fn resolve_launch(
    argv: Vec<String>,
    opts: &mut tab::TabOptions,
    desk: Option<&config::Desk>,
    cfg: &config::TabConfig,
) -> Vec<String> {
    let id = cfg.id.as_deref();
    // A terminal on another machine. What it is *called* -- the desk and
    // the tab -- is what its password is filed under, so the name is worked
    // out here, where both are known, and never written into the settings
    if let Some((host, port, user)) = config::ssh_endpoint(&argv) {
        let under = |what: &str| {
            let (w, t) = (desk.map(|w| w.id.as_str())?, id?);
            Some(format!("ssh/{w}/{t}/{what}"))
        };
        opts.remote = Some(server_spec(&host, port, &user, cfg.server.as_ref(), &under));
    }
    if let Some(mut conn) = bridge::launch_for(&argv) {
        if let (Some(d), Some(id)) = (desk.and_then(|w| w.discuss.as_ref()), id) {
            conn.persona = d.personas.get(id).filter(|p| !p.trim().is_empty()).cloned();
        }
        // Whether this model is a browser brain is not decided here. It is
        // decided by what it is aimed at, which can change while it runs
        // (see Tab::set_brain)
        opts.model = Some(conn);
    }
    // Who a git typed in this tab signs in as. Worked out here because this is
    // where the desk is known: the account is the desk's, and the choice is
    // this tab's own or its folder's project's
    if let Some(w) = desk {
        opts.git = w.git_use_here(cfg.git_account.as_deref(), opts.cwd.as_deref());
    }
    argv
}

/// Where it runs comes from the tab's group, the only thing that has a folder.
///
/// And whether that folder is on this machine at all, which is asked here
/// rather than at each launch site: a tab whose folder is not here is held
/// back instead of started, and there is more than one place tabs are started
/// from. One answer, so the two cannot disagree.
pub fn tab_options(cfg: &config::TabConfig, folder: Option<&config::Folder>) -> tab::TabOptions {
    let cwd = folder.and_then(|f| f.cwd.clone());
    // A folder on another machine is not missing from this one: it was never
    // meant to be here. Asking this machine whether that path exists would
    // hold every one of those tabs back for a reason that is not true
    let elsewhere = folder.and_then(|f| f.host.as_ref());
    let held = match elsewhere {
        Some(_) => None,
        None => tab::Held::of(cwd.as_deref()),
    };
    tab::TabOptions {
        cwd,
        group: folder.and_then(|f| f.name.clone()),
        protect: folder.map(|f| f.protect.clone()).unwrap_or_default(),
        // Known before the process starts, because the key it calls home
        // with is minted under this very name
        id: cfg.id.clone(),
        scrollback: cfg.scrollback.unwrap_or(tab::SCROLLBACK_LINES),
        encoding: tab::TabOptions::encoding_from_name(cfg.encoding.as_deref()),
        log: cfg.log,
        model: None,
        held,
        // A folder that lives on another machine makes every tab in it a
        // terminal on that machine, whatever the command says. Settled here
        // rather than at each launch site, the same as the hold above.
        // A tab whose own command is an ssh address still wins: that is
        // somebody naming a machine for that tab, and the folder does not
        // overrule it (resolve_launch fills this in after)
        remote: elsewhere
            .filter(|h| !h.is_made())
            .and_then(|h| crate::config::host_spec(h).ok()),
        // A machine that has to be made has no address to put above, so it
        // travels as itself and is asked for when a tab actually starts
        cloud: elsewhere.filter(|h| h.is_made()).cloned(),
        // Where on that machine. Sent once the shell is up, because a shell
        // over there starts where the far end puts it and there is nowhere to
        // pass a folder in the asking
        remote_cwd: elsewhere.and(cwd_string(folder)),
        // Filled in by `resolve_launch`, which is where the desk holding the
        // accounts is known
        git: crate::config::GitUse::Unset,
        // Filled in where the launch is planned: whether this tab is starting
        // clean although something was written down for it is not a thing the
        // folder knows
        lost: false,
    }
}

#[cfg(test)]
mod calling_home_tests {
    use super::*;

    /// A tab answers to one name, and both halves have to use it.
    ///
    /// The key a tab's process calls home with is minted under one name, and
    /// every call that arrives is looked up by another. They were not the same
    /// name: the key was minted from the title on screen, and the lookup is by
    /// the tab's own id. So a tab called one thing and named another had its
    /// hooks read, run, delivered -- and then thrown away as "no such tab",
    /// with the state falling back to reading the screen and the conversation
    /// id lost entirely. Nothing said so; the dot simply moved a little later
    /// and a restart came back to nothing
    #[test]
    fn a_tab_calls_home_under_the_name_it_is_looked_up_by() {
        let json = r#"{
          "desks": [ { "name":"w", "id":"w",
            "folders": [ {"name":"here","cwd":".",
              "tabs": [ {"id":"gem","name":"Gemini","command":"sh"} ]} ] } ]
        }"#;
        let cfg: config::Config = serde_json::from_str(json).expect("the settings cannot be read");
        let (desks, errs) = cfg.resolve_desks();
        assert!(errs.is_empty(), "{errs:?}");
        let desk = desks.first().expect("there is no desk");
        let ft = desk.tabs.first().expect("there is no tab");
        let opts = tab_options(&ft.cfg, desk.folder_of(ft));

        // The name the key is minted under
        assert_eq!(opts.called("Gemini"), "gem", "it makes the key from the name on screen");
        // ...is the one a call is looked up by
        let key = hooks::TabKey { id: opts.id.clone() };
        assert!(key.matches(opts.called("Gemini")), "it cannot be looked up by the name it was made under");
    }

    /// A terminal knows which account a git typed in it signs in as.
    ///
    /// The choice is the project's, made once beside its folders, and a tab
    /// that made its own outranks it. Worked out at launch, where the desk
    /// holding the accounts is known -- the tab itself only carries the name,
    /// never the token
    #[test]
    fn a_terminal_is_born_knowing_which_account_to_sign_in_as() {
        let json = r#"{
          "desks": [ { "name":"w", "id":"w",
            "git_accounts": [ {"name":"work","login":"octocat"}, {"name":"home"} ],
            "projects": [ {"name":"p","git_account":"work"} ],
            "folders": [ {"name":"here","cwd":".","project":"p",
              "tabs": [ {"id":"sh","name":"Shell","command":"sh"},
                        {"id":"own","name":"Own","command":"sh","git_account":"home"} ]} ] } ]
        }"#;
        let cfg: config::Config = serde_json::from_str(json).expect("the settings cannot be read");
        let (desks, errs) = cfg.resolve_desks();
        assert!(errs.is_empty(), "{errs:?}");
        let desk = desks.first().expect("there is no desk");
        let of = |i: usize| {
            let ft = &desk.tabs[i];
            let mut opts = tab_options(&ft.cfg, desk.folder_of(ft));
            resolve_launch(ft.cfg.command.argv(), &mut opts, Some(desk), &ft.cfg);
            opts.git
        };
        assert_eq!(of(0).written(), "work", "the project's choice did not reach its terminal");
        assert_eq!(of(1).written(), "home", "the tab's own choice was overruled");
    }

    /// A folder renamed while its tabs run shows the new name for good.
    ///
    /// The heading over a folder is read off its running tabs, and the reload
    /// that follows a rename kept each of them with the name it launched under.
    /// The list held the new name for its few seconds and then went back to the
    /// old one, for as long as the tabs ran. Nor is a tab restarted for it: a
    /// heading is not a launch condition
    #[test]
    fn a_folder_renamed_while_its_tabs_run_keeps_the_new_name() {
        let desk_named = |name: &str| {
            let json = serde_json::json!({"desks": [{"name":"w", "id":"w",
                "folders": [{"name": name, "cwd": std::env::temp_dir().display().to_string(),
                    "tabs": [{"name":"sh", "command": crate::test_shell()}]}]}]});
            let cfg: config::Config = serde_json::from_value(json).expect("the settings cannot be read");
            let (mut desks, errs) = cfg.resolve_desks();
            assert!(errs.is_empty(), "{errs:?}");
            desks.remove(0)
        };
        let (mut tabs, mut errors, mut resume) = (Vec::new(), Vec::new(), Default::default());
        apply_ws_config(&mut tabs, &desk_named("before"), 10, 40, &mut errors, &mut resume, None);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(tabs[0].group_name(), Some("before"));

        apply_ws_config(&mut tabs, &desk_named("作業フォルダ"), 10, 40, &mut errors, &mut resume, None);
        assert_eq!(tabs[0].group_name(), Some("作業フォルダ"), "the running tab kept the name it launched under");
        assert!(!tabs[0].needs_restart, "a new heading restarts the tab");
        for t in &mut tabs {
            t.kill();
        }
    }

    /// Saving the settings does not throw away what a tab was saying.
    ///
    /// Two roads start tabs, and only one of them used to be told what the
    /// tabs were having: a desk being opened asked, a settings change did not.
    /// So a tab the settings started came up knowing nothing, and what the app
    /// went on remembering for it was the empty conversation it had just been
    /// handed -- which cannot be resumed, so the real one was gone for good
    #[test]
    fn a_tab_the_settings_start_knows_the_conversation_it_was_having() {
        let here = std::env::temp_dir().display().to_string();
        let json = serde_json::json!({"desks": [{"name":"w", "id":"w",
            "folders": [{"cwd": here.clone(),
                "tabs": [{"id":"agent", "name":"sh", "command": crate::test_shell()}]}]}]});
        let cfg: config::Config = serde_json::from_value(json).expect("the settings cannot be read");
        let (desks, errs) = cfg.resolve_desks();
        assert!(errs.is_empty(), "{errs:?}");
        let desk = &desks[0];
        let last = crate::lastsession::Saved {
            version: 1,
            desks: vec![crate::lastsession::SavedWs {
                name: "w".into(),
                id: Some("w".into()),
                panes: None,
                tabs: vec![crate::lastsession::SavedTab {
                    title: "sh".into(),
                    id: Some("agent".into()),
                    cwd: Some(here),
                    program: crate::test_shell(),
                    session: "what-it-was-saying".into(),
                    source: "Minted".into(),
                }],
            }],
        };
        let (mut tabs, mut errors, mut resume) = (Vec::new(), Vec::new(), Default::default());
        apply_ws_config(&mut tabs, desk, 10, 40, &mut errors, &mut resume, Some(&last));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            tabs[0].previous.as_ref().map(|s| s.id.as_str()),
            Some("what-it-was-saying"),
            "a tab the settings started was never told what it was having"
        );
        for t in &mut tabs {
            t.kill();
        }
    }

    /// A tab nobody named is known by what it says on it, and that still has
    /// to be one name rather than two
    #[test]
    fn a_tab_with_no_name_of_its_own_answers_to_its_title() {
        let plain = tab::TabOptions::default();
        assert_eq!(plain.called("SHELL"), "SHELL");
        // An empty one in the settings is nobody's name, not a name of ""
        let blank = tab::TabOptions { id: Some(String::new()), ..Default::default() };
        assert_eq!(blank.called("SHELL"), "SHELL");
    }
}

#[cfg(test)]
mod remote_folder_tests {
    use super::*;

    /// A machine that has to be made is not a machine with an address. Sending
    /// it down the ssh road would ask this program to connect to nothing --
    /// the entry has no `at` to connect to -- so the tab carries the settings
    /// entry instead, and the sandbox is asked for when it actually starts
    #[test]
    fn a_machine_that_must_be_made_is_not_an_address() {
        let json = r#"{
          "hosts": [ {"name":"cloud","at":"","kind":"e2b","project":"/home/user/p"} ],
          "desks": [ { "name":"w",
            "folders": [ {"name":"out there","cwd":"/home/user/p","host":"cloud"} ],
            "tabs": [ {"name":"there","command":"sh","group":0} ] } ]
        }"#;
        let cfg: config::Config = serde_json::from_str(json).expect("the settings cannot be read");
        let (desks, errs) = cfg.resolve_desks();
        assert!(errs.is_empty(), "{errs:?}");
        let desk = desks.first().expect("there is no desk");
        let ft = desk.tabs.first().expect("there is no tab");
        let opts = tab_options(&ft.cfg, desk.folder_of(ft));
        assert!(opts.remote.is_none(), "a machine with no address is treated as an address");
        assert_eq!(opts.cloud.as_ref().map(|h| h.name.as_str()), Some("cloud"));
        // Whichever kind it is, the folder is not this machine's to check
        assert!(opts.held.is_none());
        assert_eq!(opts.remote_cwd.as_deref(), Some("/home/user/p"));
    }

    /// The settings a person writes reach the tab as a terminal on that
    /// machine. Every step, from the file down: the folder's `host` is read,
    /// the name finds the machine, and the tab ends up pointed at it
    #[test]
    fn a_written_down_machine_reaches_the_tab() {
        let json = r#"{
          "hosts": [ {"name":"bench","at":"ssh://tester@127.0.0.1:2225","project":"/srv/p"} ],
          "desks": [ { "name":"w",
            "folders": [ {"name":"over there","cwd":"/srv/p/work","host":"bench"} ],
            "tabs": [ {"name":"there","command":"sh","group":0} ] } ]
        }"#;
        let cfg: config::Config = serde_json::from_str(json).expect("the settings cannot be read");
        let (desks, errs) = cfg.resolve_desks();
        assert!(errs.is_empty(), "{errs:?}");
        let desk = desks.first().expect("there is no desk");
        let folder = desk.folders.first().expect("there is no folder");
        assert!(folder.host.is_some(), "the folder did not find its machine");
        // The path is that machine's, so it is not joined to anything here
        assert_eq!(folder.cwd.as_deref(), Some(std::path::Path::new("/srv/p/work")));
        let ft = desk.tabs.first().expect("there is no tab");
        let opts = tab_options(&ft.cfg, desk.folder_of(ft));
        assert!(opts.remote.is_some(), "the tab is not a terminal on that machine");
        assert_eq!(opts.remote_cwd.as_deref(), Some("/srv/p/work"));
        assert!(opts.held.is_none());
    }

    /// A folder on another machine makes its tabs terminals on that machine,
    /// and is not held back for not being here.
    ///
    /// Both halves matter. Without the first the tab starts a shell on this
    /// machine in a path that means nothing here; without the second every one
    /// of them is held back saying "that folder is missing", which is true of
    /// this machine and beside the point
    #[test]
    fn a_folder_somewhere_else_opens_its_tabs_there() {
        let host = config::HostSpec {
            name: "bench".into(),
            at: "ssh://me@example.test:22".into(),
            ..Default::default()
        };
        let there = config::Folder {
            name: Some("api".into()),
            id: None,
            host: Some(host),
            cwd: Some(std::path::PathBuf::from("/srv/api/work")),
            source: Default::default(),
            protect: Vec::new(),
            project: None,
            work_item: None,
            summary: None,
            auto_label: false,
            drawn: None,
        };
        let cfg = config::TabConfig::default();
        let opts = tab_options(&cfg, Some(&there));
        let spec = opts.remote.as_ref().expect("it is not a terminal on that machine");
        assert_eq!(spec.host, "example.test");
        assert_eq!(spec.user, "me");
        assert_eq!(opts.remote_cwd.as_deref(), Some("/srv/api/work"), "it cannot say where it will stand");
        assert!(opts.held.is_none(), "it holds it back because it is not here");

        // A folder on this machine is what it always was
        let here = config::Folder { host: None, ..there.clone() };
        let mine = tab_options(&cfg, Some(&here));
        assert!(mine.remote.is_none());
        assert!(mine.remote_cwd.is_none());
    }
}

/// The folder, as the far end spells a path. Kept as written rather than as a
/// local path, because a server's paths are not this machine's
fn cwd_string(folder: Option<&config::Folder>) -> Option<String> {
    let at = folder?.cwd.as_ref()?.display().to_string();
    (!at.trim().is_empty()).then_some(at)
}

/// Launches the tabs for a desk (called on first activation)
/// Opens the browsers declared in config.
///
/// If one fails to open, the rest still run. A browser failing to launch is
/// never a reason to stop the whole desk.
pub fn open_declared_browsers(desk: &config::Desk, caps: &hooks::Caps, errors: &mut Vec<String>) {
    // Don't touch ones already open. Reopening them would restart the page from
    // scratch, wiping out whatever the user was looking at every time settings are saved.
    let open_now = caps.hosted_names();
    let already = |name: &str| open_now.iter().any(|n| n == name);
    for b in &desk.browsers {
        if already(&b.id) {
            caps.note_declared(&b.id);
            continue;
        }
        let profile = shikisha_shared::BrowserProfile::new(
            b.browser_profile.as_deref().unwrap_or_default(),
            b.private,
        )
        .calling_itself(b.user_agent.clone());
        match caps.browser_open(&b.id, &b.url, profile) {
            Ok(()) => caps.note_declared(&b.id),
            Err(e) => errors.push(crate::i18n::tp(
                "err.desk.browser_open",
                &[("id", &b.id), ("e", &format!("{e:#}"))],
            )),
        }
    }
    // A tab written as "browser https://..." gets the same treatment.
    // The name automation addresses it by is that tab's ID (or display name if no ID)
    for ft in &desk.tabs {
        let argv = ft.cfg.command.argv();
        let Some(url) = config::browser_url_of(&argv) else {
            continue;
        };
        let name = ft
            .cfg
            .id
            .clone()
            .or_else(|| ft.cfg.name.clone())
            .unwrap_or_else(|| "browser".into());
        if !already(&name) {
            let profile = shikisha_shared::BrowserProfile::new(
                ft.cfg.browser_profile.as_deref().unwrap_or_default(),
                ft.cfg.private,
            )
            .calling_itself(ft.cfg.user_agent.clone());
            if let Err(e) = caps.browser_open(&name, &url, profile) {
                errors.push(crate::i18n::tp(
                    "err.desk.browser_open",
                    &[("id", &name), ("e", &format!("{e:#}"))],
                ));
                continue;
            }
        }
        caps.note_declared(&name);
    }
    apply_browser_chrome(desk, caps);
}

/// How a tab is starting: on the conversation it was having, or clean -- and
/// whether starting clean cost it one.
#[derive(Debug, Clone, PartialEq)]
pub struct Carried {
    pub plan: tab::Resume,
    /// Whether a conversation that was written down for this tab is not being
    /// brought back. The tab comes up looking exactly like every other clean
    /// tab, so this is what the caption holds its offer of the way back open
    /// for -- past the first thing the person types, which is usually when
    /// they notice
    pub lost: bool,
}

impl Carried {
    fn fresh() -> Self {
        Self { plan: tab::Resume::Fresh, lost: false }
    }
    fn lost(why: String, title: &str) -> Self {
        append_hook_log(&format!("\"{title}\" starts clean: {why}"));
        Self { plan: tab::Resume::Fresh, lost: true }
    }
}

/// The conversation a tab should be launched back into, if there is one.
///
/// Three things all have to hold, and one of them failing is ordinary rather
/// than exceptional: a tab that is new since last time has nothing to come
/// back to, and saying so on every launch would be noise. The others are not
/// ordinary and are written down, because each leaves a tab looking exactly
/// like every other fresh one while something has gone:
///
/// * the record of the conversation it was having is no longer on this machine
/// * this desk remembers conversations for this CLI in this folder, and none
///   of them could be told to be this tab's
/// * this tab was told to start clean although its CLI could have carried one
///
/// The tab is recognised by the same things `lastsession` writes down, in the
/// same spelling: the program is `argv[0]` and the folder is the resolved
/// `cwd`, exactly as a running tab would report them.
pub fn carried_conversation(
    carry: Option<&crate::lastsession::Saved>,
    desk: &config::Desk,
    argv: &[String],
    cfg: &config::TabConfig,
    cwd: &Option<std::path::PathBuf>,
    title: &str,
) -> Carried {
    // Whether any of this is worth a word at all. A shell has no conversation
    // to lose, and "this one starts clean" said about one is a sentence about
    // something that was never there
    let carries = tab::carries_conversations(argv, &cfg.profile);
    if cfg.restore_conversation == Some(false) {
        if carries {
            append_hook_log(&format!("\"{title}\" starts clean: its settings say so"));
        }
        return Carried::fresh();
    }
    let Some(saved) = carry else {
        return Carried::fresh();
    };
    let cwd = cwd.as_ref().map(|c| c.display().to_string());
    let program = argv.first().map(String::as_str).unwrap_or_default();
    let Some(session) =
        saved.conversation_of(desk, program, cwd.as_deref(), cfg.id.as_deref(), title)
    else {
        // Nothing was remembered for this tab. Ordinary when nothing was
        // remembered in this folder either -- but when something was, a
        // conversation that exists has just stopped belonging to anybody, and
        // the next start would remember the empty one this tab opens instead
        let near = saved.remembered_here(desk, program, cwd.as_deref());
        if near == 0 || !carries {
            return Carried::fresh();
        }
        return Carried::lost(
            format!(
                "this desk remembers {near} conversation(s) for {program} in this folder \
                 and nothing says which is this tab's"
            ),
            title,
        );
    };
    match tab::resumable(argv, &cfg.profile, &session.id) {
        true => Carried { plan: tab::Resume::Id(session), lost: false },
        false => Carried::lost(
            format!("{} is not on this computer any more", session.short()),
            title,
        ),
    }
}

/// The conversation a tab is launched into, whichever road starts it.
///
/// Two roads start tabs -- the settings being read (`apply_ws_config`) and a
/// desk being opened (`spawn_desk`) -- and a tab must begin the same way down
/// either. A conversation the settings name outranks the one the tab was
/// having: it was chosen for this tab deliberately, and "what this tab was
/// saying last time" is not an answer to that
fn launch_plan(
    carry: Option<&crate::lastsession::Saved>,
    desk: &config::Desk,
    argv: &[String],
    cfg: &config::TabConfig,
    cwd: &Option<std::path::PathBuf>,
    title: &str,
) -> Carried {
    let carried = match resume_plan_of(cfg.resume.as_deref()) {
        named @ tab::Resume::Id(_) => Carried { plan: named, lost: false },
        _ => carried_conversation(carry, desk, argv, cfg, cwd, title),
    };
    if let tab::Resume::Id(s) = &carried.plan {
        append_hook_log(&format!("launching \"{title}\" carrying {}", s.short()));
    }
    carried
}

#[cfg(test)]
mod keeping_running_tabs_tests {
    use super::*;

    /// Folders that exist, so that nothing is held back for being missing
    fn folders(test: &str, names: &[&str]) -> Vec<std::path::PathBuf> {
        names
            .iter()
            .map(|n| {
                let p = std::env::temp_dir().join(format!("shikisha-{test}-{}", std::process::id())).join(n);
                std::fs::create_dir_all(&p).expect("the folder could not be made");
                p
            })
            .collect()
    }

    /// One desk, with a tab called "claude" in each of these folders -- what
    /// cutting worktrees from a folder makes
    fn desk_of(id: &str, name: &str, dirs: &[&std::path::PathBuf]) -> config::Desk {
        let folders: Vec<_> = dirs
            .iter()
            .map(|d| {
                serde_json::json!({"cwd": d.display().to_string(),
                    "tabs": [{"name": "claude", "command": crate::test_shell()}]})
            })
            .collect();
        let json = serde_json::json!({"desks": [{"name": name, "id": id, "folders": folders}]});
        let cfg: config::Config = serde_json::from_value(json).expect("the settings cannot be read");
        let (mut desks, errs) = cfg.resolve_desks();
        assert!(errs.is_empty(), "{errs:?}");
        desks.remove(0)
    }

    fn start(desk: &config::Desk) -> Vec<Tab> {
        let (mut tabs, mut errors) = (Vec::new(), Vec::new());
        apply_ws_config(&mut tabs, desk, 10, 40, &mut errors, &mut Default::default(), None);
        assert!(errors.is_empty(), "{errors:?}");
        tabs
    }

    fn same_process(a: &Tab, b: &tab::SharedParser) -> bool {
        std::sync::Arc::ptr_eq(&a.parser, b)
    }

    fn stop_all(tabs: &mut [Tab]) {
        tabs.iter_mut().for_each(Tab::kill);
    }

    /// Deleting one worktree does not stop the AI at work in another.
    ///
    /// Every worktree copies its folder's tabs, so one desk had a "claude" in
    /// each. The reload that follows the deletion matched running tabs to the
    /// settings by name alone: the deleted folder's tab, first in the list,
    /// was taken as the other folder's, restarted there once idle as a new
    /// conversation, and the tab that was actually at work was stopped
    #[test]
    fn deleting_one_worktree_leaves_the_ai_in_another_running() {
        let dirs = folders("keep-worktree", &["gone", "busy"]);
        for gone_first in [true, false] {
            let (gone, busy) = (&dirs[0], &dirs[1]);
            let listed = match gone_first {
                true => vec![gone, busy],
                false => vec![busy, gone],
            };
            let mut tabs = start(&desk_of("w", "w", &listed));
            assert_eq!(tabs.len(), 2);
            let at_work = tabs
                .iter()
                .find(|t| t.cwd().is_some_and(|c| crate::uistate::same_folder(c, busy)))
                .map(|t| t.parser.clone())
                .expect("no tab works in the folder that stays");

            let (mut errors, mut resume) = (Vec::new(), Default::default());
            let msg = apply_ws_config(&mut tabs, &desk_of("w", "w", &[busy]), 10, 40, &mut errors, &mut resume, None);

            assert_eq!(tabs.len(), 1, "{msg}");
            assert!(same_process(&tabs[0], &at_work), "the AI at work was stopped (deleted folder listed first: {gone_first})");
            assert!(!tabs[0].needs_restart, "it is set to start again as a new conversation");
            assert!(!tabs[0].exited());
            assert!(msg.contains("stopped 1"), "the deleted folder's tab is not stopped: {msg}");
            stop_all(&mut tabs);
        }
    }

    /// A folder given a new path in the settings keeps its tab, and moves it
    /// once it is idle -- the one case a tab is taken across places, and only
    /// because no line stands where it was any more
    #[test]
    fn a_folder_moved_in_the_settings_keeps_its_tab_until_it_can_restart() {
        let dirs = folders("keep-moved", &["old", "new", "other"]);
        let mut tabs = start(&desk_of("w", "w", &[&dirs[0], &dirs[2]]));
        let moved = tabs[0].parser.clone();
        let other = tabs[1].parser.clone();

        let (mut errors, mut resume) = (Vec::new(), Default::default());
        apply_ws_config(&mut tabs, &desk_of("w", "w", &[&dirs[1], &dirs[2]]), 10, 40, &mut errors, &mut resume, None);

        assert_eq!(tabs.len(), 2);
        assert!(same_process(&tabs[0], &moved), "the moved folder's tab was replaced at once");
        assert!(tabs[0].needs_restart, "it never moves to the new folder");
        assert!(same_process(&tabs[1], &other) && !tabs[1].needs_restart, "the folder left alone was touched");
        stop_all(&mut tabs);
    }

    /// Deleting the desk on screen does not hand its tabs to the next desk,
    /// and does not stop that desk's own.
    ///
    /// What happened: the desk that had the AI at work was in the background,
    /// renamed in the same save that deleted the desk on screen. The renamed
    /// desk was taken for a deleted one (it was looked up by name) and its AI
    /// was stopped; the desk that took the screen was read onto the deleted
    /// desk's tabs and started again from nothing
    #[test]
    fn deleting_the_desk_on_screen_while_another_is_renamed_keeps_that_ones_ai() {
        let dirs = folders("keep-desks", &["work", "scratch"]);
        let before = vec![desk_of("default", "DEFAULT", &[&dirs[0]]), desk_of("space", "ワークスペース", &[&dirs[1]])];
        let mut parked = vec![start(&before[0]), Vec::new()];
        let at_work = parked[0][0].parser.clone();
        // On screen: the second desk
        let mut tabs = start(&before[1]);

        let after = vec![desk_of("default", "ワイアード＆エコ", &[&dirs[0]])];
        let viewed = reseat_desks(&before, &after, 1, &mut tabs, &mut parked);

        assert_eq!(viewed, None, "the deleted desk is still found");
        assert_eq!(tabs.len(), 1);
        assert!(same_process(&tabs[0], &at_work), "the renamed desk's AI was stopped or replaced");
        assert!(!tabs[0].exited());
        assert_eq!(parked.len(), 1);
        assert!(parked[0].is_empty(), "the tabs on screen are also left parked");
        stop_all(&mut tabs);
    }

    /// A background desk renamed, or moved in the list, keeps running
    #[test]
    fn a_background_desk_renamed_or_moved_keeps_its_tabs() {
        let dirs = folders("keep-renamed", &["a", "b"]);
        let before = vec![desk_of("a", "A", &[&dirs[0]]), desk_of("b", "B", &[&dirs[1]])];
        let mut tabs = start(&before[0]);
        let mut parked = vec![Vec::new(), start(&before[1])];
        let at_work = parked[1][0].parser.clone();

        // B renamed and moved to the front, a new desk named like nothing
        let after = vec![
            desk_of("b", "B renamed", &[&dirs[1]]),
            desk_of("c", "C", &[&dirs[1]]),
            desk_of("a", "A", &[&dirs[0]]),
        ];
        let viewed = reseat_desks(&before, &after, 0, &mut tabs, &mut parked);

        assert_eq!(viewed, Some(2), "the desk on screen is not found where it went");
        assert_eq!(parked.len(), 3);
        assert!(parked[0].first().is_some_and(|t| same_process(t, &at_work)), "the renamed desk lost its tabs");
        assert!(parked[1].is_empty() && parked[2].is_empty());
        assert!(!parked[0][0].exited());
        stop_all(&mut tabs);
        parked.iter_mut().for_each(|p| stop_all(p));
    }
}
