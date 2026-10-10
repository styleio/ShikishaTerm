//! The window's engine on a Mac: CEF, the Chromium the program carries.
//!
//! A Mac's own web view (WKWebView) has no DevTools protocol, and without it
//! there is no automation, no screen relay and no real input -- so the window
//! is drawn by Chromium here as on Windows, carried in the `.app` as the
//! Chromium Embedded Framework. What is here is only what CEF alone does: start
//! Chromium in this process (its helpers are a program of their own,
//! crates/chromium-helper), make a page as a view inside
//! the window and move it, carry its DevTools protocol, and the few things the
//! protocol does not cover -- downloads, the browser's own search, the keys a
//! page hears first, a page's processes dying and a page asking to close.
//! What is done with them is the window's (`super::window`) and the
//! protocol's (`super::cdp`), the same as on Windows.
//!
//! Everything runs on the program's first thread. CEF is pumped by the
//! window's loop (`external_message_pump`): it says when it wants a turn, the
//! loop gives it one (`Pages::turn`), and in between the loop sleeps.
//!
//! A page's messages to the window (`window.ipc.postMessage`) go through the
//! protocol too: a binding (`Runtime.addBinding`) the page calls, heard with
//! the context it was called from, whose origin is the protocol's word -- never
//! the page's -- for whether the page is one of the app's own.

use super::window::{Place, Rect4, Spec, Wiring};
use super::*;
use ::cef::{ImplBrowser, ImplBrowserHost, ImplFrame};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

/// The name of the binding a page posts its messages through
const BINDING: &str = "__shikishaIpc";

/// What a page's `window.ipc.postMessage` is, before anything of the page
/// runs: the binding, called with the message as text
const IPC_SHIM: &str =
    "window.ipc = window.ipc || { postMessage: (s) => window.__shikishaIpc(String(s)) };\n";

/// The version of the Chromium this program carries, when it is where it is
/// carried: inside the `.app`, beside the program. Asked before the window
/// opens, so a copy that is not whole says so instead of failing to draw
pub fn runtime_version() -> Option<String> {
    framework_beside(&std::env::current_exe().ok()?)?;
    let version = ::cef::sys::CEF_VERSION;
    Some(String::from_utf8_lossy(version.strip_suffix(&[0]).unwrap_or(version)).into_owned())
}

/// The framework, from the program, if it is there
fn framework_beside(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = exe.parent()?.join("../Frameworks").join(FRAMEWORK);
    path.exists().then_some(path)
}

thread_local! {
    /// Whether CEF has been started in this process. Once: CEF is started for
    /// the life of the process and cannot be started twice
    static INITIALIZED: Cell<bool> = const { Cell::new(false) };
    /// Browsers made and not yet gone, so the end can wait for them
    static LIVE: Cell<usize> = const { Cell::new(0) };
    /// Whether CEF is being given its turn right now
    static TURNING: Cell<bool> = const { Cell::new(false) };
}

/// One more browser, or one fewer
fn count_live(more: bool) {
    LIVE.with(|l| l.set(if more { l.get() + 1 } else { l.get().saturating_sub(1) }));
}

/// Where the framework is, from the program inside `Contents/MacOS`
const FRAMEWORK: &str = "Chromium Embedded Framework.framework/Chromium Embedded Framework";

// ── The pump ──────────────────────────────────────────────────────────────

/// When CEF last asked to be given a turn, and how to wake the loop for it.
/// Asked from any of CEF's threads, so held behind a lock
struct Pump {
    due: Option<std::time::Instant>,
    wake: Option<tao::event_loop::EventLoopProxy<Cmd>>,
}

static PUMP: std::sync::Mutex<Pump> = std::sync::Mutex::new(Pump { due: None, wake: None });

/// The longest the loop waits without giving CEF a turn. CEF's own example
/// of an external pump never waits longer than a thirtieth of a second: some
/// of its work is never asked for and is only done when it is given a turn
const MOST_BETWEEN_TURNS: std::time::Duration = std::time::Duration::from_millis(33);

/// CEF asks for a turn `delay_ms` from now (now, for zero or less)
fn schedule(delay_ms: i64) {
    let at = std::time::Instant::now() + std::time::Duration::from_millis(delay_ms.max(0) as u64);
    let mut pump = PUMP.lock().unwrap_or_else(|e| e.into_inner());
    pump.due = Some(pump.due.map_or(at, |d| d.min(at)));
    if delay_ms <= 0
        && let Some(wake) = &pump.wake
    {
        let _ = wake.send_event(Cmd::EngineTurn);
    }
}

// ── The DevTools protocol of one page ─────────────────────────────────────

/// What one page's protocol line keeps: the calls waiting for an answer, and
/// who is listening for which event
struct Wire {
    host: ::cef::BrowserHost,
    next: Cell<i32>,
    waiting: RefCell<std::collections::HashMap<i32, Box<dyn FnOnce(bool, String)>>>,
    listening: RefCell<std::collections::HashMap<String, Vec<(u64, Rc<dyn Fn(&serde_json::Value)>)>>>,
    tokens: Cell<u64>,
    /// Keeps the observer that hears the answers and events attached
    registration: RefCell<Option<::cef::Registration>>,
}

/// One page's line to the DevTools protocol
#[derive(Clone)]
pub(crate) struct Port(Rc<Wire>);

/// An event being heard. Dropping it stops the hearing
pub(crate) struct Heard {
    wire: Weak<Wire>,
    event: String,
    token: u64,
}

impl Drop for Heard {
    fn drop(&mut self) {
        if let Some(wire) = self.wire.upgrade()
            && let Ok(mut listening) = wire.listening.try_borrow_mut()
            && let Some(list) = listening.get_mut(&self.event)
        {
            list.retain(|(t, _)| *t != self.token);
        }
    }
}

impl Port {
    /// The line to `browser`'s protocol, with its observer attached
    fn of(browser: &::cef::Browser) -> Option<Self> {
        let host = browser.host()?;
        let wire = Rc::new(Wire {
            host: host.clone(),
            next: Cell::new(1),
            waiting: RefCell::new(std::collections::HashMap::new()),
            listening: RefCell::new(std::collections::HashMap::new()),
            tokens: Cell::new(1),
            registration: RefCell::new(None),
        });
        let mut observer = hooks::observer(Rc::downgrade(&wire));
        let registration = host.add_dev_tools_message_observer(Some(&mut observer));
        *wire.registration.borrow_mut() = registration;
        Some(Port(wire))
    }

    fn send(&self, method: &str, params_json: &str) -> Option<i32> {
        let id = self.0.next.get();
        self.0.next.set(id.wrapping_add(1).max(1));
        let params: serde_json::Value = serde_json::from_str(params_json).unwrap_or(serde_json::json!({}));
        let text = serde_json::json!({ "id": id, "method": method, "params": params }).to_string();
        (self.0.host.send_dev_tools_message(Some(text.as_bytes())) != 0).then_some(id)
    }

    /// Call one CDP method (the result is discarded). `params_json` can just be "{}"
    pub fn call(&self, method: &str, params_json: &str) {
        let _ = self.send(method, params_json);
    }

    /// Call one CDP method and hand its result to `done(ok, json)`, exactly once
    pub fn call_result<F>(&self, method: &str, params_json: &str, done: F)
    where
        F: FnOnce(bool, String) + 'static,
    {
        let id = self.0.next.get();
        self.0.waiting.borrow_mut().insert(id, Box::new(done));
        if self.send(method, params_json).is_none()
            && let Some(done) = self.0.waiting.borrow_mut().remove(&id)
        {
            done(false, format!("{method}: the page could not be asked"));
        }
    }

    /// Hear one event of the page's protocol, for as long as the answer is held
    pub fn listen<F>(&self, event: &str, on: F) -> Option<Heard>
    where
        F: Fn(&serde_json::Value) + 'static,
    {
        let token = self.0.tokens.get();
        self.0.tokens.set(token + 1);
        self.0.listening.borrow_mut().entry(event.to_string()).or_default().push((token, Rc::new(on)));
        Some(Heard { wire: Rc::downgrade(&self.0), event: event.to_string(), token })
    }
}

thread_local! {
    /// What CEF told this program, waiting to be acted on. Chromium is in the
    /// middle of telling its observers when it calls one, and an observer
    /// that answers on the spot -- another call to the protocol, a page
    /// loaded -- reaches back into what Chromium is still walking. So what it
    /// is told is only noted, and acted on once Chromium has finished
    /// (`Pages::turn`), in the order it was told
    static LATER: RefCell<std::collections::VecDeque<Box<dyn FnOnce()>>> =
        RefCell::new(std::collections::VecDeque::new());
}

/// Act on this once CEF has finished what it is in the middle of
fn later(f: impl FnOnce() + 'static) {
    LATER.with(|l| l.borrow_mut().push_back(Box::new(f)));
    // Told outside a turn -- a key pressed in a page is handed out by the
    // system, not by CEF's turn -- the loop is woken to act on it
    if let Some(wake) = &PUMP.lock().unwrap_or_else(|e| e.into_inner()).wake {
        let _ = wake.send_event(Cmd::EngineTurn);
    }
}

/// Everything noted while CEF was busy, acted on now. What is acted on may note
/// more; that is acted on too
fn act_on_what_was_told() {
    while let Some(next) = LATER.with(|l| l.borrow_mut().pop_front()) {
        next();
    }
}

/// An answer the protocol gave, to whoever asked
fn answered(wire: &Weak<Wire>, id: i32, ok: bool, result: &[u8]) {
    let text = String::from_utf8_lossy(result).into_owned();
    let wire = wire.clone();
    later(move || {
        let Some(wire) = wire.upgrade() else { return };
        let done = wire.waiting.borrow_mut().remove(&id);
        if let Some(done) = done {
            done(ok, text);
        }
    });
}

/// An event the protocol raised, to everyone listening for it
fn raised(wire: &Weak<Wire>, method: &str, params: &[u8]) {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(params) else { return };
    let (wire, method) = (wire.clone(), method.to_string());
    later(move || {
        let Some(wire) = wire.upgrade() else { return };
        // Who is listening when it is acted on: one who stopped listening in
        // the meantime is not told
        let listeners: Vec<Rc<dyn Fn(&serde_json::Value)>> = match wire.listening.try_borrow() {
            Ok(l) => l.get(&method).map(|list| list.iter().map(|(_, f)| Rc::clone(f)).collect()).unwrap_or_default(),
            Err(_) => return,
        };
        for on in listeners {
            on(&v);
        }
    });
}

// ── A page ────────────────────────────────────────────────────────────────

/// What a page's handlers share: what the page tells the window, and the
/// engine around it
pub(super) struct Hooks {
    wiring: Wiring,
    shared: Rc<Shared>,
    /// This program is closing the page; its closing is not the page's asking
    going: Cell<bool>,
    /// For a window a page asked for: its name and address, until it exists
    popup: RefCell<Option<(String, String)>>,
    /// Where the results of the browser's own search go
    finds: RefCell<Option<(Option<String>, Sender<Ev>)>>,
    /// For a page over a whole window: that window (its NSWindow), whose bar
    /// the page draws. Only such a page says where the window may be taken
    /// hold of -- a site in a tab that marks part of itself draggable does
    /// not get to move the window
    bar_of: Option<usize>,
}

/// The engine around the window's pages
struct Shared {
    window: Rc<tao::window::Window>,
    /// The window's own view, the pages' views are put inside
    parent: *mut std::ffi::c_void,
    contexts: RefCell<std::collections::HashMap<std::path::PathBuf, ::cef::RequestContext>>,
}

/// One page the window holds. Dropping it closes it
pub(super) struct Page {
    browser: ::cef::Browser,
    port: Port,
    hooks: Rc<Hooks>,
    _heard: Vec<Heard>,
}

impl Drop for Page {
    fn drop(&mut self) {
        self.hooks.going.set(true);
        if let Some(window) = self.hooks.bar_of {
            mac::forget_bar(window);
        }
        if let Some(host) = self.browser.host() {
            host.close_browser(1);
        }
    }
}

impl Page {
    /// A page around a browser that exists, its protocol line set up: the
    /// binding its messages come through, the script that runs before its
    /// own, the name it goes by -- and only then the address it was made for
    fn wire_up(browser: ::cef::Browser, hooks: Rc<Hooks>, init_js: &str, user_agent: Option<&str>, url: Option<&str>) -> Result<Self> {
        let port = Port::of(&browser).ok_or_else(|| anyhow!("the page's DevTools protocol could not be reached"))?;
        // Which origin each of the page's contexts runs, by its id: what a
        // message is judged by
        let origins: Rc<RefCell<std::collections::HashMap<i64, String>>> = Rc::default();
        let mut heard = Vec::new();
        {
            let origins = Rc::clone(&origins);
            heard.extend(port.listen("Runtime.executionContextCreated", move |v| {
                let c = &v["context"];
                if c["auxData"]["isDefault"].as_bool() == Some(true)
                    && let Some(id) = c["id"].as_i64()
                {
                    origins.borrow_mut().insert(id, c["origin"].as_str().unwrap_or_default().to_string());
                }
            }));
        }
        {
            let origins = Rc::clone(&origins);
            heard.extend(port.listen("Runtime.executionContextDestroyed", move |v| {
                if let Some(id) = v["executionContextId"].as_i64() {
                    origins.borrow_mut().remove(&id);
                }
            }));
        }
        {
            let origins = Rc::clone(&origins);
            heard.extend(port.listen("Runtime.executionContextsCleared", move |_| origins.borrow_mut().clear()));
        }
        {
            let origins = Rc::clone(&origins);
            let ipc = Rc::clone(&hooks.wiring.ipc);
            heard.extend(port.listen("Runtime.bindingCalled", move |v| {
                if v["name"].as_str() != Some(BINDING) {
                    return;
                }
                // A context not seen being made is no context of a page's own
                // that this window knows the address of: not heard
                let Some(at) = v["executionContextId"].as_i64().and_then(|id| origins.borrow().get(&id).cloned()) else {
                    return;
                };
                ipc(&at, v["payload"].as_str().unwrap_or_default());
            }));
        }
        port.call("Runtime.enable", "{}");
        port.call("Runtime.addBinding", &serde_json::json!({ "name": BINDING }).to_string());
        port.call("Page.enable", "{}");
        port.call(
            "Page.addScriptToEvaluateOnNewDocument",
            &serde_json::json!({ "source": format!("{IPC_SHIM}{init_js}") }).to_string(),
        );
        if let Some(ua) = user_agent {
            port.call("Emulation.setUserAgentOverride", &ua_override(ua));
        }
        if let Some(url) = url {
            // Asked last and answered in order: by the time this answers,
            // everything above is in place for the page's first document
            let browser = browser.clone();
            let url = url.to_string();
            port.call_result("Runtime.evaluate", r#"{"expression":"0"}"#, move |_, _| {
                if let Some(frame) = browser.main_frame() {
                    frame.load_url(Some(&::cef::CefString::from(url.as_str())));
                }
            });
        }
        Ok(Self { browser, port, hooks, _heard: heard })
    }

    fn host(&self) -> Option<::cef::BrowserHost> {
        self.browser.host()
    }

    pub fn port(&self) -> Port {
        self.port.clone()
    }

    pub fn load(&self, url: &str) -> Result<()> {
        let frame = self.browser.main_frame().ok_or_else(|| anyhow!("the page has no frame"))?;
        frame.load_url(Some(&::cef::CefString::from(url)));
        Ok(())
    }

    pub fn back(&self) -> Result<()> {
        self.browser.go_back();
        Ok(())
    }

    pub fn forward(&self) -> Result<()> {
        self.browser.go_forward();
        Ok(())
    }

    pub fn reload(&self) -> Result<()> {
        self.browser.reload();
        Ok(())
    }

    pub fn url(&self) -> String {
        self.browser
            .main_frame()
            .map(|f| ::cef::CefString::from(&f.url()).to_string())
            .unwrap_or_default()
    }

    pub fn can_back(&self) -> bool {
        self.browser.can_go_back() != 0
    }

    pub fn can_forward(&self) -> bool {
        self.browser.can_go_forward() != 0
    }

    /// Run a script in the page; nothing comes back this way
    pub fn run_js(&self, js: &str) {
        if let Some(frame) = self.browser.main_frame() {
            frame.execute_java_script(Some(&::cef::CefString::from(js)), None, 0);
        }
    }

    fn view(&self) -> *mut std::ffi::c_void {
        self.host().map(|h| h.window_handle()).unwrap_or(std::ptr::null_mut())
    }

    /// Put the page at a place inside the window, in logical pixels (points)
    pub fn place(&self, (x, y, w, h): Rect4) {
        mac::set_frame(self.view(), self.hooks.shared.parent, (x as f64, y as f64, w as f64, h as f64));
        if let Some(host) = self.host() {
            host.was_resized();
        }
    }

    /// Size a page made over the whole window to the window, in its pixels
    pub fn fill(&self, w: u32, h: u32) {
        let scale = self.hooks.shared.window.scale_factor().max(1.0);
        mac::set_frame(self.view(), self.hooks.shared.parent, (0.0, 0.0, w as f64 / scale, h as f64 / scale));
        if let Some(host) = self.host() {
            host.was_resized();
        }
    }

    pub fn focus(&self) -> Result<()> {
        self.host().ok_or_else(|| anyhow!("the page is gone"))?.set_focus(1);
        Ok(())
    }

    /// Chromium counts zoom in steps of 20% (a level of 1 is 120%)
    pub fn zoom(&self, factor: f64) {
        if let Some(host) = self.host() {
            host.set_zoom_level(factor.max(0.01).ln() / 1.2f64.ln());
        }
    }

    /// In front of the other pages of the window
    pub fn raise(&self) {
        mac::raise(self.view(), self.hooks.shared.parent);
    }

    /// The window moved. Chromium places its popups (select lists, tooltips)
    /// by where it last heard the window was
    pub fn window_moved(&self) {
        if let Some(host) = self.host() {
            host.notify_move_or_resize_started();
        }
    }

    /// The process whose tree plays the page's sound: this one. Chromium's
    /// audio service is one of the helpers it started, and a Mac's tap is
    /// made over this process's children (vaudio)
    pub fn sound_process(&self) -> u32 {
        std::process::id()
    }

    /// The browser's own search on this page
    pub fn finder(&self, _page: Option<String>, _tell: Sender<Ev>) -> Finder {
        Finder { last: RefCell::new(String::new()) }
    }
}

/// The browser's own search on one page: Chromium's, with every match lit
pub(super) struct Finder {
    last: RefCell<String>,
}

impl Finder {
    /// One move of the search. Where it stands comes back as `Ev::Seek`
    pub fn seek(&self, view: &Page, text: &str, step: shikisha_shared::Seek, page: Option<String>, tell: Sender<Ev>) {
        use shikisha_shared::Seek;
        let Some(host) = view.host() else { return };
        *view.hooks.finds.borrow_mut() = Some((page.clone(), tell.clone()));
        match step {
            Seek::New => {
                *self.last.borrow_mut() = text.to_string();
                if text.is_empty() {
                    host.stop_finding(1);
                    let _ = tell.send(Ev::Seek { from: page, at: 0, of: 0 });
                    return;
                }
                host.find(Some(&::cef::CefString::from(text)), 1, 0, 0);
            }
            Seek::Next => host.find(Some(&::cef::CefString::from(self.last.borrow().as_str())), 1, 0, 1),
            Seek::Prev => host.find(Some(&::cef::CefString::from(self.last.borrow().as_str())), 0, 0, 1),
            Seek::Stop => {
                host.stop_finding(1);
                let _ = tell.send(Ev::Seek { from: page, at: 0, of: 0 });
            }
        }
    }
}

// ── Making pages ──────────────────────────────────────────────────────────

/// What makes the window's pages
pub(super) struct Pages {
    shared: Rc<Shared>,
    last_turn: std::time::Instant,
}

impl Pages {
    pub fn new(window: Rc<tao::window::Window>, wake: tao::event_loop::EventLoopProxy<Cmd>) -> Result<Self> {
        start(&wake)?;
        let parent = {
            use tao::platform::macos::WindowExtMacOS;
            window.ns_view()
        };
        PUMP.lock().unwrap_or_else(|e| e.into_inner()).wake = Some(wake);
        Ok(Self {
            shared: Rc::new(Shared {
                window,
                parent,
                contexts: RefCell::new(std::collections::HashMap::new()),
            }),
            last_turn: std::time::Instant::now(),
        })
    }

    /// When CEF wants its next turn, or the longest the loop may wait
    pub fn next_turn(&self) -> Option<std::time::Instant> {
        let most = self.last_turn + MOST_BETWEEN_TURNS;
        let due = PUMP.lock().ok().and_then(|p| p.due);
        Some(due.map_or(most, |d| d.min(most)))
    }

    /// Give CEF its turn when it is due
    pub fn turn(&mut self) {
        // A turn inside a turn: the system handing out an event while CEF
        // runs is not a moment to run CEF again
        if TURNING.with(|t| t.replace(true)) {
            return;
        }
        let now = std::time::Instant::now();
        let due = {
            let mut pump = PUMP.lock().unwrap_or_else(|e| e.into_inner());
            let due = pump.due.is_some_and(|d| d <= now) || now >= self.last_turn + MOST_BETWEEN_TURNS;
            if due {
                pump.due = None;
            }
            due
        };
        if due {
            self.last_turn = now;
            ::cef::do_message_loop_work();
        }
        TURNING.with(|t| t.set(false));
        act_on_what_was_told();
    }

    /// A page over the whole of `window`
    pub fn fill(&mut self, window: &tao::window::Window, spec: Spec) -> Result<Page> {
        let parent = {
            use tao::platform::macos::WindowExtMacOS;
            window.ns_view()
        };
        let size = window.inner_size().to_logical::<f64>(window.scale_factor());
        let bar_of = {
            use tao::platform::macos::WindowExtMacOS;
            window.ns_window() as usize
        };
        let page = self.make(spec, parent, (0, 0, size.width as i32, size.height as i32), Some(bar_of))?;
        Ok(page)
    }

    /// A page placed inside the main window, at its seat
    pub fn child(&mut self, spec: Spec) -> Result<Page> {
        let rect = match &spec.place {
            Place::At(seat) => seat.get(),
            Place::Fill => (0, 0, 0, 0),
        };
        self.make(spec, self.shared.parent, rect, None)
    }

    /// A private page's folder is about to be removed: its store goes first
    pub fn forget_store(&mut self, dir: &std::path::Path) {
        self.shared.contexts.borrow_mut().remove(dir);
    }

    fn make(&mut self, spec: Spec, parent: *mut std::ffi::c_void, rect: Rect4, bar_of: Option<usize>) -> Result<Page> {
        let hooks = Rc::new(Hooks {
            wiring: spec.wiring,
            shared: Rc::clone(&self.shared),
            going: Cell::new(false),
            popup: RefCell::new(None),
            finds: RefCell::new(None),
            bar_of,
        });
        let mut context = context_for(&self.shared, &spec.store, spec.through)?;
        let info = ::cef::WindowInfo::default().set_as_child(
            parent,
            &::cef::Rect { x: 0, y: 0, width: rect.2.max(0), height: rect.3.max(0) },
        );
        let settings = ::cef::BrowserSettings {
            background_color: spec
                .background
                .map(|(r, g, b, a)| (u32::from(a) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b))
                .unwrap_or(0),
            ..Default::default()
        };
        let mut client = hooks::client(Rc::clone(&hooks));
        let browser = ::cef::browser_host_create_browser_sync(
            Some(&info),
            Some(&mut client),
            Some(&::cef::CefString::from("about:blank")),
            Some(&settings),
            None,
            Some(&mut context),
        )
        .ok_or_else(|| anyhow!("Chromium did not make the page"))?;
        count_live(true);
        let page = Page::wire_up(browser, hooks, &spec.init_js, spec.user_agent.as_deref(), Some(&spec.url))?;
        mac::set_frame(page.view(), parent, (rect.0 as f64, rect.1 as f64, rect.2 as f64, rect.3 as f64));
        if spec.focus
            && let Some(host) = page.host()
        {
            host.set_focus(1);
        }
        Ok(page)
    }
}

/// The store a page keeps its cookies in, one per folder (pages given the same
/// folder are the same visitor), with its network through `through` when said
fn context_for(shared: &Shared, store: &std::path::Path, through: Option<u16>) -> Result<::cef::RequestContext> {
    use ::cef::ImplPreferenceManager;
    if let Some(found) = shared.contexts.borrow().get(store) {
        return Ok(found.clone());
    }
    // The shell's folder is the one Chromium was started with: its store is
    // the global one, and a second store over the same folder is refused
    if through.is_none() && store == super::shell_data_dir() {
        let global = ::cef::request_context_get_global_context()
            .ok_or_else(|| anyhow!("Chromium has no store of its own"))?;
        shared.contexts.borrow_mut().insert(store.to_path_buf(), global.clone());
        return Ok(global);
    }
    let _ = std::fs::create_dir_all(store);
    let settings = ::cef::RequestContextSettings {
        cache_path: ::cef::CefString::from(store.to_string_lossy().as_ref()),
        persist_session_cookies: 1,
        ..Default::default()
    };
    let context = ::cef::request_context_create_context(Some(&settings), None)
        .ok_or_else(|| anyhow!("Chromium did not make a store for {}", store.display()))?;
    if let Some(port) = through {
        // `<-loopback>` is the whole point, and it has to be said out loud: a
        // browser leaves loopback addresses out of its proxy by default, so
        // `localhost:3000` would go to *this* machine -- which is the one
        // mistake this feature exists to prevent
        use ::cef::{ImplDictionaryValue, ImplValue};
        if let (Some(dict), Some(mut value)) = (::cef::dictionary_value_create(), ::cef::value_create()) {
            let s = |t: &str| ::cef::CefString::from(t);
            dict.set_string(Some(&s("mode")), Some(&s("fixed_servers")));
            dict.set_string(Some(&s("server")), Some(&s(&format!("http://127.0.0.1:{port}"))));
            dict.set_string(Some(&s("bypass_list")), Some(&s("<-loopback>")));
            let mut dict = dict;
            value.set_dictionary(Some(&mut dict));
            let mut error = ::cef::CefString::default();
            if context.set_preference(Some(&s("proxy")), Some(&mut value), Some(&mut error)) == 0 {
                shikisha_core::append_hook_log(&format!("[browser] the way through 127.0.0.1:{port} was refused: {error}"));
            }
        }
    }
    shared.contexts.borrow_mut().insert(store.to_path_buf(), context.clone());
    Ok(context)
}

/// Start Chromium in this process, once
fn start(wake: &tao::event_loop::EventLoopProxy<Cmd>) -> Result<()> {
    if INITIALIZED.with(|i| i.get()) {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    if framework_beside(&exe).is_none() {
        bail_out()?;
    }
    // Kept for the life of the process: the framework is never unloaded
    let loader = Box::leak(Box::new(::cef::library_loader::LibraryLoader::new(&exe, false)));
    if !loader.load() {
        return Err(anyhow!("the Chromium framework inside the .app could not be loaded"));
    }
    let _ = ::cef::api_hash(::cef::sys::CEF_API_VERSION_LAST, 0);
    // CEF asks the application to say when it is in the middle of handing out
    // an event; the application tao made is taught to
    mac::teach_the_application();
    PUMP.lock().unwrap_or_else(|e| e.into_inner()).wake = Some(wake.clone());
    let args = ::cef::args::Args::new();
    let root = shikisha_core::config::browser_data_dir();
    let _ = std::fs::create_dir_all(&root);
    let settings = ::cef::Settings {
        no_sandbox: 0,
        external_message_pump: 1,
        multi_threaded_message_loop: 0,
        root_cache_path: ::cef::CefString::from(root.to_string_lossy().as_ref()),
        cache_path: ::cef::CefString::from(super::shell_data_dir().to_string_lossy().as_ref()),
        persist_session_cookies: 1,
        log_severity: ::cef::LogSeverity::WARNING,
        log_file: ::cef::CefString::from(shikisha_core::config::logs_dir().join("chromium.log").to_string_lossy().as_ref()),
        ..Default::default()
    };
    let mut app = hooks::app();
    if ::cef::initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()) != 1 {
        return Err(anyhow!("Chromium could not be started"));
    }
    INITIALIZED.with(|i| i.set(true));
    Ok(())
}

/// The switches `SHIKISHA_CHROMIUM_ARGS` hands Chromium, without their dashes:
/// `--remote-debugging-port=9401 --foo` is `remote-debugging-port=9401`, `foo`
fn extra_switches() -> Vec<String> {
    std::env::var("SHIKISHA_CHROMIUM_ARGS")
        .unwrap_or_default()
        .split_whitespace()
        .map(|s| s.trim_start_matches('-').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The program was started outside its `.app`, where the framework is not
fn bail_out() -> Result<()> {
    Err(anyhow!(
        "the window is drawn by the Chromium inside SHIKISHA-TERM.app; started outside it, there is none to draw with"
    ))
}

/// The loop has ended and its pages were closed with it: CEF is given the
/// turns it needs to let them go, and then stopped. A page still open after a
/// few seconds is not waited for: the process is ending either way
pub(super) fn wind_down() {
    PUMP.lock().unwrap_or_else(|e| e.into_inner()).wake = None;
    if !INITIALIZED.with(|i| i.get()) {
        return;
    }
    let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while LIVE.with(|l| l.get()) > 0 && std::time::Instant::now() < until {
        ::cef::do_message_loop_work();
        act_on_what_was_told();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // What is still noted holds pages and their lines, which go before CEF does
    LATER.with(|l| l.borrow_mut().clear());
    ::cef::shutdown();
}

// ── What only CEF reports ─────────────────────────────────────────────────

thread_local! {
    /// The downloads still being saved, by the id their reports carry: what
    /// Cancel reaches, and when each last reported (a large file reports
    /// every few kilobytes, and a line redrawn that often says nothing more)
    static SAVING: RefCell<std::collections::HashMap<String, (::cef::DownloadItemCallback, std::time::Instant)>> =
        RefCell::new(std::collections::HashMap::new());
    /// The pages whose search keys open the board's search row. Any other page
    /// keeps the browser's own
    static FIND_KEYS: RefCell<std::collections::HashSet<String>> = RefCell::new(std::collections::HashSet::new());
}

/// Give a page's search keys to the board (on) or back to the browser (off)
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

/// Stop a download still being saved. One already over is left as it is
pub(super) fn cancel_download(id: &str) {
    use ::cef::ImplDownloadItemCallback;
    let callback = SAVING.with(|s| s.borrow().get(id).map(|(c, _)| c.clone()));
    if let Some(callback) = callback {
        callback.cancel();
    }
}

/// Why a download stopped, in the few words a person can act on
fn interrupted(reason: ::cef::DownloadInterruptReason) -> (shikisha_shared::DownloadState, &'static str) {
    use ::cef::DownloadInterruptReason as R;
    use shikisha_shared::DownloadState::{Cancelled, Failed};
    let is = |r: R| reason == r;
    if is(R::USER_CANCELED) || is(R::USER_SHUTDOWN) {
        (Cancelled, "")
    } else if [R::NETWORK_FAILED, R::NETWORK_TIMEOUT, R::NETWORK_DISCONNECTED, R::NETWORK_SERVER_DOWN, R::NETWORK_INVALID_REQUEST]
        .into_iter()
        .any(is)
    {
        (Failed, "network")
    } else if [
        R::SERVER_FAILED,
        R::SERVER_NO_RANGE,
        R::SERVER_BAD_CONTENT,
        R::SERVER_UNAUTHORIZED,
        R::SERVER_CERT_PROBLEM,
        R::SERVER_FORBIDDEN,
        R::SERVER_UNREACHABLE,
        R::SERVER_CONTENT_LENGTH_MISMATCH,
        R::SERVER_CROSS_ORIGIN_REDIRECT,
        R::FILE_TOO_SHORT,
    ]
    .into_iter()
    .any(is)
    {
        (Failed, "server")
    } else if [R::FILE_NO_SPACE, R::FILE_FAILED, R::FILE_TRANSIENT_ERROR, R::FILE_HASH_MISMATCH].into_iter().any(is) {
        (Failed, "disk")
    } else if is(R::FILE_ACCESS_DENIED) {
        (Failed, "denied")
    } else if is(R::FILE_TOO_LARGE) || is(R::FILE_NAME_TOO_LONG) {
        (Failed, "toolarge")
    } else if is(R::FILE_VIRUS_INFECTED) || is(R::FILE_BLOCKED) || is(R::FILE_SECURITY_CHECK_FAILED) {
        (Failed, "blocked")
    } else {
        (Failed, "unknown")
    }
}

/// Everything a download says about itself, as one report
fn download_of(item: &::cef::DownloadItem) -> shikisha_shared::Download {
    use ::cef::ImplDownloadItem;
    use shikisha_shared::DownloadState;
    let text = |s: ::cef::CefStringUserfree| ::cef::CefString::from(&s).to_string();
    let path = text(item.full_path());
    let (state, why) = if item.is_complete() != 0 {
        (DownloadState::Done, "")
    } else if item.is_canceled() != 0 {
        (DownloadState::Cancelled, "")
    } else if item.is_interrupted() != 0 {
        interrupted(item.interrupt_reason())
    } else {
        (DownloadState::Going, "")
    };
    shikisha_shared::Download {
        id: format!("c{}", item.id()),
        name: std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        url: text(item.url()),
        path,
        got: item.received_bytes().max(0) as u64,
        total: item.total_bytes().max(0) as u64,
        state,
        why: why.to_string(),
        far: false,
    }
}

/// The handlers CEF calls, each holding what the page shares. Kept in a module
/// of their own: CEF's macros name its types as they are, and they would
/// otherwise meet this program's own (`Browser`, `Rect`)
mod hooks {
    use super::{Hooks, Weak, Wire};
    use cef::*;
    use std::rc::Rc;

    wrap_dev_tools_message_observer! {
        pub struct Observer {
            wire: Weak<Wire>,
        }

        impl DevToolsMessageObserver {
            fn on_dev_tools_method_result(&self, _browser: Option<&mut Browser>, message_id: ::std::os::raw::c_int, success: ::std::os::raw::c_int, result: Option<&[u8]>) {
                super::answered(&self.wire, message_id, success != 0, result.unwrap_or_default());
            }

            fn on_dev_tools_event(&self, _browser: Option<&mut Browser>, method: Option<&CefString>, params: Option<&[u8]>) {
                let method = method.map(|m| m.to_string()).unwrap_or_default();
                super::raised(&self.wire, &method, params.unwrap_or_default());
            }
        }
    }

    pub(super) fn observer(wire: Weak<Wire>) -> DevToolsMessageObserver {
        Observer::new(wire)
    }

    wrap_browser_process_handler! {
        pub struct Process;

        impl BrowserProcessHandler {
            fn on_schedule_message_pump_work(&self, delay_ms: i64) {
                super::schedule(delay_ms);
            }
        }
    }

    wrap_app! {
        pub struct Application;

        impl App {
            fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
                Some(Process::new())
            }

            /// Switches handed to Chromium from outside -- the debug tools'
            /// DevTools port -- the way WebView2 takes them from
            /// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS. Only this process's:
            /// the helpers are told what they need by Chromium itself
            fn on_before_command_line_processing(
                &self,
                process_type: Option<&CefString>,
                command_line: Option<&mut CommandLine>,
            ) {
                if process_type.is_some_and(|t| !t.to_string().is_empty()) {
                    return;
                }
                let Some(line) = command_line else { return };
                for switch in super::extra_switches() {
                    match switch.split_once('=') {
                        Some((name, value)) => {
                            line.append_switch_with_value(Some(&CefString::from(name)), Some(&CefString::from(value)))
                        }
                        None => line.append_switch(Some(&CefString::from(switch.as_str()))),
                    }
                }
            }
        }
    }

    pub(super) fn app() -> App {
        Application::new()
    }

    wrap_client! {
        pub struct PageClient {
            hooks: Rc<Hooks>,
        }

        impl Client {
            fn life_span_handler(&self) -> Option<LifeSpanHandler> {
                Some(Life::new(Rc::clone(&self.hooks)))
            }
            fn load_handler(&self) -> Option<LoadHandler> {
                Some(Load::new(Rc::clone(&self.hooks)))
            }
            fn request_handler(&self) -> Option<RequestHandler> {
                Some(Requests::new(Rc::clone(&self.hooks)))
            }
            fn download_handler(&self) -> Option<DownloadHandler> {
                Some(Downloads::new(Rc::clone(&self.hooks)))
            }
            fn find_handler(&self) -> Option<FindHandler> {
                Some(Finds::new(Rc::clone(&self.hooks)))
            }
            fn keyboard_handler(&self) -> Option<KeyboardHandler> {
                Some(Keys::new(Rc::clone(&self.hooks)))
            }
            fn drag_handler(&self) -> Option<DragHandler> {
                Some(Drags::new(Rc::clone(&self.hooks)))
            }
        }
    }

    wrap_drag_handler! {
        pub struct Drags {
            hooks: Rc<Hooks>,
        }

        impl DragHandler {
            /// Where the page marks itself as a window's bar (CSS `app-region:
            /// drag`, less what is marked `no-drag`): heard only from a page
            /// over a whole window, and handed to the application, which hands
            /// a press there to the system as a press on a title bar
            fn on_draggable_regions_changed(
                &self,
                _browser: Option<&mut Browser>,
                frame: Option<&mut Frame>,
                regions: Option<&[DraggableRegion]>,
            ) {
                let Some(window) = self.hooks.bar_of else { return };
                // The page's own document, not a frame inside it
                if frame.is_some_and(|f| f.is_main() == 0) {
                    return;
                }
                let regions = regions
                    .unwrap_or_default()
                    .iter()
                    .map(|r| {
                        let b = &r.bounds;
                        (f64::from(b.x), f64::from(b.y), f64::from(b.width), f64::from(b.height), r.draggable != 0)
                    })
                    .collect();
                super::mac::set_bar(window, regions);
            }
        }
    }

    pub(super) fn client(hooks: Rc<Hooks>) -> Client {
        PageClient::new(hooks)
    }

    wrap_life_span_handler! {
        pub struct Life {
            hooks: Rc<Hooks>,
        }

        impl LifeSpanHandler {
            /// A window the page asked for: made as a view in the opener's
            /// seat, with handlers of its own, and adopted once it exists
            fn on_before_popup(
                &self,
                _browser: Option<&mut Browser>,
                _frame: Option<&mut Frame>,
                _popup_id: ::std::os::raw::c_int,
                target_url: Option<&CefString>,
                _target_frame_name: Option<&CefString>,
                _target_disposition: WindowOpenDisposition,
                _user_gesture: ::std::os::raw::c_int,
                _popup_features: Option<&PopupFeatures>,
                window_info: Option<&mut WindowInfo>,
                client: Option<&mut Option<Client>>,
                _settings: Option<&mut BrowserSettings>,
                _extra_info: Option<&mut Option<DictionaryValue>>,
                _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
            ) -> ::std::os::raw::c_int {
                let Some(popups) = self.hooks.wiring.popups.clone() else { return 1 };
                // Never make a window while the loop is in the middle of
                // counting them. Refusing is what happens then
                let Some(name) = popups.next_name() else { return 1 };
                let uri = target_url.map(|u| u.to_string()).unwrap_or_default();
                let (x, y, w, h) = popups.seat.get();
                let shared = Rc::clone(&self.hooks.shared);
                if let Some(info) = window_info {
                    *info = WindowInfo::default().set_as_child(
                        shared.parent,
                        &Rect { x: x.max(0), y: y.max(0), width: w.max(0), height: h.max(0) },
                    );
                }
                let hooks = Rc::new(Hooks {
                    wiring: popups.wiring(&name),
                    shared,
                    going: std::cell::Cell::new(false),
                    popup: std::cell::RefCell::new(Some((name, uri))),
                    finds: std::cell::RefCell::new(None),
                    bar_of: None,
                });
                if let Some(client) = client {
                    *client = Some(PageClient::new(hooks));
                }
                0
            }

            fn on_after_created(&self, browser: Option<&mut Browser>) {
                let Some((name, uri)) = self.hooks.popup.borrow_mut().take() else { return };
                let (Some(browser), Some(popups)) = (browser, self.hooks.wiring.popups.clone()) else { return };
                super::count_live(true);
                let init = popups.init_js();
                match super::Page::wire_up(browser.clone(), Rc::clone(&self.hooks), &init, popups.user_agent.as_deref(), None) {
                    Ok(page) => {
                        page.place(popups.seat.get());
                        popups.made(name, page, &uri);
                    }
                    Err(e) => popups.failed(&uri, &e),
                }
            }

            /// The page asked to close its window (`window.close()`), unless
            /// it is this program closing it
            fn do_close(&self, _browser: Option<&mut Browser>) -> ::std::os::raw::c_int {
                if !self.hooks.going.get()
                    && let Some(closing) = &self.hooks.wiring.closing
                {
                    closing();
                }
                0
            }

            fn on_before_close(&self, _browser: Option<&mut Browser>) {
                super::count_live(false);
            }
        }
    }

    wrap_load_handler! {
        pub struct Load {
            hooks: Rc<Hooks>,
        }

        impl LoadHandler {
            /// On as the page starts loading (the moment it is pressed, before
            /// any answer), off when it has finished: lit for the whole wait
            fn on_loading_state_change(
                &self,
                _browser: Option<&mut Browser>,
                is_loading: ::std::os::raw::c_int,
                _can_go_back: ::std::os::raw::c_int,
                _can_go_forward: ::std::os::raw::c_int,
            ) {
                if let Some(loading) = &self.hooks.wiring.loading {
                    loading(is_loading != 0);
                }
            }
        }
    }

    wrap_request_handler! {
        pub struct Requests {
            hooks: Rc<Hooks>,
        }

        impl RequestHandler {
            /// One of the page's processes died. Out of memory is told apart:
            /// a program that answers that by building another browser is a
            /// program making it worse on a timer
            fn on_render_process_terminated(
                &self,
                _browser: Option<&mut Browser>,
                status: TerminationStatus,
                _error_code: ::std::os::raw::c_int,
                _error_string: Option<&CefString>,
            ) {
                if let Some(died) = &self.hooks.wiring.died {
                    died(status == TerminationStatus::PROCESS_OOM);
                }
            }
        }
    }

    wrap_download_handler! {
        pub struct Downloads {
            hooks: Rc<Hooks>,
        }

        impl DownloadHandler {
            fn can_download(&self, _browser: Option<&mut Browser>, _url: Option<&CefString>, _request_method: Option<&CefString>) -> ::std::os::raw::c_int {
                i32::from(self.hooks.wiring.downloads.is_some())
            }

            /// Where the file goes is the browser's own decision done the
            /// browser's way: the Downloads folder, numbered as a browser
            /// numbers a second file of one name. No dialog: the list beside
            /// the page is the one every screen -- a phone included -- reads
            fn on_before_download(
                &self,
                _browser: Option<&mut Browser>,
                download_item: Option<&mut DownloadItem>,
                suggested_name: Option<&CefString>,
                callback: Option<&mut BeforeDownloadCallback>,
            ) -> ::std::os::raw::c_int {
                let (Some(item), Some(callback)) = (download_item, callback) else { return 0 };
                let name = shikisha_core::downloads::safe_name(&suggested_name.map(|n| n.to_string()).unwrap_or_default());
                let dir = shikisha_core::downloads::folder();
                let Some(path) = shikisha_core::downloads::take_place(&dir, &name) else { return 0 };
                callback.cont(Some(&CefString::from(path.to_string_lossy().as_ref())), 0);
                if let Some((page, _)) = &self.hooks.wiring.downloads {
                    shikisha_core::append_hook_log(&format!(
                        "[browser] {:?} saving c{}: {}",
                        page.as_deref().unwrap_or("?"),
                        item.id(),
                        CefString::from(&item.url())
                    ));
                }
                1
            }

            fn on_download_updated(
                &self,
                _browser: Option<&mut Browser>,
                download_item: Option<&mut DownloadItem>,
                callback: Option<&mut DownloadItemCallback>,
            ) {
                let Some(item) = download_item else { return };
                let Some((page, tell)) = &self.hooks.wiring.downloads else { return };
                let report = super::download_of(item);
                let going = report.state == shikisha_shared::DownloadState::Going;
                let now = std::time::Instant::now();
                let say = super::SAVING.with(|s| {
                    let mut s = s.borrow_mut();
                    if !going {
                        s.remove(&report.id);
                        return true;
                    }
                    match (s.get_mut(&report.id), callback) {
                        (Some((_, said)), _) if now.duration_since(*said) < std::time::Duration::from_millis(250) => false,
                        (Some((_, said)), _) => {
                            *said = now;
                            true
                        }
                        (None, Some(callback)) => {
                            s.insert(report.id.clone(), (callback.clone(), now));
                            true
                        }
                        (None, None) => true,
                    }
                });
                if !going {
                    shikisha_core::append_hook_log(&format!(
                        "[browser] {} ended {:?} {}: {}",
                        report.id, report.state, report.why, report.path
                    ));
                }
                if say {
                    let _ = tell.send(shikisha_shared::Ev::Download { from: page.clone(), item: report });
                }
            }
        }
    }

    wrap_find_handler! {
        pub struct Finds {
            hooks: Rc<Hooks>,
        }

        impl FindHandler {
            fn on_find_result(
                &self,
                _browser: Option<&mut Browser>,
                _identifier: ::std::os::raw::c_int,
                count: ::std::os::raw::c_int,
                _selection_rect: Option<&Rect>,
                active_match_ordinal: ::std::os::raw::c_int,
                _final_update: ::std::os::raw::c_int,
            ) {
                if let Some((page, tell)) = self.hooks.finds.borrow().as_ref() {
                    let of = count.max(0) as u32;
                    let at = if of == 0 { 0 } else { (active_match_ordinal.max(0) as u32).min(of) };
                    let _ = tell.send(shikisha_shared::Ev::Seek { from: page.clone(), at, of });
                }
            }
        }
    }

    wrap_keyboard_handler! {
        pub struct Keys {
            hooks: Rc<Hooks>,
        }

        impl KeyboardHandler {
            /// ⌘F, ⌘G and ⇧⌘G pressed in this page -- a Mac's keys for
            /// searching a page -- and F3, open the board's search row and
            /// move through it instead of the browser's own. Pages of the
            /// app's own (the settings) keep the browser's: they are not
            /// browser tabs, and the board's row stands over browser tabs only
            fn on_pre_key_event(
                &self,
                browser: Option<&mut Browser>,
                event: Option<&KeyEvent>,
                _os_event: *mut u8,
                _is_keyboard_shortcut: Option<&mut ::std::os::raw::c_int>,
            ) -> ::std::os::raw::c_int {
                let (Some(event), Some(browser)) = (event, browser) else { return 0 };
                let Some((page, tell, ours)) = &self.hooks.wiring.find_keys else { return 0 };
                if event.type_ != KeyEventType::RAWKEYDOWN {
                    return 0;
                }
                let flag = |f: sys::cef_event_flags_t| event.modifiers & f.0 as u32 != 0;
                let command = flag(sys::cef_event_flags_t::EVENTFLAG_COMMAND_DOWN);
                let shift = flag(sys::cef_event_flags_t::EVENTFLAG_SHIFT_DOWN);
                let option = flag(sys::cef_event_flags_t::EVENTFLAG_ALT_DOWN);
                let what = match event.windows_key_code {
                    0x46 if command && !shift && !option => "open",
                    0x47 if command && !option => {
                        if shift {
                            "prev"
                        } else {
                            "next"
                        }
                    }
                    0x72 if !command && !option => {
                        if shift {
                            "prev"
                        } else {
                            "next"
                        }
                    }
                    _ => return 0,
                };
                let at = browser.main_frame().map(|f| CefString::from(&f.url()).to_string()).unwrap_or_default();
                if ours(&at) || !super::FIND_KEYS.with(|k| k.borrow().contains(page)) {
                    return 0;
                }
                let _ = tell.send(shikisha_shared::Ev::SeekAsk { from: Some(page.clone()), what: what.to_string(), text: String::new() });
                1
            }
        }
    }
}

/// The few calls into AppKit the pages' views need
mod mac {
    use objc2::msg_send;
    use objc2::runtime::{AnyObject, Bool};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    /// Put a page's view at a place in its window's view, in points, counted
    /// from the top left as the window counts them. A view that does not say it
    /// counts from the top (`isFlipped`) counts from the bottom
    pub(super) fn set_frame(view: *mut std::ffi::c_void, parent: *mut std::ffi::c_void, (x, y, w, h): (f64, f64, f64, f64)) {
        if view.is_null() || parent.is_null() {
            return;
        }
        let view = view.cast::<AnyObject>();
        let parent = parent.cast::<AnyObject>();
        unsafe {
            let bounds: NSRect = msg_send![parent, bounds];
            let flipped: Bool = msg_send![parent, isFlipped];
            let top = if flipped.as_bool() { y } else { bounds.size.height - y - h.max(0.0) };
            let frame = NSRect::new(NSPoint::new(x, top), NSSize::new(w.max(0.0), h.max(0.0)));
            let _: () = msg_send![view, setFrame: frame];
        }
    }

    /// In front of the other views of its window
    pub(super) fn raise(view: *mut std::ffi::c_void, parent: *mut std::ffi::c_void) {
        if view.is_null() || parent.is_null() {
            return;
        }
        // NSWindowAbove
        const ABOVE: isize = 1;
        unsafe {
            let none: *mut AnyObject = std::ptr::null_mut();
            let _: () = msg_send![parent.cast::<AnyObject>(), addSubview: view.cast::<AnyObject>(), positioned: ABOVE, relativeTo: none];
        }
    }

    /// Where each window may be taken hold of, as its page last said: rects in
    /// the page's pixels from the window's top left, each one either part of
    /// the bar or carved out of it (a button sitting on it). By the window's
    /// address; touched only on the main thread
    static BARS: std::sync::Mutex<Vec<(usize, Vec<(f64, f64, f64, f64, bool)>)>> = std::sync::Mutex::new(Vec::new());

    pub(super) fn set_bar(window: usize, regions: Vec<(f64, f64, f64, f64, bool)>) {
        let mut bars = BARS.lock().unwrap_or_else(|e| e.into_inner());
        bars.retain(|(w, _)| *w != window);
        if !regions.is_empty() {
            bars.push((window, regions));
        }
    }

    pub(super) fn forget_bar(window: usize) {
        BARS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(w, _)| *w != window);
    }

    /// Whether a point of `window` (from its top left) is on its bar: inside
    /// a part of it, and not on anything carved out of it
    fn on_bar(window: usize, (x, y): (f64, f64)) -> bool {
        let bars = BARS.lock().unwrap_or_else(|e| e.into_inner());
        let Some((_, regions)) = bars.iter().find(|(w, _)| *w == window) else { return false };
        let inside = |&(rx, ry, rw, rh, _): &(f64, f64, f64, f64, bool)| x >= rx && x < rx + rw && y >= ry && y < ry + rh;
        regions.iter().any(|r| r.4 && inside(r)) && !regions.iter().any(|r| !r.4 && inside(r))
    }

    /// A press on a window's bar, given to the system the way a press on a
    /// title bar is: it drags the window from the point pressed, and a second
    /// press in a row does what System Settings says a double-click does.
    /// The page does not see it. `false` when the press is somewhere else
    fn took_bar_press(event: *mut AnyObject) -> bool {
        unsafe {
            // NSEventTypeLeftMouseDown
            let kind: usize = msg_send![event, type];
            if kind != 1 {
                return false;
            }
            let window: *mut AnyObject = msg_send![event, window];
            if window.is_null() {
                return false;
            }
            let content: *mut AnyObject = msg_send![window, contentView];
            if content.is_null() {
                return false;
            }
            let bounds: NSRect = msg_send![content, bounds];
            let at: NSPoint = msg_send![event, locationInWindow];
            if !on_bar(window as usize, (at.x, bounds.size.height - at.y)) {
                return false;
            }
            // A press on a window behind another app's still brings it forward
            let app: *mut AnyObject = msg_send![objc2::class!(NSApplication), sharedApplication];
            let active: Bool = msg_send![app, isActive];
            if !active.as_bool() {
                let _: () = msg_send![app, activateIgnoringOtherApps: true];
            }
            let none: *const AnyObject = std::ptr::null();
            let _: () = msg_send![window, makeKeyAndOrderFront: none];
            let clicks: isize = msg_send![event, clickCount];
            if clicks >= 2 {
                super::super::frame::bar_double_clicked_mac(window);
            } else {
                let _: () = msg_send![window, performWindowDragWithEvent: event];
            }
            true
        }
    }

    /// Whether the application is in the middle of handing out an event
    static SENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// The application's own `sendEvent:`, as tao made it
    static ORIGINAL_SEND: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

    extern "C" fn is_handling_send_event(_this: *mut AnyObject, _sel: objc2::runtime::Sel) -> Bool {
        Bool::new(SENDING.load(std::sync::atomic::Ordering::SeqCst))
    }

    extern "C" fn set_handling_send_event(_this: *mut AnyObject, _sel: objc2::runtime::Sel, on: Bool) {
        SENDING.store(on.as_bool(), std::sync::atomic::Ordering::SeqCst);
    }

    extern "C" fn send_event(this: *mut AnyObject, sel: objc2::runtime::Sel, event: *mut AnyObject) {
        if !event.is_null() && took_bar_press(event) {
            return;
        }
        let was = SENDING.swap(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(original) = ORIGINAL_SEND.get() {
            let original: extern "C" fn(*mut AnyObject, objc2::runtime::Sel, *mut AnyObject) =
                unsafe { std::mem::transmute(*original) };
            original(this, sel, event);
        }
        SENDING.store(was, std::sync::atomic::Ordering::SeqCst);
    }

    /// Teach the application tao made what Chromium asks of it: to say whether
    /// it is handing out an event (`CrAppProtocol`), to be told so
    /// (`CrAppControlProtocol`), and to say so around each event it hands out.
    /// Added to tao's own application class rather than replacing it, so tao's
    /// handling of events (its `sendEvent:`) stays in place, called from ours
    pub(super) fn teach_the_application() {
        use objc2::ffi;
        unsafe {
            let Some(class) = objc2::runtime::AnyClass::get(c"TaoApp") else { return };
            let class = class as *const objc2::runtime::AnyClass as *mut objc2::runtime::AnyClass;
            let is = objc2::sel!(isHandlingSendEvent);
            let set = objc2::sel!(setHandlingSendEvent:);
            let send = objc2::sel!(sendEvent:);
            ffi::class_addMethod(class.cast(), is, std::mem::transmute::<extern "C" fn(*mut AnyObject, objc2::runtime::Sel) -> Bool, objc2::runtime::Imp>(is_handling_send_event), c"c@:".as_ptr());
            ffi::class_addMethod(class.cast(), set, std::mem::transmute::<extern "C" fn(*mut AnyObject, objc2::runtime::Sel, Bool), objc2::runtime::Imp>(set_handling_send_event), c"v@:c".as_ptr());
            let previous = ffi::class_replaceMethod(class.cast(), send, std::mem::transmute::<extern "C" fn(*mut AnyObject, objc2::runtime::Sel, *mut AnyObject), objc2::runtime::Imp>(send_event), c"v@:@".as_ptr());
            if let Some(previous) = previous {
                let _ = ORIGINAL_SEND.set(previous as usize);
            }
            for name in [c"CrAppProtocol", c"CrAppControlProtocol", c"CefAppProtocol"] {
                let protocol = ffi::objc_getProtocol(name.as_ptr());
                if !protocol.is_null() {
                    ffi::class_addProtocol(class.cast(), protocol);
                }
            }
        }
    }
}
