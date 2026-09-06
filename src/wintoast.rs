//! A real Windows notification.
//!
//! The board already says which tab is waiting, and Slack or Telegram can say
//! it to a phone. Neither reaches the person who is at this PC with the window
//! behind a browser. That is what the Action Center is for: a banner that
//! arrives over whatever is on top, and a line that stays in the tray until it
//! is read.
//!
//! Windows will not let just any program put something there. A notification
//! belongs to an **AppUserModelID**, and the shell only recognises an ID it has
//! seen in a Start Menu shortcut. A packaged copy (the Store one) is given both
//! by its package and needs nothing from us. An unpackaged copy — the portable
//! zip — has to write that shortcut itself, which is why this file does more
//! than build a bit of XML.
//!
//! The shortcut is written the first time a notification is actually asked
//! for, never at startup: a program that has been told to notify nobody has no
//! business putting itself in anyone's Start Menu.
//!
//! ## Clicking it
//!
//! A click is answered in-process, while this program is running: the window
//! comes forward and, if the notification came from one tab, that tab is the
//! one showing. Activation after the program has exited is a different
//! mechanism (a registered COM server, a second process launched cold) and is
//! deliberately not implemented — there is nothing useful to show once the
//! terminal it is about has gone.

#[cfg(windows)]
use std::sync::{Mutex, OnceLock};

/// What the shell knows this program as. Only used by the unpackaged copy;
/// the packaged one is given an ID by its package and must keep it.
#[cfg(windows)]
const AUMID: &str = "WIREDECOK.K.SHIKISHA-TERM.Portable";

/// The tab a click should bring up, left here by the activation handler for
/// the main loop to pick up.
#[cfg(windows)]
static CLICKED: OnceLock<Mutex<Option<usize>>> = OnceLock::new();

#[cfg(windows)]
fn clicked() -> &'static Mutex<Option<usize>> {
    CLICKED.get_or_init(|| Mutex::new(None))
}

/// The tab a person clicked a notification for, if they clicked one since the
/// last time this was asked. Taken, not read: a click is acted on once.
pub fn clicked_tab() -> Option<usize> {
    #[cfg(windows)]
    {
        clicked().lock().ok().and_then(|mut c| c.take())
    }
    #[cfg(not(windows))]
    None
}

/// Bring this program's own window to the front.
///
/// Called when a notification is clicked, which is the one moment a program is
/// entitled to do this: Windows refuses the foreground to a process the person
/// has not just interacted with, and clicking a banner is that interaction. It
/// is asked three ways because the permission is granted for a moment and the
/// plainest call is the one most often refused.
pub fn raise() {
    #[cfg(windows)]
    win::raise();
}

/// Show a notification. `tab` is the tab it is about, for the click to return to.
///
/// Errors come back as text rather than being swallowed: a notification that
/// silently does not appear is the hardest kind of fault to be told about, so
/// the caller writes it to the hook log like any other failed send.
pub fn show(title: &str, body: &str, tab: Option<usize>) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::show(title, body, tab)
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body, tab);
        Err("windows only".into())
    }
}

#[cfg(windows)]
mod win {
    use super::{AUMID, clicked};
    use std::sync::OnceLock;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager, ToastNotifier};
    use windows::core::{HSTRING, Interface};

    /// Toasts already shown, kept alive.
    ///
    /// The `Activated` handler lives on the notification object. Dropping that
    /// object drops the handler with it, and a banner sitting on screen with
    /// nothing behind it does nothing when clicked. They are let go of after a
    /// while — see `forget_old`.
    static SHOWN: OnceLock<std::sync::Mutex<Vec<(std::time::Instant, ToastNotification)>>> =
        OnceLock::new();

    fn shown() -> &'static std::sync::Mutex<Vec<(std::time::Instant, ToastNotification)>> {
        SHOWN.get_or_init(|| std::sync::Mutex::new(Vec::new()))
    }

    /// Windows drops a banner from the Action Center after a day at the most.
    /// Nothing older than that can still be clicked, so nothing older is kept.
    fn forget_old() {
        if let Ok(mut v) = shown().lock() {
            v.retain(|(t, _)| t.elapsed() < std::time::Duration::from_secs(60 * 60 * 24));
        }
    }

    /// The notifier, made once. Making it is what fails when the shell does not
    /// recognise this program, so the error is worth keeping and repeating
    /// rather than re-attempting on every answer.
    static NOTIFIER: OnceLock<Result<Sendable, String>> = OnceLock::new();

    /// `ToastNotifier` is an apartment-threaded COM object in name, but the
    /// WinRT projection is `Send` for every use made of it here (one thread
    /// shows, one thread is called back). Wrapped rather than asserted at each
    /// use, so the claim is written down in one place.
    struct Sendable(ToastNotifier);
    unsafe impl Send for Sendable {}
    unsafe impl Sync for Sendable {}

    fn notifier() -> Result<&'static ToastNotifier, String> {
        match NOTIFIER.get_or_init(|| {
            if crate::config::packaged() {
                // The package is the identity. Asking for one of our own here
                // would be asking to be somebody else.
                ToastNotificationManager::CreateToastNotifier()
                    .map(Sendable)
                    .map_err(|e| format!("{e}"))
            } else {
                register()?;
                ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))
                    .map(Sendable)
                    .map_err(|e| format!("{e}"))
            }
        }) {
            Ok(n) => Ok(&n.0),
            Err(e) => Err(e.clone()),
        }
    }

    /// Escape for XML text. The tab's name and the answer's first line are
    /// whatever a program printed, and an unescaped `&` makes the whole
    /// notification fail to parse — which looks exactly like it never fired.
    pub(super) fn esc(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    pub fn show(title: &str, body: &str, tab: Option<usize>) -> Result<(), String> {
        let notifier = notifier()?;
        // Two lines: what finished, and what it said. The second is capped by
        // Windows itself at four lines of display, so nothing is cut here that
        // the Action Center would not cut anyway.
        // No `launch`, and no `activationType`. That is not an oversight.
        //
        // Those attributes say "when this is clicked, start me up and hand me
        // this string", which for an unpackaged program means a COM server
        // registered under a CLSID that Windows can start from cold. Without
        // one the notification is accepted -- it lands in the Action Center's
        // database, correctly formed, and can be read back from there -- and
        // then never drawn, because the shell will not put up a banner it has
        // no way to act on. Measured on this machine: six notifications in the
        // database, not one of them ever on screen.
        //
        // The click is answered in this process instead, by the handler
        // attached below, which needs neither attribute: the notification
        // object is right here, and so is the window it is about.
        let xml = format!(
            "<toast>\
               <visual><binding template=\"ToastGeneric\">\
                 <text>{}</text><text>{}</text>\
               </binding></visual>\
             </toast>",
            esc(title),
            esc(body)
        );
        let doc = XmlDocument::new().map_err(|e| format!("{e}"))?;
        doc.LoadXml(&HSTRING::from(xml)).map_err(|e| format!("{e}"))?;
        let toast = ToastNotification::CreateToastNotification(&doc).map_err(|e| format!("{e}"))?;

        let want = tab;
        toast
            .Activated(&TypedEventHandler::<ToastNotification, windows::core::IInspectable>::new(
                move |_, _| {
                    if let Ok(mut c) = clicked().lock() {
                        // A click with no tab behind it still means "show me
                        // the window", which the main loop reads as tab 0.
                        *c = Some(want.unwrap_or(0));
                    }
                    Ok(())
                },
            ))
            .map_err(|e| format!("{e}"))?;

        notifier.Show(&toast).map_err(|e| format!("{e}"))?;
        forget_old();
        if let Ok(mut v) = shown().lock() {
            v.push((std::time::Instant::now(), toast));
        }
        Ok(())
    }

    /// Our own window, brought forward.
    ///
    /// Found by asking which visible top-level windows belong to this process.
    /// There is one -- pages open inside it rather than beside it -- and the
    /// dialog class is skipped so that a file picker left open is never
    /// mistaken for the board.
    pub fn raise() {
        use windows::Win32::Foundation::{HWND, LPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            BringWindowToTop, EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
            SW_RESTORE, SetForegroundWindow, ShowWindow, SwitchToThisWindow,
        };

        struct Found {
            pid: u32,
            hwnd: HWND,
        }
        unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            let found = unsafe { &mut *(lparam.0 as *mut Found) };
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            if pid != found.pid || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
                return true.into();
            }
            let mut buf = [0u16; 64];
            let n = unsafe { GetClassNameW(hwnd, &mut buf) };
            let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            // "#32770" is the standard dialog class -- a picker, not the board.
            if class == "#32770" {
                return true.into();
            }
            found.hwnd = hwnd;
            false.into()
        }

        let mut found = Found { pid: std::process::id(), hwnd: HWND(std::ptr::null_mut()) };
        unsafe {
            let _ = EnumWindows(Some(cb), LPARAM(&mut found as *mut Found as isize));
            if found.hwnd.0.is_null() {
                return;
            }
            // Minimised is the case this exists for: the answer arrived while
            // the window was put away.
            let _ = ShowWindow(found.hwnd, SW_RESTORE);
            let _ = BringWindowToTop(found.hwnd);
            SwitchToThisWindow(found.hwnd, true);
            let _ = SetForegroundWindow(found.hwnd);
        }
    }

    /// Make this program something the shell will accept a notification from.
    ///
    /// Two halves, and both are required: the running process says which ID it
    /// is, and a Start Menu shortcut carrying that same ID is what makes the
    /// shell believe it. Without the shortcut, creating the notifier fails
    /// with a flat "element not found" and nothing ever appears.
    fn register() -> Result<(), String> {
        use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
        unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(AUMID)) }
            .map_err(|e| format!("{e}"))?;
        write_shortcut()
    }

    /// Where the shortcut goes: the person's own Start Menu, not the machine's.
    /// Nothing here needs an administrator, and nothing here touches anybody
    /// else's account.
    fn shortcut_path() -> Result<std::path::PathBuf, String> {
        let appdata = std::env::var("APPDATA").map_err(|_| "APPDATA".to_string())?;
        Ok(std::path::PathBuf::from(appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join("SHIKISHA-TERM.lnk"))
    }

    /// Write the Start Menu shortcut, if it is not already the one we want.
    ///
    /// Rewritten when the program has moved: the portable copy can be run from
    /// a stick, a downloads folder, or wherever it was unzipped this time, and
    /// a shortcut pointing at where it used to be is a Start Menu entry that
    /// opens nothing.
    fn write_shortcut() -> Result<(), String> {
        use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
        use windows::Win32::System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            IPersistFile,
        };
        use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
        use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

        let link = shortcut_path()?;
        let exe = std::env::current_exe().map_err(|e| format!("{e}"))?;
        if link.exists() && shortcut_points_at(&link, &exe) {
            return Ok(());
        }
        if let Some(dir) = link.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{e}"))?;
        }
        unsafe {
            // Already initialised by the window on this thread in the normal
            // case; a second call answers S_FALSE and is not an error.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| format!("{e}"))?;
            shell
                .SetPath(&HSTRING::from(exe.as_os_str()))
                .map_err(|e| format!("{e}"))?;
            if let Some(dir) = exe.parent() {
                shell
                    .SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))
                    .map_err(|e| format!("{e}"))?;
            }
            // The ID is the whole point of the file. Everything else about it
            // is just a shortcut that happens to be useful.
            let props: IPropertyStore = shell.cast().map_err(|e| format!("{e}"))?;
            props
                .SetValue(&PKEY_AppUserModel_ID, &PROPVARIANT::from(AUMID))
                .map_err(|e| format!("{e}"))?;
            props.Commit().map_err(|e| format!("{e}"))?;
            let file: IPersistFile = shell.cast().map_err(|e| format!("{e}"))?;
            file.Save(&HSTRING::from(link.as_os_str()), true)
                .map_err(|e| format!("{e}"))?;
        }
        crate::append_hook_log(&format!(
            "wintoast: wrote {} so Windows will accept notifications from this copy",
            link.display()
        ));
        Ok(())
    }

    /// Does the shortcut already point at this exe?
    fn shortcut_points_at(link: &std::path::Path, exe: &std::path::Path) -> bool {
        use windows::Win32::System::Com::{
            CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ,
        };
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        unsafe {
            let Ok(shell) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER)
            else {
                return false;
            };
            let Ok(file) = shell.cast::<IPersistFile>() else {
                return false;
            };
            if file.Load(&HSTRING::from(link.as_os_str()), STGM_READ).is_err() {
                return false;
            }
            let mut buf = [0u16; 260];
            if shell.GetPath(&mut buf, std::ptr::null_mut(), 0).is_err() {
                return false;
            }
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let got = String::from_utf16_lossy(&buf[..n]);
            got.eq_ignore_ascii_case(&exe.to_string_lossy())
        }
    }
}

#[cfg(test)]
mod tests {
    /// The two lines a notification carries are whatever a program printed.
    /// An unescaped `&` in a tab's name would make the XML fail to parse, and
    /// a notification that fails to parse looks exactly like one that was
    /// never sent.
    #[cfg(windows)]
    #[test]
    fn markup_in_a_tab_name_cannot_break_the_toast() {
        let xml = format!("<text>{}</text>", super::win::esc("R&D <build> \"x\""));
        assert_eq!(xml, "<text>R&amp;D &lt;build&gt; &quot;x&quot;</text>");
    }

    /// Shows a real notification, on this machine, for a person to look at.
    ///
    /// Ignored by default and run by hand (`cargo test -- --ignored
    /// really_shows_a_notification --nocapture`): it registers an
    /// AppUserModelID for whichever binary is running, which for a test run is
    /// the test binary. The point of having it at all is that everything this
    /// file does either works on the machine or does not, and nothing short of
    /// asking Windows tells you which.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn really_shows_a_notification() {
        super::show("SHIKISHA-TERM", "テスト通知です", Some(2)).expect("toast");
        // Long enough to see it, and to click it.
        std::thread::sleep(std::time::Duration::from_secs(12));
        println!("clicked tab: {:?}", super::clicked_tab());
    }
}
