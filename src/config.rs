//! config/config.json: defines the workspace / tab layout. See DESIGN.md chapter 7.4.
//! Looked up in the config folder beside the exe, then the config folder in the
//! current directory.
//!
//! Terminology: a "workspace" is the unit you switch between (like a virtual
//! desktop). Its externalized contents form a "workspace definition file".
//!
//! Settings are split into 3 kinds by role (everything the user owns lives
//! under the config folder):
//!   config/config.json  ... global settings + workspace list (rarely changed)
//!   workspaces/*.json   ... workspace definition files (copyable/shareable units)
//!   config/secrets.json ... credentials (can be encrypted, never share)

use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    /// List of workspaces (projects). Switched between like virtual desktops
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSpec>,
    /// Working folders written directly when workspaces are not used
    #[serde(default)]
    pub folders: Vec<FolderConfig>,
    /// The colour chosen for a project, against the folder git shares between
    /// its branches. Nothing here means every project still has a colour --
    /// one worked out from its own name -- so this only ever holds answers
    /// somebody actually gave
    #[serde(default)]
    pub folder_colors: std::collections::HashMap<String, String>,
    /// Global automation shared by everything (e.g. "scripts/common" or "scripts/hooks.lua")
    #[serde(default)]
    pub automation: Option<String>,
    #[serde(default)]
    pub lua: Option<String>,
    /// Max depth of the auto-forward chain (default 10).
    /// Incremented by 1 each time it auto-forwards between tabs; reset to 0 by manual human input
    #[serde(default)]
    pub max_chain: Option<u32>,
    /// Whether automation may switch which tab is on screen (default: yes).
    ///
    /// Only `shikisha.show()` ever moves the view; handing work to a tab does not.
    /// This is the person's answer to that request — see main::ViewMove
    pub auto_switch: Option<bool>,
    /// Whether to start from the last-opened workspace (default: yes)
    pub restore_workspace: Option<bool>,
    /// Whether to overlay the browser on the terminal (default: overlay).
    /// Turning it off makes it a standalone window you can move yourself, but it no longer feels like a tab
    pub browser_overlay: Option<bool>,
    /// Wait time (ms) before a response is considered finished.
    /// If the profile specifies its own value, that takes priority
    pub done_confirm_ms: Option<u64>,
    /// How often, in seconds, automation is told again that a tab is still
    /// working. 0 or unset means never (the default).
    ///
    /// A tab that has been working for twenty minutes without a word is either
    /// thinking hard or hung, and nothing in this app can tell those apart --
    /// but the automation watching it can, because it knows what it asked for.
    /// It is only ever asked again about a tab it was already told about, and
    /// never about one waiting on a person.
    ///
    /// Off by default, and generously set when on: cutting a long think short
    /// is the expensive mistake here, not noticing a hang a minute late.
    pub busy_repeat_sec: Option<u64>,
    /// Whether a program running in a tab may put text on the Windows
    /// clipboard (default: yes).
    ///
    /// This is how tmux, Neovim, fzf and most full-screen tools copy: they do
    /// not call any Windows API -- they cannot, over ssh -- they write the text
    /// into the terminal and let the terminal do it. With this off, copying
    /// inside those tools silently does nothing.
    ///
    /// Reading is never allowed, and there is no setting for it: a program that
    /// could read the clipboard could read whatever was copied last, from
    /// anywhere, including the far end of an ssh session.
    pub tui_clipboard: Option<bool>,
    /// Width of the left tab bar, in pixels, or 0 when it is put away. Omitted
    /// means the built-in width. Dragging the bar's edge writes it back here,
    /// which is how it survives a restart.
    ///
    /// It used to be counted in terminal columns, from the days when the app
    /// drew the bar itself out of characters. The window has been drawing it
    /// for a long time now, and through all of that the number did nothing at
    /// all -- the bar was a fixed width in the stylesheet and never asked.
    #[serde(default)]
    pub tab_bar_width: Option<u16>,
    /// Registered notification destinations (Lua can only send to destinations registered here).
    /// Recommended to keep tokens separated out in secrets.json (gitignored)
    #[serde(default)]
    pub notify: std::collections::HashMap<String, crate::notify::Destination>,
    /// The destination `shikisha.notify(text)` reaches when none is named —
    /// like the default assistant AI, but for notifications. Unset with
    /// exactly one destination configured, that one serves as the primary
    #[serde(default)]
    pub primary_notify: Option<String>,
    /// Load secrets such as notification destinations from a separate file (e.g. "secrets.json")
    #[serde(default)]
    pub secrets: Option<String>,
    /// The AI that writes automation code ("claude" / "codex" / "gemini").
    /// Uses whichever is found if empty
    #[serde(default)]
    pub ai_engine: Option<String>,
    /// Capabilities granted to automation (file/HTTP). Default is empty = nothing allowed.
    /// Not editable from the GUI since this is an advanced feature
    #[serde(default)]
    pub capabilities: crate::caps::CapabilitySpec,
    /// Remote UI viewable from a phone etc. Disabled by default
    #[serde(default)]
    pub remote: RemoteSpec,
    /// Who may drive this app from outside, over its named pipe. The default
    /// is the processes this app started and nothing else — see api.rs
    #[serde(default)]
    pub external_api: crate::api::ApiSpec,
    /// How the terminal is drawn
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub keys: KeyBinds,
    /// Bounds for files pasted/attached into the sub-input bar (saved beside the tab)
    #[serde(default)]
    pub attach: AttachSpec,
    /// Quick actions shown in the sub-input bar. Each inserts text (beginner) or,
    /// when `lua` is set, runs Lua (advanced).
    #[serde(default)]
    pub actions: Vec<ActionSpec>,
    /// Bounds and stall behavior for ad-hoc "operate a tab" (🎯) sessions.
    #[serde(default)]
    pub operate: OperateSpec,
    /// Display language ("ja" etc). Follows the OS setting if omitted
    #[serde(default)]
    pub language: Option<String>,
    /// Where the browser (WebView2) stores its data. Holds cache and login state.
    ///   "local" (default) ... each PC's %LOCALAPPDATA% (not Drive-synced, lightweight)
    ///   "portable"         ... data\webview2 beside the app (shared across PCs via Drive, logins shared too)
    ///   anything else      ... used as an absolute path
    #[serde(default)]
    pub browser_data: Option<String>,
    /// What the browser calls itself when it asks a site for a page.
    ///
    /// Empty means the engine's own, which is Edge's, word for word — this is
    /// a real Chromium and it says so. Some sites keep a list of the browsers
    /// they will hand a sign-in page to, and answer everything else with
    /// "this browser may not be secure"; naming a browser they know is the
    /// only way past that. Applies to pages opened after it is set
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Auxiliary key row for the relay screen (phone remote control). Listed left to right.
    /// Usable names: esc tab space enter backspace delete
    ///   left up down right home end pageup pagedown
    ///   f1-f12 ctrl alt (ctrl/alt are fixed toggles).
    /// Uses cast_keys_default() when omitted
    #[serde(default)]
    pub cast_keys: Option<Vec<String>>,
    /// Connection info for the model bridge (OpenAI-compatible API). name -> {base_url, api_key, headers}.
    /// The bridge that lets discussions and browser operation use cheap/local models (DeepSeek/Qwen/Ollama etc).
    /// A `model <name>/<model>` tab looks this up when it launches
    #[serde(default)]
    pub providers: std::collections::HashMap<String, ProviderSpec>,
}

/// Connection info for an OpenAI-compatible API (DeepSeek cloud / Ollama local / OpenRouter / Azure etc).
/// Two orthogonal axes -- "connection (base_url + auth)" x "model name" -- let cloud/local and model kind vary independently
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProviderSpec {
    /// OpenAI-compatible base URL (e.g. https://api.deepseek.com/v1, http://localhost:11434/v1).
    /// Full paths/queries such as Azure's are used as-is
    #[serde(default)]
    pub base_url: String,
    /// Auth key. "@name" refers to a token in secrets.json's tokens (a literal value also works).
    /// Becomes an Authorization: Bearer <resolved value> header when headers is not specified.
    /// Can be omitted where not needed, e.g. local (Ollama)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Explicit outgoing headers (for Azure's `api-key` or a custom gateway). Values also support
    /// "@name" secrets references. When given, the default api_key Bearer header is not sent -- these are sent instead
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    /// How long to wait for a whole reply, in seconds. **0 waits as long as it
    /// takes.** Left out, `PROVIDER_TIMEOUT_DEFAULT_SEC` applies.
    ///
    /// The reply is asked for in one piece, so this covers everything the far
    /// end does: loading the model, thinking, and writing the answer. A cloud
    /// API is done in seconds and wants a short leash — a limit is the only
    /// thing that tells "still working" apart from "never coming back". A model
    /// on the machine next door is a different animal: a 27B thinking model
    /// took 320 seconds to answer "just say OK", most of it thinking, and a
    /// fixed 180 meant it could never once finish. Which of the two this is, is
    /// not something the app can know — so it is asked.
    #[serde(default)]
    pub timeout_sec: Option<u64>,
}

/// How long to wait for a whole reply from a provider that does not say.
pub const PROVIDER_TIMEOUT_DEFAULT_SEC: u64 = 180;

/// A provider resolved into what one request needs.
///
/// Carried together because they are decided together and travel together; as
/// a loose tuple, adding the third one meant touching every hand it passed
/// through and the compiler could not say which of the two strings was which.
#[derive(Debug, Clone)]
pub struct ProviderConn {
    pub url: String,
    pub headers: std::collections::HashMap<String, String>,
    /// `None` means wait as long as it takes (the person asked for 0).
    pub timeout: Option<std::time::Duration>,
}

/// The tab bar's width as the window should open it, in pixels.
///
/// One place decides it, because three would disagree: the page is built with
/// it, a drag sends a new one back, and the settings screen writes the same
/// field by hand.
pub const TAB_BAR_DEFAULT_PX: u16 = 290;
/// Narrow enough to be a sliver, wide enough that a tab name is still a name.
/// Below the floor there is only one honest width left, which is none at all
pub const TAB_BAR_MIN_PX: u16 = 150;
pub const TAB_BAR_MAX_PX: u16 = 640;

/// A width as it may actually be used: put away (0), or inside the bounds.
pub fn clamp_tab_bar(px: u16) -> u16 {
    if px == 0 {
        0
    } else {
        px.clamp(TAB_BAR_MIN_PX, TAB_BAR_MAX_PX)
    }
}

pub fn tab_bar_px() -> u16 {
    load()
        .and_then(|c| c.tab_bar_width)
        .map(clamp_tab_bar)
        .unwrap_or(TAB_BAR_DEFAULT_PX)
}

/// Default order of the auxiliary key row. Frequently used Enter/Space/Backspace and the
/// arrow keys come first; F1-F12 and Ctrl/Alt come later (reachable by scrolling sideways).
/// Users can freely override this via cast_keys in config
pub fn cast_keys_default() -> Vec<String> {
    [
        "esc", "tab", "left", "up", "down", "right", "space", "enter", "backspace", "ctrl", "alt",
        "home", "end", "pageup", "pagedown", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9",
        "f10", "f11", "f12",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Get the auxiliary key row from config (default if unset). Passed to the relay screen client
pub fn cast_keys() -> Vec<String> {
    load()
        .and_then(|c| c.cast_keys)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(cast_keys_default)
}

/// Decide where WebView2 stores its data, based on config. To avoid Drive cache churn
/// and EBWebView sync notifications, the default is the non-synced local folder (%LOCALAPPDATA%)
/// What the browser should call itself, if anything was asked for.
///
/// Read once when the browser window opens: a name that changed under a page
/// mid-visit would be a different browser halfway through a login
pub fn user_agent() -> Option<String> {
    load()
        .and_then(|c| c.user_agent)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn browser_data_dir() -> std::path::PathBuf {
    let mode = load()
        .and_then(|c| c.browser_data)
        .unwrap_or_default();
    match mode.trim() {
        "portable" => root_dir().join("data").join("webview2"),
        "" | "local" => local_appdata().join("ShikishaTerm").join("webview2"),
        other => std::path::PathBuf::from(other),
    }
}

fn local_appdata() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root_dir().join("data"))
}

/// Remote UI settings. Off by default since this lets AI be operated from afar
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSpec {
    #[serde(default)]
    pub enabled: bool,
    /// "auto" (tries Tailscale, then LAN) / "127.0.0.1" / an explicit IP
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_remote_port")]
    pub port: u16,
    /// Explicitly allow exposing this outside the private network
    #[serde(default)]
    pub allow_public: bool,
    /// Optional second factor on top of the URL token. Empty = off (the
    /// token alone opens the board — the user's own risk to accept). Set,
    /// the phone must enter it once per app run; notification URLs then
    /// carry only the token, never this
    #[serde(default)]
    pub password: String,
    /// Keep the pairing on the phone: the token stays in the URL (bookmarkable)
    /// and in persistent storage, so a discarded tab or a closed browser does
    /// not cost a QR scan. The "disconnect" control still ends every session at
    /// once — the phone's screen goes dark and its touches reach nothing — but
    /// the token is unchanged, so that phone can pair again by opening the link;
    /// shutting it out for good means changing this string. Off (default) = the
    /// token lives only in the tab's session storage and every disconnect
    /// rotates it as well
    #[serde(default)]
    pub sticky_token: bool,
    /// The token itself when sticky: written by the person (or generated
    /// into the settings field for them). Plain text in config.json — the
    /// trade they accepted. Used only when `sticky_token` is on and it is at
    /// least 16 characters; otherwise the usual persisted random token
    #[serde(default)]
    pub fixed_token: String,
}

impl Default for RemoteSpec {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_bind(),
            port: default_remote_port(),
            allow_public: false,
            password: String::new(),
            sticky_token: false,
            fixed_token: String::new(),
        }
    }
}

/// Bounds for a pasted/attached file. Nothing here executes the file (it is saved
/// beside the tab and only its path is handed to the AI), so `extensions` is a UX
/// guard and a nudge to be deliberate — power users can widen it at their own risk.
#[derive(Debug, Clone, Deserialize)]
pub struct AttachSpec {
    /// Max size of one attachment, in megabytes.
    #[serde(default = "default_attach_mb")]
    pub max_mb: u32,
    /// Extensions the user opted into (lowercase, no dot).
    #[serde(default = "default_attach_ext")]
    pub extensions: Vec<String>,
}

impl Default for AttachSpec {
    fn default() -> Self {
        Self {
            max_mb: default_attach_mb(),
            extensions: default_attach_ext(),
        }
    }
}

fn default_attach_mb() -> u32 {
    25
}

/// A one-click action in the sub-input bar. `body` is text to insert into the
/// composer (the beginner default) or, when `lua` is true, Lua run in the scoped
/// sandbox (advanced). Advanced is per-action, so a list can mix both freely.
#[derive(Debug, Clone, Deserialize)]
pub struct ActionSpec {
    /// Button label shown in the bar.
    pub label: String,
    /// Text to insert, or Lua source when `lua` is set.
    #[serde(default)]
    pub body: String,
    /// Advanced: `body` is Lua run in the sandbox, not text to insert.
    #[serde(default)]
    pub lua: bool,
}

/// The quick actions for the sub-input bar (empty if none configured).
pub fn actions() -> Vec<ActionSpec> {
    load().map(|c| c.actions).unwrap_or_default()
}

/// Bounds and stall behavior for an "operate a tab" (🎯) session. The limits are
/// a runaway safety net; `on_limit` decides what happens when one is reached.
/// Every limit accepts 0 to mean "no limit". These also feed a configured
/// browser Agent tab and browser rallies (same built-in orchestrator).
#[derive(Debug, Clone, Deserialize)]
pub struct OperateSpec {
    /// Operator turns before the safety net trips. 0 = unlimited.
    #[serde(default = "default_operate_rounds")]
    pub max_rounds: u32,
    /// Wall-clock seconds before the safety net trips. 0 = unlimited.
    #[serde(default = "default_operate_seconds")]
    pub max_seconds: u32,
    /// Operator output characters (a rough token proxy) before the net trips. 0 = unlimited.
    #[serde(default = "default_operate_tokens")]
    pub max_tokens: u32,
    /// What to do when a limit is reached:
    ///   "stop"     ... halt and tell the operator (default; the safe textbook choice)
    ///   "continue" ... reset the budget and keep going, trusting the operator to
    ///                  judge DONE itself (never stop on the user mid-task)
    #[serde(default = "default_operate_on_limit")]
    pub on_limit: String,
    /// After each browser action, how long to wait for the page to settle (its
    /// text to stop changing) before reading it back, in milliseconds. Guards
    /// against reading a half-rendered page. 0 disables the wait.
    #[serde(default = "default_operate_settle_ms")]
    pub settle_ms: u32,
    /// A brake before the operator acts, so a person can hold a risky step:
    ///   "off"   ... run every action immediately (default)
    ///   "sends" ... pause for approval only before a submit/click/auth step
    ///   "all"   ... pause for approval before every action
    /// Approval is a button shown on the target page; declining holds the run.
    #[serde(default = "default_operate_confirm")]
    pub confirm: String,
}

impl Default for OperateSpec {
    fn default() -> Self {
        Self {
            max_rounds: default_operate_rounds(),
            max_seconds: default_operate_seconds(),
            max_tokens: default_operate_tokens(),
            on_limit: default_operate_on_limit(),
            settle_ms: default_operate_settle_ms(),
            confirm: default_operate_confirm(),
        }
    }
}

fn default_operate_rounds() -> u32 {
    40
}
fn default_operate_seconds() -> u32 {
    900
}
fn default_operate_tokens() -> u32 {
    400_000
}
fn default_operate_on_limit() -> String {
    "stop".into()
}
fn default_operate_settle_ms() -> u32 {
    1800
}
fn default_operate_confirm() -> String {
    "off".into()
}

/// The operate limits/policy (defaults if none configured).
pub fn operate() -> OperateSpec {
    load().map(|c| c.operate).unwrap_or_default()
}

fn default_attach_ext() -> Vec<String> {
    ["jpg", "jpeg", "png", "gif", "webp", "pdf"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn default_bind() -> String {
    "auto".into()
}
fn default_remote_port() -> u16 {
    8787
}

/// secrets.json: a file holding only credentials, kept separate (never share)
#[derive(Debug, Deserialize, Default)]
pub struct Secrets {
    #[serde(default)]
    pub notify: std::collections::HashMap<String, crate::notify::Destination>,
    /// Auth info used by the HTTP gateway (not readable from scripts)
    #[serde(default)]
    pub tokens: std::collections::HashMap<String, String>,
    /// Description of each token (shown in the GUI list; the value itself never is)
    #[serde(default)]
    pub descriptions: std::collections::HashMap<String, String>,
    /// Remote UI token. Setting this pins the URL and avoids needing to re-pair
    #[serde(default)]
    pub remote_token: Option<String>,
}

impl Config {
    /// Merge the secrets file's contents into config.json's notify
    /// (secrets wins on name collision). secrets may be encrypted
    pub fn resolve_notify(
        &self,
        password: Option<&str>,
    ) -> (
        std::collections::HashMap<String, crate::notify::Destination>,
        Option<String>,
    ) {
        let mut map = self.notify.clone();
        let mut tokens: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut err = None;
        if let Some(path) = self.secrets_path().filter(|p| p.exists()) {
            match crate::crypto::read_maybe_encrypted(&path, password).and_then(|t| {
                serde_json::from_str::<Secrets>(&t)
                    .with_context(|| crate::i18n::t("err.config.secrets_json_invalid"))
            }) {
                Ok(s) => {
                    map.extend(s.notify);
                    tokens = s.tokens;
                }
                Err(e) => err = Some(format!("secrets: {e:#}")),
            }
        }
        // A "@name" webhook/token is expanded from the tokens store, so the
        // sensitive value stays encrypted in secrets.json rather than sitting in
        // config.json (same convention as a provider's api_key).
        let deref = |v: &str| -> String {
            match v.strip_prefix('@') {
                Some(k) => tokens.get(k).cloned().unwrap_or_default(),
                None => v.to_string(),
            }
        };
        for d in map.values_mut() {
            match d {
                crate::notify::Destination::Slack { webhook } => *webhook = deref(webhook),
                crate::notify::Destination::Telegram { token, chat_id } => {
                    *token = deref(token);
                    *chat_id = deref(chat_id);
                }
            }
        }
        (map, err)
    }

    /// Path to the secrets file. A relative path is resolved next to config.json.
    ///
    /// Without an explicit setting, this defaults to config/secrets.json. Otherwise
    /// the app would be unable to read secrets the settings GUI created in the
    /// default location (registered, but unusable). The reader treats a missing
    /// file as empty
    pub fn secrets_path(&self) -> Option<std::path::PathBuf> {
        let p = self.secrets.as_deref().unwrap_or("secrets.json");
        if std::path::Path::new(p).is_absolute() {
            return Some(std::path::PathBuf::from(p));
        }
        let mut c = config_file_path();
        c.set_file_name(p);
        Some(c)
    }

    /// Remote UI token (used if present in secrets)
    pub fn remote_token(&self, password: Option<&str>) -> Option<String> {
        let path = self.secrets_path()?;
        crate::crypto::read_maybe_encrypted(&path, password)
            .ok()
            .and_then(|t| serde_json::from_str::<Secrets>(&t).ok())
            .and_then(|s| s.remote_token)
            .filter(|t| t.len() >= 16)
    }

    /// Retrieve the auth info used by the HTTP gateway (never passed to scripts)
    pub fn resolve_tokens(
        &self,
        password: Option<&str>,
    ) -> std::collections::HashMap<String, String> {
        let Some(path) = self.secrets_path() else {
            return Default::default();
        };
        crate::crypto::read_maybe_encrypted(&path, password)
            .ok()
            .and_then(|t| serde_json::from_str::<Secrets>(&t).ok())
            .map(|s| s.tokens)
            .unwrap_or_default()
    }

    /// Resolve connection info from a provider name. Returns (base_url, outgoing headers).
    /// An "@name" inside a value is expanded from secrets.json's tokens.
    /// If headers is unset and api_key is present, builds an Authorization: Bearer header.
    /// This is passed into the model bridge child process's env (key decryption happens only here, in the parent)
    pub fn resolve_provider(
        &self,
        name: &str,
        password: Option<&str>,
    ) -> Option<ProviderConn> {
        let p = self.providers.get(name)?;
        if p.base_url.trim().is_empty() {
            return None;
        }
        let tokens = self.resolve_tokens(password);
        let deref = |v: &str| -> String {
            match v.strip_prefix('@') {
                Some(k) => tokens.get(k).cloned().unwrap_or_default(),
                None => v.to_string(),
            }
        };
        let mut headers = std::collections::HashMap::new();
        if !p.headers.is_empty() {
            for (k, v) in &p.headers {
                headers.insert(k.clone(), deref(v));
            }
        } else if let Some(key) = p.api_key.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            headers.insert("Authorization".into(), format!("Bearer {}", deref(key)));
        }
        Some(ProviderConn {
            url: p.base_url.trim().to_string(),
            headers,
            timeout: match p.timeout_sec.unwrap_or(PROVIDER_TIMEOUT_DEFAULT_SEC) {
                0 => None,
                secs => Some(std::time::Duration::from_secs(secs)),
            },
        })
    }
}

// -- Secrets store (equivalent to GitHub Secrets) ---------------------------
// Referenced by key name; the value itself is never returned. Writing encrypts
// it if a password is set. Reads/writes the whole JSON so other entries such
// as notify and remote_token aren't clobbered

/// Read the secrets file as JSON (empty if missing)
fn read_secrets_value(
    path: &std::path::Path,
    password: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let text = crate::crypto::read_maybe_encrypted(path, password)?;
    Ok(serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({})))
}

/// Write the secrets file back (encrypted if a password is set)
fn write_secrets_value(
    path: &std::path::Path,
    password: Option<&str>,
    root: &serde_json::Value,
) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(root)?;
    match password {
        Some(pw) if !pw.is_empty() => {
            let env = crate::crypto::encrypt(&json, pw)?;
            crate::crypto::write_atomic(path, &serde_json::to_string_pretty(&env)?)
        }
        _ => crate::crypto::write_atomic(path, &json),
    }
}

/// Whether this is a valid key name (alphanumeric and _ - . only). Rejects substitution tricks and odd characters
pub fn valid_secret_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// List of secrets (keys and descriptions only). **Values are never returned**
pub fn list_secrets(
    path: &std::path::Path,
    password: Option<&str>,
) -> anyhow::Result<Vec<(String, String)>> {
    let root = read_secrets_value(path, password)?;
    let descs = root.get("descriptions").and_then(|v| v.as_object());
    let mut out: Vec<(String, String)> = root
        .get("tokens")
        .and_then(|v| v.as_object())
        .map(|t| {
            t.keys()
                .map(|k| {
                    let d = descs
                        .and_then(|d| d.get(k))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    (k.clone(), d)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Add/update a secret (write-only; once saved, the value can't be read back)
pub fn upsert_secret(
    path: &std::path::Path,
    password: Option<&str>,
    key: &str,
    desc: &str,
    value: &str,
) -> anyhow::Result<()> {
    if !valid_secret_key(key) {
        anyhow::bail!(crate::i18n::t("err.config.invalid_key_chars"));
    }
    let mut root = read_secrets_value(path, password)?;
    if !root.get("tokens").map(|v| v.is_object()).unwrap_or(false) {
        root["tokens"] = serde_json::json!({});
    }
    root["tokens"][key] = serde_json::json!(value);
    if !root
        .get("descriptions")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        root["descriptions"] = serde_json::json!({});
    }
    root["descriptions"][key] = serde_json::json!(desc);
    write_secrets_value(path, password, &root)
}

/// Delete a secret
pub fn delete_secret(
    path: &std::path::Path,
    password: Option<&str>,
    key: &str,
) -> anyhow::Result<()> {
    let mut root = read_secrets_value(path, password)?;
    if let Some(t) = root.get_mut("tokens").and_then(|v| v.as_object_mut()) {
        t.remove(key);
    }
    if let Some(d) = root.get_mut("descriptions").and_then(|v| v.as_object_mut()) {
        d.remove(key);
    }
    write_secrets_value(path, password, &root)
}

/// A workspace entry inside config.json. Either inline tabs or a reference to a definition file
#[derive(Debug, Deserialize)]
pub struct WorkspaceSpec {
    pub name: String,
    /// Reference to a workspace definition file (e.g. "workspaces/projectx.json")
    #[serde(default)]
    pub file: Option<String>,
    /// Inline definition
    #[serde(default)]
    pub folders: Vec<FolderConfig>,
    /// Automation shared across this workspace (used when a tab doesn't specify its own)
    #[serde(default)]
    pub automation: Option<String>,
    /// Browsers opened alongside this workspace. Referred to by id from automation
    #[serde(default)]
    pub browsers: Vec<BrowserConfig>,
    #[serde(default)]
    pub lua: Option<String>,
    /// Secret keys this workspace's rally is allowed to use (default is empty = deny all)
    #[serde(default)]
    pub secrets_allow: Vec<String>,
    /// Allow all secrets, knowingly accepting the risk
    #[serde(default)]
    pub secrets_allow_all: bool,
    /// Stop conditions (the referee). Read by built-in controllers such as browser-operation mode
    #[serde(default)]
    pub stops: Vec<StopCond>,
    /// AI-vs-AI discussion settings (when present, the built-in discussion orchestrator is put into each AI tab)
    #[serde(default)]
    pub discuss: Option<DiscussSpec>,
}

/// Contents of a workspace definition file (workspaces/*.json)
#[derive(Debug, Deserialize)]
pub struct WorkspaceFile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub folders: Vec<FolderConfig>,
    /// Automation shared across this workspace
    #[serde(default)]
    pub automation: Option<String>,
    #[serde(default)]
    pub lua: Option<String>,
    #[serde(default)]
    pub secrets_allow: Vec<String>,
    #[serde(default)]
    pub secrets_allow_all: bool,
    #[serde(default)]
    pub stops: Vec<StopCond>,
    #[serde(default)]
    pub discuss: Option<DiscussSpec>,
}

/// AI-vs-AI (N-party) discussion settings. Per workspace. Read by the built-in discussion orchestrator.
/// Participants (agents) are listed in turn order. Cycled round-robin; once max_rounds is reached, the judge (if any) rules
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DiscussSpec {
    /// ids of the participating AI tabs (in turn order)
    #[serde(default)]
    pub agents: Vec<String>,
    /// How turns are cycled. Currently only "round-robin"
    #[serde(default = "default_order")]
    pub order: String,
    /// Max number of rounds each participant speaks (once exceeded, goes to the judge/ends)
    #[serde(default = "default_rounds")]
    pub max_rounds: u32,
    /// Tab id of the judge (referee). If omitted, hitting the round limit just folds up as "discussion ended"
    #[serde(default)]
    pub judge: Option<String>,
    /// How the judge renders its verdict: "winner" / "synthesis". Default is winner
    #[serde(default = "default_verdict")]
    pub verdict: String,
    /// Tab id of the moderator. When order="moderated", nominates the next speaker
    #[serde(default)]
    pub moderator: Option<String>,
    /// Each tab's stance/persona (tab id -> persona text).
    /// e.g. {"safety":"You are a safety-first faction...", ...}.
    /// Told to that AI at the start. Empty means a plain (neutral) AI
    #[serde(default)]
    pub personas: std::collections::HashMap<String, String>,
}

fn default_order() -> String {
    "round-robin".into()
}
fn default_rounds() -> u32 {
    6
}
fn default_verdict() -> String {
    "winner".into()
}

/// Stop conditions (the referee). Held per workspace. Evaluated top to bottom; the first match wins.
/// Defines "when this collaboration ends (success/failure)". Can span multiple participants (tabs)
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct StopCond {
    /// Kind of monitoring: screen|css|xpath|console|rounds|time|tokens
    pub when: String,
    /// Tab id being watched (for screen/css/xpath/console; defaults to the target being operated)
    #[serde(default)]
    pub tab: Option<String>,
    /// String pattern (screen = browser body text, console = tab output)
    #[serde(default)]
    pub pattern: Option<String>,
    /// Selector (for css/xpath; either "#id" or an xpath string)
    #[serde(default)]
    pub sel: Option<String>,
    /// Threshold (rounds = count, tokens = estimate)
    #[serde(default)]
    pub max: Option<i64>,
    /// Seconds (for time)
    #[serde(default)]
    pub sec: Option<i64>,
    /// Verdict: "success" | "fail"
    #[serde(default)]
    pub outcome: String,
    /// Exit code
    #[serde(default)]
    pub code: i32,
    /// Reason (human-readable, kept in the record)
    #[serde(default)]
    pub reason: Option<String>,
}

/// Turn the list of stop conditions into a Lua table literal to pass to the built-in controller.
/// Strings are quoted safely as if by %q (done on the Rust side, not via Lua's string.format)
pub fn stops_to_lua(stops: &[StopCond]) -> String {
    fn q(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }
    let mut b = String::from("{\n");
    for s in stops {
        if s.when.trim().is_empty() {
            continue;
        }
        b.push_str("  { when=");
        b.push_str(&q(&s.when));
        if let Some(t) = &s.tab {
            b.push_str(", tab=");
            b.push_str(&q(t));
        }
        if let Some(p) = &s.pattern {
            b.push_str(", pattern=");
            b.push_str(&q(p));
        }
        if let Some(sel) = &s.sel {
            b.push_str(", sel=");
            b.push_str(&q(sel));
        }
        if let Some(m) = s.max {
            b.push_str(&format!(", max={m}"));
        }
        if let Some(sec) = s.sec {
            b.push_str(&format!(", sec={sec}"));
        }
        let outcome = if s.outcome.is_empty() { "success" } else { &s.outcome };
        b.push_str(", outcome=");
        b.push_str(&q(outcome));
        b.push_str(&format!(", code={}", s.code));
        b.push_str(", reason=");
        b.push_str(&q(s.reason.as_deref().unwrap_or("")));
        b.push_str(" },\n");
    }
    b.push('}');
    b
}

/// Controls shown above the browser. Just holds which ones to show.
///
/// All false by default = nothing shown. Doesn't crowd a project's screen with
/// controls it doesn't need. The user picks each one individually
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
pub struct NavSpec {
    #[serde(default)]
    pub back: bool,
    #[serde(default)]
    pub forward: bool,
    #[serde(default)]
    pub reload: bool,
    /// The second reload: fetch it all again instead of using what is held.
    /// Its own switch, because it is its own button
    #[serde(default)]
    pub reload_hard: bool,
    /// URL bar. Lets a person navigate to any page
    #[serde(default)]
    pub url: bool,
}

impl NavSpec {
    /// If none are shown, the bar itself isn't needed
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Show all of them. Used when the spec is omitted, as in `browser_nav(id)`
    pub fn all() -> Self {
        Self { back: true, forward: true, reload: true, reload_hard: true, url: true }
    }
}

/// The banner shown below a page.
///
/// Its mere presence means "show it". Unlike the nav bar, it needs actual text,
/// so a plain show/hide boolean isn't enough
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct AskSpec {
    /// Text shown on the left of the banner
    #[serde(default)]
    pub text: String,
    /// Button label. Uses the default wording if empty
    #[serde(default)]
    pub label: String,
}

/// A single browser opened alongside the workspace
#[derive(Debug, Clone, Deserialize)]
pub struct BrowserConfig {
    /// Name referred to from automation (e.g. "br")
    pub id: String,
    /// URL opened initially. http/https only
    pub url: String,
    /// Browser profile name (defaults to "default"). Ignored if private is true
    #[serde(default)]
    pub browser_profile: Option<String>,
    /// Private (disposable) browser
    #[serde(default)]
    pub private: bool,
    /// What this page calls itself. Falls back to the app-wide setting
    #[serde(default)]
    pub user_agent: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct TabConfig {
    /// Tab name (generated from the command name if omitted)
    pub name: Option<String>,
    /// Name referred to from automation (optional). Setting this means renaming the
    /// tab won't break scripts. If omitted, the tab can be referred to by its name
    #[serde(default)]
    pub id: Option<String>,
    /// Launch command: "ssh user@host" or ["ssh", "user@host"]
    pub command: CommandSpec,
    /// Explicit detection profile (auto-selected from the command name if omitted)
    pub profile: Option<String>,
    /// Input lock (soft lock). Prevents accidental input into a mid-pipeline tab.
    /// Can be released at runtime with Ctrl+B l or by clicking the lock icon
    #[serde(default)]
    pub locked: bool,
    /// Automatically restart when the child process exits.
    /// Used to recover from an SSH disconnect or after a CLI tool self-updates
    #[serde(default)]
    pub auto_restart: bool,
    /// What this tab is aimed at (🎯): the id of the tab it drives, or absent.
    ///
    /// Written by the picker on screen -- picking IS the setting, so there is no
    /// separate "default target" to keep in step with this. It is the aim only:
    /// the operator is briefed when a goal is given, not at launch, and the
    /// tab's own `automation` keeps running either way (the aim borrows the
    /// pane's script while it is attached, and gives it back).
    #[serde(default)]
    pub drives: Option<String>,
    /// A conversation id to resume at launch, instead of starting a new one.
    /// Written by the Vault when a past conversation is reopened; the CLI is
    /// asked to resume it through the same resume flags a restart would use
    #[serde(default)]
    pub resume: Option<String>,
    /// Whether this tab comes back to the conversation it was having when the
    /// app last closed (default: yes).
    ///
    /// The shape of the screen is put back either way -- panes are furniture.
    /// This is about the contents, which is a different promise: the CLI is
    /// handed the id it was running and asked to resume it, so what is on
    /// screen after reopening the app is the conversation itself, not an empty
    /// prompt. Per tab, because the answer differs per tab: the one you live in
    /// all day should come back, and a scratch tab you keep for one-off
    /// questions is better off clean
    #[serde(default)]
    pub restore_conversation: Option<bool>,
    /// Scrollback line count (defaults to 5000)
    #[serde(default)]
    pub scrollback: Option<usize>,
    /// Character encoding ("shift_jis" etc). Defaults to UTF-8
    #[serde(default)]
    pub encoding: Option<String>,
    /// Save the session log under logs/
    #[serde(default)]
    pub log: bool,
    /// Notification destination to ping each time this tab's AI finishes a
    /// response (a beginner-friendly shortcut for an on_done that calls notify).
    #[serde(default)]
    pub notify_on_done: Option<String>,
    /// Automation dedicated to this tab (matched with the highest priority).
    /// A directory means per-event files; a .lua file means function definitions
    #[serde(default)]
    pub automation: Option<String>,
    /// Old name. Used when automation is not set
    #[serde(default)]
    pub lua: Option<String>,
    /// Controls shown on a browser tab (back/forward/reload/URL bar).
    /// Meaningless for a terminal tab, so it's not read there
    #[serde(default)]
    pub nav: Option<NavSpec>,
    /// What this browser tab calls itself. Falls back to the app-wide setting.
    /// Per tab, because one page can need a name another must not have: a site
    /// that will not sign you in unless you are Chrome, beside a site that
    /// serves something different to anything calling itself Chrome
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Banner shown below a browser tab (text and button label)
    #[serde(default)]
    pub ask: Option<AskSpec>,
    /// Browser profile name (the box holding cookies/login). Defaults to "default".
    /// Tabs sharing a profile name share login state, the same idea as Chrome's "person".
    /// Ignored when private is true
    #[serde(default)]
    pub browser_profile: Option<String>,
    /// Private (disposable) browser. When true, opens in a temporary area that
    /// keeps no cookies/history and is wiped on close. browser_profile is unused in that case
    #[serde(default)]
    pub private: bool,
    /// Child tabs for display purposes (forwarding relationships are decided by Lua; this is display hierarchy only)
    #[serde(default)]
    pub children: Vec<TabConfig>,
}

/// A folder, and the tabs that work in it.
///
/// The folder is written here and nowhere else. A tab used to carry its own,
/// which meant a reviewer could be pointed at a different folder from the tab
/// it was reviewing -- two AIs in one workspace looking at different files,
/// with nothing on screen to say so. There is now one folder per group and no
/// field on a tab to disagree with it.
///
/// A group is not something anyone has to make. Tabs written without one land
/// in a single group with no name, which is drawn as no heading at all, so a
/// person who has never heard the word still has one.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct FolderConfig {
    /// Shown as the heading, when there is more than one group. Absent means
    /// the folder speaks for itself (its branch, or its last path component)
    #[serde(default)]
    pub name: Option<String>,
    /// Name referred to from automation. As with a tab, setting it means
    /// renaming the group won't break scripts
    #[serde(default)]
    pub id: Option<String>,
    /// The folder every tab in here starts in. A relative path is resolved
    /// against the config file's location. Absent means beside the app.
    /// A folder inside Docker/WSL cannot be named this way (use the command's
    /// own -w / --cd)
    #[serde(default)]
    pub cwd: Option<String>,
    /// Where this folder came from, so a machine that does not have it can make
    /// it. Written when it is made; asked of a person only when it is absent
    #[serde(default)]
    pub source: Option<SourceSpec>,
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
}

/// Where a working folder came from.
///
/// Settings are shared between machines, and a folder named only by its path is
/// a folder the second machine cannot make. What it takes to make one again is
/// three facts, so they are written down at the moment it is made rather than
/// asked for later — nobody should have to remember which branch a folder held
/// six weeks ago, and nothing else in the settings can be asked.
///
/// The repository is named by its **remote URL**, never by the folder it was
/// cut from: a path is the thing that differs between machines, which is the
/// problem this exists to solve. The URL is the same everywhere.
///
/// Every field is optional so that a half-written one — this file gets edited
/// by hand — costs its own folder rather than the whole settings file.
#[derive(Debug, Deserialize, serde::Serialize, Clone, Default, PartialEq, Eq)]
pub struct SourceSpec {
    /// The repository's remote, without credentials. Never a path
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The branch this folder holds, spelled exactly as git spells it —
    /// slashes and all. The folder's `name` is a label and flattens
    /// `work/2` to `work-2`, which cannot be turned back into a branch
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// What the branch grew from, for the case where it no longer exists
    /// anywhere and has to be started again
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// `"folder"` for a working folder that is deliberately not a repository.
    /// Recorded so that the one question is asked once and never again
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// The same, read: what is actually known about where a folder came from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Source {
    /// A branch of a repository, with everything needed to expand it anywhere
    Worktree { origin: String, branch: String, base: String },
    /// An ordinary folder, said so on purpose
    Plain,
    /// Nothing was written down. The one case that has to ask a person —
    /// settings written by hand, or a folder made before any of this existed
    #[default]
    Unknown,
}

impl SourceSpec {
    /// What this amounts to, with the half-written cases folded into
    /// "nobody said".
    pub fn read(&self) -> Source {
        if self.kind.as_deref() == Some("folder") {
            return Source::Plain;
        }
        let some = |v: &Option<String>| {
            v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
        };
        match (some(&self.origin), some(&self.branch)) {
            (Some(origin), Some(branch)) => Source::Worktree {
                origin,
                branch,
                base: some(&self.base).unwrap_or_default(),
            },
            _ => Source::Unknown,
        }
    }

    /// An ordinary folder, recorded as one.
    pub fn plain() -> Self {
        Self { kind: Some("folder".into()), ..Default::default() }
    }

    /// A branch of a repository, recorded as one.
    pub fn worktree(origin: &str, branch: &str, base: &str) -> Self {
        Self {
            origin: Some(origin.to_string()),
            branch: Some(branch.to_string()),
            base: (!base.trim().is_empty()).then(|| base.to_string()),
            kind: None,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum CommandSpec {
    Line(String),
    Argv(Vec<String>),
}

impl Default for CommandSpec {
    /// The nothing-written state. argv ends up empty
    fn default() -> Self {
        CommandSpec::Argv(Vec::new())
    }
}

impl CommandSpec {
    /// Normalize a whitespace-separated string or an array into argv.
    /// Use the array form if a path contains whitespace
    pub fn argv(&self) -> Vec<String> {
        match self {
            CommandSpec::Line(s) => s.split_whitespace().map(str::to_string).collect(),
            CommandSpec::Argv(v) => v.clone(),
        }
    }
}

/// A working folder resolved at launch time: its path is a real one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folder {
    pub name: Option<String>,
    pub id: Option<String>,
    /// Where its tabs start. Absent means wherever the app itself is
    pub cwd: Option<std::path::PathBuf>,
    /// What it would take to make this folder on a machine that does not have
    /// it. [`Source::Unknown`] is the case that has to ask
    pub source: Source,
}

/// A workspace resolved at launch time (tabs are flattened; depth preserves the hierarchy)
pub struct Workspace {
    pub name: String,
    /// The folders this workspace works in. Always at least one, so that
    /// nothing downstream has to answer "what if a tab is in none"
    pub folders: Vec<Folder>,
    pub tabs: Vec<FlatTab>,
    /// Automation at the workspace level
    pub automation: Option<String>,
    /// Browsers opened alongside it
    pub browsers: Vec<BrowserConfig>,
    /// Secret keys this workspace's rally is allowed to use (default is empty = deny all)
    pub secrets_allow: Vec<String>,
    /// Allow all secrets, knowingly accepting the risk
    pub secrets_allow_all: bool,
    /// Stop conditions (the referee)
    pub stops: Vec<StopCond>,
    /// AI-vs-AI discussion settings
    pub discuss: Option<DiscussSpec>,
}

impl Workspace {
    /// The working folder a tab belongs to. Everything about where it runs
    /// lives there, because a tab has nothing of its own to disagree with
    pub fn folder_of(&self, t: &FlatTab) -> Option<&Folder> {
        self.folders.get(t.folder)
    }

    /// Where a tab starts: its folder's path, because a tab has none of its own.
    pub fn cwd_of(&self, t: &FlatTab) -> Option<std::path::PathBuf> {
        self.folder_of(t).and_then(|f| f.cwd.clone())
    }
}

/// Whether this tab is a browser. Returns the URL if so.
///
/// Told apart the same way as ssh/docker/wsl: by the head of the command string.
/// The settings screen's "type" field follows this same rule
pub fn browser_url_of(argv: &[String]) -> Option<String> {
    let (head, rest) = argv.split_first()?;
    if !head.eq_ignore_ascii_case("browser") && !head.eq_ignore_ascii_case("web") {
        return None;
    }
    let url = rest.first()?.trim().to_string();
    (!url.is_empty()).then_some(url)
}

impl TabConfig {
    /// Prefer automation, falling back to the old name lua
    pub fn automation_path(&self) -> Option<String> {
        self.automation.clone().or_else(|| self.lua.clone())
    }
}

impl Config {
    pub fn automation_path(&self) -> Option<String> {
        self.automation.clone().or_else(|| self.lua.clone())
    }
}

pub struct FlatTab {
    pub cfg: TabConfig,
    /// Display indent depth (0 = parent)
    pub depth: u16,
    /// Which of the workspace's working folders this tab belongs to, and
    /// therefore where it starts
    pub folder: usize,
}

/// Verify each tab can be addressed uniquely from automation.
/// If the same name is used more than once the destination is ambiguous, so this warns at startup
pub fn duplicate_keys(ws: &Workspace) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = Default::default();
    for t in &ws.tabs {
        let key = t
            .cfg
            .id
            .clone()
            .or_else(|| t.cfg.name.clone())
            .unwrap_or_default();
        if key.is_empty() {
            continue;
        }
        *seen.entry(key).or_insert(0) += 1;
    }
    let mut dups: Vec<String> = seen
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(k, _)| k)
        .collect();
    dups.sort();
    dups
}

/// Flatten children depth-first (keeps display order matching tab numbers)
fn flatten(tabs: &[TabConfig], depth: u16, folder: usize, out: &mut Vec<FlatTab>) {
    for t in tabs {
        out.push(FlatTab {
            cfg: t.clone(),
            depth,
            folder,
        });
        flatten(&t.children, depth + 1, folder, out);
    }
}

/// The folders a definition asks for, and at least one of them.
///
/// A workspace with nothing written in it still has the folder everything
/// lands in, so no caller has to answer "and if it has none".
fn foldered(folders: &[FolderConfig]) -> Vec<FolderConfig> {
    let mut out = folders.to_vec();
    if out.is_empty() {
        out.push(FolderConfig::default());
    }
    out
}

/// Turns written groups into ones with a real folder, and lays their tabs out
/// in one list in the order they are shown.
fn resolve_folders(defs: &[FolderConfig]) -> (Vec<Folder>, Vec<FlatTab>) {
    let mut folders = Vec::with_capacity(defs.len());
    let mut tabs = Vec::new();
    for (at, def) in defs.iter().enumerate() {
        folders.push(Folder {
            name: def.name.clone().filter(|n| !n.trim().is_empty()),
            id: def.id.clone().filter(|i| !i.trim().is_empty()),
            // Relative stays relative to the settings, so that a whole folder
            // of them can be carried to another machine
            cwd: def.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(|c| {
                let p = std::path::PathBuf::from(c);
                match p.is_absolute() {
                    true => p,
                    false => root_dir().join(p),
                }
            }),
            source: def.source.as_ref().map(SourceSpec::read).unwrap_or_default(),
        });
        flatten(&def.tabs, 0, at, &mut tabs);
    }
    (folders, tabs)
}

/// A byte-order mark is not JSON.
///
/// Windows puts one there without being asked — Notepad's "UTF-8" and
/// PowerShell's `Set-Content -Encoding utf8` both write it — and the parser
/// then refuses the whole file at "line 1 column 1". For `config.json` that is
/// worse than an error: loading moves on to the next candidate, so the app
/// comes up with settings from somewhere else entirely and the edit the person
/// just made appears to have done nothing.
fn without_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Which key does what.
///
/// Named by the action rather than by the key, because that is the question a
/// person actually has: not "what does Ctrl+B % do" but "what do I press to
/// split the screen". The list of names lives in `keys.rs` with the actions
/// themselves; nothing about them is repeated here.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct KeyBinds {
    /// The key pressed before the others. `ctrl+b` unless said otherwise
    #[serde(default)]
    pub prefix: Option<String>,
    /// Action name to key. A bare character means "after the prefix"; anything
    /// with a modifier stands on its own; `off` gives the key back
    #[serde(default, flatten)]
    pub binds: std::collections::HashMap<String, String>,
}

/// The look of the terminal: what it is written in, how big, and in what
/// colours.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Appearance {
    /// The font stack, as CSS writes it. Empty = the built-in one, chosen for
    /// drawing box characters and Japanese in one cell each
    #[serde(default)]
    pub font: Option<String>,
    /// Point size. Changed at any time with Ctrl+wheel, and kept here
    #[serde(default)]
    pub font_size: Option<u8>,
    /// The colour scheme. Either the name of one -- from the schemes this
    /// machine already has, or the ones built in -- or a scheme written out
    /// here in the shape they are published in
    #[serde(default)]
    pub theme: Option<serde_json::Value>,
}

impl Appearance {
    /// The font stack for the page, already quoted as CSS wants it
    pub fn font_css(&self) -> String {
        match self.font.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
            // Written by a person, so it may be one name or a whole stack.
            // A bare name is quoted; a stack is passed through as written
            Some(f) if f.contains(',') || f.contains('"') => f.to_string(),
            Some(f) => format!("\"{f}\", monospace"),
            None => {
                // Fonts that draw box-drawing characters and symbols in one
                // cell. Japanese falls back to the monospaced MS Gothic
                // (Meiryo is not monospaced)
                "\"Cascadia Mono\",\"Consolas\",\"MS Gothic\",\"MS ゴシック\",monospace".into()
            }
        }
    }

    pub fn size_px(&self) -> u8 {
        self.font_size.unwrap_or(14).clamp(8, 32)
    }

    /// The colours to draw with.
    pub fn scheme(&self) -> crate::theme::Scheme {
        crate::theme::resolve(self.theme.as_ref())
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| {
        crate::i18n::tp(
            "err.config.read_failed",
            &[("path", &path.display().to_string())],
        )
    })?;
    serde_json::from_str(without_bom(&text)).with_context(|| {
        crate::i18n::tp(
            "err.config.json_invalid",
            &[("path", &path.display().to_string())],
        )
    })
}

/// Remembers the colour chosen for a project.
///
/// Against the folder git shares between the branches of one repository, so
/// every branch changes together -- the colour says "these are one project",
/// and a branch with its own would be saying the opposite. An empty colour
/// forgets the choice and hands the project back to the one worked out from
/// its name.
pub fn set_folder_color(family: &Path, color: &str) -> Result<()> {
    let path = config_file_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let mut root: serde_json::Value =
        serde_json::from_str(without_bom(&text)).unwrap_or_else(|_| serde_json::json!({}));
    if !root.get("folder_colors").map(|c| c.is_object()).unwrap_or(false) {
        root["folder_colors"] = serde_json::json!({});
    }
    let key = family.display().to_string();
    let map = root["folder_colors"].as_object_mut().expect("作ったばかり");
    match color.trim() {
        "" => {
            map.remove(&key);
        }
        c => {
            map.insert(key, serde_json::json!(c));
        }
    }
    crate::crypto::write_atomic(&path, &serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

/// Adds a folder to a workspace's settings, with the same tabs as another.
///
/// This is what "work on another branch too" writes down. It edits the file
/// the person owns rather than keeping a second list of its own, so what
/// happened is visible in the settings screen afterwards and survives a
/// restart without anything else having to remember it.
///
/// Everything already in the file is left exactly as it was -- it is read as
/// values, not as our own types, so a key this version has never heard of
/// still comes out the other side.
pub fn append_folder(ws_name: &str, like: Option<&Path>, cwd: &Path, name: Option<&str>) -> Result<()> {
    append_folder_at(&config_file_path(), ws_name, like, cwd, name)
}

/// The same, told which settings file to edit. Split out so it can be checked
/// against a file of its own rather than against whatever this machine has
pub fn append_folder_at(
    path: &Path,
    ws_name: &str,
    like: Option<&Path>,
    cwd: &Path,
    name: Option<&str>,
) -> Result<()> {
    with_folders(path, ws_name, |folders| {
        // The tabs to bring along: whoever is already working in the folder
        // this was asked for from. Same faces, new branch
        let tabs = like
            .and_then(|want| {
                folders.iter().find(|g| {
                    g.get("cwd")
                        .and_then(|c| c.as_str())
                        .map(resolve_folder_cwd)
                        .is_some_and(|c| c == want)
                })
            })
            .and_then(|g| g.get("tabs").cloned())
            .unwrap_or_else(|| serde_json::json!([]));
        // What marks the copies apart. The branch when there is one, since two
        // branches can end in the same word (`feature/login`, `fix/login`) and
        // their folders would then hand out the same name twice
        let mark = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| n.replace('/', "-"))
            .or_else(|| cwd.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        let mut folder =
            serde_json::json!({ "cwd": cwd.display().to_string(), "tabs": retag(tabs, &mark) });
        if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
            folder["name"] = serde_json::json!(n);
        }
        // Beside the folders it belongs with. A branch of one project written
        // after an unrelated one reads as unrelated: the list is drawn in the
        // order this is written in, and a family that is not next to itself is
        // a family nobody can see
        let family = crate::repo::family_of(cwd);
        let last_of_family = family.as_ref().and_then(|f| {
            folders.iter().rposition(|g| {
                g.get("cwd")
                    .and_then(|c| c.as_str())
                    .map(resolve_folder_cwd)
                    .and_then(|c| crate::repo::family_of(&c))
                    .as_ref()
                    == Some(f)
            })
        });
        match last_of_family {
            Some(at) => folders.insert(at + 1, folder),
            None => folders.push(folder),
        }
        Ok(())
    })
}

/// Renames a folder in the list. An empty name hands it back to what the
/// folder itself says -- its branch, or its own last part
pub fn rename_folder(ws_name: &str, cwd: &Path, name: &str) -> Result<()> {
    with_folders(&config_file_path(), ws_name, |folders| {
        let Some(g) = find_folder(folders, cwd) else {
            return Ok(());
        };
        match name.trim() {
            "" => {
                if let Some(o) = g.as_object_mut() {
                    o.remove("name");
                }
            }
            n => g["name"] = serde_json::json!(n),
        }
        Ok(())
    })
}

/// Takes a folder out of the list, and its tabs with it.
///
/// The folder on disk is not touched. Closing a thing on screen and deleting
/// somebody's work are different acts, and only one of them can be undone by
/// opening it again.
pub fn remove_folder(ws_name: &str, cwd: &Path) -> Result<()> {
    with_folders(&config_file_path(), ws_name, |folders| {
        if folders.len() <= 1 {
            anyhow::bail!(crate::i18n::t("err.worktree.last_folder"));
        }
        let at = folders.iter().position(|g| {
            g.get("cwd")
                .and_then(|c| c.as_str())
                .map(resolve_folder_cwd)
                .is_some_and(|c| c == cwd)
        });
        if let Some(i) = at {
            folders.remove(i);
        }
        Ok(())
    })
}

/// The group working in this folder, if it is in the list.
fn find_folder<'a>(
    groups: &'a mut [serde_json::Value],
    cwd: &Path,
) -> Option<&'a mut serde_json::Value> {
    groups.iter_mut().find(|g| {
        g.get("cwd")
            .and_then(|c| c.as_str())
            .map(resolve_folder_cwd)
            .is_some_and(|c| c == cwd)
    })
}

/// Opens a workspace's groups, hands them over to be changed, and writes the
/// result back where it came from.
///
/// One way in, because there are three things that change a group and each of
/// them would otherwise carry its own copy of "find the workspace, follow it
/// to the file it lives in, fold the loose tabs, write it out atomically".
fn with_folders(
    path: &Path,
    ws_name: &str,
    edit: impl FnOnce(&mut Vec<serde_json::Value>) -> Result<()>,
) -> Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into());
    let mut root: serde_json::Value =
        serde_json::from_str(without_bom(&text)).unwrap_or_else(|_| serde_json::json!({}));

    // A workspace kept in a file of its own is edited there; the entry in the
    // settings only names it
    let mut file_at: Option<std::path::PathBuf> = None;
    {
        let list = root
            .get("workspaces")
            .and_then(|w| w.as_array())
            .map(|a| a.to_vec())
            .unwrap_or_default();
        for w in list {
            if w.get("name").and_then(|n| n.as_str()) == Some(ws_name) {
                if let Some(f) = w.get("file").and_then(|f| f.as_str()) {
                    file_at = Some(resolve_data_path(f));
                }
                break;
            }
        }
    }
    let mut side = match &file_at {
        Some(p) => Some(serde_json::from_str::<serde_json::Value>(without_bom(
            &std::fs::read_to_string(p)?,
        ))?),
        None => None,
    };
    #[allow(clippy::let_and_return)]

    // The object that holds the groups: the workspace's own, the file it names,
    // or the settings themselves when no workspace was ever made
    let holder: &mut serde_json::Value = match (&mut side, ws_name.is_empty()) {
        (Some(v), _) => v,
        (None, true) => &mut root,
        (None, false) => root
            .get_mut("workspaces")
            .and_then(|w| w.as_array_mut())
            .and_then(|a| {
                a.iter_mut()
                    .find(|w| w.get("name").and_then(|n| n.as_str()) == Some(ws_name))
            })
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.worktree.no_workspace")))?,
    };
    ensure_folders(holder);

    let folders = holder
        .get_mut("folders")
        .and_then(|g| g.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.worktree.no_workspace")))?;
    edit(folders)?;

    let out = |v: &serde_json::Value| serde_json::to_string_pretty(v).unwrap_or_default();
    match (&side, &file_at) {
        (Some(v), Some(p)) => crate::crypto::write_atomic(p, &out(v))?,
        _ => crate::crypto::write_atomic(path, &out(&root))?,
    }
    Ok(())
}

/// Makes sure there is a list of folders to put something in.
fn ensure_folders(holder: &mut serde_json::Value) {
    if !holder.get("folders").map(|f| f.is_array()).unwrap_or(false) {
        holder["folders"] = serde_json::json!([]);
    }
}

/// The tabs of the working folder at this path, making one if there is none.
/// One answer to "where does a tab go", used by everything that adds one
fn folder_tabs_at<'a>(holder: &'a mut serde_json::Value, cwd: Option<&Path>) -> &'a mut Vec<serde_json::Value> {
    ensure_folders(holder);
    let folders = holder["folders"].as_array_mut().expect("作ったばかり");
    let at = folders.iter().position(|g| {
        let here = g.get("cwd").and_then(|c| c.as_str()).map(resolve_folder_cwd);
        here.as_deref() == cwd
    });
    let at = match at {
        Some(i) => i,
        None => {
            let mut g = serde_json::json!({ "tabs": [] });
            if let Some(c) = cwd {
                g["cwd"] = serde_json::json!(c.display().to_string());
            }
            folders.push(g);
            folders.len() - 1
        }
    };
    folders[at]["tabs"].as_array_mut().expect("配列")
}

/// Copies of tabs need names automation can still tell apart. The one it uses
/// is the id, so that is the one that takes the folder's mark; the name on
/// screen is left alone, because the heading above it already says which
/// branch this is
fn retag(tabs: serde_json::Value, mark: &str) -> serde_json::Value {
    let mut out = tabs;
    fn walk(v: &mut serde_json::Value, mark: &str) {
        let Some(list) = v.as_array_mut() else { return };
        for t in list {
            if let Some(id) = t.get("id").and_then(|i| i.as_str()).map(str::to_string) {
                t["id"] = serde_json::json!(format!("{id}@{mark}"));
            }
            if let Some(kids) = t.get_mut("children") {
                walk(kids, mark);
            }
        }
    }
    walk(&mut out, mark);
    out
}

/// A group's folder as an absolute path, the same way launching resolves it.
fn resolve_folder_cwd(c: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(c.trim());
    match p.is_absolute() {
        true => p,
        false => root_dir().join(p),
    }
}

/// The names one written path may be stored under, best first.
///
/// The folder was called `projects` before it was called `workspaces`, and
/// settings written under either name are still out there, so a path naming
/// one is also looked for under the other. Kept apart from the looking so it
/// can be checked without a folder to look in -- the check used to change the
/// process's own working folder to make one, which every other test running
/// beside it then saw
fn data_path_candidates(p: &str) -> Vec<String> {
    let mut out = vec![p.to_string()];
    if let Some(rest) = p.strip_prefix("projects/") {
        out.push(format!("workspaces/{rest}"));
    } else if let Some(rest) = p.strip_prefix("workspaces/") {
        out.push(format!("projects/{rest}"));
    }
    out
}

/// Resolve a data file path, preferring beside the exe (portable layout).
/// Configs pointing at the old projects/ name also fall back to workspaces/ (compat)
pub fn resolve_data_path(p: &str) -> std::path::PathBuf {
    let candidates = data_path_candidates(p);
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf));
    for cand in &candidates {
        if let Some(dir) = &exe_dir {
            let full = dir.join(cand);
            if full.exists() {
                return full;
            }
        }
        let local = std::path::PathBuf::from(cand);
        if local.exists() {
            return local;
        }
    }
    std::path::PathBuf::from(p)
}

impl Config {
    /// Resolve the workspace definitions.
    /// If workspaces isn't defined, inline tabs are treated as a single unnamed workspace
    pub fn resolve_workspaces(&self) -> (Vec<Workspace>, Vec<String>) {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        if self.workspaces.is_empty() {
            if !self.folders.is_empty() {
                let (folders, tabs) = resolve_folders(&foldered(&self.folders));
                out.push(Workspace {
                    name: "DEFAULT".into(),
                    folders,
                    tabs,
                    automation: None,
                    browsers: Vec::new(),
                    secrets_allow: Vec::new(),
                    secrets_allow_all: false,
                    stops: Vec::new(),
                    discuss: None,
                });
            }
            return (out, errors);
        }
        for ws in &self.workspaces {
            #[allow(clippy::type_complexity)]
            #[allow(clippy::type_complexity)]
            let (folder_defs, file_name, file_lua, file_secrets, file_stops, file_discuss): (
                Vec<FolderConfig>,
                Option<String>,
                Option<String>,
                (Vec<String>, bool),
                Vec<StopCond>,
                Option<DiscussSpec>,
            ) = match &ws.file {
                Some(f) => match read_json::<WorkspaceFile>(&resolve_data_path(f)) {
                    Ok(p) => (
                        foldered(&p.folders),
                        p.name,
                        p.automation.or(p.lua),
                        (p.secrets_allow, p.secrets_allow_all),
                        p.stops,
                        p.discuss,
                    ),
                    Err(e) => {
                        errors.push(format!("{}: {e:#}", ws.name));
                        continue;
                    }
                },
                None => (
                    foldered(&ws.folders),
                    None,
                    None,
                    (Vec::new(), false),
                    Vec::new(),
                    None,
                ),
            };
            let (folders, tabs) = resolve_folders(&folder_defs);
            out.push(Workspace {
                // Prefer the display name from config; fall back to the definition file's name if empty
                name: if ws.name.is_empty() {
                    file_name.unwrap_or_else(|| "UNNAMED".into())
                } else {
                    ws.name.clone()
                },
                folders,
                tabs,
                // Prefer config's setting; fall back to the definition file's if absent
                automation: ws.automation.clone().or_else(|| ws.lua.clone()).or(file_lua),
                browsers: ws.browsers.clone(),
                // Prefer config's setting; fall back to the definition file's if absent
                secrets_allow: if ws.secrets_allow.is_empty() {
                    file_secrets.0
                } else {
                    ws.secrets_allow.clone()
                },
                secrets_allow_all: ws.secrets_allow_all || file_secrets.1,
                // Prefer config's setting; fall back to the definition file's if absent
                stops: if ws.stops.is_empty() { file_stops } else { ws.stops.clone() },
                discuss: ws.discuss.clone().or(file_discuss),
            });
        }
        (out, errors)
    }
}

/// Where the program itself was installed. What ships with it and is only ever
/// read -- lang, profiles, the automation manual -- sits here, in both layouts.
pub fn exe_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Whether this process is running from an installed package (the Store).
///
/// `GetCurrentPackageFullName` answers `APPMODEL_ERROR_NO_PACKAGE` when the
/// process has none. Asking only for the length -- with nowhere to put the name
/// -- is the cheapest way to put the question, and the "buffer too small" answer
/// that comes back is itself a yes.
fn packaged() -> bool {
    const APPMODEL_ERROR_NO_PACKAGE: u32 = 15700;
    let mut len: u32 = 0;
    let rc = unsafe {
        windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName(
            &mut len,
            std::ptr::null_mut(),
        )
    };
    rc != APPMODEL_ERROR_NO_PACKAGE
}

/// Root of the layout that holds what belongs to the person using it: config,
/// data, logs, workspaces.
///
/// Portable by default -- beside the exe. That is the promise the download
/// makes: unzip it anywhere, copy the folder to another machine whole, delete
/// the folder and nothing of it is left behind.
///
/// A copy installed from the Store cannot keep that promise. It runs from
/// `Program Files\WindowsApps`, which is read-only to the very program stored
/// there, so the first attempt to save a setting would fail -- on a fresh
/// install, with nowhere to write the log that would say why. So an installed
/// copy keeps those things under LOCALAPPDATA instead. Nothing changes for the
/// download: unpackaged, this is the exe's own folder exactly as before.
pub fn root_dir() -> std::path::PathBuf {
    use std::sync::OnceLock;
    static ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        if packaged()
            && let Some(local) = std::env::var_os("LOCALAPPDATA")
        {
            return std::path::PathBuf::from(local).join("SHIKISHA-TERM");
        }
        exe_dir()
    })
    .clone()
}

/// Search order for the config file. Prefers the new layout (config folder),
/// but also reads the old layout (config.json directly under root). Current-dir-relative paths come last
fn config_candidates() -> Vec<std::path::PathBuf> {
    let root = root_dir();
    vec![
        root.join("config").join("config.json"), // new: the config folder beside the exe
        root.join("config.json"),                // old: directly beside the exe (migration target)
        std::path::PathBuf::from("config/config.json"), // new: relative to current dir
        std::path::PathBuf::from("config.json"),        // old: relative to current dir
    ]
}

/// Move the old layout (config.json / secrets.json directly under root) into the new config folder.
/// Done only once. Not fatal if it fails, since loading still falls back to the old layout
pub fn migrate_legacy_config() {
    let root = root_dir();
    let new_cfg = root.join("config").join("config.json");
    let old_cfg = root.join("config.json");
    if new_cfg.exists() || !old_cfg.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(new_cfg.parent().unwrap());
    // rename if on the same volume; otherwise copy then delete
    if std::fs::rename(&old_cfg, &new_cfg).is_err()
        && std::fs::copy(&old_cfg, &new_cfg).is_ok()
    {
        let _ = std::fs::remove_file(&old_cfg);
    }
    // Move secrets.json alongside it too, if present
    let (old_s, new_s) = (root.join("secrets.json"), root.join("config").join("secrets.json"));
    if old_s.exists() && !new_s.exists() {
        if std::fs::rename(&old_s, &new_s).is_err() && std::fs::copy(&old_s, &new_s).is_ok() {
            let _ = std::fs::remove_file(&old_s);
        }
    }
}

/// Path to the config file the web GUI edits.
/// Returns the existing file's path if present, otherwise the path where a new one would be created beside the exe.
/// Home for state files (ones a human doesn't edit), gathered under the root's data folder.
/// The exe is the only file directly at the root (folders are config / data / logs / lang / workspaces / scripts)
pub fn state_path(name: &str) -> std::path::PathBuf {
    let p = root_dir().join("data");
    let _ = std::fs::create_dir_all(&p);
    p.join(name)
}

/// Home for logs. Pinned to the root's logs folder rather than the current directory
/// (if the log destination changed depending on how the app was launched, crash records would get lost)
pub fn logs_dir() -> std::path::PathBuf {
    let p = root_dir().join("logs");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// Home for the name of the last-open workspace.
///
/// Not written back into config.json -- that would interrupt a user mid-edit,
/// and the change-watcher would react to its own write and trigger a reload
fn last_workspace_path() -> std::path::PathBuf {
    state_path("last-workspace")
}

/// Name of the last-open workspace
pub fn load_last_workspace() -> Option<String> {
    let s = std::fs::read_to_string(last_workspace_path()).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Remember the name of the currently open workspace. Fails silently if it can't
/// (being unable to remember it is no reason for things to stop working)
pub fn save_last_workspace(name: &str) {
    let _ = crate::crypto::write_atomic(&last_workspace_path(), name);
}

/// Write one appearance value back into the settings file, leaving the rest of
/// it exactly as the person wrote it.
///
/// Read-modify-write on the parsed JSON rather than serialising our own idea of
/// the config: a settings file is a person's own document, with their key order
/// and anything we do not know about still in it. Rewriting it wholesale to
/// record a font size would be a poor trade.
/// Add a tab to one workspace and write the settings back.
///
/// The reopen the Vault performs: a resumed conversation becomes a real tab in
/// the workspace, the way dragging a past session into a workspace makes it a
/// member of it. Read-modify-write on the parsed JSON, like every other change
/// here, so the person's own file keeps its shape and its order -- the new tab
/// simply lands at the end of the workspace it was reopened into.
///
/// Returns whether it was written. A workspace that has vanished since the
/// page listed it is a false rather than a new tab in the wrong place.
pub fn append_tab(workspace: &str, tab: serde_json::Value, cwd: Option<&Path>) -> bool {
    let path = config_file_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        crate::append_hook_log("could not reopen into a tab: settings are not readable");
        return false;
    };
    let Some(list) = doc.get_mut("workspaces").and_then(|w| w.as_array_mut()) else {
        return false;
    };
    let Some(ws) = list
        .iter_mut()
        .find(|w| w.get("name").and_then(|n| n.as_str()) == Some(workspace))
    else {
        return false;
    };
    folder_tabs_at(ws, cwd).push(tab);
    match serde_json::to_string_pretty(&doc) {
        Ok(out) => crate::crypto::write_atomic(&path, &out).is_ok(),
        Err(_) => false,
    }
}

/// Record which tab an operator is aimed at (🎯), or clear it.
///
/// The aim is chosen on screen and belongs in the settings file for the same
/// reason the tab bar's width does: it must survive the next start, and one
/// answer must not live in two places. There is no separate "default target"
/// setting — the thing you pick IS the setting, and this is where it lands.
///
/// Read-modify-write on the parsed JSON, like every other change here, so the
/// person's own file keeps its shape and its order. The tab is found by the
/// name it is written under, in the flat `tabs` list or inside any workspace.
/// Returns whether it was written.
pub fn save_tab_aim(tab_name: &str, target: Option<&str>) -> bool {
    let path = config_file_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        crate::append_hook_log("could not record the aim: settings are not readable");
        return false;
    };
    // Every list of tabs the file can hold: the flat one and each workspace's
    let mut lists: Vec<&mut serde_json::Value> = Vec::new();
    let (flat, spaces) = {
        let obj = doc.as_object_mut();
        match obj {
            Some(o) => {
                let (mut f, mut w) = (None, None);
                for (k, v) in o.iter_mut() {
                    match k.as_str() {
                        "tabs" => f = Some(v),
                        "workspaces" => w = Some(v),
                        _ => {}
                    }
                }
                (f, w)
            }
            None => (None, None),
        }
    };
    if let Some(f) = flat {
        lists.push(f);
    }
    if let Some(list) = spaces.and_then(|w| w.as_array_mut()) {
        for ws in list {
            if let Some(t) = ws.get_mut("tabs") {
                lists.push(t);
            }
        }
    }
    let mut written = false;
    for tabs in lists {
        let Some(tabs) = tabs.as_array_mut() else { continue };
        for tab in tabs.iter_mut() {
            let named = ["id", "name"].iter().any(|k| {
                tab.get(k).and_then(|v| v.as_str()).map(str::trim) == Some(tab_name)
            });
            if !named {
                continue;
            }
            let Some(obj) = tab.as_object_mut() else { continue };
            match target {
                Some(t) => {
                    obj.insert("drives".into(), serde_json::Value::String(t.to_string()));
                }
                // Cleared aims leave nothing behind: an empty key in a person's
                // file is a question they would have to answer for themselves
                None => {
                    obj.remove("drives");
                }
            }
            written = true;
        }
    }
    if !written {
        return false;
    }
    match serde_json::to_string_pretty(&doc) {
        Ok(out) => crate::crypto::write_atomic(&path, &out).is_ok(),
        Err(_) => false,
    }
}

pub fn save_appearance(key: &str, value: serde_json::Value) {
    save_setting(&["appearance", key], value);
}

/// Record one setting back into the settings file, leaving every other line of
/// it as the person wrote it. `at` is the nesting, outermost first.
///
/// This is for the settings a person changes by *using* the app rather than by
/// editing it -- the terminal's zoom, the width of the tab bar. They belong in
/// the same file as everything else, or they would not survive the next start,
/// and there would be two places to look for one answer.
pub fn save_setting(at: &[&str], value: serde_json::Value) {
    let path = config_file_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        // A file we cannot read is not one to rewrite. The change stays for
        // this run and the person keeps their settings
        crate::append_hook_log(&format!(
            "could not record {}: settings are not readable",
            at.join(".")
        ));
        return;
    };
    if !doc.is_object() {
        doc = serde_json::json!({});
    }
    let mut node = &mut doc;
    for key in at {
        node = &mut node[key];
    }
    *node = value;
    if let Ok(out) = serde_json::to_string_pretty(&doc) {
        let _ = crate::crypto::write_atomic(&path, &out);
    }
}

pub fn config_file_path() -> std::path::PathBuf {
    let candidates = config_candidates();
    candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| candidates[0].clone())
}

pub fn load() -> Option<Config> {
    for path in config_candidates() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<Config>(without_bom(&text)) {
            Ok(c) if !c.folders.is_empty() || !c.workspaces.is_empty() => return Some(c),
            _ => continue,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long to wait for a reply is the person's to set, and 0 means "as
    /// long as it takes".
    ///
    /// A limit is worth having: it is the only thing that tells "still working"
    /// apart from "never coming back". But one fixed number cannot serve both a
    /// cloud API that answers in seconds and a 27B thinking model on the
    /// machine next door, which took 320 seconds to answer "just say OK" — at
    /// the old fixed 180 it could never once finish, and said so in words
    /// ("timeout: global") that named neither the wait nor its length.
    #[test]
    fn the_wait_for_a_reply_is_settable_and_zero_means_forever() {
        let resolved = |secs: Option<u64>| {
            let mut cfg = Config::default();
            cfg.providers.insert(
                "p".into(),
                ProviderSpec {
                    base_url: "http://localhost:11434/v1".into(),
                    timeout_sec: secs,
                    ..Default::default()
                },
            );
            cfg.resolve_provider("p", None).expect("解決できる").timeout
        };
        assert_eq!(
            resolved(None),
            Some(std::time::Duration::from_secs(PROVIDER_TIMEOUT_DEFAULT_SEC)),
            "書かなければ既定の待ち時間"
        );
        assert_eq!(
            resolved(Some(600)),
            Some(std::time::Duration::from_secs(600)),
            "書いた秒数のとおりに待つ"
        );
        assert_eq!(resolved(Some(0)), None, "0 は待ち続ける（上限なし）");
    }

    /// The download keeps everything beside the exe, and must go on doing so.
    ///
    /// A Store copy has to put the person's config, data and logs under
    /// LOCALAPPDATA, because the folder it runs from is read-only to it. That
    /// belongs to the packaged copy alone: the download's promise is that the
    /// folder holds the whole of it -- copy it to another machine and the
    /// settings come along, delete it and nothing is left behind. A test run is
    /// never packaged, so this is the download's layout being asserted.
    #[test]
    fn the_portable_layout_keeps_everything_beside_the_exe() {
        assert!(!packaged(), "a test run should not be a packaged one");
        assert_eq!(root_dir(), exe_dir(), "ポータブル配置が exe の隣から離れた");
        for p in [logs_dir(), state_path("x")] {
            assert!(p.starts_with(exe_dir()), "{p:?} が exe の隣から外れた");
        }
    }

    #[test]
    fn a_group_is_the_only_thing_that_says_where_work_happens() {
        let cfg: Config = serde_json::from_str(
            r#"{"folders": [
                 {"name": "main", "cwd": "D:/work/proj",
                  "tabs": [{"name": "実装", "command": "claude"},
                           {"name": "レビュー", "command": "codex"}]},
                 {"name": "feature/login", "cwd": "scripts",
                  "tabs": [{"name": "実装", "command": "claude"}]}]}"#,
        )
        .unwrap();
        let ws = &cfg.resolve_workspaces().0[0];
        assert_eq!(ws.folders.len(), 2);
        assert_eq!(ws.tabs.len(), 3);
        // Everyone in a group works in the one folder -- the whole point, since
        // a reviewer pointed somewhere else reviews nothing
        assert_eq!(ws.cwd_of(&ws.tabs[0]), Some("D:/work/proj".into()));
        assert_eq!(ws.cwd_of(&ws.tabs[1]), ws.cwd_of(&ws.tabs[0]));
        // Relative stays relative to the settings, so a folder of them travels
        assert_eq!(ws.cwd_of(&ws.tabs[2]), Some(root_dir().join("scripts")));
    }

    /// Writing down "work on another branch too", and reading it back the way
    /// launching would.
    #[test]
    fn another_folder_is_written_with_the_same_faces() {
        let dir = std::env::temp_dir().join(format!("shikisha-append-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        std::fs::write(
            &file,
            r#"{"max_chain": 7, "workspaces": [{"name": "Demo", "secrets_allow": ["x"],
                 "folders": [{"cwd": "D:/work/proj", "tabs": [
                   {"name": "実装", "id": "coder", "command": "claude"},
                   {"name": "レビュー", "id": "rev", "command": "codex"}]}]}]}"#,
        )
        .unwrap();

        append_folder_at(
            &file,
            "Demo",
            Some(Path::new("D:/work/proj")),
            Path::new("D:/work/proj.worktrees/feature/login"),
            Some("feature/login"),
        )
        .unwrap();

        let cfg: Config = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        // Nothing else in the file was disturbed, including a key nobody read
        assert_eq!(cfg.max_chain, Some(7));
        assert_eq!(cfg.workspaces[0].secrets_allow, ["x"]);

        let ws = &cfg.resolve_workspaces().0[0];
        assert_eq!(ws.folders.len(), 2, "元の1つと、足した1つ");
        assert_eq!(ws.folders[0].cwd.as_deref(), Some(Path::new("D:/work/proj")));
        assert_eq!(ws.folders[1].name.as_deref(), Some("feature/login"));
        assert_eq!(
            ws.folders[1].cwd.as_deref(),
            Some(Path::new("D:/work/proj.worktrees/feature/login"))
        );
        // The same faces, working in the new folder
        let names = |g: usize| {
            ws.tabs
                .iter()
                .filter(|t| t.folder == g)
                .map(|t| t.cfg.name.clone().unwrap_or_default())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(1), names(0));
        assert_eq!(names(1), ["実装", "レビュー"]);
        // Automation still has one name per tab: the copies are marked with the
        // folder they went to, while what is on screen stays readable
        let ids = ws
            .tabs
            .iter()
            .filter(|t| t.folder == 1)
            .map(|t| t.cfg.id.clone().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["coder@feature-login", "rev@feature-login"]);
        assert!(duplicate_keys(ws).is_empty(), "自動化から指す名前がぶつかっていない");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_config_saved_by_a_windows_editor_still_loads() {
        // Notepad and PowerShell both write a BOM when told "UTF-8". Before
        // this, such a config parsed as nothing and the app silently ran on
        // whatever config.json it found next — looking, to the person who had
        // just edited theirs, as though the edit had not taken
        let cfg: Config =
            serde_json::from_str(without_bom("\u{feff}{\"max_chain\": 7}")).unwrap();
        assert_eq!(cfg.max_chain, Some(7));
    }

    #[test]
    fn secret_store_roundtrips_and_never_reveals_values() {
        let dir = std::env::temp_dir().join("shikisha-secrets-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("secrets.json");

        // Register in plaintext -> the list shows only key and description (no value)
        upsert_secret(&path, None, "diary_saas", "日記SaaSのログイン", "hunter2秘密").unwrap();
        let list = list_secrets(&path, None).unwrap();
        assert_eq!(list, vec![("diary_saas".into(), "日記SaaSのログイン".into())]);
        // The value really is stored (retrievable via resolve_tokens)
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("hunter2秘密"), "値が保存されていない");
        // But the value never appears in the listing API
        assert!(!format!("{list:?}").contains("hunter2"), "一覧に値が漏れている");

        // Update / add
        upsert_secret(&path, None, "diary_saas", "説明更新", "newpass").unwrap();
        upsert_secret(&path, None, "github", "PAT", "ghp_xxx").unwrap();
        let keys: Vec<String> = list_secrets(&path, None).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["diary_saas".to_string(), "github".to_string()], "整列済み");

        // Delete
        delete_secret(&path, None, "github").unwrap();
        let keys: Vec<String> = list_secrets(&path, None).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["diary_saas".to_string()]);

        // With a master password set, it's saved encrypted
        upsert_secret(&path, Some("master"), "enc_key", "暗号化テスト", "topsecret").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(crate::crypto::is_encrypted(&raw), "パスワードありなら暗号化される");
        assert!(!raw.contains("topsecret"), "暗号化後は生値が見えない");
        // With the correct password the list can be read; the value still doesn't appear
        let list = list_secrets(&path, Some("master")).unwrap();
        assert!(list.iter().any(|(k, _)| k == "enc_key"));
        // An invalid key is rejected
        assert!(upsert_secret(&path, None, "../evil", "x", "y").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn children_flatten_with_depth() {
        let cfg: Config = serde_json::from_str(
            r#"{"folders":[{"tabs":[
                {"name":"A","command":"a","children":[
                    {"name":"B","command":"b","locked":true},
                    {"name":"C","command":"c"}
                ]}
            ]}]}"#,
        )
        .unwrap();
        let (ws, errs) = cfg.resolve_workspaces();
        assert!(errs.is_empty());
        let tabs = &ws[0].tabs;
        assert_eq!(tabs.len(), 3, "親子が平坦化される");
        assert_eq!(tabs[0].depth, 0);
        assert_eq!(tabs[1].depth, 1);
        assert!(tabs[1].cfg.locked, "lockedが読める");
        assert_eq!(tabs[2].cfg.name.as_deref(), Some("C"));
    }

    #[test]
    fn legacy_projects_path_falls_back_to_workspaces() {
        // An existing config pointing at the old name projects/ should still be
        // able to read workspaces/, and the other way round
        assert_eq!(
            data_path_candidates("projects/x.json"),
            ["projects/x.json", "workspaces/x.json"],
            "projects/ 指定が workspaces/ にフォールバックする"
        );
        assert_eq!(
            data_path_candidates("workspaces/x.json"),
            ["workspaces/x.json", "projects/x.json"]
        );
        // Anything else is only ever itself
        assert_eq!(data_path_candidates("scripts/x.lua"), ["scripts/x.lua"]);
    }

    #[test]
    fn inline_workspaces_are_resolved() {
        let cfg: Config = serde_json::from_str(
            r#"{"workspaces":[
                {"name":"X","tabs":[{"name":"a","command":"a"}]},
                {"name":"Y","tabs":[{"name":"b","command":"b"}]}
            ]}"#,
        )
        .unwrap();
        let (ws, _) = cfg.resolve_workspaces();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[1].name, "Y");
    }

    #[test]
    fn missing_workspace_file_is_reported_not_fatal() {
        let cfg: Config = serde_json::from_str(
            r#"{"workspaces":[
                {"name":"Bad","file":"workspaces/does-not-exist.json"},
                {"name":"Good","tabs":[{"name":"a","command":"a"}]}
            ]}"#,
        )
        .unwrap();
        let (ws, errs) = cfg.resolve_workspaces();
        assert_eq!(ws.len(), 1, "壊れた定義は飛ばして続行");
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn operate_defaults_match_the_baked_in_safety_net() {
        // A config with no `operate` block falls back to the historical limits and
        // the safe "stop" policy, so behavior is unchanged until the user opts in.
        let cfg: Config = serde_json::from_str(r#"{"workspaces":[]}"#).unwrap();
        assert_eq!(cfg.operate.max_rounds, 40);
        assert_eq!(cfg.operate.max_seconds, 900);
        assert_eq!(cfg.operate.max_tokens, 400_000);
        assert_eq!(cfg.operate.on_limit, "stop");
        assert_eq!(cfg.operate.settle_ms, 1800);
        assert_eq!(cfg.operate.confirm, "off");
    }

    #[test]
    fn operate_accepts_partial_overrides_and_unlimited_zeros() {
        // Only some fields set: the rest keep their defaults. 0 means "no limit".
        let cfg: Config =
            serde_json::from_str(r#"{"workspaces":[],"operate":{"max_rounds":0,"on_limit":"continue"}}"#)
                .unwrap();
        assert_eq!(cfg.operate.max_rounds, 0, "0 = unlimited rounds");
        assert_eq!(cfg.operate.on_limit, "continue");
        assert_eq!(cfg.operate.max_seconds, 900, "untouched field keeps its default");
    }
}

#[cfg(test)]
mod browser_kind_tests {
    use super::browser_url_of;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// The settings screen and the app itself must classify "type = browser" by the same rule.
    ///
    /// If the check were split across two places, a tab could look like a browser
    /// on screen but launch a shell instead -- a mismatch
    #[test]
    fn a_browser_tab_is_told_apart_by_its_command() {
        assert_eq!(
            browser_url_of(&v(&["browser", "https://example.com/"])).as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            browser_url_of(&v(&["web", "http://127.0.0.1:8080/"])).as_deref(),
            Some("http://127.0.0.1:8080/"),
            "web という綴りも通す"
        );
        assert_eq!(
            browser_url_of(&v(&["BROWSER", "https://a.example/"])).as_deref(),
            Some("https://a.example/"),
            "大文字小文字は問わない"
        );

        // Things that are not a browser
        assert!(browser_url_of(&v(&["cmd.exe"])).is_none());
        assert!(browser_url_of(&v(&["claude"])).is_none());
        assert!(browser_url_of(&v(&["browser"])).is_none(), "URLが無い");
        assert!(browser_url_of(&v(&["browser", "  "])).is_none(), "空白だけ");
        assert!(browser_url_of(&[]).is_none());
        // Don't sweep in some other command that merely starts with "browser"
        assert!(browser_url_of(&v(&["browserify", "x"])).is_none());
    }

    /// A browser tab's appearance can be decided by config alone, with no Lua.
    ///
    /// Only what's written shows up; anything not written falls back to the default (hidden)
    #[test]
    fn a_browser_tab_can_be_dressed_from_the_settings_alone() {
        let t: super::TabConfig = serde_json::from_str(
            r#"{
                "name": "解析",
                "command": "browser https://example.com/",
                "nav": { "reload": true, "url": true },
                "ask": { "text": "読み終わったら押してください", "label": "解析する" }
            }"#,
        )
        .unwrap();
        let nav = t.nav.expect("上のバーが読めていない");
        assert!(nav.reload && nav.url, "書いたものが出ない");
        assert!(!nav.back && !nav.forward, "書いていないものまで出る");
        let ask = t.ask.expect("帯が読めていない");
        assert_eq!(ask.label, "解析する");

        // If nothing is written, neither is shown
        let bare: super::TabConfig =
            serde_json::from_str(r#"{"command": "browser https://example.com/"}"#).unwrap();
        assert!(bare.nav.is_none() && bare.ask.is_none());

        // The banner's mere presence means "show it" -- shown even with empty contents
        let empty: super::TabConfig =
            serde_json::from_str(r#"{"command": "browser https://x/", "ask": {}}"#).unwrap();
        assert!(empty.ask.is_some(), "空の帯を無かったことにしている");
    }
}
