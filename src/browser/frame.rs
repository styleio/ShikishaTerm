//! The window itself, as each system dresses it: its shadow, its icon, where
//! it opens in the pile, the program's icon in the notification area, and the
//! end of a session. Nothing here draws a page; that is the engine's
//! (`super::engine`), and what the window does is the loop's (`super::window`).

use super::*;

/// The program's own window, as this system draws an undecorated one.
///
/// On Windows the frame is ours to draw, and the system keeps what it is
/// better at: resizing from the edges (tao hit-tests them for an undecorated
/// window) and the drop shadow, which `with_undecorated_shadow` asks for by
/// name.
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
        b.with_undecorated_shadow(true)
    };
    b
}

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

/// Away from Windows the program's icon lives in the menu bar, which comes
/// with the window drawn there; until it does, there is none
#[cfg(not(windows))]
#[derive(Clone, Copy)]
pub(super) struct Tray;

#[cfg(not(windows))]
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
    #[cfg(not(windows))]
    let _ = (window, out);
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
