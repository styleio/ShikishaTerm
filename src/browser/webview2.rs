//! The window's engine on Windows: WebView2, which is Chromium, driven
//! through wry and its COM interfaces.
//!
//! Windows carries WebView2 and Microsoft keeps it up to date, Chromium's
//! security fixes included, without this program having to ship anything.
//! What is here is only what WebView2 alone can do: make a page and move it,
//! carry its DevTools protocol, and the few things the protocol does not
//! cover -- downloads, the browser's own search, the keys a page hears first,
//! a page's processes dying and a page asking to close. What is done with
//! them is the window's (`super::window`) and the protocol's (`super::cdp`),
//! the same on every system.

use super::window::{Place, Popups, Rect4, Spec, Wiring};
use super::*;
use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceiver};
use windows::core::{HSTRING, PCWSTR};

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
    use windows::core::PWSTR;

    let mut raw = PWSTR::null();
    unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut raw) }.ok()?;
    let v = webview2_com::take_pwstr(raw);
    (!v.is_empty()).then_some(v)
}

// ── The DevTools protocol of one page ─────────────────────────────────────

/// One page's line to the DevTools protocol. COM objects are thread-bound,
/// so it is only ever used on the window's own thread, which is also where
/// its answers and events arrive
#[derive(Clone)]
pub(crate) struct Port(ICoreWebView2);

/// An event being heard. Dropping it stops the hearing
pub(crate) struct Heard {
    receiver: ICoreWebView2DevToolsProtocolEventReceiver,
    token: i64,
}

impl Drop for Heard {
    fn drop(&mut self) {
        unsafe {
            let _ = self.receiver.remove_DevToolsProtocolEventReceived(self.token);
        }
    }
}

impl Port {
    /// Call one CDP method (the result is discarded). `params_json` can just be "{}"
    pub fn call(&self, method: &str, params_json: &str) {
        let method = HSTRING::from(method);
        let params = HSTRING::from(params_json);
        let handler = webview2_com::CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_hr, _json| Ok(())));
        unsafe {
            let _ = self.0.CallDevToolsProtocolMethod(PCWSTR(method.as_ptr()), PCWSTR(params.as_ptr()), &handler);
        }
    }

    /// Call one CDP method and hand its result to `done(ok, json)`.
    ///
    /// `done` is guaranteed to run exactly once: either from the completion
    /// handler, or right here when the call can't even be issued (in which
    /// case the handler would never fire and a waiter would hang until its
    /// timeout for no reason)
    pub fn call_result<F>(&self, method: &str, params_json: &str, done: F)
    where
        F: FnOnce(bool, String) + 'static,
    {
        let method_h = HSTRING::from(method);
        let params = HSTRING::from(params_json);
        let done = std::rc::Rc::new(std::cell::RefCell::new(Some(done)));
        let in_handler = std::rc::Rc::clone(&done);
        let context = method.to_string();
        let handler = webview2_com::CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
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
        let issued =
            unsafe { self.0.CallDevToolsProtocolMethod(PCWSTR(method_h.as_ptr()), PCWSTR(params.as_ptr()), &handler) };
        if let Err(e) = issued
            && let Some(f) = done.borrow_mut().take()
        {
            f(false, format!("{method}: {e:?}"));
        }
    }

    /// Hear one event of the page's protocol, for as long as the answer is held
    pub fn listen<F>(&self, event: &str, on: F) -> Option<Heard>
    where
        F: Fn(&serde_json::Value) + 'static,
    {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DevToolsProtocolEventReceivedEventArgs;
        let handler = webview2_com::DevToolsProtocolEventReceivedEventHandler::create(Box::new(
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
            let receiver = self.0.GetDevToolsProtocolEventReceiver(PCWSTR(name.as_ptr())).ok()?;
            receiver.add_DevToolsProtocolEventReceived(&handler, &mut token).ok()?;
            Some(Heard { receiver, token })
        }
    }
}

// ── A page ────────────────────────────────────────────────────────────────

/// One page the window holds. Dropping it closes it
pub(super) struct Page {
    view: Option<wry::WebView>,
    port: Port,
    /// Made over the whole of a window rather than inside it. wry destroys
    /// the container window of a page placed inside the window and not of
    /// one made over the whole (wry 0.56), so every put-away of the board left
    /// one more empty child window under the board that came back; this one
    /// is destroyed here
    whole: bool,
}

impl Drop for Page {
    fn drop(&mut self) {
        let stale = self.whole.then(|| {
            use wry::WebViewExtWindows;
            self.view.as_ref().map(|v| v.hwnd().0)
        });
        self.view = None;
        if let Some(Some(h)) = stale {
            use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;
            let _ = unsafe { DestroyWindow(h) };
        }
    }
}

impl Page {
    fn v(&self) -> &wry::WebView {
        self.view.as_ref().expect("a page is only without its view while it is being dropped")
    }

    pub fn port(&self) -> Port {
        self.port.clone()
    }

    pub fn load(&self, url: &str) -> Result<()> {
        Ok(self.v().load_url(url)?)
    }

    pub fn back(&self) -> Result<()> {
        Ok(self.v().go_back()?)
    }

    pub fn forward(&self) -> Result<()> {
        Ok(self.v().go_forward()?)
    }

    pub fn reload(&self) -> Result<()> {
        Ok(self.v().reload()?)
    }

    pub fn url(&self) -> String {
        self.v().url().unwrap_or_default()
    }

    pub fn can_back(&self) -> bool {
        self.v().can_go_back().unwrap_or(false)
    }

    pub fn can_forward(&self) -> bool {
        self.v().can_go_forward().unwrap_or(false)
    }

    /// Run a script in the page; nothing comes back this way
    pub fn run_js(&self, js: &str) {
        let _ = self.v().evaluate_script(js);
    }

    /// Put the page at a place inside the window, in logical pixels
    pub fn place(&self, rect: Rect4) {
        let _ = self.v().set_bounds(to_rect(rect));
    }

    /// Size a page made over the whole window to the window, in its pixels
    pub fn fill(&self, w: u32, h: u32) {
        let _ = self.v().set_bounds(wry::Rect {
            position: wry::dpi::PhysicalPosition::new(0, 0).into(),
            size: wry::dpi::PhysicalSize::new(w, h).into(),
        });
    }

    pub fn focus(&self) -> Result<()> {
        Ok(self.v().focus()?)
    }

    pub fn zoom(&self, factor: f64) {
        let _ = self.v().zoom(factor);
    }

    /// In front of the other pages of the window
    pub fn raise(&self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos};
        use wry::WebViewExtWindows;
        unsafe {
            SetWindowPos(self.v().hwnd().0, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// The window moved. Chromium places its popups (select lists, tooltips)
    /// by where it last heard the window was
    pub fn window_moved(&self) {
        use wry::WebViewExtWindows;
        let _ = unsafe { self.v().controller().NotifyParentWindowPositionChanged() };
    }

    /// The process that plays the page's sound
    pub fn sound_process(&self) -> u32 {
        let mut plays: u32 = 0;
        let _ = unsafe { self.port.0.BrowserProcessId(&mut plays) };
        plays
    }

    /// The browser's own search on this page
    pub fn finder(&self, page: Option<String>, tell: Sender<Ev>) -> Finder {
        Finder::of(&self.port.0, page, tell)
    }
}

/// Convert a position and size into wry's shape
fn to_rect((x, y, w, h): Rect4) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(x, y).into(),
        size: wry::dpi::LogicalSize::new(w.max(0), h.max(0)).into(),
    }
}

// ── Making pages ──────────────────────────────────────────────────────────

/// What makes the window's pages: the main window they are placed in, and a
/// browser store (a `WebContext`) per folder, which tabs with the same folder
/// share
pub(super) struct Pages {
    window: std::rc::Rc<tao::window::Window>,
    contexts: std::collections::HashMap<std::path::PathBuf, wry::WebContext>,
}

impl Pages {
    pub fn new(window: std::rc::Rc<tao::window::Window>, _wake: tao::event_loop::EventLoopProxy<Cmd>) -> Result<Self> {
        Ok(Self { window, contexts: std::collections::HashMap::new() })
    }

    /// WebView2 runs off the window's own messages, so the loop is never
    /// asked to give it a turn of its own
    pub fn next_turn(&self) -> Option<std::time::Instant> {
        None
    }

    pub fn turn(&mut self) {}

    /// A page over the whole of `window`
    pub fn fill(&mut self, window: &tao::window::Window, spec: Spec) -> Result<Page> {
        let store = spec.store.clone();
        let ctx = self.contexts.entry(store.clone()).or_insert_with(|| wry::WebContext::new(Some(store)));
        let b = dress(wry::WebViewBuilder::new_with_web_context(ctx), &spec, None);
        let view = b.build(window)?;
        Ok(finish(view, spec.wiring, spec.user_agent.as_deref(), true))
    }

    /// A page placed inside the main window, at its seat
    pub fn child(&mut self, spec: Spec) -> Result<Page> {
        let store = spec.store.clone();
        let ctx = self.contexts.entry(store.clone()).or_insert_with(|| wry::WebContext::new(Some(store)));
        let b = dress(wry::WebViewBuilder::new_with_web_context(ctx), &spec, Some(&self.window));
        let view = b.build_as_child(&*self.window)?;
        Ok(finish(view, spec.wiring, spec.user_agent.as_deref(), false))
    }

    /// A private page's folder is about to be removed: its store goes first
    pub fn forget_store(&mut self, dir: &std::path::Path) {
        self.contexts.remove(dir);
    }
}

/// Everything about a page that is said as it is made
fn dress<'a>(
    mut b: wry::WebViewBuilder<'a>,
    spec: &Spec,
    window: Option<&std::rc::Rc<tao::window::Window>>,
) -> wry::WebViewBuilder<'a> {
    use wry::WebViewBuilderExtWindows as _;
    if let Some(port) = spec.through {
        // `<-loopback>` is the whole point, and it has to be said out loud: a
        // browser leaves loopback addresses out of its proxy by default, so
        // `localhost:3000` would go to *this* machine -- which is the one
        // mistake this feature exists to prevent. Written as plain arguments
        // rather than through wry's proxy setting, which offers no way to say it.
        //
        // wry's own defaults are repeated here because naming arguments
        // replaces them: without them a placed page gets the mini menu and the
        // smart screen back
        b = b.with_additional_browser_args(format!(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --proxy-server=http://127.0.0.1:{port} --proxy-bypass-list=<-loopback>"
        ));
    }
    if let Some(ua) = spec.user_agent.as_deref() {
        b = b.with_user_agent(ua);
    }
    if let Some(bg) = spec.background {
        b = b.with_background_color(bg);
    }
    if let Place::At(seat) = &spec.place {
        b = b.with_bounds(to_rect(seat.get()));
    }
    b = b.with_url(&spec.url).with_focused(spec.focus).with_initialization_script(&spec.init_js);
    wire(b, &spec.wiring, window)
}

/// What a page tells the window, as wry takes it
fn wire<'a>(
    mut b: wry::WebViewBuilder<'a>,
    wiring: &Wiring,
    window: Option<&std::rc::Rc<tao::window::Window>>,
) -> wry::WebViewBuilder<'a> {
    let ipc = std::rc::Rc::clone(&wiring.ipc);
    b = b.with_ipc_handler(move |req| ipc(&req.uri().to_string(), req.body()));
    if let Some(loading) = &wiring.loading {
        let (started, finished) = (std::rc::Rc::clone(loading), std::rc::Rc::clone(loading));
        b = b
            .with_navigation_handler(move |_url| {
                started(true);
                // Don't block the navigation. This is only here to emit a signal
                true
            })
            .with_on_page_load_handler(move |e, _url| {
                if matches!(e, wry::PageLoadEvent::Finished) {
                    finished(false);
                }
            });
    }
    if let (Some(popups), Some(window)) = (&wiring.popups, window) {
        b = b.with_new_window_req_handler(adopt_windows(popups.clone(), std::rc::Rc::clone(window)));
    }
    b
}

/// What is said to a page once it exists: who it is (in the other place
/// that gets asked), and the reports only WebView2 makes
fn finish(view: wry::WebView, wiring: Wiring, user_agent: Option<&str>, whole: bool) -> Page {
    use wry::WebViewExtWindows;
    let port = Port(view.webview());
    if let Some(ua) = user_agent {
        port.call("Emulation.setUserAgentOverride", &ua_override(ua));
    }
    if let Some(died) = wiring.died {
        on_process_failed(&port.0, move |memory| died(memory));
    }
    if let Some(closing) = wiring.closing {
        on_close_requested(&port.0, move || closing());
    }
    refuse_notifications(&port.0);
    if let Some((page, tell)) = wiring.downloads {
        let name = page.clone().unwrap_or_default();
        if !arm_downloads(&port.0, page, tell) {
            shikisha_core::append_hook_log(&format!(
                "[browser] '{name}': this WebView2 cannot report downloads; its own bubble shows them"
            ));
        }
    }
    if let Some((page, tell, ours)) = wiring.find_keys {
        on_find_keys(&view, page, tell, move |at| ours(at));
    }
    Page { view: Some(view), port, whole }
}

/// What a new-window request hands back, synchronously: the answer has to be
/// built inside the request, and this is the only place it can be
type WindowAnswer = Box<dyn Fn(String, wry::NewWindowFeatures) -> wry::NewWindowResponse>;

/// A window a page asked for, made in the page's own environment (same store,
/// real opener) and placed in its seat (see `window::Popups`)
fn adopt_windows(popups: Popups, window: std::rc::Rc<tao::window::Window>) -> WindowAnswer {
    use wry::{NewWindowResponse, WebViewBuilder, WebViewBuilderExtWindows};
    Box::new(move |uri, features| {
        // Never make a window while the loop is in the middle of counting
        // them. Refusing is what happens today anyway
        let Some(name) = popups.next_name() else { return NewWindowResponse::Deny };
        let wiring = popups.wiring(&name);
        let mut b = WebViewBuilder::new();
        if let Some(ua) = popups.user_agent.as_deref() {
            b = b.with_user_agent(ua);
        }
        // No URL is given: the runtime navigates the window it is handed, and
        // setting one here would load the page twice
        let b = b
            .with_environment(features.opener.environment.clone())
            .with_bounds(to_rect(popups.seat.get()))
            .with_focused(super::window::may_take_focus(&window))
            .with_initialization_script(popups.init_js());
        let b = wire(b, &wiring, Some(&window));
        match b.build_as_child(&*window) {
            Ok(v) => {
                let page = finish(v, wiring, popups.user_agent.as_deref(), false);
                let raw = page.port.0.clone();
                popups.made(name, page, &uri);
                NewWindowResponse::Create { webview: raw }
            }
            Err(e) => {
                popups.failed(&uri, &e);
                NewWindowResponse::Deny
            }
        }
    })
}

// ── What only WebView2 reports ────────────────────────────────────────────

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
fn on_process_failed<F: Fn(bool) + 'static>(webview: &ICoreWebView2, f: F) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PROCESS_FAILED_REASON, COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY,
        ICoreWebView2ProcessFailedEventArgs2,
    };
    use windows::core::Interface as _;
    let handler = webview2_com::ProcessFailedEventHandler::create(Box::new(move |_sender, args| {
        // The reason is only there on the second version of these
        // arguments. An older runtime says nothing about why, and
        // "unknown" is treated as "not memory": worth one attempt
        let memory = args.as_ref().and_then(|a| a.cast::<ICoreWebView2ProcessFailedEventArgs2>().ok()).is_some_and(|a2| {
            let mut reason = COREWEBVIEW2_PROCESS_FAILED_REASON::default();
            unsafe { a2.Reason(&mut reason) }.is_ok() && reason == COREWEBVIEW2_PROCESS_FAILED_REASON_OUT_OF_MEMORY
        });
        f(memory);
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = webview.add_ProcessFailed(&handler, &mut token);
    }
}

/// Hear the page asking to close its window (`window.close()`).
///
/// The one way a window a script opened can say it is done: its messages
/// stop arriving once it leaves its first about:blank, and a sign-in
/// window that closes itself would otherwise stay in front of its page
/// A page asking WebView2 for a permission: its notifications are refused,
/// always. A page's `Notification` is this program's own (the script put in
/// its place: `shikisha_core::pagenotice`), shown as this program's banner
/// and asked about on the board, the same on a Mac. What reaches here is what
/// that cannot stand in for -- a service worker's -- and WebView2's own
/// notification would be shown apart from this program's
fn refuse_notifications(webview: &ICoreWebView2) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS, COREWEBVIEW2_PERMISSION_STATE_DENY,
    };
    let handler = webview2_com::PermissionRequestedEventHandler::create(Box::new(move |_sender, args| {
        if let Some(args) = args {
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            unsafe {
                if args.PermissionKind(&mut kind).is_ok() && kind == COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS {
                    let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                }
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = webview.add_PermissionRequested(&handler, &mut token);
    }
}

fn on_close_requested<F: Fn() + 'static>(webview: &ICoreWebView2, f: F) {
    let handler = webview2_com::WindowCloseRequestedEventHandler::create(Box::new(move |_sender, _args| {
        f();
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = webview.add_WindowCloseRequested(&handler, &mut token);
    }
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
/// Whether a page's own Ctrl+F has a box of the browser's to open: WebView2
/// draws its own, which a page without the board's search row keeps
pub(super) const FINDS_ITSELF: bool = true;

pub(super) fn find_keys_for(page: &str, on: bool) {
    FIND_KEYS.with(|k| {
        let mut k = k.borrow_mut();
        if on {
            k.insert(page.to_string());
        } else {
            k.remove(page);
        }
    });
}

/// Nothing is left to let go once the loop has ended: WebView2 is the
/// system's, and each page let go of its own as it was dropped
pub(super) fn wind_down() {}

/// Stop a download still being saved. One already over is left as it is
pub(super) fn cancel_download(id: &str) {
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
fn arm_downloads(webview: &ICoreWebView2, page: Option<String>, tell: Sender<Ev>) -> bool {
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
                    let _ = tell.send(Ev::Download { from: page.clone(), item });
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
            shikisha_core::append_hook_log(&format!("[browser] {:?} saving {id}: {url}", page.as_deref().unwrap_or("?")));
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
    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
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
fn on_find_keys(view: &wry::WebView, page: String, tell: Sender<Ev>, ours: impl Fn(&str) -> bool + 'static) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN};
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
            k if k == u32::from(VK_F3) && !ctrl && !alt => {
                if shift {
                    "prev"
                } else {
                    "next"
                }
            }
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
        let _ = tell.send(Ev::SeekAsk { from: Some(page.clone()), what: what.to_string(), text: String::new() });
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
pub(super) struct Finder {
    find: Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Find>,
}

impl Finder {
    /// The browser's search for this page, with where it stands reported
    /// each time it moves. `None` when this runtime has none to lend
    fn of(webview: &ICoreWebView2, page: Option<String>, tell: Sender<Ev>) -> Self {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_28;
        use windows::core::Interface as _;
        let find = webview.cast::<ICoreWebView2_28>().ok().and_then(|w| unsafe { w.Find() }.ok());
        if let Some(f) = &find {
            let say = {
                let (tell, page) = (tell.clone(), page.clone());
                std::rc::Rc::new(move |f: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Find| {
                    let (at, of) = standing(f);
                    let _ = tell.send(Ev::Seek { from: page.clone(), at, of });
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
    pub fn seek(&self, view: &Page, text: &str, step: shikisha_shared::Seek, page: Option<String>, tell: Sender<Ev>) {
        use shikisha_shared::Seek;
        let Some(f) = &self.find else {
            super::cdp::seek_by_script(&view.port, text, step, page, tell);
            return;
        };
        unsafe {
            match step {
                Seek::New => {
                    let Some(options) = find_options(&view.port.0, text) else { return };
                    let (f2, tell2, page2) = (f.clone(), tell.clone(), page.clone());
                    let done = webview2_com::FindStartCompletedHandler::create(Box::new(move |_hr| {
                        let (at, of) = standing(&f2);
                        let _ = tell2.send(Ev::Seek { from: page2, at, of });
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
                    let _ = tell.send(Ev::Seek { from: page, at: 0, of: 0 });
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
