//! The window itself, as each system dresses it: its shadow, its icon, where
//! it opens in the pile, the program's icon in the notification area, and the
//! end of a session. Nothing here draws a page; that is the engine's
//! (`super::engine`), and what the window does is the loop's (`super::window`).

use super::*;

/// The program's own window, its bar drawn by the page.
///
/// On Windows the frame is ours to draw, and the system keeps what it is
/// better at: resizing from the edges (tao hit-tests them for an undecorated
/// window) and the drop shadow, which `with_undecorated_shadow` asks for by
/// name.
///
/// A Mac keeps its own frame and its three buttons where every Mac window has
/// them, at the top left, with the page reaching up under a bar made clear:
/// the page draws the rest of the bar around them (`FRAME_JS`), and nothing
/// a Mac's person reaches for is somewhere else.
///
/// What this costs, said plainly: Windows 11's Snap Layouts flyout appears
/// when the pointer rests on a *system* maximize button, and ours is not
/// one. Dragging to an edge, Win+arrow, and double-clicking the bar all
/// still snap, because those are the window manager's, not the button's.
pub(super) fn main_builder(title: &str) -> tao::window::WindowBuilder {
    let b = tao::window::WindowBuilder::new().with_title(title);
    #[cfg(windows)]
    let b = {
        use tao::platform::windows::WindowBuilderExtWindows;
        b.with_decorations(false).with_undecorated_shadow(true)
    };
    #[cfg(target_os = "macos")]
    let b = {
        use tao::platform::macos::WindowBuilderExtMacOS;
        // The three buttons are left where AppKit puts them, 8 points in from
        // the top and the left: the middle of a 32-point bar, which is the
        // page's. Moved by hand (tao's traffic-light inset) they sat above the
        // top edge, because the move squeezed the bar they stand in and was
        // only made again when tao's own view drew, which under CEF's is
        // almost never
        b.with_titlebar_transparent(true)
            .with_title_hidden(true)
            .with_fullsize_content_view(true)
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let b = b.with_decorations(false);
    b
}

/// What the page is told about the frame around it, before anything of it
/// runs: on a Mac, that the system's three buttons are at the top left of its
/// bar, so it leaves them room and draws none of its own
pub(super) const FRAME_JS: &str = if cfg!(target_os = "macos") { "window.__shikisha_frame = \"mac\";\n" } else { "" };

/// The tool's window over a picture of the screen: off the taskbar, and
/// without a shadow that would fall across the picture
pub(super) fn tool_builder() -> tao::window::WindowBuilder {
    let b = tao::window::WindowBuilder::new();
    #[cfg(windows)]
    let b = {
        use tao::platform::windows::WindowBuilderExtWindows;
        b.with_skip_taskbar(true).with_undecorated_shadow(false)
    };
    b
}

/// What the program's window needs once it exists.
///
/// A new window lands on top of the pile even when it is not the active one,
/// and there it covers whatever the person is reading; a program started
/// `--behind` puts it at the bottom, where it is still on the taskbar for
/// anyone who wants to watch. And the window wears the program's own icon
pub(super) fn settle(window: &tao::window::Window) {
    #[cfg(windows)]
    {
        use tao::platform::windows::WindowExtWindows;
        if shikisha_core::stays_behind() {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
            };
            unsafe {
                SetWindowPos(window.hwnd() as _, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
        wear_our_own_icon(window.hwnd());
        let _ = MAIN_HWND.set(window.hwnd());
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// The window's handle, for the few things that must be told which window
/// they belong to (the Store's update dialog)
#[cfg(windows)]
static MAIN_HWND: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

#[cfg(windows)]
pub fn main_hwnd() -> isize {
    MAIN_HWND.get().copied().unwrap_or(0)
}

/// There is no such handle to name away from Windows
#[cfg(not(windows))]
pub fn main_hwnd() -> isize {
    0
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
#[cfg(windows)]
fn wear_our_own_icon(hwnd: isize) {
    use shikisha_core::tray::OUR_ICON;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON,
        SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
    };
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
        wear(ICON_SMALL, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        wear(ICON_BIG, GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON));
    }
}

/// The program's icon in the notification area. Its presses, and a second
/// copy's request for the window, arrive through the window's own procedure
/// (see `shikisha_core::tray`)
#[cfg(windows)]
#[derive(Clone, Copy)]
pub(super) struct Tray(shikisha_core::tray::Tray);

#[cfg(windows)]
impl Tray {
    pub fn add(
        window: &tao::window::Window,
        title: &str,
        tell: Sender<Ev>,
        revive: tao::event_loop::EventLoopProxy<Cmd>,
        down: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Option<Tray> {
        use tao::platform::windows::WindowExtWindows;
        Some(Tray(shikisha_core::tray::Tray::add(
            window.hwnd(),
            title,
            move |pressed| {
                // Pressing the icon while the screen is down means "bring it
                // back", not "show me the window": there is no window to show.
                // A person pressing is never a storm, so whatever was tried is
                // forgotten
                if matches!(pressed, shikisha_core::tray::Pressed::Open) && down.load(std::sync::atomic::Ordering::SeqCst) {
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
        )))
    }

    pub fn notice(&self, title: &str, text: &str) {
        self.0.notice(title, text);
    }

    pub fn remove(&self) {
        self.0.remove();
    }
}

/// A Mac's: the program's icon in the menu bar, with the same two lines,
/// "Open" and "Quit". It is where the program is while its window is put
/// away, besides the Dock (whose icon brings the window back too, see
/// `Event::Reopen` in the loop). Its notices are notifications: a Mac's
/// menu bar has no balloons
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
pub(super) struct Tray;

#[cfg(target_os = "macos")]
thread_local! {
    /// The icon itself. Made, kept and let go on the window's thread, the only
    /// one a Mac's menu bar is touched from
    static MENU_BAR_ICON: std::cell::RefCell<Option<tray_icon::TrayIcon>> = const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "macos")]
impl Tray {
    pub fn add(
        _window: &tao::window::Window,
        title: &str,
        tell: Sender<Ev>,
        revive: tao::event_loop::EventLoopProxy<Cmd>,
        down: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Option<Tray> {
        use tray_icon::menu::{Menu, MenuItem};
        let menu = Menu::new();
        let open = MenuItem::new(shikisha_core::i18n::t("tray.open"), true, None);
        let quit = MenuItem::new(shikisha_core::i18n::t("tray.quit"), true, None);
        menu.append(&open).ok()?;
        menu.append(&quit).ok()?;
        let icon = menu_bar_picture()?;
        let made = tray_icon::TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(title)
            .with_icon(icon)
            .build();
        let made = match made {
            Ok(m) => m,
            Err(e) => {
                shikisha_core::append_hook_log(&format!("[tray] the menu bar icon could not be made: {e}"));
                return None;
            }
        };
        let quitting = tell.clone();
        on_menu(open.id(), move || {
            // The screen down is a screen to bring back, not a window to show
            if down.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = revive.send_event(Cmd::DisplayWanted { asked: true });
            } else {
                let _ = tell.send(Ev::TrayOpen);
            }
        });
        on_menu(quit.id(), move || {
            let _ = quitting.send(Ev::TrayQuit);
        });
        MENU_BAR_ICON.with(|i| *i.borrow_mut() = Some(made));
        Some(Tray)
    }

    /// Said as a notification, the way a Mac says things from the menu bar
    pub fn notice(&self, title: &str, text: &str) {
        use shikisha_shared::Toasts;
        if let Err(e) = crate::macnote::MacBanners.show(title, text, None) {
            shikisha_core::append_hook_log(&format!("[tray] {title}: {text} (not shown: {e})"));
        }
    }

    pub fn remove(&self) {
        MENU_BAR_ICON.with(|i| i.borrow_mut().take());
    }
}

/// What each menu line of this program's does, by the line's id. One list for
/// the menu bar's icon and the menus at the top of the screen alike: a press is
/// heard by one handler, and there can be only one
#[cfg(target_os = "macos")]
static MENU_ACTIONS: std::sync::Mutex<Vec<(tray_icon::menu::MenuId, Box<dyn Fn() + Send>)>> =
    std::sync::Mutex::new(Vec::new());

/// Do `act` when the menu line `id` is chosen
#[cfg(target_os = "macos")]
fn on_menu(id: &tray_icon::menu::MenuId, act: impl Fn() + Send + 'static) {
    use tray_icon::menu::MenuEvent;
    static HEARD: std::sync::Once = std::sync::Once::new();
    HEARD.call_once(|| {
        MenuEvent::set_event_handler(Some(|e: MenuEvent| {
            let actions = MENU_ACTIONS.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((_, act)) = actions.iter().find(|(id, _)| *id == e.id) {
                act();
            }
        }));
    });
    MENU_ACTIONS.lock().unwrap_or_else(|p| p.into_inner()).push((id.clone(), Box::new(act)));
}

#[cfg(target_os = "macos")]
thread_local! {
    /// The menus at the top of the screen, kept for as long as they are shown
    static MENU_BAR: std::cell::RefCell<Option<tray_icon::menu::Menu>> = const { std::cell::RefCell::new(None) };
}

/// The menus at the top of a Mac's screen: the app's own (About, Settings ⌘,
/// Hide, Quit ⌘Q), Edit and Window, as every Mac program has them.
///
/// Edit is not decoration. A Mac hands ⌘C, ⌘V, ⌘X, ⌘A and ⌘Z to whatever is
/// typed into through these lines: without them a page's boxes take no paste
/// at all. And Quit is this program's own line, not the system's, so leaving
/// asks first exactly as the tray's Quit does
#[cfg(target_os = "macos")]
pub(super) fn menu_bar(tell: Sender<Ev>) {
    use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
    use tray_icon::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem as Line, Submenu};
    let t = |k: &str| shikisha_core::i18n::t(k);
    let cmd = |code| Some(Accelerator::new(Modifiers::META, code));
    let settings = MenuItem::new(t("menu.mac.settings"), true, cmd(Code::Comma));
    let quit = MenuItem::new(t("menu.mac.quit"), true, cmd(Code::KeyQ));
    let about = AboutMetadata {
        name: Some("SHIKISHA-TERM".into()),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        ..Default::default()
    };
    let made = (|| -> tray_icon::menu::Result<Menu> {
        let app = Submenu::with_items(
            "SHIKISHA-TERM",
            true,
            &[
                &Line::about(Some(&t("menu.mac.about")), Some(about)),
                &Line::separator(),
                &settings,
                &Line::separator(),
                &Line::services(Some(&t("menu.mac.services"))),
                &Line::separator(),
                &Line::hide(Some(&t("menu.mac.hide"))),
                &Line::hide_others(Some(&t("menu.mac.hide_others"))),
                &Line::show_all(Some(&t("menu.mac.show_all"))),
                &Line::separator(),
                &quit,
            ],
        )?;
        let edit = Submenu::with_items(
            t("menu.mac.edit"),
            true,
            &[
                &Line::undo(Some(&t("menu.mac.undo"))),
                &Line::redo(Some(&t("menu.mac.redo"))),
                &Line::separator(),
                &Line::cut(Some(&t("menu.mac.cut"))),
                &Line::copy(Some(&t("menu.mac.copy"))),
                &Line::paste(Some(&t("menu.mac.paste"))),
                &Line::select_all(Some(&t("menu.mac.select_all"))),
            ],
        )?;
        let window = Submenu::with_items(
            t("menu.mac.window"),
            true,
            &[
                &Line::minimize(Some(&t("menu.mac.minimize"))),
                &Line::maximize(Some(&t("menu.mac.zoom"))),
                &Line::separator(),
                &Line::close_window(Some(&t("menu.mac.close"))),
            ],
        )?;
        let bar = Menu::with_items(&[&app, &edit, &window])?;
        bar.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        Ok(bar)
    })();
    match made {
        Ok(bar) => MENU_BAR.with(|m| *m.borrow_mut() = Some(bar)),
        Err(e) => {
            shikisha_core::append_hook_log(&format!("[menu] the menus could not be made: {e}"));
            return;
        }
    }
    let opening = tell.clone();
    on_menu(settings.id(), move || {
        let _ = opening.send(Ev::OpenSettings {
            section: None,
            ret: false,
            folder: None,
            tabpos: None,
            tabname: None,
            tabkey: None,
            sheet: false,
        });
    });
    on_menu(quit.id(), move || {
        let _ = tell.send(Ev::TrayQuit);
    });
}

/// The program's picture, small enough for the menu bar (the bar scales it to
/// its own height)
#[cfg(target_os = "macos")]
fn menu_bar_picture() -> Option<tray_icon::Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../../assets/pwa/icon-192.png")));
    let mut reader = decoder.read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut rgba).ok()?;
    if frame.color_type != png::ColorType::Rgba || frame.bit_depth != png::BitDepth::Eight {
        return None;
    }
    rgba.truncate(frame.buffer_size());
    tray_icon::Icon::from_rgba(rgba, frame.width, frame.height).ok()
}

/// Where no window is drawn there is no icon for one
#[cfg(not(any(windows, target_os = "macos")))]
#[derive(Clone, Copy)]
pub(super) struct Tray;

#[cfg(not(any(windows, target_os = "macos")))]
impl Tray {
    pub fn add(
        _window: &tao::window::Window,
        _title: &str,
        _tell: Sender<Ev>,
        _revive: tao::event_loop::EventLoopProxy<Cmd>,
        _down: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Option<Tray> {
        None
    }

    pub fn notice(&self, _title: &str, _text: &str) {}

    pub fn remove(&self) {}
}

/// Keep a window out of pictures of the screen (the tool's count-down card)
pub(super) fn keep_out_of_pictures(window: &tao::window::Window, out: bool) {
    #[cfg(windows)]
    {
        use tao::platform::windows::WindowExtWindows;
        crate::snip::keep_out_of_pictures(window.hwnd(), out);
    }
    // A Mac's window that is not to be shared is left out of every picture
    // of the screen (NSWindowSharingNone), and back in (ReadOnly) after
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        use tao::platform::macos::WindowExtMacOS;
        let ns_window = window.ns_window().cast::<AnyObject>();
        if !ns_window.is_null() {
            let sharing: usize = if out { 0 } else { 1 };
            unsafe {
                let _: () = msg_send![ns_window, setSharingType: sharing];
            }
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = (window, out);
}

/// The bar taken hold of, on a window whose page tells it so: the system moves
/// the window with the pointer from here on. `at` is where in the window the
/// bar was pressed, in the page's pixels from its top left (see `Ev::Window`).
///
/// The page says so only after a few pixels of travel and a message later, and
/// the system drags from wherever the pointer is when it is asked; so the
/// window is first put back under the pointer by as far as it has gone since
/// the press. Not a maximised window: the system restores it under the pointer
/// as the drag begins, at the same share of its width, and moving it first
/// left it somewhere else. A Mac's bar never gets here: the application hands
/// a press on it to the system itself (`took_bar_press` in cef_engine.rs)
pub(super) fn drag(window: &tao::window::Window, at: Option<(f64, f64)>) {
    if let (Some((x, y)), false) = (at, window.is_maximized()) {
        let scale = window.scale_factor();
        if let (Ok(now), Ok(inner), Ok(outer)) = (window.cursor_position(), window.inner_position(), window.outer_position()) {
            let pressed = (inner.x as f64 + x * scale, inner.y as f64 + y * scale);
            let (dx, dy) = ((now.x - pressed.0).round() as i32, (now.y - pressed.1).round() as i32);
            if dx != 0 || dy != 0 {
                window.set_outer_position(tao::dpi::PhysicalPosition::new(outer.x + dx, outer.y + dy));
            }
        }
    }
    let _ = window.drag_window();
}

/// A Mac's bar double-clicked: what the person chose in System Settings
/// ("Double-click a window's title bar to"): fill the screen, zoom, minimise,
/// or nothing. Zoom is the window's own, not a maximise: it goes to the size
/// that fits and, pressed again, back
#[cfg(target_os = "macos")]
pub(super) fn bar_double_clicked_mac(ns_window: *mut objc2::runtime::AnyObject) {
    use objc2::runtime::{AnyObject, Bool, Sel};
    use objc2::{msg_send, sel};
    use objc2_foundation::{NSString, NSUserDefaults};
    if ns_window.is_null() {
        return;
    }
    let defaults = NSUserDefaults::standardUserDefaults();
    // What System Settings writes: Fill, Maximize (its "Zoom"), Minimize or
    // None. Before the choice had more than two answers it was a yes or no to
    // minimising
    let action = defaults
        .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let old = defaults.boolForKey(&NSString::from_str("AppleMiniaturizeOnDoubleClick"));
            if old { "Minimize" } else { "Maximize" }.to_string()
        });
    let act: Sel = match action.as_str() {
        "None" => return,
        "Minimize" => sel!(performMiniaturize:),
        // The window's own fill, the one a title bar's double-click uses. A
        // Mac without one (before macOS 15) has no Fill to choose either, but
        // zooms if it somehow says so
        "Fill" => {
            let fill = sel!(_zoomFill:);
            let can: Bool = unsafe { msg_send![ns_window, respondsToSelector: fill] };
            if can.as_bool() { fill } else { sel!(performZoom:) }
        }
        _ => sel!(performZoom:),
    };
    // On the next turn of the run loop, not now: this can be heard inside the
    // loop's handler, and the window's animated resize draws the window while
    // it runs -- which asks that same handler, still held, and the program
    // stopped for good on the first double-click
    let none: *const AnyObject = std::ptr::null();
    unsafe {
        let _: () = msg_send![ns_window, performSelector: act, withObject: none, afterDelay: 0.0f64];
    }
}

/// The system is ending the session: signing out, restarting, shutting down,
/// or an installer asking programs to let go of their files.
///
/// tao hears `WM_ENDSESSION`, calls the loop finished and keeps pumping --
/// and the next message any window of this thread gets is a panic ("cannot
/// move state from Destroyed") inside a window procedure, where a panic cannot
/// unwind, so the process aborts. Four of those were in the log, each at a
/// shutdown, each leaving a crash report and a "did not close properly" for
/// the next start to explain. Windows ends the process as soon as this
/// message is answered anyway, so it is ended here, on purpose, before
/// anything else can arrive
pub(super) fn session_ending() {
    shikisha_core::append_hook_log("The system is ending the session: closing");
    shikisha_core::lastexit::mark_closed();
    std::process::exit(0);
}
