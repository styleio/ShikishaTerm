//! Running with no shell at all.
//!
//! The same runtime the window drives, driven by nothing: tabs are opened,
//! their state is watched, automation fires, and the board is served to
//! whoever connects -- a browser on this machine, a phone, a laptop across a
//! tailnet. What a shell would have provided (a page to click on, a banner, a
//! file dialog, a clipboard) is simply absent, and the traits that stand for
//! those answer honestly instead of pretending.
//!
//! This is what a VPS or a cloud VM runs. It is also the shape the window's own
//! loop is being moved onto: one runtime, driven from either side.

use crate::tab::Tab;
use crate::{config, remote, view};
use anyhow::{Result, bail};
use std::time::{Duration, Instant};

/// How often tab state is re-read. Detection is cheap to do slowly
const DETECT_EVERY: Duration = Duration::from_millis(200);
/// How often the loop wakes at all. Fast enough that a screen going out on
/// every turn is smooth, cheap enough that an idle runtime costs nothing
const TICK: Duration = Duration::from_millis(16);
/// The terminal a tab gets when no window is deciding its size. A phone can
/// resize it afterwards; this is only what it starts at
const ROWS: u16 = 43;
const COLS: u16 = 140;

/// Boot the runtime, serve the board, and keep going until stopped.
pub fn run() -> Result<()> {
    let cfg = config::load().ok_or_else(|| anyhow::anyhow!("no config to run from"))?;
    if !cfg.remote.enabled {
        bail!("serve needs remote turned on: nothing could reach this runtime otherwise");
    }

    // The same reading of the config the window does: folders resolved, tabs
    // grouped, the workspace last in use picked up again
    let (workspaces, mut errors) = cfg.resolve_workspaces();
    let at = config::load_last_workspace()
        .and_then(|name| workspaces.iter().position(|w| w.name == name))
        .unwrap_or(0);
    let ws = workspaces
        .get(at)
        .ok_or_else(|| anyhow::anyhow!("no workspace to open"))?;

    let mut tabs: Vec<Tab> = Vec::new();
    crate::workspace::spawn_workspace(ws, ROWS, COLS, &mut tabs, &mut errors, None);
    for e in &errors {
        crate::append_hook_log(&format!("serve: {e}"));
    }
    if tabs.is_empty() {
        bail!("the workspace opened no tabs");
    }

    // Where to listen, and the key that opens it: the settings decide both,
    // exactly as they do for the window
    let (ip, _note) = crate::netaddr::resolve_bind(&cfg.remote.bind, cfg.remote.allow_public)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let token = crate::remote_token(&cfg, None);
    let ui = remote::RemoteUi::start_with(
        ip,
        cfg.remote.port,
        token,
        cfg.remote.password.clone(),
        cfg.remote.sticky_token,
    )?;
    println!("{}", ui.url);
    crate::append_hook_log(&format!("serve: listening, {} tab(s)", tabs.len()));

    // What a viewer is told about the world, rebuilt whenever it changes. The
    // window fills far more of this in; a runtime with no window reports what it
    // actually knows and leaves the rest at its default
    let mut view_state = view::Ui {
        ws_names: workspaces.iter().map(|w| w.name.clone()).collect(),
        ws_index: at,
        active: 1,
        remote_on: true,
        auto: Some(true),
        ..Default::default()
    };
    // What is laid out, named by the tabs that actually opened
    let titles: Vec<String> = tabs.iter().map(|t| t.title.clone()).collect();
    view_state.surfaces = view::surfaces_of(
        Some(ws),
        &titles.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &[],
    );
    let mut last_ui = String::new();

    let start = Instant::now();
    let mut last_detect = Instant::now() - DETECT_EVERY;
    let mut last_push = Instant::now() - Duration::from_secs(1);
    let mut sent: Vec<String> = Vec::new();

    loop {
        std::thread::sleep(TICK);

        if last_detect.elapsed() >= DETECT_EVERY {
            last_detect = Instant::now();
            for t in tabs.iter_mut() {
                t.tick(start);
            }
        }

        if !ui.has_state_clients() {
            continue;
        }

        // The picture first: a viewer that has just arrived needs to know what
        // the tabs are before a screen means anything
        view_state.now_ms = start.elapsed().as_millis() as u64;
        let state = view::ui_state_of(&tabs, &view_state, None);
        if let Ok(json) = serde_json::to_string(&state) {
            if json != last_ui {
                ui.push_state(format!("{{\"ui\":{json}}}"));
                last_ui = json;
            }
        }
        if last_push.elapsed() < view::remote_floor(ui.max_pending()) {
            continue;
        }
        let now: Vec<String> = tabs
            .first()
            .map(|t| {
                let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                crate::shell::screen_rows(p.screen())
            })
            .unwrap_or_default();
        *ui.snapshot.lock().unwrap() = remote::Snapshot {
            ui: Some(state),
            screen_html: now.join("
"),
            workspace: ws.name.clone(),
            auto_enabled: true,
            cols: COLS,
            tabs: tabs
                .iter()
                .enumerate()
                .map(|(i, t)| remote::RemoteTab {
                    index: i + 1,
                    name: t.title.clone(),
                    state: t.state.label().to_string(),
                    locked: t.locked,
                    output: t.last_response.clone().unwrap_or_default(),
                    screen: String::new(),
                    cwd: String::new(),
                    record_id: t.session.as_ref().map(|s| s.id.clone()).unwrap_or_default(),
                    record_glob: String::new(),
                })
                .collect(),
        };

        match view::screen_push(&sent, &now) {
            view::ScreenPush::Nothing => {}
            view::ScreenPush::Rows(moved) => {
                let rows = {
                    let list: Vec<(usize, &str)> =
                        moved.iter().map(|&i| (i, now[i].as_str())).collect();
                    serde_json::to_string(&list)
                };
                if let Ok(rows) = rows {
                    ui.push_state(format!("{{\"rows\":{rows}}}"));
                    sent = now;
                    last_push = Instant::now();
                }
            }
            view::ScreenPush::Whole => {
                if let Ok(scr) = serde_json::to_string(&now.join("\n")) {
                    ui.push_state(format!("{{\"screen_html\":{scr}}}"));
                    sent = now;
                    last_push = Instant::now();
                }
            }
        }
    }
}
