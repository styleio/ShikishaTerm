//! The window's engine on Windows: WebView2, which is Chromium, driven
//! through its COM interfaces and the DevTools protocol (CDP).
//!
//! Everything that touches a WebView2, an HWND or the CDP of a page is here,
//! and only here: the window's loop (`run_window`), the pages it holds, the
//! tool window over a picture of the screen, and the CDP calls. The rest of
//! `browser` -- the handle the program holds, the commands it sends, the
//! reports it hears -- is the same on every system, and reaches this through
//! the few names `browser.rs` takes from it.

use super::*;

/// The WebView2 runtime on this machine, if it has one.
///
/// Every window this program opens is a WebView2, so a machine without the
/// runtime cannot show anything at all -- including the message saying why.
/// That is why this is asked before the first window rather than discovered
/// through its failure: by then there is nowhere left to say it.
///
/// Windows 11 carries the runtime in the box. Windows 10 does not, and neither
/// does an image somebody has stripped, which is where this was found.
pub fn runtime_version() -> Option<String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::GetAvailableCoreWebView2BrowserVersionString;
    use windows::core::{PCWSTR, PWSTR};

    let mut raw = PWSTR::null();
    unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut raw) }.ok()?;
    let v = webview2_com::take_pwstr(raw);
    (!v.is_empty()).then_some(v)
}

/// Windows the pages asked for, by the page that asked. Newest last.
///
/// A pane holds one page at a time, so a window opened by one of them stands
/// in the same seat, in front of it. Which is also what it means to everything
/// else: "this pane" is what a person is looking at and what automation is
/// driving, and while a sign-in window is up, that is the sign-in window.
type Overlays = std::collections::HashMap<String, Vec<(String, wry::WebView)>>;

/// The board's page, while there is one (none while the window is put away)
fn main_view(shell: &Option<(wry::WebContext, wry::WebView)>) -> Option<&wry::WebView> {
    shell.as_ref().map(|(_, view)| view)
}

fn target<'a>(
    main: Option<&'a wry::WebView>,
    children: &'a std::collections::HashMap<String, wry::WebView>,
    overlays: &'a Overlays,
    to: &Option<String>,
) -> Option<&'a wry::WebView> {
    match to {
        None => main,
        Some(name) => overlays
            .get(name)
            .and_then(|stack| stack.last())
            .map(|(_, v)| v)
            .or_else(|| children.get(name)),
    }
}

/// What a new-window handler has to hand back to the loop it cannot touch.
#[derive(Default)]
struct Adoptions {
    /// (the page that asked, the new page's name, the page itself)
    made: Vec<(String, String, wry::WebView)>,
    /// Names of adopted pages that asked to be let go
    shut: Vec<String>,
    /// Counts the names apart
    next: u32,
}

/// One command line for the whole answer, because the answer has to be given
/// synchronously and this is the only place it can be built.
type WindowAnswer = Box<dyn Fn(String, wry::NewWindowFeatures) -> wry::NewWindowResponse>;

/// Take in a window a page asked for, rather than refusing it in silence.
///
/// wry's answer to `window.open` when nobody says otherwise is "no", with
/// nothing said: the call returns null, the `target="_blank"` link does
/// nothing, and a sign-in button that hands off to a popup — which is most of
/// them — is a button that does nothing at all. There is no error for anyone
/// to read, on either side.
///
/// So the window is made here and handed back, which is what keeps the two
/// pages related: same environment, real opener, so `window.opener`,
/// `postMessage` and the closing handshake all still work. It is placed in
/// the seat its opener occupies, because a pane is where a person is looking.
fn adopt_windows(
    opener: String,
    seat: std::rc::Rc<std::cell::Cell<(i32, i32, i32, i32)>>,
    window: std::rc::Rc<tao::window::Window>,
    inbox: std::rc::Rc<std::cell::RefCell<Adoptions>>,
    proxy: tao::event_loop::EventLoopProxy<Cmd>,
    ev_tx: Sender<Ev>,
    // A sign-in window that called itself something else than the page that
    // opened it would be a second browser arriving in the middle of a login
    user_agent: Option<String>,
) -> WindowAnswer {
    use wry::{NewWindowResponse, WebViewBuilder, WebViewBuilderExtWindows};
    Box::new(move |uri, features| {
        let name = {
            let mut ib = match inbox.try_borrow_mut() {
                Ok(ib) => ib,
                // Never make a window while the loop is in the middle of
                // counting them. Refusing is what happens today anyway
                Err(_) => return NewWindowResponse::Deny,
            };
            ib.next += 1;
            format!("{opener}#window{}", ib.next)
        };
        // No URL is given: the runtime navigates the window it is handed, and
        // setting one here would load the page twice
        let ipc_name = name.clone();
        let ipc_inbox = std::rc::Rc::clone(&inbox);
        let ipc_proxy = proxy.clone();
        let nav_tx = ev_tx.clone();
        let nav_who = opener.clone();
        let fin_tx = ev_tx.clone();
        let fin_who = opener.clone();
        let mut b = WebViewBuilder::new();
        if let Some(ua) = user_agent.as_deref() {
            b = b.with_user_agent(ua);
        }
        let built = b
            .with_environment(features.opener.environment.clone())
            .with_bounds(to_rect(seat.get()))
            .with_focused(may_take_focus(&window))
            .with_initialization_script(format!("{}{PLACED_JS}{POPUP_JS}", *INIT_JS))
            // Reported as the pane, not as itself: what the bar above the pane
            // should say is loading is whatever the pane is showing
            .with_navigation_handler(move |_url| {
                let _ = nav_tx.send(Ev::Loading { from: Some(nav_who.clone()), busy: true });
                true
            })
            .with_on_page_load_handler(move |e, _url| {
                if matches!(e, wry::PageLoadEvent::Finished) {
                    let _ = fin_tx.send(Ev::Loading { from: Some(fin_who.clone()), busy: false });
                }
            })
            .with_ipc_handler(move |req| {
                let body: &str = req.body();
                if !body.contains("popupclose") {
                    return;
                }
                if let Ok(mut ib) = ipc_inbox.try_borrow_mut() {
                    ib.shut.push(ipc_name.clone());
                    let _ = ipc_proxy.send_event(Cmd::Adopt);
                }
            })
            // A window opened by a window belongs to the same seat
            .with_new_window_req_handler(adopt_windows(
                opener.clone(),
                std::rc::Rc::clone(&seat),
                std::rc::Rc::clone(&window),
                std::rc::Rc::clone(&inbox),
                proxy.clone(),
                ev_tx.clone(),
                user_agent.clone(),
            ))
            .build_as_child(&*window);
        match built {
            Ok(v) => {
                let raw = cdp::webview_of(&v);
                if let Some(ua) = user_agent.as_deref() {
                    cdp::call(&raw, "Emulation.setUserAgentOverride", &ua_override(ua));
                }
                // A link that saves a file often opens a window to do it in:
                // what it saves is the pane's, like everything else it does
                cdp::arm_downloads(&raw, Some(opener.clone()), ev_tx.clone());
                cdp::on_find_keys(&v, opener.clone(), ev_tx.clone(), |_| false);
                {
                    let inbox = std::rc::Rc::clone(&inbox);
                    let proxy = proxy.clone();
                    let name = name.clone();
                    cdp::on_close_requested(&raw, move || {
                        if let Ok(mut ib) = inbox.try_borrow_mut() {
                            ib.shut.push(name.clone());
                            let _ = proxy.send_event(Cmd::Adopt);
                        }
                    });
                }
                shikisha_core::append_hook_log(&format!(
                    "[browser] '{opener}' opened a window -> '{name}' ({uri})"
                ));
                if let Ok(mut ib) = inbox.try_borrow_mut() {
                    ib.made.push((opener.clone(), name, v));
                }
                let _ = proxy.send_event(Cmd::Adopt);
                NewWindowResponse::Create { webview: raw }
            }
            Err(e) => {
                shikisha_core::append_hook_log(&format!(
                    "[browser] '{opener}' asked for a window ({uri}) and it could not be made: {e}"
                ));
                NewWindowResponse::Deny
            }
        }
    })
}


/// Convert a position and size into wry's shape
fn to_rect((x, y, w, h): (i32, i32, i32, i32)) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(x, y).into(),
        size: wry::dpi::LogicalSize::new(w.max(0), h.max(0)).into(),
    }
}


/// The window's handle, for the few things that must be told which window
/// they belong to (the Store's update dialog)
static MAIN_HWND: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

pub fn main_hwnd() -> isize {
    MAIN_HWND.get().copied().unwrap_or(0)
}

/// Put our own icon on the window.
///
/// A window that never says which icon it wants gets Windows' default — the
/// grey generic window shape — in its title bar and in Alt+Tab, while the
/// taskbar goes and finds the one inside the exe. Two pictures for one program,
/// and the one in the corner of our own window was not even ours.
///
/// It is loaded out of our own resource (id 1, which is where `build.rs` puts
/// `assets\icon.ico`) rather than from a file beside the exe: one copy of the
/// artwork, nothing extra to ship, and nothing that can go missing.
///
/// Each size is asked for by name. An icon file holds several drawings, and
/// `LoadImage` picks the one nearest what it is asked for; asking for
/// "whatever" (0, 0) takes the first entry and squeezes it, which is how a
/// 256-pixel drawing ends up as a smear in a 16-pixel corner.
fn wear_our_own_icon(hwnd: isize) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW,
        SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
    };
    use shikisha_core::tray::OUR_ICON;
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let hwnd = hwnd as *mut std::ffi::c_void;
        let wear = |which: u32, w: i32, h: i32| {
            let icon = LoadImageW(module, OUR_ICON, IMAGE_ICON, w, h, LR_DEFAULTCOLOR);
            if !icon.is_null() {
                SendMessageW(hwnd, WM_SETICON, which as usize, icon as isize);
            }
        };
        // The title bar and Alt+Tab, then the taskbar and the switcher's big
        // tile. The system is asked how large those are, because it is not the
        // same answer on a 150% display as on a 100% one
        wear(
            ICON_SMALL,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
        );
        wear(
            ICON_BIG,
            GetSystemMetrics(SM_CXICON),
            GetSystemMetrics(SM_CYICON),
        );
    }
}

/// Whether a page in this window may take the keyboard now.
///
/// Always, unless the program was started `--behind`: then only while a person
/// has the window in front. Giving a page the keyboard makes its window the
/// active one, and a window of a program started from the terminal in front
/// is let through to the front by Windows (see `shikisha_core::stays_behind`)
fn may_take_focus(window: &tao::window::Window) -> bool {
    !shikisha_core::stays_behind() || window.is_focused()
}

// `wears_the_icon`: whether this window carries the program's icon in the
// notification area. The one window whose process outlives it does; a client,
// a probe and a self-check do not (see `Browser::spawn_resident`)
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
    use tao::window::WindowBuilder;
    use wry::{WebContext, WebViewBuilder};

    // The program's first thread when it is the program's window (`Browser::host`).
    // A probe holds one from a test's own thread, which only Windows allows
    let mut ev_loop = EventLoopBuilder::<Cmd>::with_user_event();
    tao::platform::windows::EventLoopBuilderExtWindows::with_any_thread(&mut ev_loop, true);
    let mut ev_loop = ev_loop.build();
    proxy_tx
        .send(ev_loop.create_proxy())
        .map_err(|_| anyhow!(shikisha_core::i18n::t("err.browser.proxy_connect_failed")))?;

    // Shared, because a page asking to open a window is answered on the
    // message loop, and the answer is a page built inside this same window
    // The frame is ours to draw. The system keeps what it is better at --
    // resizing from the edges (tao hit-tests them for an undecorated window)
    // and the drop shadow, which `with_undecorated_shadow` asks for by name --
    // and the page draws the bar, because the bar is where the panels are
    // opened from and a system bar has nowhere to put them.
    //
    // What this costs, said plainly: Windows 11's Snap Layouts flyout appears
    // when the pointer rests on a *system* maximize button, and ours is not
    // one. Dragging to an edge, Win+arrow, and double-clicking the bar all
    // still snap, because those are the window manager's, not the button's.
    let window = std::rc::Rc::new({
        let b = WindowBuilder::new()
            .with_title(title)
            .with_decorations(false)
            .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 900.0))
            // Shown without being made the active window (tao shows it with
            // SW_SHOWNOACTIVATE), which is only the first half: see below
            .with_focused(!shikisha_core::stays_behind());
        let b = {
            use tao::platform::windows::WindowBuilderExtWindows;
            b.with_undecorated_shadow(true)
        };
        b.build(&ev_loop)?
    });
    // The second half: a new window lands on top of the pile even when it is
    // not the active one, and there it covers whatever the person is reading.
    // At the bottom it is still on the taskbar, for anyone who wants to watch
    if shikisha_core::stays_behind() {
        use tao::platform::windows::WindowExtWindows;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
        };
        unsafe {
            SetWindowPos(
                window.hwnd() as _,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
    // Whether the screen is down and waiting to be asked for. Read by the
    // icon's handler, which Windows calls from wherever it likes
    let display_down = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The notification-area icon. Its presses, and a second copy's request
    // for the window, arrive through the window's own procedure (see tray.rs)
    let tray = wears_the_icon.then(|| {
        use tao::platform::windows::WindowExtWindows;
        let tell = ev_tx.clone();
        // Pressing the icon while the screen is down means "bring it back",
        // not "show me the window": there is no window to show. A person
        // pressing is never a storm, so whatever was tried is forgotten
        let down = std::sync::Arc::clone(&display_down);
        let revive = ev_loop.create_proxy();
        shikisha_core::tray::Tray::add(
            window.hwnd(),
            title,
            move |pressed| {
                if matches!(pressed, shikisha_core::tray::Pressed::Open)
                    && down.load(std::sync::atomic::Ordering::SeqCst)
                {
                    let _ = revive.send_event(Cmd::DisplayWanted { asked: true });
                    return;
                }
                let _ = match pressed {
                    shikisha_core::tray::Pressed::Open => tell.send(Ev::TrayOpen),
                    shikisha_core::tray::Pressed::Quit => tell.send(Ev::TrayQuit),
                    shikisha_core::tray::Pressed::Nothing => Ok(()),
                };
            },
            &shikisha_core::i18n::t("tray.open"),
            &shikisha_core::i18n::t("tray.quit"),
        )
    });
    {
        use tao::platform::windows::WindowExtWindows;
        wear_our_own_icon(window.hwnd());
        let _ = MAIN_HWND.set(window.hwnd());
    }
    // Watched by the window that wears the icon, and only by it: that is the
    // process holding the work, and the icon is where it can be said. What is
    // said is what is going on and who is holding the memory -- the screen is
    // never taken away for it (see `shikisha_core::pressure`)
    if tray.is_some() {
        let wake = ev_loop.create_proxy();
        shikisha_core::pressure::watch(move |title, text| {
            wake.send_event(Cmd::TrayNotice { title, text }).is_ok()
        });
    }

    // The board's own page. Built here, and built again whenever the window
    // comes back from being put away: while it is away the page is dropped
    // altogether, because the page is where the memory is. Chromium keeps a
    // browser process and a renderer for it -- some two hundred megabytes
    // for an empty board -- and hiding the window releases none of that.
    // The pages placed inside the window (a browser a rally is driving, the
    // settings) are kept: those hold state a person would lose.
    //
    // The shell gets an explicit folder for the same reason the pages do: without
    // one, WebView2 writes "<exe>.WebView2" next to the binary — into the folder
    // that is meant to hold nothing but the exe
    //
    // Every message a page posts is judged by the address it was posted from
    // (WebView2 reports it with the message; it is not something the page
    // writes). The board's own address is the app's, and only a page speaking
    // from one of the app's addresses is heard in full — see `heard`. The
    // settings server's address joins the list when it starts (Cmd::Trust)
    let own = std::rc::Rc::new(std::cell::RefCell::new(vec![url.to_string()]));
    // How a dying page tells this loop. Made before the page that uses it
    let died = ev_loop.create_proxy();
    // The questions put to pages and not yet answered, shared with every
    // page's handler: an answer is taken only from the page that was asked
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Asked::default()));
    let shell_of = {
        let window = std::rc::Rc::clone(&window);
        let ev_tx = ev_tx.clone();
        let url = url.to_string();
        let own = std::rc::Rc::clone(&own);
        let asked = std::rc::Rc::clone(&asked);
        move || -> Result<(WebContext, wry::WebView)> {
            let ipc = ev_tx.clone();
            let own = std::rc::Rc::clone(&own);
            let asked = std::rc::Rc::clone(&asked);
            let refused = std::cell::Cell::new(0u8);
            // The bar the page draws acts on this window, so the handler holds
            // it. Same thread as the message loop, which is where these calls
            // have to be made from anyway
            let win = std::rc::Rc::clone(&window);
            let mut ctx = WebContext::new(Some(shell_data_dir()));
            let view = WebViewBuilder::new_with_web_context(&mut ctx)
                .with_url(&url)
                .with_initialization_script(&*INIT_JS)
                .with_focused(may_take_focus(&window))
                .with_ipc_handler(move |req| {
                    let at = req.uri().to_string();
                    let body: &str = req.body();
                    // The board is the app's own page. Were it ever led to
                    // another site, that site would hold the whole keyboard;
                    // so a board speaking from anywhere else is not the board
                    if !from_ours(&own.borrow(), &at) {
                        note_refused(&refused, None, &at, body);
                        return;
                    }
                    let ev = heard(body, None, true, &mut asked.borrow_mut());
                    // The bar asking this window to do something to itself.
                    // Answered on the spot: the loop that would otherwise be
                    // told is a tick away, and a drag that starts a tick late
                    // is a drag the pointer has already left behind
                    if let Some(Ev::Window { act }) = ev.as_ref() {
                        shikisha_core::append_hook_log(&format!("window act: {act}"));
                        match act.as_str() {
                            "drag" => {
                                let _ = win.drag_window();
                            }
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
                })
                .build(&*window)?;
            // Hear about it if this page's own processes die. Set here, where
            // the page is made, so that a page made again after one died is
            // watched too -- the second failure is the one that matters
            cdp::on_process_failed(&cdp::webview_of(&view), {
                let wake = died.clone();
                move |memory| {
                    let _ = wake.send_event(Cmd::DisplayDied { memory });
                }
            });
            Ok((ctx, view))
        }
    };
    let mut shell: Option<(WebContext, wry::WebView)> = Some(shell_of()?);

    // Pages placed inside the same window. Looked up by name
    let mut children: std::collections::HashMap<String, wry::WebView> =
        std::collections::HashMap::new();
    // What the browser calls itself. Read once, here: a name that changed
    // under a page would be a different browser halfway through a login
    let user_agent = shikisha_core::config::user_agent();
    // Windows those pages asked to open, kept in the seat of whoever asked
    let mut overlays: Overlays = std::collections::HashMap::new();
    // How those handlers hand their work back to this loop
    let adoptions = std::rc::Rc::new(std::cell::RefCell::new(Adoptions::default()));
    // ...and how they wake it to come and collect
    let adopt_wake = ev_loop.create_proxy();
    // WebContext per profile. One per data folder (tabs with the same name share it).
    // Not needed after creation on Windows, but keeping it around is harmless, so hold it keyed by folder
    let mut web_ctxs: std::collections::HashMap<std::path::PathBuf, WebContext> =
        std::collections::HashMap::new();
    // Temp folders for children placed in private mode (child name -> folder). Removed on close
    let mut ephemeral_dirs: std::collections::HashMap<String, std::path::PathBuf> =
        std::collections::HashMap::new();

    // Screencasts. One per target. Frames only arrive while this is held
    let mut casts: std::collections::HashMap<Option<String>, cdp::Cast> =
        std::collections::HashMap::new();
    // Compositor wakes for genuine input on hidden pages. A page hidden the
    // normal way (bounds 0×0) has no surface, and mouse input needs one to
    // hit-test against — its ack never comes. For the duration of a wake the
    // page gets a real-sized surface parked outside the client area (never
    // painted, so nothing flickers), plus a tiny throwaway screencast to
    // keep frames flowing. The bool remembers whether the bounds were
    // borrowed, so release restores exactly the layout's last word. Kept
    // separate from `casts` so a phone relay and a wake never fight
    let mut wakes: std::collections::HashMap<Option<String>, (cdp::Cast, bool)> =
        std::collections::HashMap::new();
    // Each child's last layout-given rect: how a wake tells hidden (0×0)
    // from merely unfocused, and what it restores on release
    // Shared per child: a handler that opens a window into this seat reads it
    // without reaching into the loop's own state, and a resize lands on the
    // window that is standing in the seat as well
    let mut child_sizes: std::collections::HashMap<
        String,
        std::rc::Rc<std::cell::Cell<shikisha_shared::Rect>>,
    > = std::collections::HashMap::new();
    // The browser zoom put on a cast target fitted to its viewer (see
    // `cdp::view_metrics`). A press arrives as a share of the picture, and the
    // picture is that many times wider than the page is in its own pixels, so
    // a press is divided by it -- or it lands that many times too far along
    let mut zooms: std::collections::HashMap<Option<String>, f64> = std::collections::HashMap::new();
    // The last size this window had while somebody could see it. What the
    // board is held at while the window is put away
    let mut last_size: Option<(u32, u32)> = None;
    // Basic-auth arming. One per target. Only answers 401s while this is held
    let mut auths: std::collections::HashMap<Option<String>, cdp::AuthArm> =
        std::collections::HashMap::new();
    // Automatic handling of JS dialogs. One per child. Without this, automation freezes on things like "leave this page?" confirmations
    let mut dialogs: std::collections::HashMap<Option<String>, cdp::DialogArm> =
        std::collections::HashMap::new();
    // Pages whose console is being heard. One per page, held until it is let
    // go of or the page closes; the lines leave as reports
    let mut consoles: std::collections::HashMap<Option<String>, cdp::ConsoleArm> =
        std::collections::HashMap::new();
    // The browser's own search, one per page searched, by the name of the view
    // standing in the seat (a window a page opened stands over the page)
    let mut finders: std::collections::HashMap<String, cdp::Finder> = std::collections::HashMap::new();
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
    // being asked is Windows ending the session (see `Event::LoopDestroyed`)
    let mut asked_to_end = false;
    ev_loop.run_return(move |event, elwt, control| {
        *control = ControlFlow::Wait;
        match event {
            Event::UserEvent(cmd) => match cmd {
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
                    // on to its real address WebView2 no longer delivers its
                    // messages -- so everything asked while a sign-in window
                    // was up went unanswered. The DevTools protocol answers
                    // the call that asked, which needs no message at all
                    let standing = to
                        .as_ref()
                        .and_then(|name| overlays.get(name))
                        .and_then(|stack| stack.last())
                        .map(|(_, v)| v);
                    if let Some(v) = standing {
                        let tx = ev_tx.clone();
                        let params = serde_json::json!({
                            "expression": format!("(async function(){{ {js} }})()"),
                            "awaitPromise": true,
                            "returnByValue": true,
                        })
                        .to_string();
                        cdp::call_result(&cdp::webview_of(v), "Runtime.evaluate", &params, move |ok, json| {
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
                        let _ = v.evaluate_script(&wrap_eval(id, &js));
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
                        if !wakes.contains_key(&to) && !casts.contains_key(&to)
                            && let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                                let wv = cdp::webview_of(v);
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
                                    let _ = v.set_bounds(to_rect((-4000, 0, 1280, 900)));
                                }
                                if let Some(cast) =
                                    cdp::start_with(&wv, cdp::WAKE_PARAMS, |_, _, _| {})
                                {
                                    wakes.insert(to.clone(), (cast, hidden));
                                } else if hidden
                                    && let Some(r) = to
                                        .as_ref()
                                        .and_then(|n| child_sizes.get(n))
                                        .map(|seat| seat.get())
                                    {
                                        let _ = v.set_bounds(to_rect(r));
                                    }
                            }
                    } else if let Some((cast, borrowed)) = wakes.remove(&to) {
                        cdp::stop(cast);
                        if borrowed {
                            // Give back whatever the layout last decreed —
                            // including a rect that changed mid-wake
                            if let (Some(v), Some(r)) = (
                                target(main_view(&shell), &children, &overlays, &to),
                                to.as_ref()
                                    .and_then(|n| child_sizes.get(n))
                                    .map(|seat| seat.get()),
                            ) {
                                let _ = v.set_bounds(to_rect(r));
                            }
                        }
                    }
                }
                Cmd::Cdp { id, to, method, params } => {
                    // Same guard as Eval: an unplaced page must answer, not hang
                    if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        let tx = ev_tx.clone();
                        cdp::call_result(
                            &cdp::webview_of(v),
                            &method,
                            &params,
                            move |ok, json| {
                                let _ = tx.send(Ev::Result { id, ok, value: json });
                            },
                        );
                    } else {
                        let _ = ev_tx.send(Ev::Result {
                            id,
                            ok: false,
                            value: shikisha_core::i18n::tp(
                                "err.browser.page_not_placed",
                                &[("to", &to.unwrap_or_default())],
                            ),
                        });
                    }
                }
                Cmd::BasicAuth { to, user, pass } => {
                    // If credentials are already armed, swap them; otherwise enable Fetch and arm them
                    if let Some(arm) = auths.get(&to) {
                        *arm.creds.borrow_mut() = (user, pass);
                    } else if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        let wv = cdp::webview_of(v);
                        match cdp::arm_basic_auth(&wv, &user, &pass) {
                            Some(arm) => {
                                auths.insert(to.clone(), arm);
                            }
                            None => shikisha_core::append_hook_log(&shikisha_core::i18n::t(
                                "err.browser.log_basic_auth_failed",
                            )),
                        }
                    }
                }
                Cmd::Console { to, on } => {
                    if !on {
                        consoles.remove(&to);
                    } else if !consoles.contains_key(&to)
                        && let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                            let tx = ev_tx.clone();
                            let from = to.clone();
                            if let Some(arm) = cdp::arm_console(&cdp::webview_of(v), move |entry| {
                                let _ = tx.send(Ev::ConsoleLine { from: from.clone(), entry });
                            }) {
                                consoles.insert(to, arm);
                            }
                        }
                }
                Cmd::DevtoolsOpen { conn, to, out } => {
                    match target(main_view(&shell), &children, &overlays, &to) {
                        Some(v) => screens.open(conn, to, cdp::webview_of(v), out),
                        None => shikisha_core::append_hook_log(&format!(
                            "[devtools] screen {conn}: no such page here"
                        )),
                    }
                }
                Cmd::DevtoolsSay { conn, text } => screens.say(conn, text),
                Cmd::DevtoolsClose { conn } => screens.close(conn),
                Cmd::AddChild { name, url, rect, profile, through } => {
                    // Creating a WebView2 controller runs synchronously ON THIS
                    // event-loop thread, and this thread also pumps the whole
                    // window's messages — while it runs, every click is frozen.
                    // Log how long it took so a "dead window right after
                    // startup" report can be matched against it.
                    let born = std::time::Instant::now();
                    let bounds = to_rect(rect);
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
                    // This page's own name if it was given one, the app's
                    // otherwise
                    let ua = profile.user_agent.clone().or_else(|| user_agent.clone());
                    let ctx = web_ctxs
                        .entry(data_dir.clone())
                        .or_insert_with(|| WebContext::new(Some(data_dir.clone())));
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
                    let nav_tx = ev_tx.clone();
                    let nav_who = name.clone();
                    let fin_tx = ev_tx.clone();
                    let fin_who = name.clone();
                    let mut b = WebViewBuilder::new_with_web_context(ctx);
                    if let Some(port) = through {
                        shikisha_core::append_hook_log(&format!(
                            "[browser] '{name}' reaches the network through 127.0.0.1:{port}"
                        ));
                    }
                    if let Some(port) = through {
                        use wry::WebViewBuilderExtWindows as _;
                        // `<-loopback>` is the whole point, and it has to be
                        // said out loud: a browser leaves loopback addresses
                        // out of its proxy by default, so `localhost:3000`
                        // would go to *this* machine -- which is the one
                        // mistake this feature exists to prevent. Written as
                        // plain arguments rather than through wry's proxy
                        // setting, which offers no way to say it.
                        //
                        // wry's own defaults are repeated here because naming
                        // arguments replaces them: without them a placed page
                        // gets the mini menu and the smart screen back
                        b = b.with_additional_browser_args(format!(
                            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection                              --proxy-server=http://127.0.0.1:{port}                              --proxy-bypass-list=<-loopback>"
                        ));
                    }
                    if let Some(ua) = ua.as_deref() {
                        b = b.with_user_agent(ua);
                    }
                    match b
                        .with_url(&url)
                        .with_bounds(bounds)
                        .with_focused(may_take_focus(&window))
                        .with_initialization_script(format!("{}{PLACED_JS}", *INIT_JS))
                        .with_navigation_handler(move |_url| {
                            let _ = nav_tx.send(Ev::Loading { from: Some(nav_who.clone()), busy: true });
                            true // Don't block the navigation. This is only here to emit a signal
                        })
                        .with_on_page_load_handler(move |e, _url| {
                            if matches!(e, wry::PageLoadEvent::Finished) {
                                let _ = fin_tx.send(Ev::Loading { from: Some(fin_who.clone()), busy: false });
                            }
                        })
                        // What to do when this page asks to open a window.
                        // Without an answer here the request is refused in
                        // silence, which is how a sign-in popup becomes a
                        // button that does nothing
                        .with_new_window_req_handler(adopt_windows(
                            name.clone(),
                            std::rc::Rc::clone(&seat),
                            std::rc::Rc::clone(&window),
                            std::rc::Rc::clone(&adoptions),
                            adopt_wake.clone(),
                            ev_tx.clone(),
                            ua.clone(),
                        ))
                        .with_ipc_handler(move |req| {
                            let at = req.uri().to_string();
                            let body: &str = req.body();
                            // The app's own pages (settings, a result view) are
                            // served from the board's address and keep their
                            // full voice. Anything else in this pane is
                            // somebody's website: it may report, not ask
                            let ours = from_ours(&ipc_own.borrow(), &at);
                            // There's no way to know who pressed it except here
                            let ev = heard(body, Some(&who), ours, &mut ipc_asked.borrow_mut());
                            match ev {
                                Some(ev) => {
                                    let _ = ipc.send(ev);
                                }
                                None => note_refused(&refused, Some(&who), &at, body),
                            }
                        })
                        .build_as_child(&*window)
                    {
                        Ok(v) => {
                            // Arm automatic dialog handling right after placing it (don't let "leave page?" freeze it)
                            let wvh = cdp::webview_of(&v);
                            // ...and say the same thing about who we are in
                            // the other place it gets asked
                            if let Some(ua) = ua.as_deref() {
                                cdp::call(&wvh, "Emulation.setUserAgentOverride", &ua_override(ua));
                            }
                            if let Some(arm) = cdp::arm_dialogs(&wvh) {
                                dialogs.insert(Some(name.clone()), arm);
                            }
                            // What it saves goes in the list beside it, and its
                            // Ctrl+F opens the board's search bar
                            if !cdp::arm_downloads(&wvh, Some(name.clone()), ev_tx.clone()) {
                                shikisha_core::append_hook_log(&format!(
                                    "[browser] '{name}': this WebView2 cannot report downloads; its own bubble shows them"
                                ));
                            }
                            {
                                let own = std::rc::Rc::clone(&own);
                                cdp::on_find_keys(&v, name.clone(), ev_tx.clone(), move |at| from_ours(&own.borrow(), at));
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
                        use windows_sys::Win32::UI::WindowsAndMessaging::{
                            HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
                        };
                        use wry::WebViewExtWindows;
                        unsafe {
                            SetWindowPos(
                                v.hwnd().0,
                                HWND_TOP,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                            );
                        }
                    }
                }
                Cmd::ChildBounds { name, rect } => {
                    // The other half of the same rule as Resized: a minimized
                    // window's layout is not a layout anybody asked for, and a
                    // page being watched from a phone must not be re-shaped by
                    // it. The seat is left alone as well, so what comes back
                    // when the window does is the size it had
                    if !window.is_minimized()
                        && let Some(v) = children.get(&name) {
                        let _ = v.set_bounds(to_rect(rect));
                        match child_sizes.get(&name) {
                            Some(seat) => seat.set(rect),
                            None => {
                                child_sizes.insert(
                                    name.clone(),
                                    std::rc::Rc::new(std::cell::Cell::new(rect)),
                                );
                            }
                        }
                        // A window this page opened stands in the same seat,
                        // so it is moved and hidden by the same layout
                        for (_, v) in overlays.get(&name).into_iter().flatten() {
                            let _ = v.set_bounds(to_rect(rect));
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
                    // its throwaway folder. WebView2 can take a moment to
                    // release the lock, so this is best-effort
                    // (anything missed gets swept up by sweep_private at startup)
                    if let Some(dir) = ephemeral_dirs.remove(&name) {
                        web_ctxs.remove(&dir);
                        erase_when_released(dir);
                    }
                }
                Cmd::Focus { to } => {
                    // A window that is put away cannot take focus, and
                    // Windows says so with a bare 0x80070057. Asking anyway
                    // wrote two lines of that into the log every time a phone
                    // touched a tab while the PC was minimized -- an error
                    // where nothing was wrong, next to the ones that matter
                    if !window.is_minimized()
                        && may_take_focus(&window)
                        && let Some(v) = target(main_view(&shell), &children, &overlays, &to)
                        && let Err(e) = v.focus() {
                            shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                                "err.browser.log_focus_failed",
                                &[("to", &format!("{to:?}")), ("e", &format!("{e}"))],
                            ));
                        }
                }
                Cmd::Move { to, go } => match target(main_view(&shell), &children, &overlays, &to) {
                    Some(v) => {
                        let r = match &go {
                            Go::Back => v.go_back(),
                            Go::Forward => v.go_forward(),
                            Go::Reload => v.reload(),
                            // wry's reload is the ordinary one. The flag that
                            // says "ignore what you already have" exists only
                            // in the DevTools protocol, so that is where this
                            // one goes
                            Go::Hard => {
                                cdp::call(
                                    &cdp::webview_of(v),
                                    "Page.reload",
                                    "{\"ignoreCache\":true}",
                                );
                                Ok(())
                            }
                            Go::To(u) => v.load_url(u),
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
                            let wv = cdp::webview_of(v);
                            let finder = finders
                                .entry(key)
                                .or_insert_with(|| cdp::Finder::of(&wv, to.clone(), ev_tx.clone()));
                            finder.seek(&wv, &text, step, to, ev_tx.clone());
                        }
                        None => {
                            let _ = ev_tx.send(Ev::Seek { from: to, at: 0, of: 0 });
                        }
                    }
                }
                Cmd::CancelDownload { id } => cdp::cancel_download(&id),
                Cmd::FindKeys { to: Some(name), on } => cdp::find_keys_for(&name, on),
                Cmd::FindKeys { to: None, .. } => {}
                Cmd::Where { to } => {
                    if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        let _ = where_tx.send(Ev::Where {
                            from: to,
                            url: v.url().unwrap_or_default(),
                            can_back: v.can_go_back().unwrap_or(false),
                            can_forward: v.can_go_forward().unwrap_or(false),
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
                                cdp::kick(&cdp::webview_of(view));
                            }
                        } else if let Some(view) = target(main_view(&shell), &children, &overlays, &to) {
                            let wv = cdp::webview_of(view);
                            let tx = ev_tx.clone();
                            let from = to.clone();
                            let dims = cast_dims.clone();
                            // Whose sound goes with this picture. Asked
                            // once, here, because this is where the page is
                            let mut plays: u32 = 0;
                            let _ = unsafe { wv.BrowserProcessId(&mut plays) };
                            sound_pid.store(plays, std::sync::atomic::Ordering::SeqCst);
                            if let Some(cast) = cdp::start(&wv, move |data, w, h| {
                                dims.set((w, h));
                                let _ = tx.send(Ev::Frame {
                                    from: from.clone(),
                                    data,
                                    w: w as u32,
                                    h: h as u32,
                                });
                            }) {
                                casts.insert(to.clone(), cast);
                            } else {
                                shikisha_core::append_hook_log(&shikisha_core::i18n::t(
                                    "err.browser.log_screencast_failed",
                                ));
                            }
                        }
                    } else if let Some(cast) = casts.remove(&to) {
                        sound_pid.store(0, std::sync::atomic::Ordering::SeqCst);
                        // Give the page its own shape back before the stream goes away
                        if zooms.remove(&to).is_some()
                            && let Some(view) = target(main_view(&shell), &children, &overlays, &to) {
                                let wv = cdp::webview_of(view);
                                cdp::call(&wv, "Emulation.clearDeviceMetricsOverride", "{}");
                                let _ = view.zoom(1.0);
                            }
                        cdp::stop(cast);
                    }
                }
                Cmd::Inject { to, input } => {
                    if let Some(view) = target(main_view(&shell), &children, &overlays, &to) {
                        let wv = cdp::webview_of(view);
                        let (cw, ch) = cast_dims.get();
                        // The page's own pixels a press is counted in (see `zooms`)
                        let z = zooms.get(&to).copied().unwrap_or(1.0);
                        let (cw, ch) = (cw / z, ch / z);
                        match input {
                            Input::Mouse { phase, x, y, down, clicks } => {
                                let (ev, held) = shikisha_core::cdp::mouse_event(
                                    &phase, x * cw, y * ch, down, mouse_down, clicks,
                                );
                                mouse_down = held;
                                cdp::call(&wv, "Input.dispatchMouseEvent", &ev.to_string());
                            }
                            Input::Wheel { x, y, dx, dy } => {
                                let ev = shikisha_core::cdp::wheel_event(x * cw, y * ch, dx, dy);
                                cdp::call(&wv, "Input.dispatchMouseEvent", &ev.to_string());
                            }
                            Input::Text { text } => {
                                for ev in shikisha_core::cdp::text_events(&text) {
                                    cdp::call(&wv, "Input.dispatchKeyEvent", &ev.to_string());
                                }
                            }
                            Input::View { w, h, dpr } => {
                                // The viewer supplies the layout size. Measuring from
                                // the PC's frame kept sites at desktop width and made
                                // a minimized window determine the phone's layout.
                                let zoom = shikisha_core::cdp::viewer_zoom(w, h, dpr);
                                if let Some(m) = shikisha_core::cdp::view_metrics(w, h, zoom) {
                                    cdp::call(&wv, "Emulation.setDeviceMetricsOverride", &m.to_string());
                                    let _ = view.zoom(zoom);
                                    zooms.insert(to.clone(), zoom);
                                    // A static page may have sent its one frame
                                    // before the new metrics took effect.
                                    if casts.contains_key(&to) {
                                        cdp::kick(&wv);
                                    }
                                }
                            }
                            Input::Key { named, ctrl, alt } => {
                                for ev in shikisha_core::cdp::key_events(&named, ctrl, alt) {
                                    cdp::call(&wv, "Input.dispatchKeyEvent", &ev.to_string());
                                }
                            }
                        }
                    }
                }
                Cmd::Hide => {
                    window.set_visible(false);
                    // Let go of the board's page, and of everything holding a
                    // reference to it -- a reference kept would keep Chromium's
                    // processes alive, and with them the memory this is for
                    wakes.remove(&None);
                    casts.remove(&None);
                    auths.remove(&None);
                    dialogs.remove(&None);
                    // The page's container window outlives the page: wry
                    // destroys it for a child page and not for a top-level
                    // one (wry 0.56), so every put-away left one more empty
                    // WRY_WEBVIEW child under the board that came back
                    let stale = {
                        use wry::WebViewExtWindows;
                        main_view(&shell).map(|v| v.hwnd().0)
                    };
                    shell = None;
                    if let Some(h) = stale {
                        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;
                        let _ = unsafe { DestroyWindow(h) };
                    }
                    shikisha_core::append_hook_log("Board page dropped while the window is put away");
                }
                Cmd::Show => {
                    // The window first, the page second. A window put away from
                    // minimized comes back minimized, and a page built into a
                    // window with no size is refused (0x80070057): the board
                    // never came back, and what returned was an empty frame
                    // with any placed pages floating in it
                    window.set_visible(true);
                    if window.is_minimized() {
                        window.set_minimized(false);
                    }
                    if shell.is_none() {
                        let born = std::time::Instant::now();
                        match shell_of() {
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
                                {
                                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                                        HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                                        SetWindowPos,
                                    };
                                    use wry::WebViewExtWindows;
                                    let raise = |v: &wry::WebView| unsafe {
                                        SetWindowPos(
                                            v.hwnd().0,
                                            HWND_TOP,
                                            0,
                                            0,
                                            0,
                                            0,
                                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                        );
                                    };
                                    for v in children.values() {
                                        raise(v);
                                    }
                                    for stack in overlays.values() {
                                        for (_, v) in stack {
                                            raise(v);
                                        }
                                    }
                                }
                                shikisha_core::append_hook_log(&format!(
                                    "Board page built again in {} ms",
                                    born.elapsed().as_millis()
                                ));
                            }
                            Err(e) => shikisha_core::append_hook_log(&format!(
                                "Board page could not be built again: {e:#}"
                            )),
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
                    shikisha_core::append_hook_log(&format!(
                        "the screen stopped (out of memory: {memory})"
                    ));
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
                        match SnipWindow::build(elwt, &snip_wake, &snip_own, &snip_tx) {
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
                        let _ = w.view.evaluate_script(&format!("window.__snipWait && window.__snipWait({left});"));
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
                        let _ = w.view.evaluate_script(&format!("window.__snipSaved && window.__snipSaved({how:?});"));
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
                        let _ = w.view.evaluate_script(&format!("window.__snipAnswer && window.__snipAnswer({json});"));
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
            Event::WindowEvent { window_id, event, .. }
                if snip.as_ref().is_some_and(|w| w.window.id() == window_id) =>
            {
                if let WindowEvent::CloseRequested = event {
                    snip_gen += 1;
                    if let Some(w) = snip.as_ref() {
                        w.put_away();
                    }
                }
            }
            // The ✕. Not the end of the program: the conductor answers with
            // `Hide` or `Close`, having asked the person if an AI is at work
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                let _ = ev_tx.send(Ev::CloseRequested);
            }
            // The board follows the window's size from here, not from the
            // runtime's own hook. wry sizes a top-level page by subclassing
            // the window for WM_SIZE -- and takes that subclass down again
            // whenever ANY page in the window is dropped, a child page
            // included (InnerWebView::drop detaches the parent subclass
            // unconditionally, wry 0.56). So from the first closed settings
            // page or browser tab onward, maximizing grew the window frame
            // and left the board at the size it was: the "pane widens, the
            // body does not" picture (2026-09-09). Setting the bounds here on
            // every Resized is the same call the hook made, from a place
            // nothing takes down. Doing it twice while the hook still lives
            // costs nothing
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                // The bar draws its middle button from this. A window can be
                // maximised without that button being pressed -- dragged to
                // an edge, Win+Up -- and a bar that said otherwise would be
                // showing the wrong one of two glyphs
                if let Some(v) = main_view(&shell) {
                    let _ = v.evaluate_script(&format!(
                        "window.__maximized && window.__maximized({});",
                        window.is_maximized()
                    ));
                }
                // What size to draw the board at. Not the one that arrives
                // while the window is put away: Windows reports a minimized
                // window as a small rectangle of its own -- 128x220 on this
                // machine, not zero, so a guard against zero never saw it --
                // and the board laid itself out to it, handed it down to the
                // page it was relaying, and the phone watching from another
                // room was sent that page 128 pixels wide, one character per
                // line. Nobody is looking at this window while it is away,
                // and its size then says nothing about how wide a page should
                // be, so the last size that meant something is held instead
                let (w, h) = if window.is_minimized() {
                    // Hold it where it was. Not resizing from here is only
                    // half of it: wry keeps a WM_SIZE hook of its own, and
                    // whatever that makes of a window with no client area is
                    // what the page would be drawn at
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
                    let _ = v.set_bounds(wry::Rect {
                        position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                        size: wry::dpi::PhysicalSize::new(w, h).into(),
                    });
                }
                // A relay uses the viewer's dimensions, independent of this
                // window. Its override ends when the viewer stops watching.
            }
            // The move notice the same hook gave, for the same reason:
            // Chromium places its popups (select lists, tooltips) by where
            // it last heard the window was, and a moved window whose page
            // was never told drops them in the old place. The hook's third
            // duty, handing focus to the board when the window gets it, is
            // left alone on purpose: Windows already gives focus back to
            // the page that had it, and a settings page being typed into
            // must not lose it to the board on every Alt+Tab
            Event::WindowEvent {
                event: WindowEvent::Moved(_),
                ..
            } => {
                use wry::WebViewExtWindows;
                if let Some(v) = main_view(&shell) {
                    let _ = unsafe { v.controller().NotifyParentWindowPositionChanged() };
                }
            }
            // Windows is ending the session: signing out, restarting, shutting
            // down, or an installer asking programs to let go of their files.
            // tao hears `WM_ENDSESSION`, calls the loop finished and keeps
            // pumping -- and the next message any window of this thread gets
            // is a panic ("cannot move state from Destroyed") inside a window
            // procedure, where a panic cannot unwind, so the process aborts.
            // Four of those are in the log, each at a shutdown, each leaving a
            // crash report and a "did not close properly" for the next start
            // to explain. Windows ends the process as soon as this message is
            // answered anyway, so it is ended here, on purpose, before
            // anything else can arrive
            Event::LoopDestroyed if !asked_to_end => {
                shikisha_core::append_hook_log("Windows is ending the session: closing");
                shikisha_core::lastexit::mark_closed();
                std::process::exit(0);
            }
            _ => {}
        }
    });

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
    view: wry::WebView,
    _ctx: wry::WebContext,
    window: tao::window::Window,
}

impl SnipWindow {
    fn build(
        target: &tao::event_loop::EventLoopWindowTarget<Cmd>,
        wake: &tao::event_loop::EventLoopProxy<Cmd>,
        own: &std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        ask: &Sender<Ev>,
    ) -> Result<Self> {
        use tao::platform::windows::WindowBuilderExtWindows;
        let window = tao::window::WindowBuilder::new()
            .with_title("SHIKISHA-TERM")
            .with_decorations(false)
            .with_always_on_top(true)
            .with_visible(false)
            .with_skip_taskbar(true)
            .with_undecorated_shadow(false)
            .with_inner_size(tao::dpi::LogicalSize::new(280.0, 72.0))
            .build(target)?;
        // Its own folder beside the board's: a second page on the board's
        // folder would have to be opened with exactly the board's options,
        // and a mismatch there is a page that silently never comes up
        let mut ctx = wry::WebContext::new(Some(shell_data_dir().join("snip")));
        let wake = wake.clone();
        let own = std::rc::Rc::clone(own);
        let ask = ask.clone();
        let view = wry::WebViewBuilder::new_with_web_context(&mut ctx)
            .with_url("about:blank")
            .with_background_color((0, 0, 0, 255))
            .with_ipc_handler(move |req| {
                // Heard only from the app's own address. The tool page is the
                // app's; anything else in this window is not the tool
                if !from_ours(&own.borrow(), &req.uri().to_string()) {
                    return;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(req.body()) else {
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
            })
            .build(&window)?;
        Ok(Self { view, _ctx: ctx, window })
    }

    /// The small card that counts down, in the top corner of the screen the
    /// pointer is on. Left out of the picture, and letting the pointer through,
    /// so the person can arrange the screen underneath it
    fn wait(&self, base: &str, tool: &str, delay: u8) {
        use tao::platform::windows::WindowExtWindows;
        let Some(scr) = crate::snip::screen_at_pointer() else { return };
        let scale = self.scale_at(scr);
        let (w, h) = ((280.0 * scale) as i32, (72.0 * scale) as i32);
        let margin = (16.0 * scale) as i32;
        self.window.set_outer_position(tao::dpi::PhysicalPosition::new(scr.x + scr.w - w - margin, scr.y + margin));
        self.window.set_inner_size(tao::dpi::PhysicalSize::new(w as u32, h as u32));
        crate::snip::keep_out_of_pictures(self.window.hwnd(), true);
        let _ = self.window.set_ignore_cursor_events(true);
        let _ = self.view.load_url(&format!("{base}/snip?tool={}&wait={delay}", pct(tool)));
        self.window.set_visible(true);
    }

    /// Take the picture, and lay the tool over the screen it was taken of
    fn shoot(&self, base: &str, tool: &str) {
        use tao::platform::windows::WindowExtWindows;
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
        crate::snip::keep_out_of_pictures(self.window.hwnd(), false);
        self.window.set_outer_position(tao::dpi::PhysicalPosition::new(scr.x, scr.y));
        self.window.set_inner_size(tao::dpi::PhysicalSize::new(scr.w as u32, scr.h as u32));
        let _ = self.view.load_url(&format!("{base}/snip?tool={}&n={n}", pct(tool)));
        self.window.set_visible(true);
        self.window.set_focus();
        let _ = self.view.focus();
    }

    /// Gone from the screen, and the picture with it
    fn put_away(&self) {
        use tao::platform::windows::WindowExtWindows;
        self.window.set_visible(false);
        let _ = self.window.set_ignore_cursor_events(false);
        crate::snip::keep_out_of_pictures(self.window.hwnd(), false);
        crate::snip::drop_frame();
        let _ = self.view.load_url("about:blank");
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

/// Screencasting and input injection over CDP (Chrome DevTools Protocol).
///
/// WebView2 is Chromium under the hood, so it speaks the developer-tools
/// protocol. Using it lets us receive "only what changed" as JPEG frames
/// (lighter than VNC), and inject mouse, wheel, and text input as
/// **genuine input** (not synthetic events).
///
/// COM objects are thread-bound, so calls must always be made from the
/// window's event-loop thread (inside `run_window`). Frame notifications also arrive on that same thread.
pub(crate) mod cdp {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceivedEventArgs,
        ICoreWebView2DevToolsProtocolEventReceiver,
    };
    use webview2_com::{
        CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
    };
    use windows::core::{HSTRING, PCWSTR};

    use shikisha_core::cdp::CAST_PARAMS;

    /// Wake parameters: the cheapest cast that still forces the compositor
    /// to produce frames. The frames themselves are thrown away — the point
    /// is that a hidden page becomes able to process (and ack) mouse input
    pub const WAKE_PARAMS: &str =
        "{\"format\":\"jpeg\",\"quality\":10,\"maxWidth\":32,\"maxHeight\":32,\"everyNthFrame\":10}";

    /// What's needed to tear down a screencast (notifications only arrive while this is held)
    pub struct Cast {
        pub receiver: ICoreWebView2DevToolsProtocolEventReceiver,
        pub token: i64,
        pub webview: ICoreWebView2,
    }

    /// Hear the page asking to close its window (`window.close()`).
    ///
    /// The one way a window a script opened can say it is done: its messages
    /// stop arriving once it leaves its first about:blank, and a sign-in
    /// window that closes itself would otherwise stay in front of its page
    /// Be told when one of this page's processes dies, and why.
    ///
    /// A page is drawn by processes of its own, and losing them does not take
    /// the program with it -- the tabs carry on, the agents carry on, and
    /// what is lost is the view. So it is worth hearing about: told, the
    /// window can put the view back.
    ///
    /// The reason matters more than the fact. `OUT_OF_MEMORY` means the
    /// machine had nothing left to give, and a program that answers that by
    /// building another browser is a program making it worse on a timer
    pub fn on_process_failed<F: Fn(bool) + 'static>(webview: &ICoreWebView2, f: F) {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_PROCESS_FAILED_REASON, COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY,
            ICoreWebView2ProcessFailedEventArgs2,
        };
        use windows::core::Interface as _;
        let handler = webview2_com::ProcessFailedEventHandler::create(Box::new(move |_sender, args| {
            // The reason is only there on the second version of these
            // arguments. An older runtime says nothing about why, and
            // "unknown" is treated as "not memory": worth one attempt
            let memory = args
                .as_ref()
                .and_then(|a| a.cast::<ICoreWebView2ProcessFailedEventArgs2>().ok())
                .is_some_and(|a2| {
                    let mut reason = COREWEBVIEW2_PROCESS_FAILED_REASON::default();
                    unsafe { a2.Reason(&mut reason) }.is_ok()
                        && reason == COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY
                });
            f(memory);
            Ok(())
        }));
        let mut token = 0i64;
        unsafe {
            let _ = webview.add_ProcessFailed(&handler, &mut token);
        }
    }

    pub fn on_close_requested<F: Fn() + 'static>(webview: &ICoreWebView2, f: F) {
        let handler = webview2_com::WindowCloseRequestedEventHandler::create(Box::new(move |_sender, _args| {
            f();
            Ok(())
        }));
        let mut token = 0i64;
        unsafe {
            let _ = webview.add_WindowCloseRequested(&handler, &mut token);
        }
    }

    /// Call one CDP method (the result is discarded). `params_json` can just be "{}"
    pub fn call(webview: &ICoreWebView2, method: &str, params_json: &str) {
        let method = HSTRING::from(method);
        let params = HSTRING::from(params_json);
        let handler =
            CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_hr, _json| Ok(())));
        unsafe {
            let _ = webview.CallDevToolsProtocolMethod(
                PCWSTR(method.as_ptr()),
                PCWSTR(params.as_ptr()),
                &handler,
            );
        }
    }

    /// Call one CDP method and hand its result to `done(ok, json)`.
    ///
    /// `done` is guaranteed to run exactly once: either from the completion
    /// handler, or right here when the call can't even be issued (in which
    /// case the handler would never fire and a waiter would hang until its
    /// timeout for no reason)
    pub fn call_result<F>(webview: &ICoreWebView2, method: &str, params_json: &str, done: F)
    where
        F: FnOnce(bool, String) + 'static,
    {
        let method_h = HSTRING::from(method);
        let params = HSTRING::from(params_json);
        let done = std::rc::Rc::new(std::cell::RefCell::new(Some(done)));
        let in_handler = std::rc::Rc::clone(&done);
        let context = method.to_string();
        let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
            move |hr: windows::core::Result<()>, json: String| {
                if let Some(f) = in_handler.borrow_mut().take() {
                    match hr {
                        Ok(()) => f(true, json),
                        Err(e) => f(false, format!("{context}: {e:?} {json}")),
                    }
                }
                Ok(())
            },
        ));
        let issued = unsafe {
            webview.CallDevToolsProtocolMethod(
                PCWSTR(method_h.as_ptr()),
                PCWSTR(params.as_ptr()),
                &handler,
            )
        };
        if let Err(e) = issued
            && let Some(f) = done.borrow_mut().take() {
                f(false, format!("{method}: {e:?}"));
            }
    }

    /// Pull the underlying `ICoreWebView2` out of a wry `WebView`
    pub fn webview_of(view: &wry::WebView) -> ICoreWebView2 {
        use wry::WebViewExtWindows;
        view.webview()
    }

    /// Start screencasting. Calls `on_frame(base64_jpeg, css_w, css_h)`
    /// every time a frame arrives.
    /// This also sends the frame ack automatically (without it, the next frame never comes)
    pub fn start<F>(webview: &ICoreWebView2, on_frame: F) -> Option<Cast>
    where
        F: FnMut(String, f64, f64) + 'static,
    {
        start_with(webview, CAST_PARAMS, on_frame)
    }

    /// `start` with explicit cast parameters (the wake path wants tiny frames)
    pub fn start_with<F>(webview: &ICoreWebView2, params: &str, on_frame: F) -> Option<Cast>
    where
        F: FnMut(String, f64, f64) + 'static,
    {
        let cb = std::rc::Rc::new(std::cell::RefCell::new(on_frame));
        let wv = webview.clone();
        let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(
            move |_sender, args: Option<ICoreWebView2DevToolsProtocolEventReceivedEventArgs>| {
                if let Some(args) = args {
                    let mut raw = windows::core::PWSTR::null();
                    unsafe {
                        if args.ParameterObjectAsJson(&mut raw).is_ok() {
                            let json = webview2_com::take_pwstr(raw);
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                                let data = v
                                    .get("data")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or_default()
                                    .to_string();
                                let meta = v.get("metadata");
                                let w = meta
                                    .and_then(|m| m.get("deviceWidth"))
                                    .and_then(|x| x.as_f64())
                                    .unwrap_or(0.0);
                                let h = meta
                                    .and_then(|m| m.get("deviceHeight"))
                                    .and_then(|x| x.as_f64())
                                    .unwrap_or(0.0);
                                let sid = v.get("sessionId").and_then(|x| x.as_i64()).unwrap_or(0);
                                // Send the ack first, then deliver the frame (avoids stalling the pipe)
                                call(
                                    &wv,
                                    "Page.screencastFrameAck",
                                    &format!("{{\"sessionId\":{sid}}}"),
                                );
                                if !data.is_empty() {
                                    (cb.borrow_mut())(data, w, h);
                                }
                            }
                        }
                    }
                }
                Ok(())
            },
        ));

        let name = HSTRING::from("Page.screencastFrame");
        let mut token = 0i64;
        unsafe {
            let receiver = webview
                .GetDevToolsProtocolEventReceiver(PCWSTR(name.as_ptr()))
                .ok()?;
            receiver
                .add_DevToolsProtocolEventReceived(&handler, &mut token)
                .ok()?;
            call(webview, "Page.enable", "{}");
            call(webview, "Page.startScreencast", params);
            Some(Cast {
                receiver,
                token,
                webview: webview.clone(),
            })
        }
    }

    /// Basic-auth arming (401s only get answered while this is held).
    ///
    /// Receiving auth challenges (authRequired) requires intercepting
    /// requests, so we catch every request via Fetch. A caught, ordinary
    /// request is passed straight through with continueRequest (holding
    /// it forever would stall the page); only auth challenges get
    /// credentials back via continueWithAuth
    pub struct AuthArm {
        pub receivers: Vec<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
        pub webview: ICoreWebView2,
        /// The credentials to return (user, pass). Held shared so it can be swapped out
        pub creds: std::rc::Rc<std::cell::RefCell<(String, String)>>,
    }

    impl Drop for AuthArm {
        /// Catching every request is only safe while somebody is answering
        /// them: `Fetch.enable` left on with nothing subscribed is a page
        /// whose requests are held forever and never continued
        fn drop(&mut self) {
            call(&self.webview, "Fetch.disable", "{}");
            unhook(&self.receivers);
        }
    }

    /// Let go of subscriptions, by the tokens they were made under.
    fn unhook(made: &[(ICoreWebView2DevToolsProtocolEventReceiver, i64)]) {
        for (receiver, token) in made {
            unsafe {
                let _ = receiver.remove_DevToolsProtocolEventReceived(*token);
            }
        }
    }

    /// Subscribe to one CDP event. Calls `on` with the received JSON
    fn subscribe<F>(
        webview: &ICoreWebView2,
        event: &str,
        on: F,
    ) -> Option<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>
    where
        F: Fn(&serde_json::Value) + 'static,
    {
        let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(
            move |_sender, args: Option<ICoreWebView2DevToolsProtocolEventReceivedEventArgs>| {
                if let Some(args) = args {
                    let mut raw = windows::core::PWSTR::null();
                    unsafe {
                        if args.ParameterObjectAsJson(&mut raw).is_ok() {
                            let json = webview2_com::take_pwstr(raw);
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                                on(&v);
                            }
                        }
                    }
                }
                Ok(())
            },
        ));
        let name = HSTRING::from(event);
        let mut token = 0i64;
        unsafe {
            let receiver = webview
                .GetDevToolsProtocolEventReceiver(PCWSTR(name.as_ptr()))
                .ok()?;
            receiver
                .add_DevToolsProtocolEventReceived(&handler, &mut token)
                .ok()?;
            Some((receiver, token))
        }
    }

    /// Hear one event of a page's protocol session, until `unlisten`
    pub fn listen<F>(webview: &ICoreWebView2, event: &str, on: F) -> Option<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>
    where
        F: Fn(&serde_json::Value) + 'static,
    {
        subscribe(webview, event, on)
    }

    pub fn unlisten(made: &(ICoreWebView2DevToolsProtocolEventReceiver, i64)) {
        unhook(std::slice::from_ref(made));
    }

    pub fn arm_basic_auth(webview: &ICoreWebView2, user: &str, pass: &str) -> Option<AuthArm> {
        let creds = std::rc::Rc::new(std::cell::RefCell::new((user.to_string(), pass.to_string())));

        // Pass a caught, ordinary request straight through (not continuing it would stall the page)
        let wv_req = webview.clone();
        let paused = subscribe(webview, "Fetch.requestPaused", move |v| {
            if let Some(id) = v.get("requestId").and_then(|x| x.as_str()) {
                call(&wv_req, "Fetch.continueRequest", &format!("{{\"requestId\":\"{id}\"}}"));
            }
        })?;

        // Return credentials for auth challenges
        let wv_auth = webview.clone();
        let creds_h = std::rc::Rc::clone(&creds);
        let required = subscribe(webview, "Fetch.authRequired", move |v| {
            let id = v.get("requestId").and_then(|x| x.as_str()).unwrap_or_default();
            let (u, p) = {
                let c = creds_h.borrow();
                (c.0.clone(), c.1.clone())
            };
            let params = serde_json::json!({
                "requestId": id,
                "authChallengeResponse": {
                    "response": "ProvideCredentials",
                    "username": u,
                    "password": p,
                }
            })
            .to_string();
            call(&wv_auth, "Fetch.continueWithAuth", &params);
        })?;

        // Catch every request, and auth too
        call(
            webview,
            "Fetch.enable",
            r#"{"patterns":[{"urlPattern":"*"}],"handleAuthRequests":true}"#,
        );
        Some(AuthArm {
            receivers: vec![paused, required],
            webview: webview.clone(),
            creds,
        })
    }

    /// Automatic handling of JS dialogs (alert / confirm / prompt / beforeunload).
    ///
    /// Without this, things like a page's "leave this page?" confirmation
    /// open as a native dialog, the CDP response channel stalls, and
    /// `browser_*` hangs with "no result returned" (automation freezes
    /// entirely). Since this is for automation, the default is
    /// accept=true = proceed: beforeunload means "navigate away", confirm
    /// means OK, alert/prompt means dismiss. Once `Page` is enabled and
    /// this is subscribed, no more native dialogs appear — we close them
    /// immediately instead. Only active while this is held (unsubscribes on drop).
    pub struct DialogArm {
        pub receivers: Vec<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
    }

    impl Drop for DialogArm {
        /// Answering dialogs automatically is this arm's doing, and stops
        /// being anybody's the moment it is let go of
        fn drop(&mut self) {
            unhook(&self.receivers);
        }
    }

    pub fn arm_dialogs(webview: &ICoreWebView2) -> Option<DialogArm> {
        let wv = webview.clone();
        let opening = subscribe(webview, "Page.javascriptDialogOpening", move |_v| {
            call(&wv, "Page.handleJavaScriptDialog", r#"{"accept":true}"#);
        })?;
        // Enable Page so the subscription actually fires (idempotent even if screencast already enabled it)
        call(webview, "Page.enable", "{}");
        Some(DialogArm { receivers: vec![opening] })
    }

    /// Hearing a page's console. Only while this is held: dropping it lets go
    /// of every subscription and turns the log reports back off
    pub struct ConsoleArm {
        pub receivers: Vec<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
        pub webview: ICoreWebView2,
    }

    impl Drop for ConsoleArm {
        /// `Runtime` stays on: the page's automation evaluates through it,
        /// and turning it off would pull it out from under a run. `Log` is
        /// only ever on for this
        fn drop(&mut self) {
            unhook(&self.receivers);
            call(&self.webview, "Log.disable", "{}");
        }
    }

    /// Start hearing a page's console: what its code logs, what it throws
    /// and nobody catches, and what the browser says about it. Each becomes
    /// one line (`console::entry_of`) handed to `on`
    pub fn arm_console<F>(webview: &ICoreWebView2, on: F) -> Option<ConsoleArm>
    where
        F: Fn(serde_json::Value) + 'static,
    {
        let on = std::rc::Rc::new(on);
        let mut receivers = Vec::new();
        for event in shikisha_core::console::EVENTS {
            let tell = std::rc::Rc::clone(&on);
            let heard = subscribe(webview, event, move |v| {
                if let Some(entry) = shikisha_core::console::entry_of(event, v, shikisha_core::sqlite::now_ms()) {
                    tell(entry);
                }
            })?;
            receivers.push(heard);
        }
        for domain in shikisha_core::console::DOMAINS {
            call(webview, &format!("{domain}.enable"), "{}");
        }
        Some(ConsoleArm { receivers, webview: webview.clone() })
    }

    thread_local! {
        /// The downloads still being saved, by the id their reports carry:
        /// what Cancel reaches. Kept by the thread that runs the window,
        /// the only one that may touch them -- and so any page it builds,
        /// a window a page opened included, adds to the same list
        static SAVING: std::cell::RefCell<
            std::collections::HashMap<String, webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadOperation>,
        > = std::cell::RefCell::new(std::collections::HashMap::new());
        static NEXT_DOWNLOAD: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    thread_local! {
        /// The pages whose Ctrl+F and F3 open the board's search row. Any other
        /// page keeps the browser's own small search box
        static FIND_KEYS: std::cell::RefCell<std::collections::HashSet<String>> =
            std::cell::RefCell::new(std::collections::HashSet::new());
    }

    /// Give a page's Ctrl+F to the board (on) or back to the browser (off)
    pub fn find_keys_for(page: &str, on: bool) {
        FIND_KEYS.with(|k| {
            let mut k = k.borrow_mut();
            if on {
                k.insert(page.to_string());
            } else {
                k.remove(page);
            }
        });
    }

    /// Stop a download still being saved. One already over is left as it is
    pub fn cancel_download(id: &str) {
        let op = SAVING.with(|s| s.borrow().get(id).cloned());
        if let Some(op) = op {
            let _ = unsafe { op.Cancel() };
        }
    }

    /// Save what this page sends to be saved, and report how each file goes.
    ///
    /// The browser's own download bubble is kept from opening: it would stand
    /// over a corner of the window that belongs to something else, and only
    /// the person at this machine could ever see it. The list in the column
    /// beside the page is the one every screen -- a phone included -- reads.
    /// Where the file goes is the browser's own decision (the Downloads
    /// folder, numbered the way a browser numbers a second file of a name)
    pub fn arm_downloads(
        webview: &ICoreWebView2,
        page: Option<String>,
        tell: std::sync::mpsc::Sender<shikisha_shared::Ev>,
    ) -> bool {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_4;
        use windows::core::Interface as _;
        let Ok(four) = webview.cast::<ICoreWebView2_4>() else {
            return false;
        };
        let handler = webview2_com::DownloadStartingEventHandler::create(Box::new(move |_sender, args| {
            let Some(args) = args else { return Ok(()) };
            unsafe {
                let _ = args.SetHandled(true);
                let op = args.DownloadOperation()?;
                let n = NEXT_DOWNLOAD.with(|c| {
                    c.set(c.get() + 1);
                    c.get()
                });
                let id = format!("w{n}");
                let mut raw = windows::core::PWSTR::null();
                let _ = op.Uri(&mut raw);
                let url = webview2_com::take_pwstr(raw);
                // One report, whatever moved: the operation is asked for all
                // of it each time, so no report can contradict another
                let said = std::rc::Rc::new(std::cell::Cell::new(std::time::Instant::now()));
                let report = {
                    let (tell, page, id, url) = (tell.clone(), page.clone(), id.clone(), url.clone());
                    std::rc::Rc::new(move |op: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadOperation| {
                        let item = download_of(op, &id, &url);
                        let _ = tell.send(shikisha_shared::Ev::Download { from: page.clone(), item });
                    })
                };
                let moved = {
                    let (report, said) = (std::rc::Rc::clone(&report), std::rc::Rc::clone(&said));
                    webview2_com::BytesReceivedChangedEventHandler::create(Box::new(move |sender, _| {
                        // A large file reports every few kilobytes, and a line
                        // redrawn that often says nothing more
                        if let Some(op) = sender
                            && said.get().elapsed() >= std::time::Duration::from_millis(250)
                        {
                            said.set(std::time::Instant::now());
                            report(&op);
                        }
                        Ok(())
                    }))
                };
                let ended = {
                    let (report, id) = (std::rc::Rc::clone(&report), id.clone());
                    webview2_com::StateChangedEventHandler::create(Box::new(move |sender, _| {
                        if let Some(op) = sender {
                            report(&op);
                            let now = download_of(&op, &id, "");
                            if now.state != shikisha_shared::DownloadState::Going {
                                SAVING.with(|s| s.borrow_mut().remove(&id));
                                shikisha_core::append_hook_log(&format!(
                                    "[browser] {id} ended {:?} {}: {}",
                                    now.state, now.why, now.path
                                ));
                            }
                        }
                        Ok(())
                    }))
                };
                let mut token = 0i64;
                let _ = op.add_BytesReceivedChanged(&moved, &mut token);
                let _ = op.add_StateChanged(&ended, &mut token);
                SAVING.with(|s| s.borrow_mut().insert(id.clone(), op.clone()));
                shikisha_core::append_hook_log(&format!(
                    "[browser] {:?} saving {id}: {url}",
                    page.as_deref().unwrap_or("?")
                ));
                report(&op);
            }
            Ok(())
        }));
        let mut token = 0i64;
        unsafe { four.add_DownloadStarting(&handler, &mut token) }.is_ok()
    }

    /// Everything a download operation says about itself, as one report
    fn download_of(
        op: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadOperation,
        id: &str,
        url: &str,
    ) -> shikisha_shared::Download {
        use shikisha_shared::DownloadState;
        use webview2_com::Microsoft::Web::WebView2::Win32::*;
        let mut raw = windows::core::PWSTR::null();
        let path = match unsafe { op.ResultFilePath(&mut raw) } {
            Ok(()) => webview2_com::take_pwstr(raw),
            Err(_) => String::new(),
        };
        let (mut got, mut total) = (0i64, 0i64);
        let _ = unsafe { op.BytesReceived(&mut got) };
        let _ = unsafe { op.TotalBytesToReceive(&mut total) };
        let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
        let _ = unsafe { op.State(&mut state) };
        let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
        let _ = unsafe { op.InterruptReason(&mut reason) };
        let (state, why) = if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
            (DownloadState::Done, "")
        } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
            interrupted(reason)
        } else {
            (DownloadState::Going, "")
        };
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        shikisha_shared::Download {
            id: id.to_string(),
            name,
            url: url.to_string(),
            path,
            got: got.max(0) as u64,
            total: total.max(0) as u64,
            state,
            why: why.to_string(),
            far: false,
        }
    }

    /// Why a download stopped, in the few words a person can act on: try
    /// again later (the network, the server), make room or pick another
    /// folder (the disk), or leave it (blocked as dangerous)
    fn interrupted(
        reason: webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON,
    ) -> (shikisha_shared::DownloadState, &'static str) {
        use shikisha_shared::DownloadState::{Cancelled, Failed, Going};
        use webview2_com::Microsoft::Web::WebView2::Win32::*;
        match reason {
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED => (Cancelled, ""),
            // Paused, which nothing here does: it will go on
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED => (Going, ""),
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_FAILED
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_SERVER_DOWN
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_INVALID_REQUEST => (Failed, "network"),
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FAILED
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_BAD_CONTENT
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CERTIFICATE_PROBLEM
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNEXPECTED_RESPONSE
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CROSS_ORIGIN_REDIRECT
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_SHORT => (Failed, "server"),
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NO_SPACE
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_FAILED
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TRANSIENT_ERROR
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_HASH_MISMATCH => (Failed, "disk"),
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_ACCESS_DENIED => (Failed, "denied"),
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_LARGE
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NAME_TOO_LONG => (Failed, "toolarge"),
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_MALICIOUS
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_BLOCKED_BY_POLICY
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_SECURITY_CHECK_FAILED => (Failed, "blocked"),
            _ => (Failed, "unknown"),
        }
    }

    /// Ctrl+F, F3 and Shift+F3 pressed in this page open the board's search
    /// bar and move through it, instead of the browser's own little box.
    ///
    /// The browser's box can only be seen at this machine and knows nothing of
    /// the bar a phone watching the same page draws; one search, one bar.
    /// Pages of the app's own (the settings) keep the browser's box: they are
    /// not browser tabs, and the board's bar stands over browser tabs only.
    /// `ours` answers whether the page is one of those, at the moment of the press
    pub fn on_find_keys(
        view: &wry::WebView,
        page: String,
        tell: std::sync::mpsc::Sender<shikisha_shared::Ev>,
        ours: impl Fn(&str) -> bool + 'static,
    ) {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
        };
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_F3, VK_MENU, VK_SHIFT};
        use wry::WebViewExtWindows as _;
        let webview = view.webview();
        let handler = webview2_com::AcceleratorKeyPressedEventHandler::create(Box::new(move |_sender, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
            let mut key = 0u32;
            unsafe {
                let _ = args.KeyEventKind(&mut kind);
                let _ = args.VirtualKey(&mut key);
            }
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
                return Ok(());
            }
            let held = |vk: u16| unsafe { GetKeyState(i32::from(vk)) } < 0;
            let (ctrl, shift, alt) = (held(VK_CONTROL), held(VK_SHIFT), held(VK_MENU));

            let what = match key {
                0x46 if ctrl && !shift && !alt => "open",
                k if k == u32::from(VK_F3) && !ctrl && !alt => if shift { "prev" } else { "next" },
                _ => return Ok(()),
            };
            let mut raw = windows::core::PWSTR::null();
            let at = match unsafe { webview.Source(&mut raw) } {
                Ok(()) => webview2_com::take_pwstr(raw),
                Err(_) => String::new(),
            };
            if ours(&at) || !FIND_KEYS.with(|k| k.borrow().contains(&page)) {
                return Ok(());
            }
            unsafe {
                let _ = args.SetHandled(true);
            }
            let _ = tell.send(shikisha_shared::Ev::SeekAsk {
                from: Some(page.clone()),
                what: what.to_string(),
                text: String::new(),
            });
            Ok(())
        }));
        let mut token = 0i64;
        unsafe {
            let _ = view.controller().add_AcceleratorKeyPressed(&handler, &mut token);
        }
    }

    /// The browser's own search on one page, kept for as long as the page is.
    ///
    /// WebView2 lends the search Edge itself has -- every frame, every kind of
    /// text, painted by the browser -- with its little box kept shut. A runtime
    /// too old to lend it is searched by the script every page is given
    /// instead (`pageops::seek_js`), the same one the server's browser uses
    pub struct Finder {
        find: Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Find>,
    }

    impl Finder {
        /// The browser's search for this page, with where it stands reported
        /// each time it moves. `None` when this runtime has none to lend
        pub fn of(
            webview: &ICoreWebView2,
            page: Option<String>,
            tell: std::sync::mpsc::Sender<shikisha_shared::Ev>,
        ) -> Self {
            use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_28;
            use windows::core::Interface as _;
            let find = webview
                .cast::<ICoreWebView2_28>()
                .ok()
                .and_then(|w| unsafe { w.Find() }.ok());
            if let Some(f) = &find {
                let say = {
                    let (tell, page) = (tell.clone(), page.clone());
                    std::rc::Rc::new(move |f: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Find| {
                        let (at, of) = standing(f);
                        let _ = tell.send(shikisha_shared::Ev::Seek { from: page.clone(), at, of });
                    })
                };
                let (a, b) = (std::rc::Rc::clone(&say), std::rc::Rc::clone(&say));
                let moved = webview2_com::FindActiveMatchIndexChangedEventHandler::create(Box::new(move |sender, _| {
                    if let Some(f) = sender {
                        a(&f);
                    }
                    Ok(())
                }));
                let counted = webview2_com::FindMatchCountChangedEventHandler::create(Box::new(move |sender, _| {
                    if let Some(f) = sender {
                        b(&f);
                    }
                    Ok(())
                }));
                let mut token = 0i64;
                unsafe {
                    let _ = f.add_ActiveMatchIndexChanged(&moved, &mut token);
                    let _ = f.add_MatchCountChanged(&counted, &mut token);
                }
            }
            Self { find }
        }

        /// One move of the search. Where it stands comes back as `Ev::Seek`,
        /// from whichever engine answered
        pub fn seek(
            &self,
            webview: &ICoreWebView2,
            text: &str,
            step: shikisha_shared::Seek,
            page: Option<String>,
            tell: std::sync::mpsc::Sender<shikisha_shared::Ev>,
        ) {
            use shikisha_shared::Seek;
            let Some(f) = &self.find else {
                let js = shikisha_core::pageops::seek_js(text, step);
                let params = serde_json::json!({
                    "expression": format!("(function () {{ {js} }})()"),
                    "returnByValue": true,
                })
                .to_string();
                call_result(webview, "Runtime.evaluate", &params, move |ok, json| {
                    let said = serde_json::from_str::<serde_json::Value>(&json)
                        .ok()
                        .and_then(|v| v.pointer("/result/value").cloned())
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    let (at, of) = if ok { shikisha_core::pageops::seek_answer(&said) } else { (0, 0) };
                    let _ = tell.send(shikisha_shared::Ev::Seek { from: page, at, of });
                });
                return;
            };
            unsafe {
                match step {
                    Seek::New => {
                        let Some(options) = find_options(webview, text) else { return };
                        let (f2, tell2, page2) = (f.clone(), tell.clone(), page.clone());
                        let done = webview2_com::FindStartCompletedHandler::create(Box::new(move |_hr| {
                            let (at, of) = standing(&f2);
                            let _ = tell2.send(shikisha_shared::Ev::Seek { from: page2, at, of });
                            Ok(())
                        }));
                        let _ = f.Start(&options, &done);
                    }
                    Seek::Next => {
                        let _ = f.FindNext();
                    }
                    Seek::Prev => {
                        let _ = f.FindPrevious();
                    }
                    Seek::Stop => {
                        let _ = f.Stop();
                        let _ = tell.send(shikisha_shared::Ev::Seek { from: page, at: 0, of: 0 });
                    }
                }
            }
        }
    }

    /// The match stood on (from 1) and how many there are
    fn standing(f: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Find) -> (u32, u32) {
        let (mut index, mut count) = (-1i32, 0i32);
        unsafe {
            let _ = f.ActiveMatchIndex(&mut index);
            let _ = f.MatchCount(&mut count);
        }
        let of = count.max(0) as u32;
        // The browser counts the match stood on from 1, as a person does, and
        // says 0 or -1 for none (measured: the first match of a new search is 1)
        let at = if index <= 0 || of == 0 { 0 } else { (index as u32).min(of) };
        (at, of)
    }

    /// What to search for: the words as typed, any case, every match lit,
    /// and the browser's own box kept shut
    fn find_options(
        webview: &ICoreWebView2,
        text: &str,
    ) -> Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2FindOptions> {
        use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2_2, ICoreWebView2Environment15};
        use windows::core::Interface as _;
        unsafe {
            let env = webview.cast::<ICoreWebView2_2>().ok()?.Environment().ok()?;
            let options = env.cast::<ICoreWebView2Environment15>().ok()?.CreateFindOptions().ok()?;
            let term = HSTRING::from(text);
            options.SetFindTerm(PCWSTR(term.as_ptr())).ok()?;
            options.SetIsCaseSensitive(false).ok()?;
            options.SetShouldMatchWord(false).ok()?;
            options.SetShouldHighlightAllMatches(true).ok()?;
            options.SetSuppressDefaultFindDialog(true).ok()?;
            Some(options)
        }
    }

    /// Force a fresh first frame, even if the page has not changed.
    /// Used when a new viewer joins but the page is static and no new change is coming
    pub fn kick(webview: &ICoreWebView2) {
        call(webview, "Page.stopScreencast", "{}");
        call(webview, "Page.startScreencast", CAST_PARAMS);
    }

    /// Stop the screencast and unsubscribe its notifications too
    pub fn stop(cast: Cast) {
        unsafe {
            call(&cast.webview, "Page.stopScreencast", "{}");
            let _ = cast
                .receiver
                .remove_DevToolsProtocolEventReceived(cast.token);
        }
    }
}
