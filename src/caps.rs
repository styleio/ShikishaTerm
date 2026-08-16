//! Capabilities granted to automation (file/HTTP). See DESIGN.md chapter 8.5.
//!
//! Nothing is allowed by default. Only what's written in the config file can be used.
//! Not editable from the GUI (this is an advanced feature, and a mistake here has a large blast radius).
//!
//! The scheme is "named gateways". Scripts can't build up their own paths or URLs --
//! they can only call registered names -- so a destination can't be swapped out and
//! credentials can't be exfiltrated. A whitelist for raw paths/URLs also exists, but is empty by default.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;

use anyhow::{Result, bail};
use serde::Deserialize;

/// Files that must never be touched even inside an allowed directory
/// (prevents self-modification and credential exfiltration)
fn is_forbidden(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(name.as_str(), "config.json" | "secrets.json" | ".env") {
        return true;
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // The automation scripts themselves, and encrypted files
    matches!(ext.as_str(), "lua" | "enc")
}

fn rel_is_safe(rel: &str) -> bool {
    let p = Path::new(rel);
    !p.is_absolute()
        && !rel.is_empty()
        && !p
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CapabilitySpec {
    /// Named file gateways
    #[serde(default)]
    pub files: HashMap<String, FileCap>,
    /// Named HTTP gateways
    #[serde(default)]
    pub http: HashMap<String, HttpCap>,
    /// Advanced: directories where raw paths are allowed (empty by default)
    #[serde(default)]
    pub allow_dirs: Vec<String>,
    /// Advanced: hosts where raw URLs are allowed. Matched by exact equality (empty by default)
    #[serde(default)]
    pub allow_hosts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileCap {
    pub dir: String,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub write: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpCap {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    /// Name of the credential to pull from secrets' tokens and attach
    #[serde(default)]
    pub auth_from_secrets: Option<String>,
    /// Header name to carry the credential in (defaults to Authorization)
    #[serde(default = "default_auth_header")]
    pub auth_header: String,
}

fn default_method() -> String {
    "POST".into()
}
fn default_auth_header() -> String {
    "Authorization".into()
}

struct HttpJob {
    url: String,
    method: String,
    body: String,
    auth: Option<(String, String)>,
}

pub struct Capabilities {
    /// What comes from config. Swapped out on reload
    spec: std::cell::RefCell<CapabilitySpec>,
    base: PathBuf,
    tokens: std::cell::RefCell<HashMap<String, String>>,
    tx: std::cell::RefCell<Option<mpsc::Sender<HttpJob>>>,
    /// Browsers looked up by name. Rc/RefCell is fine since hooks run on a single thread
    /// Whether a banner's button was pressed (per name). Cleared once read
    pressed: std::cell::RefCell<HashMap<String, bool>>,
    /// Whether to overlay the terminal
    /// If we have a host window, its handle. Keeping it here means it doesn't become a separate window
    host: std::cell::RefCell<Option<std::rc::Rc<crate::browser::Browser>>>,
    /// The area inside the window where the browser is placed
    area: std::cell::Cell<(i32, i32, i32, i32)>,
    /// Names of pages placed inside the window.
    ///
    /// Without remembering this, there's no way to know where to fix the position
    /// or where to close. Otherwise pages stayed put wherever they were opened and
    /// didn't follow when the window moved
    /// Pages placed in the window (owning workspace, display name).
    /// A Vec so the placement order is preserved
    hosted: std::cell::RefCell<Vec<(usize, String)>>,
    /// The workspace currently being viewed. Names are only meaningful within it
    ws: std::cell::Cell<usize>,
    /// Which page is currently shown in which area. Skipped if unchanged
    shown: std::cell::RefCell<(Option<String>, (i32, i32, i32, i32))>,
    /// Controls shown above a page (per name).
    ///
    /// Unlike the banner, these aren't drawn inside the page. The page is pushed
    /// down a notch and the app draws in the space that opens up. So this side has
    /// to remember it too, and it doesn't disappear across navigation
    nav: std::cell::RefCell<HashMap<String, crate::config::NavSpec>>,
    /// Pages opened because config declared them (name within the window).
    ///
    /// Needs to be distinguished from pages automation opened on its own, and from
    /// the settings screen. Only pages config opened are allowed to be closed when
    /// they disappear from config
    declared: std::cell::RefCell<std::collections::HashSet<String>>,
    /// Secret keys this workspace is allowed to use (default is empty = deny all).
    /// Narrows things down here so a rally (AI-authored Lua) can't repurpose a key meant for something else
    secret_allow: std::cell::RefCell<std::collections::HashSet<String>>,
    /// Allow all secrets, knowingly accepting the risk (a per-workspace toggle)
    secret_allow_all: std::cell::Cell<bool>,
}

/// Default wait time when touching a page.
/// Just checking whether an element exists, so there's no point making this longer
const OP_MS: u64 = 5_000;

impl Capabilities {
    /// The nothing-allowed state (default)
    pub fn disabled() -> Self {
        Self {
            spec: std::cell::RefCell::new(CapabilitySpec::default()),
            base: PathBuf::from("."),
            tokens: std::cell::RefCell::new(HashMap::new()),
            tx: std::cell::RefCell::new(None),
            pressed: std::cell::RefCell::new(HashMap::new()),
            host: std::cell::RefCell::new(None),
            area: std::cell::Cell::new((0, 0, 0, 0)),
            hosted: std::cell::RefCell::new(Vec::new()),
            ws: std::cell::Cell::new(0),
            shown: std::cell::RefCell::new((None, (0, 0, 0, 0))),
            nav: std::cell::RefCell::new(HashMap::new()),
            declared: std::cell::RefCell::new(std::collections::HashSet::new()),
            secret_allow: std::cell::RefCell::new(std::collections::HashSet::new()),
            secret_allow_all: std::cell::Cell::new(false),
        }
    }

    pub fn new(spec: CapabilitySpec, base: PathBuf, tokens: HashMap<String, String>) -> Self {
        let me = Self {
            base,
            ..Self::disabled()
        };
        me.set_config(spec, tokens);
        me
    }

    /// Swap out only the config-sourced parts.
    ///
    /// Placed pages, banners, and bars carry over. Merely reloading config
    /// must not make things vanish from the screen, or stick around when they shouldn't
    pub fn set_config(&self, spec: CapabilitySpec, tokens: HashMap<String, String>) {
        let wants_http = !spec.http.is_empty() || !spec.allow_hosts.is_empty();
        *self.spec.borrow_mut() = spec;
        *self.tokens.borrow_mut() = tokens;
        if !wants_http || self.tx.borrow().is_some() {
            return;
        }
        *self.tx.borrow_mut() = Some(Self::start_sender());
    }

    /// Communication runs on a dedicated thread so it doesn't block the UI
    fn start_sender() -> mpsc::Sender<HttpJob> {
        {
            let (tx, rx) = mpsc::channel::<HttpJob>();
            std::thread::spawn(move || {
                let agent = ureq::Agent::config_builder()
                    .timeout_global(Some(std::time::Duration::from_secs(15)))
                    .build()
                    .new_agent();
                while let Ok(job) = rx.recv() {
                    // GET has no body, so its type differs (ureq)
                    let result = match job.method.to_ascii_uppercase().as_str() {
                        "GET" => {
                            let mut r = agent.get(&job.url);
                            if let Some((h, v)) = &job.auth {
                                r = r.header(h.as_str(), v.as_str());
                            }
                            r.call().map(|x| x.status().as_u16())
                        }
                        m => {
                            let mut r = if m == "PUT" {
                                agent.put(&job.url)
                            } else {
                                agent.post(&job.url)
                            };
                            if let Some((h, v)) = &job.auth {
                                r = r.header(h.as_str(), v.as_str());
                            }
                            r.header("Content-Type", "application/json")
                                .send(&job.body)
                                .map(|x| x.status().as_u16())
                        }
                    };
                    match result {
                        Ok(code) => crate::append_hook_log(&format!(
                            "http {} {} -> {code}",
                            job.method, job.url
                        )),
                        Err(e) => crate::append_hook_log(&crate::i18n::tp(
                            "err.caps.http_failed",
                            &[("url", &job.url), ("e", &format!("{e}"))],
                        )),
                    }
                }
            });
            tx
        }
    }

    /// Resolve the path for a named gateway
    fn named_path(&self, name: &str, rel: &str, want_write: bool) -> Result<PathBuf> {
        let spec = self.spec.borrow();
        let Some(cap) = spec.files.get(name) else {
            bail!(crate::i18n::tp(
                "err.caps.file_cap_unregistered",
                &[("name", name)]
            ));
        };
        if want_write && !cap.write {
            bail!(crate::i18n::tp("err.caps.write_not_allowed", &[("name", name)]));
        }
        if !want_write && !cap.read {
            bail!(crate::i18n::tp("err.caps.read_not_allowed", &[("name", name)]));
        }
        if !rel_is_safe(rel) {
            bail!(crate::i18n::tp("err.caps.bad_filename", &[("rel", rel)]));
        }
        let path = self.base.join(&cap.dir).join(rel);
        if is_forbidden(&path) {
            bail!(crate::i18n::tp("err.caps.file_forbidden", &[("rel", rel)]));
        }
        Ok(path)
    }

    /// A raw path (must be within allow_dirs)
    fn raw_path(&self, p: &str) -> Result<PathBuf> {
        let spec = self.spec.borrow();
        if spec.allow_dirs.is_empty() {
            bail!(crate::i18n::t("err.caps.raw_path_not_allowed"));
        }
        let target = self.base.join(p);
        let parent = target.parent().unwrap_or(Path::new("."));
        let canon_parent = parent.canonicalize().map_err(|_| {
            anyhow::anyhow!(crate::i18n::tp(
                "err.caps.folder_missing",
                &[("path", &parent.display().to_string())]
            ))
        })?;
        let ok = spec.allow_dirs.iter().any(|d| {
            self.base
                .join(d)
                .canonicalize()
                .map(|c| canon_parent.starts_with(c))
                .unwrap_or(false)
        });
        if !ok {
            bail!(crate::i18n::tp("err.caps.location_not_allowed", &[("p", p)]));
        }
        if is_forbidden(&target) {
            bail!(crate::i18n::tp("err.caps.file_forbidden", &[("rel", p)]));
        }
        Ok(target)
    }

    pub fn read(&self, name: &str, rel: &str) -> Result<String> {
        let p = self.named_path(name, rel, false)?;
        Ok(std::fs::read_to_string(&p)?)
    }

    pub fn write(&self, name: &str, rel: &str, data: &str) -> Result<()> {
        let p = self.named_path(name, rel, true)?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::crypto::write_atomic(&p, data)?;
        crate::append_hook_log(&format!("write_file {name}/{rel} ({} bytes)", data.len()));
        Ok(())
    }

    pub fn read_raw(&self, p: &str) -> Result<String> {
        Ok(std::fs::read_to_string(self.raw_path(p)?)?)
    }

    pub fn write_raw(&self, p: &str, data: &str) -> Result<()> {
        let path = self.raw_path(p)?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::crypto::write_atomic(&path, data)?;
        crate::append_hook_log(&format!("write_path {p} ({} bytes)", data.len()));
        Ok(())
    }

    /// Send to a named HTTP gateway (fire and forget; the response is left in the log)
    pub fn http(&self, name: &str, body: &str) -> Result<()> {
        let spec = self.spec.borrow();
        let Some(cap) = spec.http.get(name) else {
            bail!(crate::i18n::tp(
                "err.caps.http_cap_unregistered",
                &[("name", name)]
            ));
        };
        let auth = cap.auth_from_secrets.as_ref().and_then(|key| {
            self.tokens
                .borrow()
                .get(key)
                .map(|v| (cap.auth_header.clone(), v.clone()))
        });
        self.dispatch(HttpJob {
            url: cap.url.clone(),
            method: cap.method.clone(),
            body: body.to_string(),
            auth,
        })
    }

    /// Mask known secret values out of text headed to the AI (GitHub-style redaction).
    ///
    /// The sandbox has no function that "reads" a secret's raw value, but as a
    /// safeguard against loopholes -- e.g. a value typed into a form being read
    /// back out through the DOM -- any text bound for the AI (browser read
    /// results, AI logs) should pass through here first.
    /// Values that are too short are excluded, since they'd blot out ordinary words too (4+ chars only)
    pub fn redact(&self, text: &str) -> String {
        let tokens = self.tokens.borrow();
        let mut out = text.to_string();
        for v in tokens.values() {
            if v.trim().chars().count() >= 4 && out.contains(v.as_str()) {
                out = out.replace(v.as_str(), "••••");
            }
        }
        out
    }

    /// Tell it which secrets this workspace may use (called on every switch).
    /// Default is deny-all. all=true is the knowingly-risky allow-all toggle
    pub fn set_secret_allow(&self, keys: Vec<String>, all: bool) {
        *self.secret_allow.borrow_mut() = keys.into_iter().collect();
        self.secret_allow_all.set(all);
    }

    /// Retrieve a secret's raw value (Rust-internal only. Never returned to Lua).
    ///
    /// Rejects keys not on the allowlist. Even if a rally tries to repurpose a key
    /// meant for something else, it's stopped here. The value itself is only used
    /// by the caller (browser_fill_secret etc) for auth or form-filling, and never surfaces into the AI's world
    pub fn secret_value(&self, key: &str) -> Result<String> {
        if !self.secret_allow_all.get() && !self.secret_allow.borrow().contains(key) {
            bail!(crate::i18n::tp("err.caps.secret_not_allowed", &[("key", key)]));
        }
        self.tokens.borrow().get(key).cloned().ok_or_else(|| {
            anyhow::anyhow!(crate::i18n::tp(
                "err.caps.secret_unregistered",
                &[("key", key)]
            ))
        })
    }

    /// Tell it where to place things inside the window. Reset every time config reloads
    pub fn set_host(
        &self,
        host: Option<(std::rc::Rc<crate::browser::Browser>, (i32, i32, i32, i32))>,
    ) {
        match host {
            Some((h, area)) => {
                *self.host.borrow_mut() = Some(h);
                self.area.set(area);
            }
            None => *self.host.borrow_mut() = None,
        }
    }

    /// Open a browser (navigates there if the same name already exists).
    /// profile specifies how data is stored (profile name / private)
    pub fn browser_open(
        &self,
        name: &str,
        url: &str,
        profile: crate::browser::BrowserProfile,
    ) -> Result<()> {
        // If there's a host window, place it inside that. A separate window would mean handling position and stacking order ourselves
        let host = self
            .host
            .borrow()
            .as_ref()
            .map(std::rc::Rc::clone)
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.caps.no_host_window")))?;
        let ws = self.ws.get();
        host.open_child(&Self::key(ws, name), url, self.area.get(), profile)?;
        let mut hosted = self.hosted.borrow_mut();
        if !hosted.iter().any(|(w, x)| *w == ws && x == name) {
            hosted.push((ws, name.to_string()));
        }
        // Newly placed items get their position decided on the next redraw
        *self.shown.borrow_mut() = (None, (0, 0, 0, 0));
        crate::append_hook_log(&crate::i18n::tp(
            "err.caps.log_browser_open",
            &[("name", name), ("url", url)],
        ));
        Ok(())
    }

    /// Names of pages placed inside the window (in placement order).
    /// Becomes the tab ordering as-is
    pub fn hosted_names(&self) -> Vec<String> {
        let ws = self.ws.get();
        self.hosted
            .borrow()
            .iter()
            .filter(|(w, _)| *w == ws)
            .map(|(_, n)| n.clone())
            .collect()
    }

    /// Tell it which workspace is currently being viewed. Called on every switch
    pub fn set_workspace(&self, ws: usize) {
        self.ws.set(ws);
    }

    /// The actual name used when placing something in the window.
    /// A different workspace means a different page even under the same display name
    fn key(ws: usize, name: &str) -> String {
        format!("{ws}/{name}")
    }

    /// Tell it the content area. Called every time the window is resized
    pub fn set_area(&self, area: (i32, i32, i32, i32)) {
        self.area.set(area);
    }

    /// Show only one page, and collapse the rest.
    ///
    /// It's a tab bar, so only one page needs to be visible at a time.
    /// "Collapse" means setting width and height to 0. Removing it outright would force a reload.
    ///
    /// Redraws happen 60 times a second. Sends nothing if nothing changed
    pub fn show_only(&self, name: Option<&str>) {
        let want = (name.map(str::to_string), self.area.get());
        if *self.shown.borrow() == want {
            return;
        }
        let Some(h) = self.host.borrow().as_ref().map(std::rc::Rc::clone) else {
            return;
        };
        let ws = self.ws.get();
        for (w, held) in self.hosted.borrow().iter() {
            // Pages from other workspaces are kept alive but collapsed
            let r = if *w == ws && Some(held.as_str()) == name {
                want.1
            } else {
                (0, 0, 0, 0)
            };
            let _ = h.child_bounds(&Self::key(*w, held), r);
        }
        *self.shown.borrow_mut() = want;
    }

    /// Look up a page by name and hand it the operation.
    ///
    /// The page lives inside the window; this asks the window to address "this page".
    ///
    /// Never read the window's reports from here. There's only one report channel,
    /// and screen operations ride the same channel. Intercepting it from the side
    /// would drop keystrokes or tab switches. A pressed banner is received by the
    /// main loop and handed over via note_press
    fn with<T>(
        &self,
        name: &str,
        f: impl FnOnce(&crate::browser::Browser, Option<&str>) -> Result<T>,
    ) -> Result<T> {
        let ws = self.ws.get();
        if !self.hosted.borrow().iter().any(|(w, x)| *w == ws && x == name) {
            return Err(anyhow::anyhow!(crate::i18n::tp(
                "err.caps.browser_not_open",
                &[("name", name)]
            )));
        }
        let host = self
            .host
            .borrow()
            .as_ref()
            .map(std::rc::Rc::clone)
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.caps.no_host_window")))?;
        f(&host, Some(&Self::key(ws, name)))
    }

    /// Record that a banner's button was pressed.
    /// For pages placed inside the window, this arrives via the main loop, which
    /// receives the report. The name received is the in-window name (with the workspace number)
    pub fn note_press(&self, child: &str) {
        self.pressed.borrow_mut().insert(child.to_string(), true);
    }

    /// Turn an in-window name back into the human-facing display name.
    ///
    /// None if it doesn't belong to the workspace currently being viewed.
    /// An event from a page in another workspace must not be allowed to trigger this hook
    pub fn name_of_child(&self, child: &str) -> Option<String> {
        let head = format!("{}/", self.ws.get());
        child.strip_prefix(&head).map(str::to_string)
    }

    pub fn browser_find(&self, name: &str, sel: &crate::browser::Sel) -> Result<&'static str> {
        self.with(name, |b, to| Ok(b.find(to, sel, OP_MS)?.as_str()))
    }

    pub fn browser_click(&self, name: &str, sel: &crate::browser::Sel) -> Result<&'static str> {
        self.with(name, |b, to| Ok(b.click(to, sel, OP_MS)?.as_str()))
    }

    pub fn browser_fill(
        &self,
        name: &str,
        sel: &crate::browser::Sel,
        value: &str,
    ) -> Result<&'static str> {
        self.with(name, |b, to| Ok(b.fill(to, sel, value, OP_MS)?.as_str()))
    }

    pub fn browser_text(&self, name: &str, sel: &crate::browser::Sel) -> Result<Option<String>> {
        self.with(name, |b, to| b.text(to, sel, OP_MS))
    }

    pub fn browser_html(&self, name: &str) -> Result<String> {
        self.with(name, |b, to| b.html(to, 30_000))
    }

    /// Make a request from inside the page, returning a `{status,ok,url,headers,body}` JSON string
    pub fn browser_fetch(&self, name: &str, url: &str, opts: &serde_json::Value) -> Result<String> {
        self.with(name, |b, to| b.fetch(to, url, opts, 30_000))
    }

    /// Show a banner asking the human something
    pub fn browser_ask(&self, name: &str, text: &str, label: &str) -> Result<()> {
        self.forget_press(name);
        self.with(name, |b, to| b.ask(to, text, label))
    }

    /// Whether the banner's button was pressed. If so, clears it and returns true
    pub fn browser_pressed(&self, name: &str) -> Result<bool> {
        self.with(name, |_, _| Ok(()))?;
        Ok(self.forget_press(name))
    }

    /// Clear the pressed record. Keyed by the in-window name
    fn forget_press(&self, name: &str) -> bool {
        let key = Self::key(self.ws.get(), name);
        self.pressed.borrow_mut().remove(&key).unwrap_or(false)
    }

    pub fn browser_unask(&self, name: &str) -> Result<()> {
        self.forget_press(name);
        self.with(name, |b, to| b.unask(to))
    }

    /// Show controls above a page. If there's nothing to show, it's as if nothing were shown at all
    pub fn browser_nav(&self, name: &str, spec: crate::config::NavSpec) -> Result<()> {
        // Can't show controls on a page that isn't open. Rejected here
        self.with(name, |_, _| Ok(()))?;
        let key = Self::key(self.ws.get(), name);
        if spec.is_empty() {
            self.nav.borrow_mut().remove(&key);
        } else {
            self.nav.borrow_mut().insert(key, spec);
        }
        Ok(())
    }

    pub fn browser_unnav(&self, name: &str) -> Result<()> {
        self.with(name, |_, _| Ok(()))?;
        self.nav.borrow_mut().remove(&Self::key(self.ws.get(), name));
        Ok(())
    }

    /// Navigate a page. Converting the display name to the in-window name is this side's job.
    /// If the caller had to do that conversion, forgetting it would show up as "nothing happens"
    pub fn browser_go(&self, name: &str, go: crate::browser::Go) -> Result<()> {
        self.with(name, |b, to| b.go(to, go))
    }

    /// Ask where it currently is (the answer arrives as a report)
    pub fn browser_where(&self, name: &str) -> Result<()> {
        self.with(name, |b, to| b.ask_where(to))
    }

    /// Move keyboard focus to this page
    pub fn browser_focus(&self, name: &str) -> Result<()> {
        self.with(name, |b, to| b.focus(to))
    }

    /// Start/stop this page's screencast (frames arrive via reports to the main loop)
    pub fn browser_screencast(&self, name: &str, on: bool) -> Result<()> {
        self.with(name, |b, to| b.screencast(to, on))
    }

    /// Inject input into the relay screen (finger trails, swipes, characters)
    pub fn browser_inject(&self, name: &str, input: crate::browser::Input) -> Result<()> {
        self.with(name, |b, to| b.inject(to, input))
    }

    /// Press a single named control key (enter/tab/escape/arrows/…) on the
    /// page's focused element, as a real CDP key event. This is the primitive
    /// that composes with browser_fill: fill a field, then press "enter" to
    /// search or submit a form. Plain text still goes through browser_fill.
    pub fn browser_press(&self, name: &str, key: &str) -> Result<()> {
        let named = key.trim().to_lowercase();
        if !crate::browser::key_known(&named) {
            anyhow::bail!(crate::i18n::tp("err.caps.unknown_key", &[("key", key)]));
        }
        self.browser_inject(
            name,
            crate::browser::Input::Key {
                named,
                ctrl: false,
                alt: false,
            },
        )
    }

    /// Set up basic auth. Credentials are resolved as `user:pass` from a secret (must be on the allowlist).
    /// The value is decoded here and passed straight to CDP -- never exposed to Lua/AI
    pub fn browser_auth(&self, name: &str, secret_key: &str) -> Result<()> {
        let val = self.secret_value(secret_key)?;
        let (user, pass) = val.split_once(':').ok_or_else(|| {
            anyhow::anyhow!(crate::i18n::tp(
                "err.caps.secret_format",
                &[("secret_key", secret_key)]
            ))
        })?;
        self.with(name, |b, to| b.basic_auth(to, user, pass))
    }

    /// Line pages up to match config exactly.
    ///
    /// Pages removed from config get closed. Without this, a leftover page would
    /// linger at the back of the list as "a browser not in config" -- it was
    /// deleted, yet it reappears elsewhere (and won't go away until restart).
    /// Pages automation opened on its own, and the settings screen, are left untouched here
    pub fn keep_only_declared(&self, names: &[String]) -> Vec<String> {
        let ws = self.ws.get();
        let want: std::collections::HashSet<String> =
            names.iter().map(|n| Self::key(ws, n)).collect();
        let stale: Vec<String> = self
            .declared
            .borrow()
            .iter()
            .filter(|k| k.starts_with(&format!("{ws}/")) && !want.contains(*k))
            .cloned()
            .collect();
        let mut closed = Vec::new();
        for key in stale {
            self.declared.borrow_mut().remove(&key);
            if let Some(name) = self.name_of_child(&key) {
                if self.browser_close(&name).is_ok() {
                    closed.push(name);
                }
            }
        }
        closed
    }

    /// Note this down as a page config opened
    pub fn note_declared(&self, name: &str) {
        self.declared
            .borrow_mut()
            .insert(Self::key(self.ws.get(), name));
    }

    /// What to show (used by the screen-drawing loop).
    /// Takes the human-facing display name; converting to the in-window name is this side's job
    pub fn nav_of(&self, name: &str) -> Option<crate::config::NavSpec> {
        self.nav
            .borrow()
            .get(&Self::key(self.ws.get(), name))
            .copied()
    }

    pub fn browser_close(&self, name: &str) -> Result<()> {
        let ws = self.ws.get();
        let key = Self::key(ws, name);
        if let Some(h) = self.host.borrow().as_ref() {
            let _ = h.unask(Some(&key));
            h.close_child(&key)?;
        }
        self.hosted.borrow_mut().retain(|(w, x)| !(*w == ws && x == name));
        self.pressed.borrow_mut().remove(&key);
        self.nav.borrow_mut().remove(&key);
        self.declared.borrow_mut().remove(&key);
        *self.shown.borrow_mut() = (None, (0, 0, 0, 0));
        Ok(())
    }

    /// A raw URL (only hosts that exactly match allow_hosts)
    pub fn http_raw(&self, url: &str, body: &str) -> Result<()> {
        let host = host_of(url)
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::tp("err.caps.bad_url", &[("url", url)])))?;
        if !url.starts_with("https://") {
            bail!(crate::i18n::t("err.caps.https_only"));
        }
        if !self.spec.borrow().allow_hosts.iter().any(|h| h == &host) {
            bail!(crate::i18n::tp("err.caps.host_not_allowed", &[("host", &host)]));
        }
        self.dispatch(HttpJob {
            url: url.to_string(),
            method: "POST".into(),
            body: body.to_string(),
            auth: None,
        })
    }

    fn dispatch(&self, job: HttpJob) -> Result<()> {
        let tx = self.tx.borrow();
        let Some(tx) = tx.as_ref() else {
            bail!(crate::i18n::t("err.caps.http_disabled"));
        };
        tx.send(job)
            .map_err(|_| anyhow::anyhow!(crate::i18n::t("err.caps.queue_closed")))
    }
}

#[cfg(test)]
mod reload_tests {
    use super::*;

    /// Reloading config must not forget pages placed inside the window.
    ///
    /// Forgetting one leaves it stuck on screen, unable to be operated by anyone.
    /// This used to happen the instant the settings screen was saved -- clicking
    /// the tab appeared to do nothing
    #[test]
    fn reloading_the_settings_does_not_lose_the_pages_on_screen() {
        let c = Capabilities::disabled();
        // There's no stand-in window, so we just create the placement record directly
        c.hosted.borrow_mut().push((0, "settings".into()));
        c.nav
            .borrow_mut()
            .insert("0/html".into(), crate::config::NavSpec::all());
        c.note_press("0/html");

        c.set_config(CapabilitySpec::default(), HashMap::new());

        assert_eq!(c.hosted_names(), vec!["settings".to_string()], "置いたページを忘れた");
        assert!(c.nav_of("html").is_some(), "上のバーを忘れた");
        assert!(c.forget_press("html"), "押された帯を忘れた");
    }

    /// A browser removed from config must be closed.
    ///
    /// Leaving it in place re-lists it at the back as "a page not in config" --
    /// a tab that was supposedly deleted reappears elsewhere (and won't go away until restart)
    #[test]
    fn a_browser_removed_from_the_settings_does_not_come_back() {
        let c = Capabilities::disabled();
        for n in ["ai", "html"] {
            c.hosted.borrow_mut().push((0, n.to_string()));
            c.note_declared(n);
        }
        // Pages automation opened on its own are not closed even if absent from config
        c.hosted.borrow_mut().push((0, "調べ物".into()));

        let closed = c.keep_only_declared(&["ai".to_string()]);
        assert_eq!(closed, vec!["html".to_string()], "消したものが閉じていない");
        assert_eq!(
            c.hosted_names(),
            vec!["ai".to_string(), "調べ物".to_string()],
            "設定が開けていないページまで閉じた"
        );
    }

    /// Config-sourced state really must be replaced
    #[test]
    fn the_settings_themselves_are_replaced() {
        let c = Capabilities::disabled();
        assert!(c.read("tmp", "x.txt").is_err(), "何も許していないはず");

        let mut files = HashMap::new();
        files.insert(
            "tmp".to_string(),
            FileCap { dir: ".".into(), read: true, write: false },
        );
        c.set_config(
            CapabilitySpec { files, ..Default::default() },
            HashMap::new(),
        );
        // The gateway is registered (the read itself still fails since the file doesn't exist)
        let err = c.read("tmp", "居ないファイル.txt").unwrap_err().to_string();
        assert!(!err.contains("未登録"), "窓口が入れ替わっていない: {err}");
    }
}

/// Extract just the host portion from a URL (excluding port and credentials)
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let hostport = rest.split(['/', '?', '#']).next()?;
    // Reject forms like user:pass@host (prevents substitution attacks)
    if hostport.contains('@') {
        return None;
    }
    let host = hostport.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(spec: CapabilitySpec, base: PathBuf) -> Capabilities {
        Capabilities::new(spec, base, HashMap::new())
    }

    #[test]
    fn redact_masks_known_secret_values_only_when_long_enough() {
        let mut tokens = HashMap::new();
        tokens.insert("diary".to_string(), "hunter2secret".to_string());
        tokens.insert("short".to_string(), "ab".to_string());
        let c = Capabilities::new(CapabilitySpec::default(), PathBuf::from("."), tokens);
        // A known secret value gets redacted
        let masked = c.redact("Authorization: hunter2secret\n本文");
        assert!(!masked.contains("hunter2secret"), "秘密値が残っている: {masked}");
        assert!(masked.contains("••••"));
        // Values that are too short are excluded, since they'd break ordinary words
        assert_eq!(c.redact("ab cd ab"), "ab cd ab");
    }

    #[test]
    fn secret_value_respects_the_allowlist_and_default_denies() {
        let mut tokens = HashMap::new();
        tokens.insert("diary".to_string(), "hunter2secret".to_string());
        tokens.insert("github".to_string(), "ghp_xxx".to_string());
        let c = Capabilities::new(CapabilitySpec::default(), PathBuf::from("."), tokens);
        // Default is deny-all (empty allowlist)
        assert!(c.secret_value("diary").is_err(), "既定は全拒否のはず");
        // Only an allowed key can be retrieved
        c.set_secret_allow(vec!["diary".to_string()], false);
        assert_eq!(c.secret_value("diary").unwrap(), "hunter2secret");
        assert!(c.secret_value("github").is_err(), "別用途の鍵は流用できない");
        // The knowingly-risky allow-all toggle
        c.set_secret_allow(vec![], true);
        assert_eq!(c.secret_value("github").unwrap(), "ghp_xxx");
        // Even with allow-all, an unregistered key can't be retrieved
        assert!(c.secret_value("nope").is_err(), "未登録は取れない");
    }

    #[test]
    fn nothing_is_allowed_by_default() {
        let c = caps(CapabilitySpec::default(), PathBuf::from("."));
        assert!(c.write("reports", "a.md", "x").is_err());
        assert!(c.http("api", "{}").is_err());
        assert!(c.write_raw("a.md", "x").is_err());
    }

    #[test]
    fn named_file_window_confines_writes() {
        let dir = std::env::temp_dir().join("shikisha-caps");
        std::fs::create_dir_all(&dir).unwrap();
        let mut spec = CapabilitySpec::default();
        spec.files.insert(
            "reports".into(),
            FileCap {
                dir: "reports".into(),
                read: true,
                write: true,
            },
        );
        let c = caps(spec, dir.clone());

        assert!(c.write("reports", "ok.md", "hello").is_ok());
        assert_eq!(c.read("reports", "ok.md").unwrap(), "hello");
        // A path that tries to escape outward is rejected
        assert!(c.write("reports", "../escape.md", "x").is_err());
        assert!(c.write("reports", "C:/windows/x.md", "x").is_err());
        // An unregistered gateway can't be used
        assert!(c.write("other", "a.md", "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crown_jewels_are_never_writable() {
        let dir = std::env::temp_dir().join("shikisha-caps2");
        std::fs::create_dir_all(&dir).unwrap();
        let mut spec = CapabilitySpec::default();
        spec.files.insert(
            "all".into(),
            FileCap {
                dir: ".".into(),
                read: true,
                write: true,
            },
        );
        let c = caps(spec, dir.clone());
        for f in ["config.json", "secrets.json", ".env", "hack.lua", "x.enc"] {
            assert!(c.write("all", f, "x").is_err(), "{f} は拒否されるはず");
            assert!(c.read("all", f).is_err(), "{f} は拒否されるはず");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_matching_is_exact() {
        assert_eq!(host_of("https://api.github.com/x"), Some("api.github.com".into()));
        // Don't let a prefix match slip through
        assert_eq!(
            host_of("https://api.github.com.evil.com/x"),
            Some("api.github.com.evil.com".into())
        );
        // Reject substitution via a URL carrying credentials
        assert_eq!(host_of("https://api.github.com@evil.com/x"), None);

        let mut spec = CapabilitySpec::default();
        spec.allow_hosts.push("api.github.com".into());
        let c = caps(spec, PathBuf::from("."));
        assert!(c.http_raw("https://api.github.com.evil.com/x", "{}").is_err());
        assert!(c.http_raw("http://api.github.com/x", "{}").is_err(), "httpは不可");
    }
}
