//! The window's loop, whatever engine draws its pages.
//!
//! The window holds pages: the board, the pages placed inside it (a browser a
//! rally is driving, the settings), the windows those pages asked to open, and
//! the tool laid over a picture of the screen. What the conductor asks of them
//! (`Cmd`) is answered here, once, for every system. The engine -- WebView2 on
//! Windows, CEF on a Mac -- is asked only for what only it can do: make a
//! page, move it, show it, and carry its DevTools protocol (`super::engine`).
//! The window itself, its frame and its icon in the notification area, are
//! the system's (`super::frame`).

use super::*;
use super::engine::{Finder, Page, Pages};

/// A place for a page in the window, in logical pixels: x, y, width, height
pub(super) type Rect4 = (i32, i32, i32, i32);

/// Where a page is drawn
pub(super) enum Place {
    /// The whole of a window, following its size (the board, the tool)
    Fill,
    /// A seat inside the window, shared with whatever stands in it: a window
    /// a page opened is put where its opener sits, and moves with it
    At(std::rc::Rc<std::cell::Cell<Rect4>>),
}

/// What a page tells the window, each handed to the engine to wire up
pub(super) struct Wiring {
    /// A message the page posted (`window.ipc.postMessage`), with the address
    /// it was posted from -- the engine's word, never the page's
    pub ipc: std::rc::Rc<dyn Fn(&str, &str)>,
    /// The page started (true) or finished (false) loading
    pub loading: Option<std::rc::Rc<dyn Fn(bool)>>,
    /// The page's own processes died; `true` when it was for want of memory
    pub died: Option<std::rc::Rc<dyn Fn(bool)>>,
    /// The page asked to close its window (`window.close()`)
    pub closing: Option<std::rc::Rc<dyn Fn()>>,
    /// What to do when the page asks to open a window
    pub popups: Option<Popups>,
    /// Where what the page saves is reported, under which page's name
    pub downloads: Option<(Option<String>, Sender<Ev>)>,
    /// Ctrl+F and F3 open the board's search for this page, unless the page
    /// is at one of the app's own addresses then
    pub find_keys: Option<(String, Sender<Ev>, std::rc::Rc<dyn Fn(&str) -> bool>)>,
}

/// One page to make
pub(super) struct Spec {
    pub url: String,
    pub place: Place,
    /// Run in every document before its own scripts
    pub init_js: String,
    /// What the page calls itself, when not the engine's own name
    pub user_agent: Option<String>,
    /// The folder its cookies and storage live in. Two pages given the same
    /// folder are the same visitor, and only that separates them
    pub store: std::path::PathBuf,
    /// Its network goes out through this port on this machine (a proxy)
    pub through: Option<u16>,
    /// It may take the keyboard as it opens
    pub focus: bool,
    /// What shows before the page has drawn anything
    pub background: Option<(u8, u8, u8, u8)>,
    pub wiring: Wiring,
}

/// Windows the pages asked for, by the page that asked. Newest last.
///
/// A pane holds one page at a time, so a window opened by one of them stands
/// in the same seat, in front of it. Which is also what it means to everything
/// else: "this pane" is what a person is looking at and what automation is
/// driving, and while a sign-in window is up, that is the sign-in window.
type Overlays = std::collections::HashMap<String, Vec<(String, Page)>>;

/// The board's page, while there is one (none while the window is put away)
fn main_view(shell: &Option<Page>) -> Option<&Page> {
    shell.as_ref()
}

fn target<'a>(
    main: Option<&'a Page>,
    children: &'a std::collections::HashMap<String, Page>,
    overlays: &'a Overlays,
    to: &Option<String>,
) -> Option<&'a Page> {
    match to {
        None => main,
        Some(name) => overlays
            .get(name)
            .and_then(|stack| stack.last())
            .map(|(_, v)| v)
            .or_else(|| children.get(name)),
    }
}

/// What a page's request for a window has to hand back to the loop it cannot touch.
#[derive(Default)]
pub(super) struct Adoptions {
    /// (the page that asked, the new page's name, the page itself)
    made: Vec<(String, String, Page)>,
    /// Names of adopted pages that asked to be let go
    shut: Vec<String>,
    /// Counts the names apart
    next: u32,
}

/// Take in a window a page asked for, rather than refusing it in silence.
///
/// An engine's answer to `window.open` when nobody says otherwise is either
/// "no", with nothing said -- the call returns null, the `target="_blank"`
/// link does nothing, and a sign-in button that hands off to a popup, which is
/// most of them, is a button that does nothing at all -- or a window of its
/// own somewhere on the desktop, outside everything a phone or the automation
/// can see.
///
/// So the window is made by the engine, related to its opener (same store,
/// real opener, so `window.opener`, `postMessage` and the closing handshake
/// all still work), and handed here. It is placed in the seat its opener
/// occupies, because a pane is where a person is looking.
#[derive(Clone)]
pub(super) struct Popups {
    pub opener: String,
    pub seat: std::rc::Rc<std::cell::Cell<Rect4>>,
    inbox: std::rc::Rc<std::cell::RefCell<Adoptions>>,
    wake: tao::event_loop::EventLoopProxy<Cmd>,
    ev_tx: Sender<Ev>,
    /// A sign-in window that called itself something else than the page that
    /// opened it would be a second browser arriving in the middle of a login
    pub user_agent: Option<String>,
}

impl Popups {
    /// The name the next window gets, or `None` while the loop is in the
    /// middle of counting them: refusing is what happens then
    pub fn next_name(&self) -> Option<String> {
        let mut ib = self.inbox.try_borrow_mut().ok()?;
        ib.next += 1;
        Some(format!("{}#window{}", self.opener, ib.next))
    }

    /// What the window runs before its own scripts
    pub fn init_js(&self) -> String {
        format!("{}{PLACED_JS}{POPUP_JS}", *INIT_JS)
    }

    /// How a window this page opened reports. Its loading is reported as the
    /// pane, not as itself: what the bar above the pane should say is loading
    /// is whatever the pane is showing. Asking to close -- by the page's own
    /// message or the engine's -- lets it go. What it saves is the pane's, like
    /// everything else it does, and a window it opens stands in the same seat
    pub fn wiring(&self, name: &str) -> Wiring {
        let shut = {
            let (inbox, wake, name) = (std::rc::Rc::clone(&self.inbox), self.wake.clone(), name.to_string());
            std::rc::Rc::new(move || {
                if let Ok(mut ib) = inbox.try_borrow_mut() {
                    ib.shut.push(name.clone());
                    let _ = wake.send_event(Cmd::Adopt);
                }
            })
        };
        let loading = {
            let (tx, who) = (self.ev_tx.clone(), self.opener.clone());
            std::rc::Rc::new(move |busy: bool| {
                let _ = tx.send(Ev::Loading { from: Some(who.clone()), busy });
            })
        };
        let by_message = std::rc::Rc::clone(&shut);
        Wiring {
            ipc: std::rc::Rc::new(move |_at: &str, body: &str| {
                if body.contains("popupclose") {
                    by_message();
                }
            }),
            loading: Some(loading),
            died: None,
            closing: Some(shut),
            popups: Some(self.clone()),
            downloads: Some((Some(self.opener.clone()), self.ev_tx.clone())),
            find_keys: Some((self.opener.clone(), self.ev_tx.clone(), std::rc::Rc::new(|_: &str| false))),
        }
    }

    /// The window was made: it goes into the seat on the loop's next turn
    pub fn made(&self, name: String, page: Page, uri: &str) {
        shikisha_core::append_hook_log(&format!("[browser] '{}' opened a window -> '{name}' ({uri})", self.opener));
        if let Ok(mut ib) = self.inbox.try_borrow_mut() {
            ib.made.push((self.opener.clone(), name, page));
        }
        let _ = self.wake.send_event(Cmd::Adopt);
    }

    /// The window could not be made
    pub fn failed(&self, uri: &str, e: &dyn std::fmt::Display) {
        shikisha_core::append_hook_log(&format!(
            "[browser] '{}' asked for a window ({uri}) and it could not be made: {e}",
            self.opener
        ));
    }
}

/// Whether a page in this window may take the keyboard now.
///
/// Always, unless the program was started `--behind`: then only while a person
/// has the window in front. Giving a page the keyboard makes its window the
/// active one, and a window of a program started from the terminal in front
/// is let through to the front (see `shikisha_core::stays_behind`)
pub(super) fn may_take_focus(window: &tao::window::Window) -> bool {
    !shikisha_core::stays_behind() || window.is_focused()
}

// `wears_the_icon`: whether this window carries the program's icon in the
// notification area. The one window whose process outlives it does; a client,
// a probe and a self-check do not (see `Browser::host`)
pub(super) fn run_window(
    url: &str,
    title: &str,
    wears_the_icon: bool,
    proxy_tx: Sender<tao::event_loop::EventLoopProxy<Cmd>>,
    ev_tx: Sender<Ev>,
    // Filled in with the process that plays the page being cast, for a phone
    // that asks to hear it. Written here because this is the only place that
    // holds the page itself
    sound_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,
) -> Result<()> {
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;

    // The program's first thread when it is the program's window (`Browser::host`).
    // A probe holds one from a test's own thread, which only Windows allows
    let mut ev_loop = EventLoopBuilder::<Cmd>::with_user_event();
    #[cfg(windows)]
    tao::platform::windows::EventLoopBuilderExtWindows::with_any_thread(&mut ev_loop, true);
    let mut ev_loop = ev_loop.build();
    proxy_tx
        .send(ev_loop.create_proxy())
        .map_err(|_| anyhow!(shikisha_core::i18n::t("err.browser.proxy_connect_failed")))?;

    // The bar is the page's, because the bar is where the panels are opened
    // from and a system bar has nowhere to put them. What the system keeps of
    // the frame is each system's (`frame::main_builder`)
    let window = std::rc::Rc::new(
        super::frame::main_builder(title)
            .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 900.0))
            // Shown without being made the active window, which is only the
            // first half: `frame::settle` does the second
            .with_focused(!shikisha_core::stays_behind())
            .build(&ev_loop)?,
    );
    super::frame::settle(&window);
    // Whether the screen is down and waiting to be asked for. Read by the
    // icon's handler, which the system calls from wherever it likes
    let display_down = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The notification-area icon, on the one window whose process outlives it
    // A Mac's menus at the top of the screen, made with the window that
    // wears the program's icon -- the one a person quits from
    #[cfg(target_os = "macos")]
    if wears_the_icon {
        super::frame::menu_bar(ev_tx.clone());
    }
    let tray = wears_the_icon
        .then(|| super::frame::Tray::add(&window, title, ev_tx.clone(), ev_loop.create_proxy(), std::sync::Arc::clone(&display_down)))
        .flatten();
    // Watched by the window that wears the icon, and only by it: that is the
    // process holding the work, and the icon is where it can be said. What is
    // said is what is going on and who is holding the memory -- the screen is
    // never taken away for it (see `shikisha_core::pressure`)
    if tray.is_some() {
        let wake = ev_loop.create_proxy();
        shikisha_core::pressure::watch(move |title, text| wake.send_event(Cmd::TrayNotice { title, text }).is_ok());
    }

    // The pages this window holds, made by the engine
    let mut pages = Pages::new(std::rc::Rc::clone(&window), ev_loop.create_proxy())?;

    // The board's own page. Built here, and built again whenever the window
    // comes back from being put away: while it is away the page is dropped
    // altogether, because the page is where the memory is. Chromium keeps a
    // browser process and a renderer for it -- some two hundred megabytes
    // for an empty board -- and hiding the window releases none of that.
    // The pages placed inside the window (a browser a rally is driving, the
    // settings) are kept: those hold state a person would lose.
    //
    // Every message a page posts is judged by the address it was posted from
    // (the engine reports it with the message; it is not something the page
    // writes). The board's own address is the app's, and only a page speaking
    // from one of the app's addresses is heard in full — see `heard`. The
    // settings server's address joins the list when it starts (Cmd::Trust)
    let own = std::rc::Rc::new(std::cell::RefCell::new(vec![url.to_string()]));
    // How a dying page tells this loop. Made before the page that uses it
    let died = ev_loop.create_proxy();
    // The questions put to pages and not yet answered, shared with every
    // page's handler: an answer is taken only from the page that was asked
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Asked::default()));
    let shell_spec = {
        let window = std::rc::Rc::clone(&window);
        let ev_tx = ev_tx.clone();
        let url = url.to_string();
        let own = std::rc::Rc::clone(&own);
        let asked = std::rc::Rc::clone(&asked);
        move || -> Spec {
            let ipc = ev_tx.clone();
            let own = std::rc::Rc::clone(&own);
            let asked = std::rc::Rc::clone(&asked);
            let refused = std::cell::Cell::new(0u8);
            // The bar the page draws acts on this window, so the handler holds
            // it. Same thread as the message loop, which is where these calls
            // have to be made from anyway
            let win = std::rc::Rc::clone(&window);
            let wake = died.clone();
            Spec {
                url: url.clone(),
                place: Place::Fill,
                init_js: INIT_JS.clone(),
                user_agent: None,
                store: shell_data_dir(),
                through: None,
                focus: may_take_focus(&window),
                background: None,
                wiring: Wiring {
                    ipc: std::rc::Rc::new(move |at: &str, body: &str| {
                        // The board is the app's own page. Were it ever led to
                        // another site, that site would hold the whole keyboard;
                        // so a board speaking from anywhere else is not the board
                        if !from_ours(&own.borrow(), at) {
                            note_refused(&refused, None, at, body);
                            return;
                        }
                        let ev = heard(body, None, true, &mut asked.borrow_mut());
                        // The bar asking this window to do something to itself.
                        // Answered on the spot: the loop that would otherwise be
                        // told is a tick away, and a drag that starts a tick late
                        // is a drag the pointer has already left behind
                        if let Some(Ev::Window { act, at }) = ev.as_ref() {
                            shikisha_core::append_hook_log(&format!("window act: {act}"));
                            match act.as_str() {
                                "drag" => super::frame::drag(&win, *at),
                                "minimize" => win.set_minimized(true),
                                "maximize" => win.set_maximized(!win.is_maximized()),
                                // The same message the frame's own ✕ sent, so
                                // whatever closing means is decided in one place
                                "close" => {
                                    let _ = ipc.send(Ev::CloseRequested);
                                }
                                _ => {}
                            }
                            return;
                        }
                        if let Some(ev) = ev {
                            let _ = ipc.send(ev);
                        }
                    }),
                    loading: None,
                    // Hear about it if this page's own processes die. Set here,
                    // where the page is made, so that a page made again after
                    // one died is watched too -- the second failure is the one
                    // that matters
                    died: Some(std::rc::Rc::new(move |memory| {
                        let _ = wake.send_event(Cmd::DisplayDied { memory });
                    })),
                    closing: None,
                    popups: None,
                    downloads: None,
                    find_keys: None,
                },
            }
        }
    };
    let mut shell: Option<Page> = Some(pages.fill(&window, shell_spec())?);

    // Pages placed inside the same window. Looked up by name
    let mut children: std::collections::HashMap<String, Page> = std::collections::HashMap::new();
    // What the browser calls itself. Read once, here: a name that changed
    // under a page would be a different browser halfway through a login
    let user_agent = shikisha_core::config::user_agent();
    // Windows those pages asked to open, kept in the seat of whoever asked
    let mut overlays: Overlays = std::collections::HashMap::new();
    // How those requests hand their work back to this loop
    let adoptions = std::rc::Rc::new(std::cell::RefCell::new(Adoptions::default()));
    // ...and how they wake it to come and collect
    let adopt_wake = ev_loop.create_proxy();
    // Temp folders for children placed in private mode (child name -> folder). Removed on close
    let mut ephemeral_dirs: std::collections::HashMap<String, std::path::PathBuf> = std::collections::HashMap::new();

    // Screencasts. One per target. Frames only arrive while this is held
    let mut casts: std::collections::HashMap<Option<String>, cdp::Cast> = std::collections::HashMap::new();
    // Compositor wakes for genuine input on hidden pages. A page hidden the
    // normal way (bounds 0×0) has no surface, and mouse input needs one to
    // hit-test against — its ack never comes. For the duration of a wake the
    // page gets a real-sized surface parked outside the client area (never
    // painted, so nothing flickers), plus a tiny throwaway screencast to
    // keep frames flowing. The bool remembers whether the bounds were
    // borrowed, so release restores exactly the layout's last word. Kept
    // separate from `casts` so a phone relay and a wake never fight
    let mut wakes: std::collections::HashMap<Option<String>, (cdp::Cast, bool)> = std::collections::HashMap::new();
    // Each child's last layout-given rect: how a wake tells hidden (0×0)
    // from merely unfocused, and what it restores on release.
    // Shared per child: a page that opens a window into this seat reads it
    // without reaching into the loop's own state, and a resize lands on the
    // window that is standing in the seat as well
    let mut child_sizes: std::collections::HashMap<String, std::rc::Rc<std::cell::Cell<Rect4>>> =
        std::collections::HashMap::new();
    // The browser zoom put on a cast target fitted to its viewer (see
    // `cdp::view_metrics`). A press arrives as a share of the picture, and the
    // picture is that many times wider than the page is in its own pixels, so
    // a press is divided by it -- or it lands that many times too far along
    let mut zooms: std::collections::HashMap<Option<String>, f64> = std::collections::HashMap::new();
    // The last size this window had while somebody could see it. What the
    // board is held at while the window is put away
    let mut last_size: Option<(u32, u32)> = None;
    // Basic-auth arming. One per target. Only answers 401s while this is held
    let mut auths: std::collections::HashMap<Option<String>, cdp::AuthArm> = std::collections::HashMap::new();
    // Automatic handling of JS dialogs. One per child. Without this, automation freezes on things like "leave this page?" confirmations
    let mut dialogs: std::collections::HashMap<Option<String>, cdp::DialogArm> = std::collections::HashMap::new();
    // Pages whose console is being heard. One per page, held until it is let
    // go of or the page closes; the lines leave as reports
    let mut consoles: std::collections::HashMap<Option<String>, cdp::ConsoleArm> = std::collections::HashMap::new();
    // The browser's own search, one per page searched, by the name of the view
    // standing in the seat (a window a page opened stands over the page)
    let mut finders: std::collections::HashMap<String, Finder> = std::collections::HashMap::new();
    // DevTools screens connected to pages, and the sessions they speak through
    let mut screens = crate::devtools::Screens::default();
    // The most recent frame's CSS pixel dimensions (used to convert
    // coordinates for input injection).
    // Frame notification and input injection run on the same thread, so `Rc<Cell>` is enough
    let cast_dims = std::rc::Rc::new(std::cell::Cell::new((0.0f64, 0.0f64)));
    // For drag detection: is the button currently held down
    let mut mouse_down = false;

    // The window a tool from the left bar is drawn in, over a picture of the
    // screen. Built the first time one is asked for and kept, hidden, after
    let mut snip: Option<SnipWindow> = None;
    // Which press the wait belongs to. A second press replaces the first, and
    // the first one's count must not go on to take a picture
    let mut snip_gen: u64 = 0;
    let snip_tool = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    let snip_wake = ev_loop.create_proxy();
    let snip_own = std::rc::Rc::clone(&own);
    let snip_base = url.trim_end_matches('/').to_string();
    let snip_tx = ev_tx.clone();
    // The tool's window stepped aside for a save dialog, to come back after it
    let mut snip_aside = false;

    // When the screen last died, so that a screen dying over and over is
    // told apart from one that died once (see `shikisha_core::revive`).
    // Measured from a clock of this loop's own, which only goes forwards
    let started = std::time::Instant::now();
    let mut display_tried: Vec<u64> = Vec::new();
    let display_wake = ev_loop.create_proxy();

    // Reports are sent from inside the loop too, so grab a sender for "closed" ahead of time
    let closed_tx = ev_tx.clone();
    // The channel that answers "where are we now". Only known from inside the window, so it answers from here
    let where_tx = ev_tx.clone();
    // Whether this program asked the loop to end. A loop that ends without
    // being asked is the system ending the session (see `Event::LoopDestroyed`)
    let mut asked_to_end = false;
    ev_loop.run_return(move |event, elwt, control| {
        // The engine is given its turn when it is due, whatever woke the
        // loop, and may want its next one at a time it names
        pages.turn();
        *control = match pages.next_turn() {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        };
        match event {
            Event::UserEvent(cmd) => match cmd {
                // Only to wake the loop: the turn was given above
                Cmd::EngineTurn => {}
                Cmd::RegisterKeys => {
                    #[cfg(target_os = "macos")]
                    crate::hotkeys::register_here();
                }
                Cmd::Trust { origin } => {
                    let mut list = own.borrow_mut();
                    if !list.iter().any(|o| same_origin(o, &origin)) {
                        list.push(origin);
                    }
                }
                Cmd::Eval { id, to, js } => {
                    // A window a page opened, standing in front of it. Its
                    // answer cannot come back the ordinary way: a window a
                    // script opened starts as about:blank, and once it goes
                    // on to its real address its messages are no longer
                    // delivered -- so everything asked while a sign-in window
                    // was up went unanswered. The DevTools protocol answers
                    // the call that asked, which needs no message at all
                    let standing = to.as_ref().and_then(|name| overlays.get(name)).and_then(|stack| stack.last()).map(|(_, v)| v);
                    if let Some(v) = standing {
                        let tx = ev_tx.clone();
                        let params = serde_json::json!({
                            "expression": format!("(async function(){{ {js} }})()"),
                            "awaitPromise": true,
                            "returnByValue": true,
                        })
                        .to_string();
                        v.port().call_result("Runtime.evaluate", &params, move |ok, json| {
                            let (ok, value) = match ok {
                                true => evaluated(&json),
                                false => (false, serde_json::Value::String(json).to_string()),
                            };
                            let _ = tx.send(Ev::Result { id, ok, value });
                        });
                    } else
                    // When the destination can't be found, don't fall back
                    // to the main view. That would run site-facing JS against our own screen
                    if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        // Written down before the page is asked, so the answer
                        // is expected — from this page — by the time it comes
                        asked.borrow_mut().ask(id, to.clone());
                        v.run_js(&wrap_eval(id, &js));
                    } else {
                        let _ = ev_tx.send(Ev::Result {
                            id,
                            ok: false,
                            value: serde_json::Value::String(shikisha_core::i18n::tp(
                                "err.browser.page_not_placed",
                                &[("to", &to.unwrap_or_default())],
                            ))
                            .to_string(),
                        });
                    }
                }
                Cmd::Wake { to, on } => {
                    if on {
                        // A live relay cast already keeps the compositor
                        // running; arming a second screencast would replace
                        // its parameters and stopping it later would kill
                        // the relay's frames. So wake only when nothing casts
                        if !wakes.contains_key(&to)
                            && !casts.contains_key(&to)
                            && let Some(v) = target(main_view(&shell), &children, &overlays, &to)
                        {
                            // Hidden = this child is currently sized 0.
                            // Borrow it a real surface, parked outside
                            // the client area (clipped, never painted)
                            let hidden = to.as_ref().is_some_and(|name| {
                                child_sizes.get(name).is_none_or(|seat| {
                                    let (_, _, w, h) = seat.get();
                                    w <= 0 || h <= 0
                                })
                            });
                            if hidden {
                                v.place((-4000, 0, 1280, 900));
                            }
                            if let Some(cast) = cdp::start_with(&v.port(), cdp::WAKE_PARAMS, |_, _, _| {}) {
                                wakes.insert(to.clone(), (cast, hidden));
                            } else if hidden
                                && let Some(r) = to.as_ref().and_then(|n| child_sizes.get(n)).map(|seat| seat.get())
                            {
                                v.place(r);
                            }
                        }
                    } else if let Some((cast, borrowed)) = wakes.remove(&to) {
                        cdp::stop(cast);
                        if borrowed {
                            // Give back whatever the layout last decreed —
                            // including a rect that changed mid-wake
                            if let (Some(v), Some(r)) = (
                                target(main_view(&shell), &children, &overlays, &to),
                                to.as_ref().and_then(|n| child_sizes.get(n)).map(|seat| seat.get()),
                            ) {
                                v.place(r);
                            }
                        }
                    }
                }
                Cmd::Cdp { id, to, method, params } => {
                    // Same guard as Eval: an unplaced page must answer, not hang
                    if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        let tx = ev_tx.clone();
                        v.port().call_result(&method, &params, move |ok, json| {
                            let _ = tx.send(Ev::Result { id, ok, value: json });
                        });
                    } else {
                        let _ = ev_tx.send(Ev::Result {
                            id,
                            ok: false,
                            value: shikisha_core::i18n::tp("err.browser.page_not_placed", &[("to", &to.unwrap_or_default())]),
                        });
                    }
                }
                Cmd::BasicAuth { to, user, pass } => {
                    // If credentials are already armed, swap them; otherwise enable Fetch and arm them
                    if let Some(arm) = auths.get(&to) {
                        *arm.creds.borrow_mut() = (user, pass);
                    } else if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        match cdp::arm_basic_auth(&v.port(), &user, &pass) {
                            Some(arm) => {
                                auths.insert(to.clone(), arm);
                            }
                            None => shikisha_core::append_hook_log(&shikisha_core::i18n::t("err.browser.log_basic_auth_failed")),
                        }
                    }
                }
                Cmd::Console { to, on } => {
                    if !on {
                        consoles.remove(&to);
                    } else if !consoles.contains_key(&to)
                        && let Some(v) = target(main_view(&shell), &children, &overlays, &to)
                    {
                        let tx = ev_tx.clone();
                        let from = to.clone();
                        if let Some(arm) = cdp::arm_console(&v.port(), move |entry| {
                            let _ = tx.send(Ev::ConsoleLine { from: from.clone(), entry });
                        }) {
                            consoles.insert(to, arm);
                        }
                    }
                }
                Cmd::DevtoolsOpen { conn, to, out } => match target(main_view(&shell), &children, &overlays, &to) {
                    Some(v) => screens.open(conn, to, v.port(), out),
                    None => shikisha_core::append_hook_log(&format!("[devtools] screen {conn}: no such page here")),
                },
                Cmd::DevtoolsSay { conn, text } => screens.say(conn, text),
                Cmd::DevtoolsClose { conn } => screens.close(conn),
                Cmd::AddChild { name, url, rect, profile, through } => {
                    // Making a page runs on THIS event-loop thread, and this
                    // thread also pumps the whole window's messages — while it
                    // runs, every click is frozen. Log how long it took so a
                    // "dead window right after startup" report can be matched
                    // against it.
                    let born = std::time::Instant::now();
                    let seat = std::rc::Rc::new(std::cell::Cell::new(rect));
                    child_sizes.insert(name.clone(), std::rc::Rc::clone(&seat));
                    // Decide this page's data storage (profile/private).
                    // Same folder = same cookies/login, different folder = different profile.
                    // All tabs, including "default", are isolated under
                    // browser-profiles/<name> (like Chrome's "person").
                    // Private mode gets a unique temp folder on every call
                    // and is removed on close.
                    // A page whose network belongs to another machine is a
                    // different visitor from a page of the same profile whose
                    // network is this one's, so its store is a different
                    // folder -- and its browser environment a different one,
                    // which is what carrying a proxy setting requires anyway
                    let data_dir = match through {
                        Some(_) => {
                            let d = profile_dir(&profile).with_extension("through");
                            let _ = std::fs::create_dir_all(&d);
                            d
                        }
                        None => profile_dir(&profile),
                    };
                    if profile.private {
                        ephemeral_dirs.insert(name.clone(), data_dir.clone());
                    }
                    if let Some(port) = through {
                        shikisha_core::append_hook_log(&format!("[browser] '{name}' reaches the network through 127.0.0.1:{port}"));
                    }
                    // This page's own name if it was given one, the app's
                    // otherwise
                    let ua = profile.user_agent.clone().or_else(|| user_agent.clone());
                    // Equip the child with the same tools as the main view.
                    // Without them, a placed page would just be something displayed, nothing more
                    let ipc = ev_tx.clone();
                    let who = name.clone();
                    let ipc_own = std::rc::Rc::clone(&own);
                    let ipc_asked = std::rc::Rc::clone(&asked);
                    let refused = std::cell::Cell::new(0u8);
                    // Signaling "in progress" from the in-page script (at
                    // document creation) is too late. If the server is slow,
                    // the document isn't created until the response comes
                    // back, so the indicator would stay off the whole time
                    // we're waiting. Instead, turn it on when navigation
                    // starts (the moment it's pressed, before any response)
                    // and off when loading finishes. This keeps it lit for the entire wait
                    let load_tx = ev_tx.clone();
                    let load_who = name.clone();
                    let keys_own = std::rc::Rc::clone(&own);
                    let spec = Spec {
                        url: url.clone(),
                        place: Place::At(std::rc::Rc::clone(&seat)),
                        init_js: format!("{}{PLACED_JS}", *INIT_JS),
                        user_agent: ua.clone(),
                        store: data_dir,
                        through,
                        focus: may_take_focus(&window),
                        background: None,
                        wiring: Wiring {
                            ipc: std::rc::Rc::new(move |at: &str, body: &str| {
                                // The app's own pages (settings, a result view) are
                                // served from the board's address and keep their
                                // full voice. Anything else in this pane is
                                // somebody's website: it may report, not ask
                                let ours = from_ours(&ipc_own.borrow(), at);
                                // There's no way to know who pressed it except here
                                match heard(body, Some(&who), ours, &mut ipc_asked.borrow_mut()) {
                                    // A page's notification, or its asking about
                                    // them: filed under the site the engine says
                                    // the script runs on, never one the page names.
                                    // The program's own pages are not a site
                                    Some(Ev::PageNotice { from, title, body, .. }) => {
                                        if let (false, Some(site)) = (ours, shikisha_core::pagenotice::site(at)) {
                                            let _ = ipc.send(Ev::PageNotice { from, site, title, body });
                                        }
                                    }
                                    Some(Ev::NoticeAsk { from, ask, .. }) => {
                                        if let (false, Some(site)) = (ours, shikisha_core::pagenotice::site(at)) {
                                            let _ = ipc.send(Ev::NoticeAsk { from, site, ask });
                                        }
                                    }
                                    Some(ev) => {
                                        let _ = ipc.send(ev);
                                    }
                                    None => note_refused(&refused, Some(&who), at, body),
                                }
                            }),
                            loading: Some(std::rc::Rc::new(move |busy| {
                                let _ = load_tx.send(Ev::Loading { from: Some(load_who.clone()), busy });
                            })),
                            died: None,
                            closing: None,
                            // What to do when this page asks to open a window.
                            // Without an answer the request is refused in
                            // silence, which is how a sign-in popup becomes a
                            // button that does nothing
                            popups: Some(Popups {
                                opener: name.clone(),
                                seat: std::rc::Rc::clone(&seat),
                                inbox: std::rc::Rc::clone(&adoptions),
                                wake: adopt_wake.clone(),
                                ev_tx: ev_tx.clone(),
                                user_agent: ua.clone(),
                            }),
                            // What it saves goes in the list beside it, and its
                            // Ctrl+F opens the board's search bar
                            downloads: Some((Some(name.clone()), ev_tx.clone())),
                            find_keys: Some((
                                name.clone(),
                                ev_tx.clone(),
                                std::rc::Rc::new(move |at: &str| from_ours(&keys_own.borrow(), at)),
                            )),
                        },
                    };
                    match pages.child(spec) {
                        Ok(v) => {
                            // Arm automatic dialog handling right after placing it (don't let "leave page?" freeze it)
                            if let Some(arm) = cdp::arm_dialogs(&v.port()) {
                                dialogs.insert(Some(name.clone()), arm);
                            }
                            shikisha_core::append_hook_log(&format!(
                                "[browser] placed page '{}' in {} ms (window input is frozen while a page is being created)",
                                name,
                                born.elapsed().as_millis()
                            ));
                            children.insert(name, v);
                        }
                        Err(e) => shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                            "err.browser.log_place_failed",
                            &[("name", &name), ("e", &format!("{e}"))],
                        )),
                    }
                }
                Cmd::RaiseChild { name } => {
                    // The same move the board makes when it is built again,
                    // for one page rather than all of them
                    if let Some(v) = children.get(&name) {
                        v.raise();
                    }
                }
                Cmd::ChildBounds { name, rect } => {
                    // The other half of the same rule as Resized: a minimized
                    // window's layout is not a layout anybody asked for, and a
                    // page being watched from a phone must not be re-shaped by
                    // it. The seat is left alone as well, so what comes back
                    // when the window does is the size it had
                    if !window.is_minimized()
                        && let Some(v) = children.get(&name)
                    {
                        v.place(rect);
                        match child_sizes.get(&name) {
                            Some(seat) => seat.set(rect),
                            None => {
                                child_sizes.insert(name.clone(), std::rc::Rc::new(std::cell::Cell::new(rect)));
                            }
                        }
                        // A window this page opened stands in the same seat,
                        // so it is moved and hidden by the same layout
                        for (_, v) in overlays.get(&name).into_iter().flatten() {
                            v.place(rect);
                        }
                    }
                }
                // Windows that were asked for, and windows that asked to go.
                // Both are handed over by handlers running on this same loop,
                // which is why neither can be done where it is decided
                Cmd::Adopt => {
                    let (made, shut) = match adoptions.try_borrow_mut() {
                        Ok(mut ib) => (std::mem::take(&mut ib.made), std::mem::take(&mut ib.shut)),
                        Err(_) => (Vec::new(), Vec::new()),
                    };
                    for (opener, name, view) in made {
                        overlays.entry(opener).or_default().push((name, view));
                    }
                    for name in shut {
                        finders.remove(&name);
                        // Dropping the page is what closes it
                        for stack in overlays.values_mut() {
                            stack.retain(|(n, _)| n != &name);
                        }
                        overlays.retain(|_, stack| !stack.is_empty());
                    }
                }
                Cmd::RemoveChild { name } => {
                    children.remove(&name);
                    child_sizes.remove(&name);
                    // Whatever it opened goes with it
                    overlays.remove(&name);
                    if let Some((cast, _)) = wakes.remove(&Some(name.clone())) {
                        cdp::stop(cast);
                    }
                    dialogs.remove(&Some(name.clone()));
                    auths.remove(&Some(name.clone()));
                    consoles.remove(&Some(name.clone()));
                    finders.retain(|key, _| key != &name && !key.starts_with(&format!("{name}#window")));
                    screens.page_closed(&Some(name.clone()));
                    // If this child was placed in private mode, clean up
                    // its throwaway folder. The engine can take a moment to
                    // release the lock, so this is best-effort
                    // (anything missed gets swept up by sweep_private at startup)
                    if let Some(dir) = ephemeral_dirs.remove(&name) {
                        pages.forget_store(&dir);
                        erase_when_released(dir);
                    }
                }
                Cmd::Focus { to } => {
                    // A window that is put away cannot take focus, and the
                    // engine says so with an error. Asking anyway wrote two
                    // lines of that into the log every time a phone touched a
                    // tab while the PC was minimized -- an error where nothing
                    // was wrong, next to the ones that matter
                    if !window.is_minimized()
                        && may_take_focus(&window)
                        && let Some(v) = target(main_view(&shell), &children, &overlays, &to)
                        && let Err(e) = v.focus()
                    {
                        shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                            "err.browser.log_focus_failed",
                            &[("to", &format!("{to:?}")), ("e", &format!("{e}"))],
                        ));
                    }
                }
                Cmd::Move { to, go } => match target(main_view(&shell), &children, &overlays, &to) {
                    Some(v) => {
                        let r = match &go {
                            Go::Back => v.back(),
                            Go::Forward => v.forward(),
                            Go::Reload => v.reload(),
                            // The engine's reload is the ordinary one. The flag
                            // that says "ignore what you already have" is the
                            // DevTools protocol's, so that is where this one goes
                            Go::Hard => {
                                v.port().call("Page.reload", "{\"ignoreCache\":true}");
                                Ok(())
                            }
                            Go::To(u) => v.load(u),
                        };
                        if let Err(e) = r {
                            shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                                "err.browser.log_move_failed",
                                &[("go", &format!("{go:?}")), ("e", &format!("{e}"))],
                            ));
                        }
                    }
                    None => shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                        "err.browser.log_no_target",
                        &[("to", &format!("{to:?}"))],
                    )),
                },
                Cmd::Seek { to, text, step } => {
                    // What the person is looking at in that seat: a window the
                    // page opened stands over it, and is what gets searched
                    let seat = to.as_ref().and_then(|name| {
                        overlays
                            .get(name)
                            .and_then(|stack| stack.last())
                            .map(|(n, v)| (n.clone(), v))
                            .or_else(|| children.get(name).map(|v| (name.clone(), v)))
                    });
                    match seat {
                        Some((key, v)) => {
                            let finder = finders.entry(key).or_insert_with(|| v.finder(to.clone(), ev_tx.clone()));
                            finder.seek(v, &text, step, to, ev_tx.clone());
                        }
                        None => {
                            let _ = ev_tx.send(Ev::Seek { from: to, at: 0, of: 0 });
                        }
                    }
                }
                Cmd::CancelDownload { id } => super::engine::cancel_download(&id),
                Cmd::FindKeys { to: Some(name), on } => super::engine::find_keys_for(&name, on),
                Cmd::FindKeys { to: None, .. } => {}
                Cmd::Where { to } => {
                    if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        let _ = where_tx.send(Ev::Where {
                            from: to,
                            url: v.url(),
                            can_back: v.can_back(),
                            can_forward: v.can_forward(),
                        });
                    }
                }
                Cmd::Screencast { to, on } => {
                    if on {
                        if casts.contains_key(&to) {
                            // Already streaming. We won't register twice,
                            // but re-issue startScreencast to push out one
                            // fresh frame (otherwise a new viewer joining
                            // while the page is static would see nothing indefinitely)
                            if let Some(view) = target(main_view(&shell), &children, &overlays, &to) {
                                cdp::kick(&view.port());
                            }
                        } else if let Some(view) = target(main_view(&shell), &children, &overlays, &to) {
                            let tx = ev_tx.clone();
                            let from = to.clone();
                            let dims = cast_dims.clone();
                            // Whose sound goes with this picture. Asked
                            // once, here, because this is where the page is
                            sound_pid.store(view.sound_process(), std::sync::atomic::Ordering::SeqCst);
                            if let Some(cast) = cdp::start(&view.port(), move |data, w, h| {
                                dims.set((w, h));
                                let _ = tx.send(Ev::Frame { from: from.clone(), data, w: w as u32, h: h as u32 });
                            }) {
                                casts.insert(to.clone(), cast);
                            } else {
                                shikisha_core::append_hook_log(&shikisha_core::i18n::t("err.browser.log_screencast_failed"));
                            }
                        }
                    } else if let Some(cast) = casts.remove(&to) {
                        sound_pid.store(0, std::sync::atomic::Ordering::SeqCst);
                        // Give the page its own shape back before the stream goes away
                        if zooms.remove(&to).is_some()
                            && let Some(view) = target(main_view(&shell), &children, &overlays, &to)
                        {
                            view.port().call("Emulation.clearDeviceMetricsOverride", "{}");
                            view.zoom(1.0);
                        }
                        cdp::stop(cast);
                    }
                }
                Cmd::Inject { to, input } => {
                    if let Some(view) = target(main_view(&shell), &children, &overlays, &to) {
                        let port = view.port();
                        let (cw, ch) = cast_dims.get();
                        // The page's own pixels a press is counted in (see `zooms`)
                        let z = zooms.get(&to).copied().unwrap_or(1.0);
                        let (cw, ch) = (cw / z, ch / z);
                        match input {
                            Input::Mouse { phase, x, y, down, clicks } => {
                                let (ev, held) = shikisha_core::cdp::mouse_event(&phase, x * cw, y * ch, down, mouse_down, clicks);
                                mouse_down = held;
                                port.call("Input.dispatchMouseEvent", &ev.to_string());
                            }
                            Input::Wheel { x, y, dx, dy } => {
                                let ev = shikisha_core::cdp::wheel_event(x * cw, y * ch, dx, dy);
                                port.call("Input.dispatchMouseEvent", &ev.to_string());
                            }
                            Input::Text { text } => {
                                for ev in shikisha_core::cdp::text_events(&text) {
                                    port.call("Input.dispatchKeyEvent", &ev.to_string());
                                }
                            }
                            Input::View { w, h, dpr } => {
                                // The viewer supplies the layout size. Measuring from
                                // the PC's frame kept sites at desktop width and made
                                // a minimized window determine the phone's layout.
                                let zoom = shikisha_core::cdp::viewer_zoom(w, h, dpr);
                                if let Some(m) = shikisha_core::cdp::view_metrics(w, h, zoom) {
                                    port.call("Emulation.setDeviceMetricsOverride", &m.to_string());
                                    view.zoom(zoom);
                                    zooms.insert(to.clone(), zoom);
                                    // A static page may have sent its one frame
                                    // before the new metrics took effect.
                                    if casts.contains_key(&to) {
                                        cdp::kick(&port);
                                    }
                                }
                            }
                            Input::Key { named, ctrl, alt } => {
                                for ev in shikisha_core::cdp::key_events(&named, ctrl, alt) {
                                    port.call("Input.dispatchKeyEvent", &ev.to_string());
                                }
                            }
                        }
                    }
                }
                Cmd::Hide => {
                    window.set_visible(false);
                    super::frame::now_on_screen();
                    // Let go of the board's page, and of everything holding a
                    // reference to it -- a reference kept would keep Chromium's
                    // processes alive, and with them the memory this is for
                    wakes.remove(&None);
                    casts.remove(&None);
                    auths.remove(&None);
                    dialogs.remove(&None);
                    shell = None;
                    shikisha_core::append_hook_log("Board page dropped while the window is put away");
                }
                Cmd::Show => {
                    // The window first, the page second. A window put away from
                    // minimized comes back minimized, and a page built into a
                    // window with no size is refused: the board never came
                    // back, and what returned was an empty frame with any
                    // placed pages floating in it
                    window.set_visible(true);
                    if window.is_minimized() {
                        window.set_minimized(false);
                    }
                    if shell.is_none() {
                        let born = std::time::Instant::now();
                        match pages.fill(&window, shell_spec()) {
                            Ok(s) => {
                                shell = Some(s);
                                // Back however it was asked for -- the icon, a
                                // second copy, the loop -- so nothing is down
                                // any more, and a press on the icon after this
                                // is a press for the window, not for a rebuild
                                display_down.store(false, std::sync::atomic::Ordering::SeqCst);
                                // The page made last sits on top of the ones
                                // made before it, so the new board would cover
                                // every page placed inside the window. Put
                                // those back above it, each overlay above the
                                // page it stands on
                                for v in children.values() {
                                    v.raise();
                                }
                                for stack in overlays.values() {
                                    for (_, v) in stack {
                                        v.raise();
                                    }
                                }
                                shikisha_core::append_hook_log(&format!(
                                    "Board page built again in {} ms",
                                    born.elapsed().as_millis()
                                ));
                            }
                            Err(e) => shikisha_core::append_hook_log(&format!("Board page could not be built again: {e:#}")),
                        }
                    }
                    window.set_focus();
                }
                Cmd::TrayNotice { title, text } => {
                    if let Some(tray) = &tray {
                        tray.notice(&title, &text);
                    }
                }
                // The screen's browser died. The program did not: the tabs
                // are running, the agents are working, and what was lost is
                // the view of them. So it is put back -- unless putting it
                // back is the wrong move, which is what `revive` decides
                Cmd::DisplayDied { memory } => {
                    shikisha_core::append_hook_log(&format!("the screen stopped (out of memory: {memory})"));
                    let now = started.elapsed().as_millis() as u64;
                    display_tried = shikisha_core::revive::recent(&display_tried, now);
                    let next = shikisha_core::revive::decide(memory, &display_tried, now);
                    shikisha_core::append_hook_log(&format!("what to do about the screen: {next:?}"));
                    match next {
                        shikisha_core::revive::Next::Now => {
                            display_tried.push(now);
                            let _ = display_wake.send_event(Cmd::DisplayWanted { asked: false });
                        }
                        // Not this instant. Whatever went wrong is given a
                        // moment to pass, rather than being asked again
                        // while it is still going on
                        shikisha_core::revive::Next::Wait(ms) => {
                            display_tried.push(now);
                            let wake = display_wake.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(ms));
                                let _ = wake.send_event(Cmd::DisplayWanted { asked: false });
                            });
                        }
                        // Left down on purpose, and said so where it can
                        // still be said: the icon outlives the page. Pressing
                        // it brings the screen back, whenever the person is
                        // ready for it
                        shikisha_core::revive::Next::Ask => {
                            display_down.store(true, std::sync::atomic::Ordering::SeqCst);
                            if let Some(tray) = &tray {
                                tray.notice(
                                    &shikisha_core::i18n::t("tray.screen_down.title"),
                                    &shikisha_core::i18n::t("tray.screen_down.body"),
                                );
                            }
                        }
                    }
                }
                // Put the screen back. Letting go of what died is the same
                // letting-go a window being put away does; building it again
                // is what `Show` already does, placed pages raised and all,
                // so that half is asked for rather than written twice
                Cmd::DisplayWanted { asked } => {
                    display_down.store(false, std::sync::atomic::Ordering::SeqCst);
                    // Only a person clears it. An attempt this code chose is
                    // exactly what the count is counting
                    if asked {
                        display_tried.clear();
                    }
                    wakes.remove(&None);
                    casts.remove(&None);
                    auths.remove(&None);
                    dialogs.remove(&None);
                    shell = None;
                    let _ = display_wake.send_event(Cmd::Show);
                }
                Cmd::Close => {
                    asked_to_end = true;
                    *control = ControlFlow::Exit;
                }
                Cmd::Snip { tool, delay } => {
                    snip_gen += 1;
                    let press = snip_gen;
                    *snip_tool.borrow_mut() = tool.clone();
                    if snip.is_none() {
                        match SnipWindow::build(elwt, &mut pages, &snip_wake, &snip_own, &snip_tx) {
                            Ok(w) => snip = Some(w),
                            Err(e) => {
                                shikisha_core::append_hook_log(&format!("snip: no window for the tool: {e:#}"));
                                return;
                            }
                        }
                    }
                    let Some(w) = snip.as_ref() else { return };
                    if delay == 0 {
                        // Nothing to wait for: the picture is of what is on
                        // screen now, this program included
                        w.shoot(&snip_base, &tool);
                        return;
                    }
                    w.wait(&snip_base, &tool, delay);
                    let wake = snip_wake.clone();
                    std::thread::spawn(move || {
                        for left in (0..delay).rev() {
                            std::thread::sleep(std::time::Duration::from_secs(1));
                            if wake.send_event(Cmd::SnipTick { press, left }).is_err() {
                                return;
                            }
                        }
                    });
                }
                Cmd::SnipTick { press, left } => {
                    if press != snip_gen {
                        return;
                    }
                    let Some(w) = snip.as_ref() else { return };
                    if left == 0 {
                        let tool = snip_tool.borrow().clone();
                        w.shoot(&snip_base, &tool);
                    } else {
                        w.view.run_js(&format!("window.__snipWait && window.__snipWait({left});"));
                    }
                }
                Cmd::SnipClose => {
                    // A count still running belongs to a tool that is gone
                    snip_gen += 1;
                    snip_aside = false;
                    if let Some(w) = snip.as_ref() {
                        w.put_away();
                    }
                }
                Cmd::SnipAside => {
                    if let Some(w) = snip.as_ref() {
                        snip_aside = true;
                        w.window.set_visible(false);
                    }
                }
                Cmd::SnipBack { how } => {
                    // Closed while the dialog was up: there is nothing to come back to
                    if !std::mem::take(&mut snip_aside) {
                        return;
                    }
                    if let Some(w) = snip.as_ref() {
                        w.window.set_visible(true);
                        w.window.set_focus();
                        let _ = w.view.focus();
                        w.view.run_js(&format!("window.__snipSaved && window.__snipSaved({how:?});"));
                    }
                }
                Cmd::Summon { what } => {
                    let _ = ev_tx.send(Ev::Summon { what });
                }
                Cmd::SnipAnswer { json } => {
                    // The page takes an answer only for a question it is still
                    // waiting on, so one arriving after the tool was closed or
                    // opened again falls on nothing
                    if let Some(w) = snip.as_ref() {
                        w.view.run_js(&format!("window.__snipAnswer && window.__snipAnswer({json});"));
                    }
                }
            },
            // The tool's own window. Checked before anything else about windows:
            // the arms below are the board's, and read no window id -- a tool
            // closed with Alt+F4 would otherwise be the app asked to close,
            // and the tool's size would be handed to the board. Not folded into
            // one pattern: every other event of the tool's window has to stop
            // here as well
            #[allow(clippy::collapsible_match)]
            Event::WindowEvent { window_id, event, .. } if snip.as_ref().is_some_and(|w| w.window.id() == window_id) => {
                if let WindowEvent::CloseRequested = event {
                    snip_gen += 1;
                    if let Some(w) = snip.as_ref() {
                        w.put_away();
                    }
                }
            }
            // The ✕. Not the end of the program: the conductor answers with
            // `Hide` or `Close`, having asked the person if an AI is at work
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                let _ = ev_tx.send(Ev::CloseRequested);
            }
            // The board follows the window's size from here: one place that
            // sets its bounds on every resize, which nothing takes down
            Event::WindowEvent { event: WindowEvent::Resized(size), .. } => {
                // The bar draws its middle button from this. A window can be
                // maximised without that button being pressed -- dragged to
                // an edge, Win+Up -- and a bar that said otherwise would be
                // showing the wrong one of two glyphs
                if let Some(v) = main_view(&shell) {
                    v.run_js(&format!("window.__maximized && window.__maximized({}, {});",
                        window.is_maximized(), window.fullscreen().is_some()));
                }
                // What size to draw the board at. Not the one that arrives
                // while the window is put away: a minimized window is reported
                // as a small rectangle of its own -- 128x220 on one machine,
                // not zero, so a guard against zero never saw it -- and the
                // board laid itself out to it, handed it down to the page it
                // was relaying, and the phone watching from another room was
                // sent that page 128 pixels wide, one character per line.
                // Nobody is looking at this window while it is away, and its
                // size then says nothing about how wide a page should be, so
                // the last size that meant something is held instead
                let (w, h) = if window.is_minimized() {
                    match last_size {
                        Some(r) => r,
                        None => return,
                    }
                } else if size.width > 0 && size.height > 0 {
                    last_size = Some((size.width, size.height));
                    (size.width, size.height)
                } else {
                    return;
                };
                if let Some(v) = main_view(&shell) {
                    v.fill(w, h);
                }
                // A relay uses the viewer's dimensions, independent of this
                // window. Its override ends when the viewer stops watching.
            }
            // Chromium places its popups (select lists, tooltips) by where it
            // last heard the window was, and a moved window whose page was
            // never told drops them in the old place
            Event::WindowEvent { event: WindowEvent::Moved(_), .. } => {
                if let Some(v) = main_view(&shell) {
                    v.window_moved();
                }
            }
            // The system is ending the session: signing out, restarting,
            // shutting down, or an installer asking programs to let go of
            // their files (see `frame::session_ending`)
            Event::LoopDestroyed if !asked_to_end => super::frame::session_ending(),
            // A Mac's Dock icon pressed while the window is put away: the same
            // as the menu bar's Open
            #[cfg(target_os = "macos")]
            Event::Reopen { has_visible_windows: false, .. } => {
                let _ = ev_tx.send(Ev::TrayOpen);
            }
            _ => {}
        }
    });
    // The pages went with the loop; the engine is let go after them
    super::engine::wind_down();

    if let Some(tray) = &tray {
        tray.remove();
    }
    let _ = closed_tx.send(Ev::Closed);
    Ok(())
}

/// The window a tool from the left bar is drawn in.
///
/// Its own top-level window, not a page inside the board: the picture is of the
/// whole screen, and the tool is laid over exactly that screen, at its pixels,
/// in front of everything -- the board included, which may be what is being
/// pictured. Borderless, off the taskbar, and kept hidden between uses so the
/// next press does not wait for a browser to start.
///
/// The page is dropped from first, then the window: a page outliving the
/// window it is drawn into is a page drawing into nothing
struct SnipWindow {
    view: Page,
    window: tao::window::Window,
}

impl SnipWindow {
    fn build(
        target: &tao::event_loop::EventLoopWindowTarget<Cmd>,
        pages: &mut Pages,
        wake: &tao::event_loop::EventLoopProxy<Cmd>,
        own: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        ask: &Sender<Ev>,
    ) -> Result<Self> {
        let window = super::frame::tool_builder()
            .with_title("SHIKISHA-TERM")
            .with_decorations(false)
            .with_always_on_top(true)
            .with_visible(false)
            .with_inner_size(tao::dpi::LogicalSize::new(280.0, 72.0))
            .build(target)?;
        let wake = wake.clone();
        let own = std::rc::Rc::clone(own);
        let ask = ask.clone();
        let view = pages.fill(
            &window,
            Spec {
                url: "about:blank".into(),
                place: Place::Fill,
                init_js: WINDOW_POST.to_string(),
                user_agent: None,
                // Its own folder beside the board's: a second page on the
                // board's folder would have to be opened with exactly the
                // board's options, and a mismatch there is a page that
                // silently never comes up
                store: shell_data_dir().join("snip"),
                through: None,
                focus: true,
                background: Some((0, 0, 0, 255)),
                wiring: Wiring {
                    ipc: std::rc::Rc::new(move |at: &str, body: &str| {
                        // Heard only from the app's own address. The tool page is the
                        // app's; anything else in this window is not the tool
                        if !from_ours(&own.borrow(), at) {
                            return;
                        }
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
                            return;
                        };
                        let text = || v.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_string();
                        let picture = || {
                            use base64::Engine as _;
                            let b64 = v.get("png").and_then(|p| p.as_str())?;
                            base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).ok()
                        };
                        match v.get("act").and_then(|a| a.as_str()) {
                            Some("copy") => {
                                if !crate::snip::copy_text(&text()) {
                                    shikisha_core::append_hook_log("snip: the clipboard would not take the text");
                                }
                            }
                            Some("save") => {
                                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("snip.txt").to_string();
                                // The window goes first, so the dialog is not opened
                                // behind a window that stays in front of everything
                                let _ = wake.send_event(Cmd::SnipClose);
                                crate::snip::save_text(text(), name);
                            }
                            Some("close") => {
                                let _ = wake.send_event(Cmd::SnipClose);
                            }
                            // The edited picture. The clipboard takes it as pixels, so
                            // it pastes into anything that takes a picture
                            Some("copy_image") => {
                                if !picture().is_some_and(|png| crate::snip::copy_image(&png)) {
                                    shikisha_core::append_hook_log("snip: the clipboard would not take the picture");
                                }
                            }
                            // Saved without closing: the window steps aside for the
                            // dialog, which would otherwise open behind it, and comes
                            // back with the picture still being edited
                            Some("save_image") => {
                                let Some(png) = picture() else { return };
                                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("snip.png").to_string();
                                let _ = wake.send_event(Cmd::SnipAside);
                                let back = wake.clone();
                                crate::snip::save_file(png, name, "snip.save.title_image", ("PNG", "png"), move |how| {
                                    let _ = back.send_event(Cmd::SnipBack { how: how.word() });
                                });
                            }
                            // A question about the assistant AI. Which desk it is
                            // asked for is the conductor's to know, not this window's
                            Some("ask") => {
                                if let Some(msg) = v.get("msg") {
                                    let _ = ask.send(Ev::SnipAsk { msg: msg.to_string() });
                                }
                            }
                            _ => {}
                        }
                    }),
                    loading: None,
                    died: None,
                    closing: None,
                    popups: None,
                    downloads: None,
                    find_keys: None,
                },
            },
        )?;
        Ok(Self { view, window })
    }

    /// The small card that counts down, in the top corner of the screen the
    /// pointer is on. Left out of the picture, and letting the pointer through,
    /// so the person can arrange the screen underneath it
    fn wait(&self, base: &str, tool: &str, delay: u8) {
        let Some(scr) = crate::snip::screen_at_pointer() else { return };
        let scale = self.scale_at(scr);
        let (w, h) = ((280.0 * scale) as i32, (72.0 * scale) as i32);
        let margin = (16.0 * scale) as i32;
        self.window.set_outer_position(tao::dpi::PhysicalPosition::new(scr.x + scr.w - w - margin, scr.y + margin));
        self.window.set_inner_size(tao::dpi::PhysicalSize::new(w as u32, h as u32));
        super::frame::keep_out_of_pictures(&self.window, true);
        let _ = self.window.set_ignore_cursor_events(true);
        let _ = self.view.load(&format!("{base}/snip?tool={}&wait={delay}", pct(tool)));
        self.window.set_visible(true);
    }

    /// Take the picture, and lay the tool over the screen it was taken of
    fn shoot(&self, base: &str, tool: &str) {
        let Some(scr) = crate::snip::screen_at_pointer() else { return };
        let Some(bmp) = crate::snip::take(scr) else {
            shikisha_core::append_hook_log("snip: the screen could not be taken");
            self.put_away();
            return;
        };
        let n = crate::snip::hold(bmp);
        self.window.set_visible(false);
        let _ = self.window.set_ignore_cursor_events(false);
        // Taken already: the tool itself may be pictured by the next press
        super::frame::keep_out_of_pictures(&self.window, false);
        self.window.set_outer_position(tao::dpi::PhysicalPosition::new(scr.x, scr.y));
        self.window.set_inner_size(tao::dpi::PhysicalSize::new(scr.w as u32, scr.h as u32));
        let _ = self.view.load(&format!("{base}/snip?tool={}&n={n}", pct(tool)));
        self.window.set_visible(true);
        self.window.set_focus();
        let _ = self.view.focus();
    }

    /// Gone from the screen, and the picture with it
    fn put_away(&self) {
        self.window.set_visible(false);
        let _ = self.window.set_ignore_cursor_events(false);
        super::frame::keep_out_of_pictures(&self.window, false);
        crate::snip::drop_frame();
        let _ = self.view.load("about:blank");
    }

    /// How large a point is on the screen the tool is about to be put on.
    fn scale_at(&self, scr: crate::snip::Screen) -> f64 {
        self.window
            .available_monitors()
            .find(|m| {
                let p = m.position();
                p.x == scr.x && p.y == scr.y
            })
            .map(|m| m.scale_factor())
            .unwrap_or(1.0)
    }
}

/// A word safe to put in an address as it is.
fn pct(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect()
}
