//! Opening a workspace: the tabs it names, the automation it carries, and
//! moving between them.
//!
//! What a workspace *is* -- which tabs exist, what each one runs, which folder
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
            Surface::Git { key, dir: Some(d), protect, .. } => Some(hooks::TabPlace {
                key: hooks::TabKey { id: Some(key.clone()) },
                dir: d.clone(),
                // A git panel reports on a folder on this machine, and is not a
                // place files can be sent to
                remote: None,
                protect: protect.clone(),
            }),
            // A file panel is. `sftp_put("その呼び名", …)` reaches the same
            // server the screen is showing, which is the whole point of the
            // panel being a tab rather than a window of its own
            Surface::Sftp { key, dir, spec, .. } => Some(hooks::TabPlace {
                key: hooks::TabKey { id: Some(key.clone()) },
                dir: dir.clone().unwrap_or_default(),
                remote: spec.clone(),
                protect: Vec::new(),
            }),
            _ => None,
        })
        .collect()
}

pub fn build_engine(
    cfg: Option<&config::Config>,
    ws: Option<&config::Workspace>,
    errors: &mut Vec<String>,
    caps: &hooks::Caps,
) -> Option<HookEngine> {
    let base = cfg.and_then(|c| c.automation_path());
    let ws_lua = ws.and_then(|w| w.automation.clone());
    let tab_luas: Vec<(usize, TabAuto)> = ws.map(automation_by_pane).unwrap_or_default();
    let has_discuss = ws
        .and_then(|w| w.discuss.as_ref())
        .is_some_and(|d| d.agents.len() >= 2);
    // Keep the engine even with no Lua hooks when a tab wants a completion
    // notification: the on_done detection loop only runs when an engine exists,
    // so without this a notify-only workspace would never fire on_done.
    let wants_notify = ws
        .map(|w| w.tabs.iter().any(|t| t.cfg.notify_on_done.is_some()))
        .unwrap_or(false);
    // A Lua quick-action needs an engine to run in, even when nothing else does.
    let has_lua_actions = cfg
        .map(|c| c.actions.iter().any(|a| a.lua))
        .unwrap_or(false);
    if base.is_none()
        && ws_lua.is_none()
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
    if let Some(p) = &base {
        if let Some(id) = load(&mut engine, p, errors) {
            engine.set_base(id);
        }
    }
    if let Some(p) = &ws_lua {
        if let Some(id) = load(&mut engine, p, errors) {
            engine.set_workspace(id);
        }
    }
    // The referee (stop conditions) is per-workspace. Passed to the built-in commander as a Lua table.
    let stops_lua = ws
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
    // AI-vs-AI discussion: if the workspace has `discuss`, load the built-in discussion commander into each participant tab
    if let Some(w) = ws {
        if let Some(d) = &w.discuss {
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
                            "err.ws.discuss_tab_missing",
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
                            "err.ws.discuss_agent_failed",
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
                                "err.ws.discuss_judge_failed",
                                &[("j", j), ("e", &format!("{e:#}"))],
                            )),
                        },
                        None => errors.push(crate::i18n::tp(
                            "err.ws.discuss_judge_missing",
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
                                "err.ws.discuss_moderator_failed",
                                &[("m", m), ("e", &format!("{e:#}"))],
                            )),
                        },
                        None => errors.push(crate::i18n::tp(
                            "err.ws.discuss_moderator_missing",
                            &[("m", m)],
                        )),
                    }
                }
            } else if !d.agents.is_empty() {
                errors.push(crate::i18n::t("err.ws.discuss_needs_two"));
            }
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
pub fn apply_ws_config(
    tabs: &mut Vec<Tab>,
    ws: &config::Workspace,
    rows: u16,
    cols: u16,
    errors: &mut Vec<String>,
) -> String {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut staged = 0usize;

    // Close tabs no longer in config (removed via the GUI = an explicit instruction)
    let wanted: Vec<String> = ws
        .tabs
        .iter()
        .map(|f| {
            f.cfg
                .name
                .clone()
                .unwrap_or_else(|| title_of(&f.cfg.command.argv()))
        })
        .collect();
    tabs.retain_mut(|t| {
        if wanted.contains(&t.title) {
            true
        } else {
            t.kill();
            removed += 1;
            false
        }
    });

    // Update existing tabs and add new ones
    let mut ordered: Vec<Tab> = Vec::with_capacity(ws.tabs.len());
    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        if argv.is_empty() {
            continue;
        }
        // Browsers aren't child processes, so don't launch them here
        // (open_declared_browsers opens the window)
        if config::browser_url_of(&argv).is_some()
            || config::is_git_panel(&argv)
            || config::is_sftp_panel(&argv)
        {
            continue;
        }
        let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
        let mut opts = tab_options(&ft.cfg, ws.folder_of(ft));
        let argv = resolve_launch(argv, &mut opts, Some(ws), &ft.cfg);
        let cwd = opts.cwd.clone();
        match tabs.iter().position(|t| t.title == title) {
            Some(i) => {
                let mut t = tabs.remove(i);
                t.apply_live_config(
                    ft.cfg.profile.clone(),
                    ft.cfg.locked,
                    ft.cfg.auto_restart,
                    ft.depth,
                    ft.cfg.notify_on_done.clone(),
                    opts.protect.clone(),
                );
                // Changes to command, encoding, or line count require a rebuild
                if t.signature() != tab::signature_of(&argv, &opts) {
                    t.stage_restart_config(argv.clone(), opts);
                    staged += 1;
                }
                ordered.push(t);
            }
            None => match Tab::spawn_as(
                title.clone(),
                &argv,
                ft.cfg.profile.clone(),
                rows,
                cols,
                opts,
                resume_plan_of(ft.cfg.resume.as_deref()),
            ) {
                Ok(mut t) => {
                    t.locked = ft.cfg.locked;
                    t.auto_restart = ft.cfg.auto_restart;
                    t.depth = ft.depth;
                    t.id = ft.cfg.id.clone();
                    t.notify_on_done = ft.cfg.notify_on_done.clone();
                    t.notify_reply = ft.cfg.notify_reply;
                    ordered.push(t);
                    added += 1;
                }
                Err(e) => errors.push(tab::launch_problem(
                    &title,
                    argv.first().map(String::as_str).unwrap_or(""),
                    cwd.as_deref(),
                    &e.to_string(),
                )),
            },
        }
    }
    // Close whatever's left that isn't in config
    for mut t in tabs.drain(..) {
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

/// Redraws a page's top bar and bottom band to match config.
///
/// Runs not just on open, but also whenever config is reloaded.
/// Without going through here, checking a box wouldn't show up until a restart.
pub fn apply_browser_chrome(ws: &config::Workspace, caps: &hooks::Caps) {
    // Close browsers that dropped out of config. Leaving them in place would
    // make them reappear at the back of the list as a "page not in config".
    let declared: Vec<String> = ws
        .browsers
        .iter()
        .map(|b| b.id.clone())
        .chain(ws.tabs.iter().filter_map(|ft| {
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

    for ft in &ws.tabs {
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

/// Launch every tab a workspace declares.
///
/// `carry` is what was on screen when the app last closed; whether a given tab
/// actually comes back to it is that tab's own setting. A tab the Vault
/// reopened names its own conversation and outranks both: that id was chosen
/// deliberately, a moment ago, and "what this tab was saying last time" is not
/// an answer to it
pub fn spawn_workspace(
    ws: &config::Workspace,
    rows: u16,
    cols: u16,
    tabs: &mut Vec<Tab>,
    errors: &mut Vec<String>,
    carry: Option<&crate::lastsession::Saved>,
) {
    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        if argv.is_empty() {
            continue;
        }
        // Browsers aren't child processes; they're just pages placed inside the
        // window. Trying to launch one here would produce a baffling "no
        // executable named browser" failure every time, out of nowhere.
        // (open_declared_browsers opens them)
        if config::browser_url_of(&argv).is_some()
            || config::is_git_panel(&argv)
            || config::is_sftp_panel(&argv)
        {
            continue;
        }
        let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
        let mut opts = tab_options(&ft.cfg, ws.folder_of(ft));
        let argv = resolve_launch(argv, &mut opts, Some(ws), &ft.cfg);
        let cwd = opts.cwd.clone();
        let plan = match resume_plan_of(ft.cfg.resume.as_deref()) {
            named @ tab::Resume::Id(_) => named,
            _ => carried_conversation(carry, ws, &argv, &ft.cfg, &cwd, &title),
        };
        if let tab::Resume::Id(s) = &plan {
            append_hook_log(&format!("launching \"{title}\" carrying {}", s.short()));
        }
        match Tab::spawn_as(
            title.clone(),
            &argv,
            ft.cfg.profile.clone(),
            rows,
            cols,
            opts,
            plan,
        ) {
            Ok(mut tab) => {
                tab.locked = ft.cfg.locked;
                tab.auto_restart = ft.cfg.auto_restart;
                tab.depth = ft.depth;
                tab.id = ft.cfg.id.clone();
                tab.notify_on_done = ft.cfg.notify_on_done.clone();
                tab.notify_reply = ft.cfg.notify_reply;
                tabs.push(tab);
            }
            Err(e) => errors.push(tab::launch_problem(
                &title,
                argv.first().map(String::as_str).unwrap_or(""),
                cwd.as_deref(),
                &e.to_string(),
            )),
        }
    }
}

/// Switches workspaces (virtual-desktop model).
/// Switching means hiding, not stopping — tabs that go into the background keep running.
/// An unlaunched workspace gets its first launch right here.
#[allow(clippy::too_many_arguments)]
pub fn switch_workspace(
    to: usize,
    ws_index: &mut usize,
    tabs: &mut Vec<Tab>,
    ws_tabs: &mut [Vec<Tab>],
    workspaces: &[config::Workspace],
    active: &mut usize,
    panes: &mut crate::layout::Layout,
    ws_panes: &mut [crate::layout::Layout],
    rows: u16,
    cols: u16,
    errors: &mut Vec<String>,
    started_fired: &mut Vec<bool>,
    cfg: Option<&config::Config>,
    engine: &mut Option<HookEngine>,
    engines: &mut [Option<HookEngine>],
    caps: &hooks::Caps,
    last: &crate::lastsession::Saved,
) {
    // Guard against every backing array, not just `workspaces`: the per-workspace
    // `engines`/`ws_tabs` caches are resized on config reload, and a mismatch must
    // never index out of bounds (that would crash the whole app on switch).
    if to == *ws_index
        || to >= workspaces.len()
        || to >= engines.len()
        || to >= ws_tabs.len()
        || to >= ws_panes.len()
        || *ws_index >= ws_tabs.len()
        || *ws_index >= ws_panes.len()
    {
        return;
    }
    ws_tabs[*ws_index] = std::mem::take(tabs);
    // How a workspace is divided belongs to that workspace. Carrying one
    // layout across the switch would leave a project split into panes that
    // point at another project's tab numbers — the screen would look
    // deliberate and mean nothing.
    ws_panes[*ws_index] = panes.clone();
    // The Lua environment is kept per workspace (so shared variables survive switching)
    engines[*ws_index] = engine.take();
    *ws_index = to;
    // Ids only mean something within their own workspace.
    // Placed pages also only appear in the tab list for whichever one is currently viewed.
    caps.set_workspace(to);
    caps.set_workspace_id(&workspaces[to].id);
    config::save_last_workspace(&workspaces[to].name);
    *tabs = std::mem::take(&mut ws_tabs[to]);
    if tabs.is_empty() {
        // First visit this run, so these tabs are being launched for the first
        // time and the same question applies as at startup: come back to what
        // this workspace was saying, or start it clean
        spawn_workspace(&workspaces[to], rows, cols, tabs, errors, Some(last));
        // Whether or not it was carried, the way back is worth holding on to:
        // this is what Ctrl+B r reaches for on a tab nobody has spoken to yet
        for t in tabs.iter_mut() {
            t.previous = last.conversation_for(&workspaces[to].name, t);
        }
        open_declared_browsers(&workspaces[to], caps, errors);
    }
    *engine = match engines[to].take() {
        Some(e) => Some(e),
        None => build_engine(cfg, workspaces.get(to), errors, caps),
    };
    started_fired.clear();
    started_fired.resize(tabs.len(), false);
    *active = if tabs.is_empty() { 0 } else { 1 };
    *panes = std::mem::replace(&mut ws_panes[to], crate::layout::Layout::single(*active));
    panes.show(*active);
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

/// Loads Lua hooks across 3 tiers (base > workspace > tab).
/// Hook resolution favors "the more specific one wins", so only hooks a tab's
/// script doesn't define fall back to workspace, then base.
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

/// Returns the screen number (1-based) of the tab in a workspace whose id
/// (or name, if no id) matches. Used to resolve discussion participants/referee
/// from a tab id to a screen number.
pub fn surface_of_id(ws: &config::Workspace, id: &str) -> Option<usize> {
    let mut pane = 0;
    for t in &ws.tabs {
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

pub fn automation_by_pane(ws: &config::Workspace) -> Vec<(usize, TabAuto)> {
    let mut pane = 0;
    let mut out = Vec::new();
    for t in &ws.tabs {
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
    ws: Option<&config::Workspace>,
    cfg: &config::TabConfig,
) -> Vec<String> {
    let id = cfg.id.as_deref();
    // A terminal on another machine. What it is *called* -- the workspace and
    // the tab -- is what its password is filed under, so the name is worked
    // out here, where both are known, and never written into the settings
    if let Some((host, port, user)) = config::ssh_endpoint(&argv) {
        let under = |what: &str| {
            let (w, t) = (ws.map(|w| w.id.as_str())?, id?);
            Some(format!("ssh/{w}/{t}/{what}"))
        };
        opts.remote = Some(server_spec(&host, port, &user, cfg.server.as_ref(), &under));
    }
    if let Some(mut conn) = bridge::launch_for(&argv) {
        if let (Some(d), Some(id)) = (ws.and_then(|w| w.discuss.as_ref()), id) {
            conn.persona = d.personas.get(id).filter(|p| !p.trim().is_empty()).cloned();
        }
        // Whether this model is a browser brain is not decided here. It is
        // decided by what it is aimed at, which can change while it runs
        // (see Tab::set_brain)
        opts.model = Some(conn);
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
    let held = tab::Held::of(cwd.as_deref());
    tab::TabOptions {
        cwd,
        group: folder.and_then(|f| f.name.clone()),
        protect: folder.map(|f| f.protect.clone()).unwrap_or_default(),
        scrollback: cfg.scrollback.unwrap_or(tab::SCROLLBACK_LINES),
        encoding: tab::TabOptions::encoding_from_name(cfg.encoding.as_deref()),
        log: cfg.log,
        model: None,
        held,
        // Settled by resolve_launch, which is where a command line becomes a
        // decision about what to start
        remote: None,
    }
}

/// Launches the tabs for a workspace (called on first activation)
/// Opens the browsers declared in config.
///
/// If one fails to open, the rest still run. A browser failing to launch is
/// never a reason to stop the whole workspace.
pub fn open_declared_browsers(ws: &config::Workspace, caps: &hooks::Caps, errors: &mut Vec<String>) {
    // Don't touch ones already open. Reopening them would restart the page from
    // scratch, wiping out whatever the user was looking at every time settings are saved.
    let open_now = caps.hosted_names();
    let already = |name: &str| open_now.iter().any(|n| n == name);
    for b in &ws.browsers {
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
                "err.ws.browser_open",
                &[("id", &b.id), ("e", &format!("{e:#}"))],
            )),
        }
    }
    // A tab written as "browser https://..." gets the same treatment.
    // The name automation addresses it by is that tab's ID (or display name if no ID)
    for ft in &ws.tabs {
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
                    "err.ws.browser_open",
                    &[("id", &name), ("e", &format!("{e:#}"))],
                ));
                continue;
            }
        }
        caps.note_declared(&name);
    }
    apply_browser_chrome(ws, caps);
}

/// The conversation a tab should be launched back into, if there is one.
///
/// Three things all have to hold, and every one of them failing is ordinary
/// rather than exceptional: this tab may have been told to start clean, it may
/// be new since last time, and the conversation may have been deleted since. So
/// there is no message here — a tab that starts fresh is what a tab normally
/// does, and saying so on every launch would be noise.
///
/// The tab is recognised by the same four things `lastsession` writes down, in
/// the same spelling: the program is `argv[0]` and the folder is the resolved
/// `cwd`, exactly as a running tab would report them.
pub fn carried_conversation(
    carry: Option<&crate::lastsession::Saved>,
    ws: &config::Workspace,
    argv: &[String],
    cfg: &config::TabConfig,
    cwd: &Option<std::path::PathBuf>,
    title: &str,
) -> tab::Resume {
    if cfg.restore_conversation == Some(false) {
        return tab::Resume::Fresh;
    }
    let Some(saved) = carry else {
        return tab::Resume::Fresh;
    };
    let cwd = cwd.as_ref().map(|c| c.display().to_string());
    let Some(session) = saved.conversation_of(
        &ws.name,
        argv.first().map(String::as_str).unwrap_or_default(),
        cwd.as_deref(),
        cfg.id.as_deref(),
        title,
    ) else {
        return tab::Resume::Fresh;
    };
    match tab::resumable(argv, &cfg.profile, &session.id) {
        true => tab::Resume::Id(session),
        false => tab::Resume::Fresh,
    }
}
