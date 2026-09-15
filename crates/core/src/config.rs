//! config/config.json: defines the desk / tab layout. See DESIGN.md chapter 7.4.
//! Looked up in the config folder beside the exe, then the config folder in the
//! current directory.
//!
//! Terminology: a "desk" is the unit you switch between (like a virtual
//! desktop). Its externalized contents form a "desk definition file".
//!
//! Settings are split into 3 kinds by role (everything the user owns lives
//! under the config folder):
//!   config/config.json  ... global settings + desk list (rarely changed)
//!   desks/*.json   ... desk definition files (copyable/shareable units)
//!   config/secrets.json ... credentials (can be encrypted, never share)

use anyhow::{Context as _, Result};
use serde::Deserialize;
use std::path::Path;

/// A repository somebody works on, named once.
///
/// It was never written down before: a project was worked out every time it
/// was wanted, by asking git what folder a folder belonged to. That inference
/// answered most questions and none of the hard ones -- where the checkout is
/// on *this* machine, which of two projects with the same folder name this is,
/// what this project needs before it can be built -- and each of those had to
/// be guessed separately, in a different place, with a different way of being
/// wrong. Writing it down once is what stops the guessing.
///
/// Everything here is absent-able. A settings file that has never heard of
/// projects keeps working exactly as it did, on the same inference; this is
/// what a folder can say instead of leaving it to be guessed.
#[derive(Debug, Clone, Deserialize, serde::Serialize, Default, PartialEq, Eq)]
pub struct ProjectSpec {
    /// What it is called. The name folders refer to it by, so two projects
    /// whose folders happen to share a name are still two projects
    pub name: String,
    /// Where its own checkout is on this machine
    #[serde(default)]
    pub at: Option<String>,
    /// What to run in a new worktree of it, for a project that cannot carry a
    /// devcontainer -- one that is built for Windows, or for a phone, or
    /// against hardware. Where there is a devcontainer, that is read instead
    /// and this is not asked for
    #[serde(default)]
    pub setup: Option<String>,
    /// The git account the column beside a folder of this project fetches,
    /// pulls and pushes with, and reads pull request numbers with: one of the
    /// desk's `git_accounts` by name, or [`THIS_PC`]. Absent until somebody
    /// chooses -- nothing is chosen for them, so "which account did that push
    /// go out as" always has an answer somebody gave
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_account: Option<String>,
    /// What a new worktree of this project is given of what git does not carry:
    /// the files a line of `.gitignore` matches, and files from anywhere else.
    /// A line with no rule here gets the answer [`crate::worktree::default_how`]
    /// gives, which the settings screen shows beside it
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bring: Vec<BringRule>,
}

/// How one thing reaches a new worktree.
///
/// Either a line of an ignore file -- `pattern`, as written, with the file it
/// is written in as `source` (absent is the project's own `.gitignore`) -- and
/// then it covers everything that line makes git ignore; or `from` a path
/// anywhere on this machine, put at `to` inside the worktree.
#[derive(Debug, Clone, Deserialize, serde::Serialize, Default, PartialEq, Eq)]
pub struct BringRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// `copy`, `replace` (copy, then the replacements below), `link`, or `skip`
    #[serde(default)]
    pub how: String,
    /// For `replace`: what is written differently in the copy, in order
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replace: Vec<Replace>,
}

/// Every occurrence of `find` in a copied file becomes `with`.
///
/// With `regex`, `find` is a regular expression in which `^` and `$` are the
/// start and end of each line, and `with` may name its groups as `$1`
#[derive(Debug, Clone, Deserialize, serde::Serialize, Default, PartialEq, Eq)]
pub struct Replace {
    pub find: String,
    #[serde(default)]
    pub with: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub regex: bool,
}

impl BringRule {
    /// Whether this is the rule for that line of that ignore file
    pub fn is_for(&self, source: &str, pattern: &str) -> bool {
        let own = self.source.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(".gitignore");
        self.pattern.as_deref() == Some(pattern) && own == source
    }
}

/// A git account a desk signs in to a git server with.
///
/// Nothing secret is here. The token is filed under [`git_token_key`], worked
/// out from the desk and this name, so the settings can be read and copied
/// without carrying a credential; an SSH key is named by its path, the way a
/// server tab names its key.
#[derive(Debug, Clone, Deserialize, serde::Serialize, Default, PartialEq, Eq)]
pub struct GitAccountSpec {
    /// What it is called in the pickers. One word, because it is also part of
    /// the name its token is filed under
    pub name: String,
    /// The server it signs in to. Absent is `github.com`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// `ssh` for an account that signs in with a key file; anything else --
    /// absent included -- is a token over HTTPS
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The user name sent with the token. Absent sends a stand-in, which is
    /// what GitHub expects of a token
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    /// The private key file, for an `ssh` account
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The name and email a commit made with this account carries. Absent is
    /// whatever git itself is set to on this machine
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    /// The owners (users and organisations) whose repositories this account is
    /// for. Only an order: a repository owned by one of these lists this
    /// account first. Nothing is picked because of it
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
}

/// What a git account is chosen as when the choice is "the way git on this
/// PC already signs in": its credential helper, its SSH keys, its name. A
/// real account name is one word of letters, digits, `-` and `_`, so this
/// can never be one
pub const THIS_PC: &str = "@pc";

/// The GitHub server, which is what an account that names none signs in to
pub const GITHUB_HOST: &str = "github.com";

impl GitAccountSpec {
    pub fn host(&self) -> String {
        self.host
            .as_deref()
            .map(|h| h.trim().trim_end_matches('/').to_ascii_lowercase())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| GITHUB_HOST.to_string())
    }

    pub fn is_ssh(&self) -> bool {
        self.method.as_deref().is_some_and(|m| m.trim().eq_ignore_ascii_case("ssh"))
    }

    /// Whether this account says it is for repositories of `owner`
    pub fn serves(&self, owner: &str) -> bool {
        self.owners.iter().any(|o| o.trim().eq_ignore_ascii_case(owner.trim()))
    }
}

/// Where a git account's token is filed: under the desk, like every other
/// credential a desk has, and worked out rather than written down so there is
/// one spelling for the screen that stores it and the program that reads it
pub fn git_token_key(desk_id: &str, account: &str) -> String {
    format!("git/{desk_id}/{account}")
}

/// Which git account something signs in with, as the settings say -- before
/// any secret is read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum GitUse {
    /// Nobody has chosen. Nothing that needs to sign in runs
    #[default]
    Unset,
    /// The way git on this PC already signs in
    Pc,
    /// One of the desk's accounts
    Account { desk: String, spec: GitAccountSpec },
    /// A name that no account in the desk answers to any more
    Missing(String),
}

impl GitUse {
    /// What the choice is written as: an account name, [`THIS_PC`], or empty
    pub fn written(&self) -> String {
        match self {
            GitUse::Unset => String::new(),
            GitUse::Pc => THIS_PC.to_string(),
            GitUse::Account { spec, .. } => spec.name.clone(),
            GitUse::Missing(n) => n.clone(),
        }
    }

    /// Commit identity and credentials, ready for git.
    ///
    /// `sign_in` is whether what is about to run talks to a server: then a
    /// choice nobody made, or one that no longer exists, is refused in words
    /// that say where to make it. A commit needs only the name on it, so it
    /// goes ahead with git's own when nothing was chosen -- but not with an
    /// account that has gone, whose name it was meant to carry. `look` reads
    /// the secret store
    pub fn to_git(
        &self,
        sign_in: bool,
        look: &dyn Fn(&str) -> Option<String>,
    ) -> Result<crate::git::As, String> {
        match self {
            GitUse::Unset if sign_in => Err(crate::i18n::t("err.git.account.unset")),
            GitUse::Unset => Ok(crate::git::As::sealed()),
            GitUse::Pc => Ok(crate::git::As::default()),
            GitUse::Missing(name) => {
                Err(crate::i18n::tp("err.git.account.missing", &[("name", name)]))
            }
            GitUse::Account { desk, spec } => {
                let some = |v: &Option<String>| {
                    v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
                };
                let auth = match (sign_in, spec.is_ssh()) {
                    (false, _) => crate::git::Auth::Sealed,
                    (true, true) => {
                        let key = some(&spec.key).unwrap_or_default();
                        if key.is_empty() || !std::path::Path::new(&key).is_file() {
                            return Err(crate::i18n::tp(
                                "err.git.account.no_key",
                                &[("name", &spec.name), ("path", &key)],
                            ));
                        }
                        crate::git::Auth::Ssh { key: key.into() }
                    }
                    (true, false) => {
                        let token = look(&git_token_key(desk, &spec.name))
                            .map(|t| t.trim().to_string())
                            .filter(|t| !t.is_empty())
                            .ok_or_else(|| {
                                crate::i18n::tp("err.git.account.no_token", &[("name", &spec.name)])
                            })?;
                        crate::git::Auth::Token {
                            host: spec.host(),
                            login: some(&spec.login).unwrap_or_else(|| "x-access-token".into()),
                            token,
                        }
                    }
                };
                Ok(crate::git::As {
                    auth,
                    account: Some(spec.name.clone()),
                    name: some(&spec.user_name),
                    email: some(&spec.user_email),
                })
            }
        }
    }

    /// The name the pull request watch files a token under: an account, or
    /// [`THIS_PC`]. None where there is nothing to ask with
    pub fn pr_account(&self) -> Option<String> {
        match self {
            GitUse::Pc => Some(THIS_PC.to_string()),
            GitUse::Account { spec, .. } if spec.host() == GITHUB_HOST => Some(spec.name.clone()),
            _ => None,
        }
    }
}

impl Desk {
    /// A written choice, looked up among this desk's accounts
    pub fn git_use(&self, chosen: Option<&str>) -> GitUse {
        match chosen.map(str::trim).filter(|c| !c.is_empty()) {
            None => GitUse::Unset,
            Some(THIS_PC) => GitUse::Pc,
            Some(name) => match self.git_accounts.iter().find(|a| a.name == name) {
                Some(spec) => GitUse::Account { desk: self.id.clone(), spec: spec.clone() },
                None => GitUse::Missing(name.to_string()),
            },
        }
    }

    /// The account a folder's project chose, with the project's name
    pub fn git_use_of_folder(&self, cwd: &std::path::Path) -> (GitUse, Option<String>) {
        match self.project_of(cwd) {
            Some(p) => (self.git_use(p.git_account.as_deref()), Some(p.name.clone())),
            None => (GitUse::Unset, None),
        }
    }

}

/// A desk's accounts in the order to offer them for a repository of `owner`
/// on `host`: the ones that say they are for it first, then the rest of that
/// server's, then the others, each group as written. Each with whether it says
/// it is for this repository
pub fn git_accounts_for(
    accounts: &[GitAccountSpec],
    host: Option<&str>,
    owner: Option<&str>,
) -> Vec<(GitAccountSpec, bool)> {
    let mut list: Vec<(usize, GitAccountSpec, bool)> = accounts
        .iter()
        .map(|a| {
            let same_host = host.is_none_or(|h| a.host() == h.to_ascii_lowercase());
            let fits = same_host && owner.is_some_and(|o| a.serves(o));
            let rank = match (fits, same_host) {
                (true, _) => 0,
                (false, true) => 1,
                (false, false) => 2,
            };
            (rank, a.clone(), fits)
        })
        .collect();
    list.sort_by_key(|(rank, ..)| *rank);
    list.into_iter().map(|(_, a, fits)| (a, fits)).collect()
}

impl Desk {
    /// The project one of this desk's folders belongs to.
    ///
    /// Asked of a desk, never of the whole file: the same repository can be a
    /// project in the work desk and another in the personal one, each with its
    /// own setup, and a setting changed from one must not reach the other. By
    /// what the folder wrote down, and failing that by which of this desk's
    /// projects has its own checkout in the same repository.
    pub fn project_of(&self, cwd: &std::path::Path) -> Option<&ProjectSpec> {
        let named = self
            .folders
            .iter()
            .find(|f| f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, cwd)))
            .and_then(|f| f.project.as_deref())
            .map(str::trim)
            .filter(|p| !p.is_empty());
        if let Some(name) = named {
            return self.projects.iter().find(|p| p.name == name);
        }
        let family = crate::repo::family_of(cwd)?;
        self.projects.iter().find(|p| {
            p.at
                .as_deref()
                .map(std::path::Path::new)
                .and_then(crate::repo::family_of)
                .is_some_and(|f| f == family)
        })
    }
}

impl Config {
    /// The project a folder belongs to, on the desk named by `desk_id`. With no
    /// desk named, the first desk that holds a folder at that place -- which is
    /// only right for a caller that has no desk to name, so the ones that do
    /// name it
    pub fn project_of(&self, desk_id: Option<&str>, cwd: &std::path::Path) -> Option<ProjectSpec> {
        let (desks, _) = self.resolve_desks();
        let here = desks.iter().find(|d| match desk_id {
            Some(id) => d.id == id,
            None => d.folders.iter().any(|f| f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, cwd))),
        })?;
        here.project_of(cwd).cloned()
    }
}

/// A machine that is not this one, and can hold work of its own.
///
/// Named once and referred to by that name everywhere else, because the same
/// machine is the same machine whichever folder is asking -- and because a
/// person who changes a port should change it in one place. What is not here
/// is the password: a credential lives in the secrets file under a name of its
/// own, and this holds the name.
#[derive(Debug, Clone, Deserialize, serde::Serialize, Default, PartialEq, Eq)]
pub struct HostSpec {
    /// What this machine is called in the picker. Its own, not the address:
    /// two accounts on one server are two entries
    pub name: String,
    /// Where it is, written the way the world writes it: `ssh://me@host:22`
    pub at: String,
    /// The folder a project is checked out in over there. A worktree cut on
    /// that machine is cut from this
    #[serde(default)]
    pub project: Option<String>,
    /// Where branches go over there. Absent means beside the checkout's own
    /// parent, which is the only thing that can be guessed about a machine
    /// this program has never seen
    #[serde(default)]
    pub branches: Option<String>,
    /// What kind of machine it is: `ssh` for one that is already there, `e2b`
    /// for one that is made when it is wanted. Absent is `ssh`, because that
    /// is what a machine with an address written down is
    #[serde(default)]
    pub kind: Option<String>,
    /// For a machine that is made: the image it is made from
    #[serde(default)]
    pub template: Option<String>,
    /// For a machine that is made: how many minutes it lives untouched. One
    /// that nobody stops still stops on its own, which is the only reason a
    /// program may ask for one
    #[serde(default)]
    pub minutes: Option<u32>,
}

impl HostSpec {
    /// Whether this machine has to be asked for before anything can run on it.
    pub fn is_made(&self) -> bool {
        self.kind.as_deref().map(str::trim).unwrap_or("ssh").eq_ignore_ascii_case("e2b")
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    // The projects are not here: each desk keeps its own (`DeskConfig::projects`)
    /// Machines that are not this one. Empty on every install until somebody
    /// adds one, and the picker says "this PC" and nothing else until then
    #[serde(default)]
    pub hosts: Vec<HostSpec>,
    /// List of desks (projects). Switched between like virtual desktops
    #[serde(default)]
    pub desks: Vec<DeskSpec>,
    /// Working folders written directly when desks are not used
    #[serde(default)]
    pub folders: Vec<FolderConfig>,
    /// Tabs written directly here, the way they were before folders existed.
    ///
    /// Kept because a settings file outlives the version that wrote it. When
    /// this shape stopped being read, every tab in a file from the older
    /// version stopped existing -- no error, no warning, a desk that
    /// simply opened empty. Reading them and folding them into the first
    /// folder is what upgrading should have done in the first place.
    #[serde(default)]
    pub tabs: Vec<TabConfig>,

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
    /// Whether to start from the last-opened desk (default: yes)
    pub restore_desk: Option<bool>,
    /// Whether the window's ✕ puts the program away in the notification area
    /// rather than quitting (default: yes). Put away, the tabs go on working
    /// and the phone stays connected; the icon's menu is where quitting is
    pub resident: Option<bool>,
    /// Whether to ask Claude's service how much of the subscription's 5-hour
    /// and 7-day allowance is used, with the sign-in Claude Code keeps on
    /// this PC, and show it while a Claude tab is in view (default: yes).
    /// Nothing is asked on a machine with no such sign-in
    pub claude_usage: Option<bool>,
    /// Whether to look, at start and once a day, for a newer published
    /// version (default: yes). Looking is all it does: one request for the
    /// newest version number. Installing is a button on the settings screen
    pub update_check: Option<bool>,
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
    /// Width of the right-hand panel column, in pixels, or 0 when it is put
    /// away. Omitted means put away: a column nobody has opened yet takes
    /// nothing from the terminal, which is the part of the window the work is
    /// actually in.
    #[serde(default)]
    pub side_bar_width: Option<u16>,
    // Where notifications go, the model connections, the automation doors, who
    // may run what, and what git does are not here: each belongs to a desk
    // (see `DeskConfig`), whole, with nothing of the app's underneath. An app
    // answer a desk inherits until somebody unticks it is the one that sends
    // work's code to a personal account on the day nobody thought to look
    /// Load secrets from a separate file (e.g. "secrets.json")
    #[serde(default)]
    pub secrets: Option<String>,
    /// The AI that writes automation code ("claude" / "codex" / "gemini").
    /// Uses whichever is found if empty
    #[serde(default)]
    pub ai_engine: Option<String>,
    /// Keys that open the tools from any program, by what they open (see
    /// `hotkeys::ACTIONS`): "Alt+Shift+X". Only what was changed is written;
    /// the scissors not written at all have `hotkeys::DEFAULT`, written empty
    /// they have none
    #[serde(default)]
    pub hotkeys: std::collections::BTreeMap<String, String>,
    /// Remote UI viewable from a phone etc. Disabled by default.
    ///
    /// App-wide, and deliberately so. Every part of it describes one server on
    /// one machine -- an address, a port, one pairing with one phone -- and
    /// there is no reading of it under which a desk would want a different
    /// answer. What a phone can then DO is a different question, and that one is
    /// already the desk's: a touch arrives as the same intent a click does,
    /// and is held to the table and the doors of the desk on screen (see
    /// [`DeskSpec::automation_permissions`]).
    ///
    /// "Do not show this desk on a phone at all" was asked and answered: no.
    /// The person at the keyboard and the person holding the phone are the same
    /// person, so there is nothing for a curtain to keep from anybody -- and it
    /// would cost a screen of its own, since one saying "disconnected" when the
    /// truth is "this one is not shown here" is worse than not having it. The
    /// phone is this person's second window, never a guest's
    #[serde(default)]
    pub remote: RemoteSpec,
    /// Who may drive this app from outside, over its named pipe. The default
    /// is the processes this app started and nothing else — see api.rs
    ///
    /// App-wide for the same reason: there is one pipe, and it belongs to the
    /// process. Splitting the setting would not split the pipe, and a second
    /// per-desk switch would only be a second place to read one answer
    /// from. What a call may actually do is already the desk's -- the key
    /// names the tab, [`crate::runtime::subject_of`] turns that into who is
    /// calling, and the answer comes from the desk on screen. A key naming
    /// a tab that is not in it is nobody, and is answered as an AI: the side
    /// that cannot do harm if the guess is wrong
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
    /// The quick commands: buttons on pages of a grid, each handing the tab in
    /// view a command or a prompt (see `quick.rs`). App-wide, like the keys:
    /// they are this person's habits, and a secret they name is looked up in
    /// the desk on screen when one is pressed
    #[serde(default)]
    pub quick_commands: crate::quick::QuickSpec,
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
    /// Where a page opened by automation is drawn, for a runtime that has a
    /// device connected to it: `"here"` (this machine, the default -- watchable
    /// from anywhere, and works with nobody connected) or `"there"` (the
    /// connected device's own browser, which is immediate but needs somebody
    /// present). Either way the page reaches the network through this machine
    pub browser_draw: Option<String>,
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

/// The width the right-hand column comes out at when it is asked for.
///
/// Wider than the left bar because what stands in it is a list of changed
/// files and a diff beside it, not a column of names.
pub const SIDE_BAR_DEFAULT_PX: u16 = 380;
/// Below this the diff under the file list is a keyhole rather than a diff.
pub const SIDE_BAR_MIN_PX: u16 = 280;
pub const SIDE_BAR_MAX_PX: u16 = 900;

/// A width as it may actually be used: put away (0), or inside the bounds.
pub fn clamp_side_bar(px: u16) -> u16 {
    if px == 0 {
        0
    } else {
        px.clamp(SIDE_BAR_MIN_PX, SIDE_BAR_MAX_PX)
    }
}

/// How wide the column opens. Nothing written down means nothing shown: the
/// column costs the terminal its width, so it waits to be asked for.
pub fn side_bar_px() -> u16 {
    load().and_then(|c| c.side_bar_width).map(clamp_side_bar).unwrap_or(0)
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
    browser_store("webview2")
}

/// Where the browser on this machine keeps its profiles.
///
/// Beside the window's store rather than inside it: these are two different
/// browsers and their stores are different things, and one folder holding both
/// is a folder neither can be told to clear.
pub fn chromium_data_dir() -> std::path::PathBuf {
    browser_store("chromium")
}

fn browser_store(leaf: &str) -> std::path::PathBuf {
    let mode = load()
        .and_then(|c| c.browser_data)
        .unwrap_or_default();
    match mode.trim() {
        "portable" => root_dir().join("data").join(leaf),
        "" | "local" => local_appdata().join("ShikishaTerm").join(leaf),
        // A folder the person named. The window's store went straight into it,
        // so that is where it stays, and anything else goes beside it
        other if leaf == "webview2" => std::path::PathBuf::from(other),
        other => std::path::PathBuf::from(other).join(leaf),
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

/// What a secret may be used for.
///
/// The value is only half of a credential; the other half is what it is for,
/// and that half is not a secret. Kept beside the value rather than in
/// `config.json` so that a settings file shared with somebody cannot quietly
/// widen what a password on this machine is allowed to do.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct SecretMeta {
    /// Whether a script a person set going may use this.
    ///
    /// On by default, because that is what registering a password is normally
    /// for. Off is meaningful too: a key that only an AI's errands ever touch
    /// is one a person cannot spend by hand, and the two questions are asked
    /// separately for that reason
    #[serde(default = "yes")]
    pub human: bool,
    /// Whether a script that an AI's turn set going may use this.
    ///
    /// Off by default, and separately from the permission table: that table
    /// says whether an AI may *fill in a password at all*, and this says
    /// *which* ones -- "let it sign in to the forum" without also meaning
    /// "let it sign in to the thing that can delete the DNS records"
    #[serde(default)]
    pub ai: bool,
    /// The addresses this may be typed into. Empty means nowhere: a stored
    /// password is not something to hand to whatever page happens to be open.
    ///
    /// Each one is a whole address, written out: `https://example.com`,
    /// `https://example.com/api`, `https://*.example.com`. What is written is
    /// what is compared -- there is no shorthand to learn and no rule that
    /// turns one thing into another behind the person's back. See
    /// [`url_fault`] for what a line may say and [`Self::may_fill`] for how
    /// far each part reaches
    #[serde(default)]
    pub urls: Vec<String>,
    /// One line saying what this is, for the person reading the list later
    #[serde(default)]
    pub desc: String,
}

fn yes() -> bool {
    true
}

/// One address, in the four parts that decide whether a password goes in.
///
/// Both sides of the comparison are read into this, so the page and the line
/// somebody typed are held to the same reading of what an address is.
#[derive(Debug)]
pub struct Place {
    scheme: String,
    /// Lower case. `*.example.com` on an entry means the site and everything
    /// under it; a page never carries a star
    host: String,
    port: u16,
    /// Always begins with `/`. `/` means the whole site
    path: String,
}

/// The suffixes everybody shares. `*.` in front of one of these is not a site,
/// it is the whole internet with a shape, and a password would be handed to
/// whoever registers next. Not the public suffix list -- the common ones, and
/// the shape of the rest is caught by asking for two labels
const SHARED_SUFFIX: &[&str] = &[
    "com", "net", "org", "jp", "io", "dev", "app", "co", "ne", "or", "co.jp", "ne.jp", "or.jp",
    "co.uk", "com.au", "com.br", "co.kr", "com.cn",
];

fn default_port(scheme: &str) -> u16 {
    if scheme == "http" {
        80
    } else {
        443
    }
}

impl Place {
    /// Split an address into its parts. Nothing is guessed: an address with no
    /// scheme is not an address
    fn split(text: &str) -> Option<Self> {
        let t = text.trim();
        let (scheme, rest) = t.split_once("://")?;
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https") {
            return None;
        }
        // Anything after ? or # is not part of where the page is
        let rest = rest.split(['?', '#']).next().unwrap_or("");
        let (hostport, path) = match rest.find('/') {
            Some(at) => (&rest[..at], rest[at..].to_string()),
            None => (rest, "/".to_string()),
        };
        if hostport.is_empty() || hostport.contains('@') || hostport.contains('[') {
            return None; // credentials in an address, and IPv6, are not read here
        }
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().ok()?),
            None => (hostport, default_port(&scheme)),
        };
        if host.is_empty() {
            return None;
        }
        Some(Place {
            host: host.to_ascii_lowercase(),
            port,
            path,
            scheme,
        })
    }

    /// The page in front of the person
    fn page(url: &str) -> Option<Self> {
        let me = Self::split(url)?;
        if me.host.contains('*') {
            return None; // a page cannot be a pattern
        }
        Some(me)
    }

    /// A line somebody wrote in the settings
    fn entry(text: &str) -> Option<Self> {
        if url_fault(text).is_some() {
            return None;
        }
        Self::split(text)
    }

    /// Whether this entry reaches that page
    fn covers(&self, page: &Self) -> bool {
        if self.scheme != page.scheme || self.port != page.port {
            return false;
        }
        let host_ok = match self.host.strip_prefix("*.") {
            // The site itself as well as what is under it: writing
            // `*.example.com` and then not being let into example.com is the
            // kind of surprise that gets worked around with a second entry
            Some(under) => page.host == under || page.host.ends_with(&format!(".{under}")),
            None => page.host == self.host,
        };
        if !host_ok {
            return false;
        }
        let want = self.path.trim_end_matches('*');
        let want = want.strip_suffix('/').unwrap_or(want);
        // "" (the whole site) covers everything; otherwise the page's path has
        // to be that path, or something inside it
        want.is_empty()
            || page.path == want
            || page.path.starts_with(&format!("{want}/"))
    }
}

/// The secrets in the file that nothing in the settings claims any more.
///
/// A secret belongs to the thing that uses it and is let go of with it, so
/// this should be empty. It will not always be: a settings file edited by
/// hand, a desk deleted in an older version, a name changed underneath.
/// Rather than keep a screen for tidying, the settings say when there is
/// something to tidy.
///
/// Only the shapes this program files things under are judged. Anything else
/// -- a name a person invented, a key an older version wrote -- is left alone,
/// because "I do not recognise it" is not the same as "nobody wants it".
pub fn orphan_secrets(cfg: &Config, keys: &[String]) -> Vec<String> {
    let (spaces, _) = cfg.resolve_desks();
    // A destination keeps its token as "@name". Two fields carry one, and
    // the rest of the destinations have nothing to keep. Every desk's, since
    // each keeps its own
    let refs: std::collections::HashSet<String> = spaces
        .iter()
        .flat_map(|s| s.notify.values())
        .flat_map(|d| match d {
            crate::notify::Destination::Slack { webhook }
            | crate::notify::Destination::Discord { webhook } => vec![webhook.clone()],
            crate::notify::Destination::Telegram { token, .. } => vec![token.clone()],
            _ => Vec::new(),
        })
        .filter_map(|r| r.strip_prefix('@').map(str::to_string))
        .collect();
    keys.iter()
        .filter(|k| {
            let k = k.as_str();
            // provider/<desk>/<name>
            if let Some(rest) = k.strip_prefix("provider/") {
                let Some((desk, name)) = rest.split_once('/') else {
                    return false;
                };
                return !spaces.iter().any(|s| s.id == desk && s.providers.contains_key(name));
            }
            if k.starts_with("notify/") {
                return !refs.contains(k);
            }
            if let Some(rest) = k.strip_prefix("git/") {
                // git/<desk>/<account>
                let Some((desk, name)) = rest.split_once('/') else {
                    return false;
                };
                return !spaces
                    .iter()
                    .any(|s| s.id == desk && s.git_accounts.iter().any(|a| a.name == name));
            }
            if let Some(rest) = k.strip_prefix("ssh/") {
                // ssh/<desk>/<tab>/<what>
                let mut part = rest.split('/');
                let (Some(desk), Some(tab)) = (part.next(), part.next()) else {
                    return false;
                };
                return !spaces.iter().any(|s| {
                    s.id == desk
                        && s.tabs
                            .iter()
                            .any(|t| t.cfg.id.as_deref().unwrap_or_default() == tab)
                });
            }
            if k.contains('/') {
                return false; // a shape this version does not know
            }
            match k.split_once('.') {
                // <desk>.<name>, the ones automation asks for
                Some((desk, _)) => !spaces.iter().any(|s| s.id == desk),
                None => false,
            }
        })
        .cloned()
        .collect()
}

/// Why this line cannot be used, as the key of the sentence to show, or None.
///
/// The screen asks this while somebody types and the store asks it before
/// saving, so a line that is refused is refused for a reason that was already
/// on the screen.
pub fn url_fault(text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.is_empty() {
        return Some("err.secret_url.empty");
    }
    if t.chars().any(char::is_whitespace) {
        return Some("err.secret_url.unreadable");
    }
    let Some((scheme, rest)) = t.split_once("://") else {
        return Some("err.secret_url.scheme");
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Some("err.secret_url.scheme");
    }
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if host.is_empty() || host.contains('@') {
        return Some("err.secret_url.unreadable");
    }
    let stars = host.matches('*').count();
    if stars > 0 {
        // One star, at the front, followed by a dot. In the middle it either
        // means nothing or matches a name somebody else owns
        if stars > 1 || !host.starts_with("*.") {
            return Some("err.secret_url.star_place");
        }
        let under = host[2..].split(':').next().unwrap_or("");
        if under.split('.').count() < 2 || SHARED_SUFFIX.contains(&under) {
            return Some("err.secret_url.star_wide");
        }
    }
    if Place::split(t).is_none() {
        return Some("err.secret_url.unreadable");
    }
    None
}

impl Default for SecretMeta {
    fn default() -> Self {
        Self {
            human: true,
            ai: false,
            urls: Vec::new(),
            desc: String::new(),
        }
    }
}

impl SecretMeta {
    /// Whether this side of the machine may use it at all.
    ///
    /// The two answers are independent: a secret may be for people, for an
    /// AI's errands, for both, or -- while somebody is in the middle of
    /// setting one up -- for neither, which simply means nothing can use it
    pub fn may_use(&self, who: crate::grants::Subject) -> bool {
        match who {
            crate::grants::Subject::Ai => self.ai,
            crate::grants::Subject::Human => self.human,
        }
    }

    /// Whether a page at this address is one this secret may be typed into.
    ///
    /// Compared part by part after parsing, never by how the address starts:
    /// `https://github.com.evil.example/` begins with the right letters and is
    /// somebody else entirely.
    ///
    /// - the scheme has to match, so a page dropped to `http` is not the
    ///   `https` page that was allowed
    /// - the host has to match, or fall under a `*.` written at the front
    /// - a port written down has to match; left out, it is the scheme's own
    /// - a path written down is a prefix, and it ends at a `/`: `/api` covers
    ///   `/api` and `/api/keys`, and not `/apiary`. Nothing (or `/*`) covers
    ///   the whole site
    /// - what comes after `?` is never part of the decision
    pub fn may_fill(&self, url: &str) -> bool {
        let Some(page) = Place::page(url) else {
            return false;
        };
        self.urls
            .iter()
            .filter_map(|u| Place::entry(u))
            .any(|e| e.covers(&page))
    }
}

/// secrets.json: a file holding only credentials, kept separate (never share)
#[derive(Debug, Deserialize, Default)]
pub struct Secrets {
    /// Auth info used by the HTTP gateway (not readable from scripts)
    #[serde(default)]
    pub tokens: std::collections::HashMap<String, String>,
    /// Description of each token (shown in the GUI list; the value itself never is)
    #[serde(default)]
    pub descriptions: std::collections::HashMap<String, String>,
    /// What each secret is allowed to be used for, beside its value. Absent
    /// means the careful answer: not for AI, and no site to be typed into
    #[serde(default)]
    pub meta: std::collections::HashMap<String, SecretMeta>,
    /// Remote UI token. Setting this pins the URL and avoids needing to re-pair
    #[serde(default)]
    pub remote_token: Option<String>,
}

impl Config {
    /// What is wrong with the secrets file, if it cannot be read. The readers
    /// below treat a file they cannot open as empty, which is right for them
    /// and silent for the person -- this is the part that says so
    pub fn secrets_problem(&self, password: Option<&str>) -> Option<String> {
        let path = self.secrets_path().filter(|p| p.exists())?;
        crate::crypto::read_maybe_encrypted(&path, password)
            .and_then(|t| {
                serde_json::from_str::<Secrets>(&t)
                    .with_context(|| crate::i18n::t("err.config.secrets_json_invalid"))
            })
            .err()
            .map(|e| format!("secrets: {e:#}"))
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

    /// What every stored secret is for, by its full name. Read beside the
    /// values, from the same file and the same password, so the two cannot
    /// disagree about which secret is which
    pub fn resolve_secret_terms(
        &self,
        password: Option<&str>,
    ) -> std::collections::HashMap<String, SecretMeta> {
        let Some(path) = self.secrets_path() else {
            return Default::default();
        };
        list_secrets(&path, password)
            .map(|list| list.into_iter().collect())
            .unwrap_or_default()
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

/// Whether the store may hold something under this name.
///
/// Two shapes live in one flat store, and the punctuation is what tells them
/// apart. `.` separates a desk from the name a person typed
/// (`blog.github`); `/` marks the names the program makes for itself
/// (`ssh/blog/prod/password`, `provider/deepseek`), which no script can ask
/// for. Everything else is refused, so a name cannot be made to mean a
/// different one
pub fn valid_secret_key(key: &str) -> bool {
    !key.is_empty()
        && !key.contains("..")
        && !key.starts_with(['.', '/'])
        && !key.ends_with(['.', '/'])
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
}

/// Whether a person may type this as the name of a secret.
///
/// One word: no `.` and no `/`. Those two are how the store tells a
/// desk's secrets from the program's own, so a name carrying either
/// could be made to read as something it is not
pub fn valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// The name a desk's secret is stored under: what the person typed, with
/// the desk it belongs to in front. One place decides this, because the
/// screen that writes it and the script that asks for it must agree
pub fn desk_secret_key(desk_id: &str, name: &str) -> String {
    format!("{desk_id}.{name}")
}

/// One secret's value, for the program itself.
///
/// The program's own door, the same one [`crate::caps::Capabilities::secret_value`]
/// opens -- used where there is no running app to ask, such as the settings
/// server working out whether this desk's GitHub token still works. A
/// script never arrives here, and nothing that answers a page returns what this
/// hands back
pub fn secret_value(
    path: &std::path::Path,
    password: Option<&str>,
    key: &str,
) -> Option<String> {
    read_secrets_value(path, password)
        .ok()?
        .get("tokens")?
        .get(key)?
        .as_str()
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
}

/// List of secrets (names and what they are for). **Values are never returned**
pub fn list_secrets(
    path: &std::path::Path,
    password: Option<&str>,
) -> anyhow::Result<Vec<(String, SecretMeta)>> {
    let root = read_secrets_value(path, password)?;
    let metas = root.get("meta").and_then(|v| v.as_object());
    let descs = root.get("descriptions").and_then(|v| v.as_object());
    let mut out: Vec<(String, SecretMeta)> = root
        .get("tokens")
        .and_then(|v| v.as_object())
        .map(|t| {
            t.keys()
                .map(|k| {
                    let mut m: SecretMeta = metas
                        .and_then(|m| m.get(k))
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    // A file written before secrets had anything but a line of
                    // description still has that line, and it is still the
                    // answer to "what is this"
                    if m.desc.is_empty() {
                        m.desc = descs
                            .and_then(|d| d.get(k))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                    }
                    (k.clone(), m)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Add or change a secret (write-only; once saved, the value can't be read back).
///
/// `value` empty leaves whatever is stored alone, so what a secret is *for*
/// can be changed without typing the password again -- the screen has no way
/// to show it, so asking for it to toggle a checkbox would mean going to find
/// it a second time
pub fn upsert_secret(
    path: &std::path::Path,
    password: Option<&str>,
    key: &str,
    meta: &SecretMeta,
    value: &str,
) -> anyhow::Result<()> {
    if !valid_secret_key(key) {
        anyhow::bail!(crate::i18n::t("err.config.invalid_key_chars"));
    }
    let mut root = read_secrets_value(path, password)?;
    if !root.get("tokens").map(|v| v.is_object()).unwrap_or(false) {
        root["tokens"] = serde_json::json!({});
    }
    let known = root["tokens"].get(key).and_then(|v| v.as_str()).is_some();
    if value.is_empty() && !known {
        anyhow::bail!(crate::i18n::t("webui.err.empty_value"));
    }
    if !value.is_empty() {
        root["tokens"][key] = serde_json::json!(value);
    }
    if !root.get("meta").map(|v| v.is_object()).unwrap_or(false) {
        root["meta"] = serde_json::json!({});
    }
    root["meta"][key] = serde_json::to_value(meta)?;
    // The line of description used to live on its own; keep that half in step
    // so a version that only knows the old shape still says what this is
    if !root
        .get("descriptions")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        root["descriptions"] = serde_json::json!({});
    }
    root["descriptions"][key] = serde_json::json!(meta.desc);
    write_secrets_value(path, password, &root)
}

/// Everything the store knows about one secret, except its value
pub fn secret_meta(
    path: &std::path::Path,
    password: Option<&str>,
    key: &str,
) -> anyhow::Result<Option<SecretMeta>> {
    Ok(list_secrets(path, password)?
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, m)| m))
}

/// The shape the secrets file is in. Bumped when the names inside change.
///
/// Kept in the file itself rather than beside it, because the file can be
/// carried to another machine on its own -- and a copy that arrives already
/// reshaped must not be reshaped a second time
const SECRETS_SHAPE: u64 = 2;

/// Bring a secrets file written by an earlier version up to date.
///
/// Names used to be one flat word each, and which of them a desk could
/// use was a list kept in the settings. Now the name says it: a secret a
/// script can ask for belongs to one desk and carries its name, and the
/// program's own credentials stand behind a `/`. So:
///
/// - `provider_x` and `notify_x`, which only the program ever reads, become
///   `provider/x` and `notify/x`;
/// - every other name is **copied** to `<desk>.<name>` for each
///   desk that was allowed to use it, and the original is left where it
///   is -- a copy rather than a move, because two desks may have shared
///   one, and because a name nobody listed still belongs to whoever wrote it.
///
/// A copy keeps whether an AI could use it (it could, if a desk listed
/// it) and leaves the list of sites empty, which is the one thing this cannot
/// guess: nothing records where a password was being typed. The first attempt
/// to use one says so and points at the setting.
///
/// This runs apart from the ordinary migration steps because those work on a
/// JSON file with no password; this one has to be able to open an encrypted
/// store, which only becomes possible once the person has said the word.
pub fn migrate_secrets(
    path: &std::path::Path,
    password: Option<&str>,
    desks: &[Desk],
) -> anyhow::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let mut root = read_secrets_value(path, password)?;
    if root.get("secrets_shape").and_then(|v| v.as_u64()).unwrap_or(1) >= SECRETS_SHAPE {
        return Ok(false);
    }
    let Some(tokens) = root.get("tokens").and_then(|v| v.as_object()).cloned() else {
        root["secrets_shape"] = serde_json::json!(SECRETS_SHAPE);
        write_secrets_value(path, password, &root)?;
        return Ok(false);
    };
    let descs = root.get("descriptions").and_then(|v| v.as_object()).cloned();
    let desc_of = |k: &str| {
        descs
            .as_ref()
            .and_then(|d| d.get(k))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };

    let mut renames: Vec<(String, String)> = Vec::new();
    let mut copies: Vec<(String, String, SecretMeta)> = Vec::new();
    for key in tokens.keys() {
        if key.contains('/') || key.contains('.') {
            continue; // already in the new shape
        }
        if let Some(rest) = key.strip_prefix("provider_") {
            renames.push((key.clone(), format!("provider/{rest}")));
            continue;
        }
        if let Some(rest) = key.strip_prefix("notify_") {
            renames.push((key.clone(), format!("notify/{rest}")));
            continue;
        }
        for desk in desks {
            let allowed = desk.secrets_allow_all || desk.secrets_allow.iter().any(|k| k == key);
            if !allowed || desk.id.is_empty() {
                continue;
            }
            copies.push((
                key.clone(),
                desk_secret_key(&desk.id, key),
                SecretMeta {
                    // It was already usable by whatever the desk set
                    // going, an AI's turn included. Where a password may be
                    // typed is the part that was never asked, and is asked now
                    human: true,
                    ai: true,
                    urls: Vec::new(),
                    desc: desc_of(key),
                },
            ));
        }
    }

    for (from, to) in &renames {
        let val = tokens.get(from).cloned().unwrap_or_default();
        root["tokens"][to] = val;
        if let Some(t) = root["tokens"].as_object_mut() {
            t.remove(from);
        }
        let d = desc_of(from);
        for side in ["descriptions", "meta"] {
            if let Some(m) = root.get_mut(side).and_then(|v| v.as_object_mut()) {
                m.remove(from);
            }
        }
        root["descriptions"][to] = serde_json::json!(d);
        if !root.get("meta").map(|v| v.is_object()).unwrap_or(false) {
            root["meta"] = serde_json::json!({});
        }
        root["meta"][to] = serde_json::to_value(SecretMeta { desc: d, ..Default::default() })?;
    }
    for (from, to, meta) in &copies {
        if root["tokens"].get(to).is_some() {
            continue; // somebody has already made this one
        }
        root["tokens"][to] = tokens.get(from).cloned().unwrap_or_default();
        if !root.get("meta").map(|v| v.is_object()).unwrap_or(false) {
            root["meta"] = serde_json::json!({});
        }
        root["meta"][to] = serde_json::to_value(meta)?;
        root["descriptions"][to] = serde_json::json!(meta.desc);
    }
    root["secrets_shape"] = serde_json::json!(SECRETS_SHAPE);
    write_secrets_value(path, password, &root)?;
    Ok(!renames.is_empty() || !copies.is_empty())
}

/// Delete a secret
pub fn delete_secret(
    path: &std::path::Path,
    password: Option<&str>,
    key: &str,
) -> anyhow::Result<()> {
    let mut root = read_secrets_value(path, password)?;
    if let Some(t) = root.get_mut("tokens").and_then(|v| v.as_object_mut()) {
        t.shift_remove(key);
    }
    for side in ["descriptions", "meta"] {
        if let Some(d) = root.get_mut(side).and_then(|v| v.as_object_mut()) {
            d.shift_remove(key);
        }
    }
    write_secrets_value(path, password, &root)
}

/// A desk entry inside config.json. Either inline tabs or a reference to a definition file
#[derive(Debug, Deserialize)]
pub struct DeskSpec {
    pub name: String,
    /// What this desk is called by everything that is not a person: the
    /// name its secrets are filed under, and the one that survives renaming
    /// the desk on screen. Filled in from the display name when absent,
    /// the same way a tab's is (see [`settle_desk_ids`])
    #[serde(default)]
    pub id: Option<String>,
    /// Reference to a desk definition file (e.g. "desks/projectx.json")
    #[serde(default)]
    pub file: Option<String>,
    /// Inline definition
    #[serde(default)]
    pub folders: Vec<FolderConfig>,
    /// Tabs written directly here, the way they were before folders existed.
    ///
    /// Kept because a settings file outlives the version that wrote it. When
    /// this shape stopped being read, every tab in a file from the older
    /// version stopped existing -- no error, no warning, a desk that
    /// simply opened empty. Reading them and folding them into the first
    /// folder is what upgrading should have done in the first place.
    #[serde(default)]
    pub tabs: Vec<TabConfig>,

    /// Automation shared across this desk (used when a tab doesn't specify its own)
    #[serde(default)]
    pub automation: Option<String>,
    /// Browsers opened alongside this desk. Referred to by id from automation
    #[serde(default)]
    pub browsers: Vec<BrowserConfig>,
    #[serde(default)]
    pub lua: Option<String>,
    /// Secret keys this desk's rally is allowed to use (default is empty = deny all)
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

    // Everything below is this desk's alone. There is no app answer behind any
    // of it: a desk that registered no notification destination reaches none,
    // and one that registered no model connection launches no model tab. The
    // desks exist to keep two accounts apart, and an answer inherited from the
    // app until somebody thinks to untick it is the one that walks across

    /// Notification destinations (Lua can only send to destinations registered
    /// here). A value written `@name` is read from the secret store, so a
    /// webhook is not kept in the settings file in the clear
    #[serde(default)]
    pub notify: std::collections::HashMap<String, crate::notify::Destination>,
    /// The destination an unnamed `shikisha.notify(text)` reaches. Unset with
    /// exactly one destination registered, that one serves
    #[serde(default)]
    pub primary_notify: Option<String>,
    /// Model connections (OpenAI-compatible APIs): name -> {base_url, api_key,
    /// headers}. A `model <name>/<model>` tab here looks its name up in this
    /// list and nowhere else -- the account behind a connection is billed for
    /// the work and handed the code
    #[serde(default)]
    pub providers: std::collections::HashMap<String, ProviderSpec>,
    /// What automation running here may reach outside the terminal: the named
    /// file and HTTP gateways, and the folders and hosts raw paths are allowed
    /// in. Empty means nothing
    #[serde(default)]
    pub capabilities: crate::caps::CapabilitySpec,
    /// Who may call which automation command here. Only the rows somebody
    /// changed are written; every other command answers from the defaults in
    /// `grants.rs`, so a command added later arrives with the answer its
    /// author chose
    #[serde(default)]
    pub automation_permissions: crate::grants::GrantSpec,
    /// What git does here: the branches a commit refuses to land on for the
    /// folders that have not said otherwise, and how the commit message is
    /// written
    #[serde(default)]
    pub git: GitSpec,
    /// The git accounts this desk signs in with. Which one a git tab or a
    /// project uses is chosen on that tab or project; their tokens are in the
    /// secret store (see [`git_token_key`])
    #[serde(default)]
    pub git_accounts: Vec<GitAccountSpec>,
    /// The repositories worked on in this desk. This desk's own: the same
    /// repository in another desk is another project there, with its own
    /// setup, so a change made from one desk never reaches the other.
    /// Empty until something is written down, and folders are still matched to
    /// a repository by asking git while it is
    #[serde(default)]
    pub projects: Vec<ProjectSpec>,
    /// The assistant AI this desk agreed to hand pictures to, by name
    /// ("claude", "codex", "gemini"): the part of the screen framed for the
    /// AI tools. Unset is no.
    ///
    /// A name and not a yes, because what was agreed to is sending pictures to
    /// that one. Switching the assistant AI to another company's is a new
    /// question, and a stored yes would answer it for the person
    #[serde(default)]
    pub send_pictures_to: Option<String>,
}

/// Contents of a desk definition file (desks/*.json)
#[derive(Debug, Deserialize)]
pub struct DeskFile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub folders: Vec<FolderConfig>,
    /// Tabs written directly here, the way they were before folders existed.
    ///
    /// Kept because a settings file outlives the version that wrote it. When
    /// this shape stopped being read, every tab in a file from the older
    /// version stopped existing -- no error, no warning, a desk that
    /// simply opened empty. Reading them and folding them into the first
    /// folder is what upgrading should have done in the first place.
    #[serde(default)]
    pub tabs: Vec<TabConfig>,

    /// Automation shared across this desk
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

/// AI-vs-AI (N-party) discussion settings. Per desk. Read by the built-in discussion orchestrator.
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

/// Stop conditions (the referee). Held per desk. Evaluated top to bottom; the first match wins.
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

/// A single browser opened alongside the desk
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
    /// Whether that notification carries a link back: a page with this tab's
    /// answer and a box to reply in.
    ///
    /// Off unless asked for, because it is the difference between telling
    /// somebody something and letting them answer. Whoever can reach the link
    /// can type into this tab -- which, for a subscription that forbids
    /// sharing an account, is a term of that subscription and not only a
    /// question of trust.
    #[serde(default)]
    pub notify_reply: bool,
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
    /// Everything about a built-in server connection that will not fit in
    /// `ssh://user@host:port`.
    ///
    /// Absent for every other kind of tab, and absent for a server that only
    /// needs an address and a password -- which is most of them. It exists so
    /// that the address stays readable in the one place a person looks for it
    #[serde(default)]
    pub server: Option<ServerSpec>,
    /// For a git tab: the git account it fetches, pulls and pushes with -- one
    /// of the desk's `git_accounts` by name, or [`THIS_PC`]. Chosen on the
    /// tab and never worked out for it
    #[serde(default)]
    pub git_account: Option<String>,
    /// Child tabs for display purposes (forwarding relationships are decided by Lua; this is display hierarchy only)
    #[serde(default)]
    pub children: Vec<TabConfig>,
}

/// A built-in server connection, beyond its address.
///
/// None of it is a secret. A private key is named by its path, and what opens
/// that key -- like the password -- is filed in the vault under the desk
/// and the tab, so these settings can be read, copied and shared without
/// carrying a credential with them
#[derive(Debug, Deserialize, serde::Serialize, Clone, Default, PartialEq, Eq)]
pub struct ServerSpec {
    /// A private key file to authenticate with. Absent means the stored
    /// password is used instead -- the two are a choice, not a fallback chain,
    /// so that "why did it ask for a password" always has one answer
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A server to reach this one *through*, when it is not reachable directly
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<JumpSpec>,
    /// Seconds between keepalive packets, for a network that drops a
    /// connection that has been quiet. Absent or 0 means none are sent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keepalive: Option<u64>,
    /// What to run on the far end to serve files, for a server where the file
    /// service has to be started as somebody else. Absent -- the ordinary case
    /// -- asks the server for its own file service
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_command: Option<String>,
    /// The folder the file lists open at, on the far end. Absent starts where
    /// the server puts you when you sign in
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_dir: Option<String>,
}

/// The server a connection is made through.
///
/// Its own address and its own credential: a machine that can be reached from
/// the outside, standing in front of one that cannot
#[derive(Debug, Deserialize, serde::Serialize, Clone, Default, PartialEq, Eq)]
pub struct JumpSpec {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// A folder, and the tabs that work in it.
///
/// The folder is written here and nowhere else. A tab used to carry its own,
/// which meant a reviewer could be pointed at a different folder from the tab
/// it was reviewing -- two AIs in one desk looking at different files,
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
    /// The project this folder is a piece of, by name. Absent is the old
    /// answer: work it out by asking git which checkout this folder shares a
    /// repository with
    #[serde(default)]
    pub project: Option<String>,
    /// The issue or pull request this folder was made to work on, written
    /// `issue:owner/name#12` or `pr:owner/name#12`. Lets the Issue tab say a
    /// worktree already exists for it, and open that one instead of a second
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_item: Option<String>,
    /// The machine this folder is on, by the name in `hosts`. Absent is this
    /// one. A folder somewhere else is not missing from this machine -- it was
    /// never meant to be here -- so nothing about it is repaired or offered
    #[serde(default)]
    pub host: Option<String>,
    /// The branches this folder will not commit straight onto, when it wants
    /// something other than the app-wide answer. Absent means it follows that
    /// one; an empty list means this folder guards nothing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protect: Option<Vec<String>>,
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
    /// The machine it is on, already looked up. None is this one
    pub host: Option<HostSpec>,
    /// Where its tabs start. Absent means wherever the app itself is
    pub cwd: Option<std::path::PathBuf>,
    /// What it would take to make this folder on a machine that does not have
    /// it. [`Source::Unknown`] is the case that has to ask
    pub source: Source,
    /// The branches a commit made from here refuses to land on. Already the
    /// whole answer -- what this folder said, or what the app said for the
    /// folders that said nothing -- so that nothing downstream has to know
    /// there were two places to ask
    pub protect: Vec<String>,
    /// The project it says it is a piece of, by name, when it says
    pub project: Option<String>,
    /// The issue or pull request it was made for (see [`FolderConfig::work_item`])
    pub work_item: Option<String>,
}

/// A desk resolved at launch time (tabs are flattened; depth preserves the hierarchy)
#[derive(Default)]
pub struct Desk {
    pub name: String,
    /// What automation and the secret store call this desk. Unique across
    /// the settings, and unchanged by renaming what is on screen
    pub id: String,
    /// The folders this desk works in. Always at least one, so that
    /// nothing downstream has to answer "what if a tab is in none"
    pub folders: Vec<Folder>,
    pub tabs: Vec<FlatTab>,
    /// Automation at the desk level
    pub automation: Option<String>,
    /// Browsers opened alongside it
    pub browsers: Vec<BrowserConfig>,
    /// Secret keys this desk's rally is allowed to use (default is empty = deny all)
    pub secrets_allow: Vec<String>,
    /// Allow all secrets, knowingly accepting the risk
    pub secrets_allow_all: bool,
    /// Stop conditions (the referee)
    pub stops: Vec<StopCond>,
    /// AI-vs-AI discussion settings
    pub discuss: Option<DiscussSpec>,
    /// This desk's notification destinations, as written: an `@name` value is
    /// still a name here, read from the store when the desk is handed over
    /// (see [`desk_notify`])
    pub notify: std::collections::HashMap<String, crate::notify::Destination>,
    /// Where an unnamed notify goes from here
    pub primary_notify: Option<String>,
    /// This desk's model connections, as written (see [`desk_providers`])
    pub providers: std::collections::HashMap<String, ProviderSpec>,
    /// What automation running here may reach outside the terminal
    pub capabilities: crate::caps::CapabilitySpec,
    /// Who may call which automation command here. Rows it does not mention
    /// answer from the defaults in `grants.rs`
    pub automation_permissions: crate::grants::GrantSpec,
    /// What git does here. Its `protect` has already been handed to the
    /// folders, which is where anything asks about it
    pub git: GitSpec,
    /// The git accounts this desk signs in with (see [`Desk::git_use`])
    pub git_accounts: Vec<GitAccountSpec>,
    /// This desk's projects (see [`Desk::project_of`])
    pub projects: Vec<ProjectSpec>,
    /// The assistant AI this desk agreed to hand pictures to (see
    /// [`DeskSpec::send_pictures_to`])
    pub send_pictures_to: Option<String>,
}

/// A value that may be written `@name`: the secret by that name, or the value
/// itself. A name with nothing stored under it is empty, which the far end
/// refuses in its own words rather than being handed something invented
fn deref_secret(v: &str, look: &dyn Fn(&str) -> Option<String>) -> String {
    match v.strip_prefix('@') {
        Some(k) => look(k).unwrap_or_default(),
        None => v.to_string(),
    }
}

/// This desk's notification destinations, with every `@name` read from the
/// secret store. `look` is the store's own door
pub fn desk_notify(
    desk: &Desk,
    look: &dyn Fn(&str) -> Option<String>,
) -> std::collections::HashMap<String, crate::notify::Destination> {
    let mut map = desk.notify.clone();
    for d in map.values_mut() {
        match d {
            crate::notify::Destination::Slack { webhook }
            | crate::notify::Destination::Discord { webhook } => *webhook = deref_secret(webhook, look),
            crate::notify::Destination::Telegram { token, chat_id } => {
                *token = deref_secret(token, look);
                *chat_id = deref_secret(chat_id, look);
            }
            // Nothing to keep secret: neither has an address or an account.
            crate::notify::Destination::Windows {} | crate::notify::Destination::Phone {} => {}
        }
    }
    map
}

/// This desk's model connections, resolved into what one request needs.
/// A connection with no address is left out: there is nothing to ask
pub fn desk_providers(
    desk: &Desk,
    look: &dyn Fn(&str) -> Option<String>,
) -> std::collections::HashMap<String, ProviderConn> {
    desk.providers
        .iter()
        .filter_map(|(name, p)| provider_conn(p, look).map(|c| (name.clone(), c)))
        .collect()
}

/// One connection resolved. If `headers` is unset and there is an api_key, it
/// becomes an `Authorization: Bearer` header. Key decryption happens only here,
/// in the program -- a model tab's process is handed the result
pub fn provider_conn(p: &ProviderSpec, look: &dyn Fn(&str) -> Option<String>) -> Option<ProviderConn> {
    if p.base_url.trim().is_empty() {
        return None;
    }
    let mut headers = std::collections::HashMap::new();
    if !p.headers.is_empty() {
        for (k, v) in &p.headers {
            headers.insert(k.clone(), deref_secret(v, look));
        }
    } else if let Some(key) = p.api_key.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        headers.insert("Authorization".into(), format!("Bearer {}", deref_secret(key, look)));
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

impl Desk {
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

/// The connection a named machine stands for.
///
/// The address is written the way the world writes it, the same as a tab's, so
/// there is one spelling to learn. The password is not here and never is: it
/// is filed under `ssh/host/<name>/password`, worked out from the name so that
/// nobody has to write it down twice.
pub fn host_spec(host: &HostSpec) -> anyhow::Result<crate::ssh::Spec> {
    let argv = vec![host.at.trim().to_string()];
    let (addr, port, user) = ssh_endpoint(&argv)
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::tp("err.host.address", &[("at", &host.at)])))?;
    Ok(crate::ssh::Spec {
        host: addr,
        port,
        user,
        password_key: Some(format!("ssh/host/{}/password", host.name)),
        key: None,
        passphrase_key: Some(format!("ssh/host/{}/passphrase", host.name)),
        ..Default::default()
    })
}

/// Where a tab's terminal is, when it is not on this machine.
///
/// Written the way the world writes it -- `ssh://deploy@example.com:22` -- and
/// told apart by the head of the command, the same way a browser and a docker
/// tab are. A command that merely *starts with* the word `ssh` is not this:
/// that is `ssh.exe`, a program like any other, and it keeps working exactly
/// as it did. This is the app's own connection (see [`crate::ssh`]), which is
/// what lets a stored password be sent without anybody typing it.
///
/// Nothing here is a secret: a host and a user are the address on an envelope.
/// What the password is called is worked out from the desk and the tab,
/// by whoever is launching, so that it is not something to write down twice
pub fn ssh_endpoint(argv: &[String]) -> Option<(String, u16, String)> {
    endpoint_of(argv, "ssh")
}

/// The same, for a file panel: `sftp://deploy@example.com:22`.
///
/// A panel carries its own address rather than borrowing another tab's. It was
/// the other way round for one afternoon and it was a maze: a person adding a
/// file panel found a list with nothing in it and no way to say what they
/// wanted, because the thing to fill in was on a tab they had not made yet.
/// The settings for a tab are on that tab.
///
/// Nothing is lost by it. Two tabs written to the same address share one
/// connection anyway -- [`crate::ssh::Spec::route`] is what a live connection
/// is filed under -- so a terminal and its files still travel together without
/// anybody having to say so
pub fn sftp_endpoint(argv: &[String]) -> Option<(String, u16, String)> {
    endpoint_of(argv, "sftp")
}

/// Whether this tab is the file panel at all, filled in or not.
///
/// A half-written address is still a panel: the tab has to hold its place on
/// the screen and say what it needs, rather than being handed to the launcher
/// as the name of a program
pub fn is_sftp_panel(argv: &[String]) -> bool {
    argv.first().is_some_and(|h| {
        h.len() >= 7 && h[..7].eq_ignore_ascii_case("sftp://")
    })
}

fn endpoint_of(argv: &[String], scheme: &str) -> Option<(String, u16, String)> {
    let head = argv.first()?;
    let mark = format!("{scheme}://");
    if head.len() < mark.len() || !head[..mark.len()].eq_ignore_ascii_case(&mark) {
        return None;
    }
    let rest = &head[mark.len()..];
    let (user, hostport) = rest.split_once('@')?;
    if user.trim().is_empty() {
        return None;
    }
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (hostport, 22u16),
    };
    let host = host.trim_matches(['[', ']']).trim_end_matches('/');
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port, user.to_string()))
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

/// What the git panel's commit-message button does.
///
/// Two levels on purpose. `message_hint` is **added** to the built-in prompt --
/// the rules and the diff still go, and this says the extra thing ("always in
/// English", "start with a ticket number"). Replacing the whole prompt would
/// leave the AI describing a change it was never shown, which is why the field
/// that replaces things is the Lua one: there, the caller builds the whole
/// question themselves and can reach anything automation can reach.
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct GitSpec {
    /// Added to the built-in prompt. Empty means the built-in prompt alone
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_hint: Option<String>,
    /// Lua that produces the message itself. When set, the built-in template is
    /// not used at all -- this is the whole of it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_lua: Option<String>,
    /// The branches the panel will not commit straight onto, for every folder
    /// that has not said otherwise. `*` stands for any run of characters.
    ///
    /// Absent means [`crate::git::DEFAULT_PROTECTED`]; an empty list means
    /// nothing is guarded, which is what one person working alone on their own
    /// repository is entitled to want
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protect: Option<Vec<String>>,
}

impl GitSpec {
    /// The branches to guard where nobody has said anything more specific.
    pub fn protected(&self) -> Vec<String> {
        match &self.protect {
            Some(list) => list
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            None => crate::git::DEFAULT_PROTECTED
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// Whether this tab is the git panel.
///
/// Told apart by the head of the command, the same way browser/ssh/docker are.
/// The word on its own, with nothing after it: `git status` in a tab is
/// somebody wanting a terminal that runs git, and taking that away would be
/// rude. `git` alone in a terminal only prints its own usage, so nobody meant
/// that
pub fn is_git_panel(argv: &[String]) -> bool {
    matches!(argv, [head] if head.eq_ignore_ascii_case("git"))
}

/// Whether this tab is the editor.
///
/// Told apart the same way the git panel is: the word on its own. `editor
/// notes.txt` in a tab is somebody wanting to run a program called editor, and
/// taking that away would be rude.
pub fn is_editor_panel(argv: &[String]) -> bool {
    matches!(argv, [head] if head.eq_ignore_ascii_case("editor"))
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
    /// Which of the desk's working folders this tab belongs to, and
    /// therefore where it starts
    pub folder: usize,
}

/// A 5-character stand-in for a name that has no letters of its own.
///
/// FNV-1a over the name, in base 36. Short enough to keep or to change by
/// hand. The settings screen computes the same thing in its own copy
/// (`hash5` there) so that what it offers while you type is what the app
/// settles on when it reads the file; changing one means changing both.
fn hash5(s: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    // The screen hashes UTF-16 code units, because that is what a JavaScript
    // string is made of. Matching it is the whole point of this function
    for u in s.encode_utf16() {
        h ^= u as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    let base36 = |mut n: u32| {
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut out = Vec::new();
        while n > 0 {
            out.push(digits[(n % 36) as usize]);
            n /= 36;
        }
        if out.is_empty() {
            out.push(b'0');
        }
        out.reverse();
        String::from_utf8(out).expect("base-36 digits are ASCII")
    };
    let s = format!("{:0>5}", base36(h));
    s[s.len() - 5..].to_string()
}

/// The name automation would call something, inferred from the name on screen.
///
/// Latin letters and digits become a slug (`My Tab` -> `my-tab`); a name made
/// of anything else -- Japanese, say -- has no slug to give, so it stands
/// behind a short hash instead. Same rule as the settings screen's `slugId`.
pub fn slug_id(name: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            cur.push(c);
        } else if !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    if !parts.is_empty() {
        let joined = parts.join("-");
        return joined.chars().take(24).collect();
    }
    match name.trim() {
        "" => String::new(),
        t => hash5(t),
    }
}

/// `base`, or the first of `base-2`, `base-3`... that nobody has taken.
/// The same walk the settings screen does, and the same one an imported
/// desk's folders take (see `deskpack::free_name`)
pub fn unique_id(base: &str, used: &std::collections::HashSet<String>) -> String {
    if base.is_empty() {
        return String::new();
    }
    if !used.contains(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !used.contains(c))
        .expect("some suffix is free sooner or later")
}

/// Give every tab a name automation can say, and make sure no two are the same.
///
/// Automation addresses a tab by this name and by nothing else, so a tab
/// without one could not be reached, and two tabs sharing one would send work
/// to whichever happened to be first in the list -- silently, and differently
/// after a reorder. Both are settled here, once, on the way in: a tab with no
/// name of its own is given the one its display name suggests, and a name
/// already taken gets `-2` on the end.
///
/// Returns the names that had to be moved aside, so startup can say so. The
/// settings screen fills the same field as you type; this is for the files it
/// never touched -- hand-written settings, and desks brought in from
/// somewhere else
fn settle_tab_ids(tabs: &mut [FlatTab]) -> Vec<String> {
    let mut used: std::collections::HashSet<String> = tabs
        .iter()
        .filter_map(|t| t.cfg.id.as_deref().map(str::trim).filter(|s| !s.is_empty()))
        .map(str::to_string)
        .collect();
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut moved = Vec::new();
    for t in tabs.iter_mut() {
        let written = t.cfg.id.as_deref().map(str::trim).unwrap_or("").to_string();
        if !written.is_empty() && seen.insert(written.clone()) {
            continue;
        }
        // Either nothing was written, or this is the second tab to claim it
        let base = match written.is_empty() {
            false => written.clone(),
            true => {
                let name = t.cfg.name.clone().unwrap_or_default();
                let from = match name.trim().is_empty() {
                    false => name,
                    // No name on screen either: the command is what the tab
                    // will be called, so it is what the id comes from
                    true => t.cfg.command.argv().first().cloned().unwrap_or_default(),
                };
                match slug_id(&from).is_empty() {
                    false => slug_id(&from),
                    true => "tab".into(),
                }
            }
        };
        let id = unique_id(&base, &used);
        if !written.is_empty() {
            moved.push(written);
        }
        used.insert(id.clone());
        seen.insert(id.clone());
        t.cfg.id = Some(id);
    }
    moved.sort();
    moved.dedup();
    moved
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
/// A desk with nothing written in it still has the folder everything
/// lands in, so no caller has to answer "and if it has none".
fn foldered(folders: &[FolderConfig]) -> Vec<FolderConfig> {
    let mut out = folders.to_vec();
    if out.is_empty() {
        out.push(FolderConfig::default());
    }
    out
}

/// Folders as written, plus any tabs written the old way.
///
/// The old shape put tabs beside the folders instead of inside one. Those
/// belong to the folder a person would have put them in -- the first, which is
/// the one that exists when nobody has made a second.
fn foldered_with(folders: &[FolderConfig], legacy: &[TabConfig]) -> Vec<FolderConfig> {
    let mut out = foldered(folders);
    if !legacy.is_empty() {
        out[0].tabs.extend(legacy.iter().cloned());
    }
    out
}

/// Turns written groups into ones with a real folder, and lays their tabs out
/// in one list in the order they are shown.
/// The third value is the automation names that had to be moved aside because
/// two tabs claimed the same one (see [`settle_tab_ids`])
fn resolve_folders(
    defs: &[FolderConfig],
    protect: &[String],
    hosts: &[HostSpec],
) -> (Vec<Folder>, Vec<FlatTab>, Vec<String>) {
    let mut folders = Vec::with_capacity(defs.len());
    let mut tabs = Vec::new();
    for (at, def) in defs.iter().enumerate() {
        folders.push(Folder {
            name: def.name.clone().filter(|n| !n.trim().is_empty()),
            id: def.id.clone().filter(|i| !i.trim().is_empty()),
            // Looked up once, here, so that nothing downstream has to know
            // there was a name to look up. A name nothing answers to is the
            // same as none: the folder is on this machine
            host: def
                .host
                .as_deref()
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .and_then(|h| hosts.iter().find(|x| x.name == h))
                .cloned(),
            // Relative stays relative to the settings, so that a whole folder
            // of them can be carried to another machine.
            //
            // Unless the folder is on another machine, where a path is that
            // machine's and not this one's to interpret. `/srv/api` is not an
            // absolute path on Windows, so it would be joined to a folder
            // here and the folder would then be missing -- which it is, and
            // which is beside the point
            cwd: def.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(|c| {
                let p = std::path::PathBuf::from(c);
                match p.is_absolute() || def.host.as_deref().is_some_and(|h| !h.trim().is_empty()) {
                    true => p,
                    false => root_dir().join(p),
                }
            }),
            source: def.source.as_ref().map(SourceSpec::read).unwrap_or_default(),
            // Settled here, once: a folder that has said what it guards, and
            // the app's own answer for every folder that has not
            protect: match &def.protect {
                Some(list) => list
                    .iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
                None => protect.to_vec(),
            },
            project: def.project.as_deref().map(str::trim).filter(|p| !p.is_empty()).map(str::to_string),
            work_item: def.work_item.as_deref().map(str::trim).filter(|w| !w.is_empty()).map(str::to_string),
        });
        flatten(&def.tabs, 0, at, &mut tabs);
    }
    // Every tab in the desk at once: automation reaches across folders,
    // so two folders holding a "reviewer" each is the same collision as two in one
    let moved = settle_tab_ids(&mut tabs);
    (folders, tabs, moved)
}

/// A name with the spaces taken off, or nothing when that leaves nothing.
fn one_name(s: &str) -> Option<String> {
    let t = s.trim().to_string();
    (!t.is_empty()).then_some(t)
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
    let map = root["folder_colors"].as_object_mut().expect("made just above");
    match color.trim() {
        "" => {
            map.shift_remove(&key);
        }
        c => {
            map.insert(key, serde_json::json!(c));
        }
    }
    crate::crypto::write_atomic(&path, &serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

/// Adds a folder to a desk's settings, with the same tabs as another.
///
/// This is what "work on another branch too" writes down. It edits the file
/// the person owns rather than keeping a second list of its own, so what
/// happened is visible in the settings screen afterwards and survives a
/// restart without anything else having to remember it.
///
/// Everything already in the file is left exactly as it was -- it is read as
/// values, not as our own types, so a key this version has never heard of
/// still comes out the other side.
pub fn append_folder(desk_name: &str, like: Option<&Path>, cwd: &Path, name: Option<&str>) -> Result<()> {
    append_folder_at(&config_file_path(), desk_name, like, cwd, name, &Start::Same, None)
}

/// The same, saying what the new folder runs, and which machine it is on
/// (`None` is this one). For a folder on another machine `like` is the folder
/// here it was cut from, which is only used to put it beside its family.
pub fn append_folder_starting(
    desk_name: &str,
    like: Option<&Path>,
    cwd: &Path,
    name: Option<&str>,
    start: &Start,
    host: Option<&str>,
) -> Result<()> {
    append_folder_at(&config_file_path(), desk_name, like, cwd, name, start, host)
}

/// What a folder just made should run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// The same tabs as the folder it was cut from: same faces, new branch
    Same,
    /// Nothing. The folder is there, with a + to press
    Nothing,
    /// One tab, running this. `name` is what the tab is called
    One { name: String, command: String },
}

/// The same, told which settings file to edit. Split out so it can be checked
/// against a file of its own rather than against whatever this machine has
pub fn append_folder_at(
    path: &Path,
    desk_name: &str,
    like: Option<&Path>,
    cwd: &Path,
    name: Option<&str>,
    start: &Start,
    host: Option<&str>,
) -> Result<()> {
    let host = host.map(str::trim).filter(|h| !h.is_empty());
    with_folders(path, desk_name, |folders| {
        // The tabs to bring along: whoever is already working in the folder
        // this was asked for from. Same faces, new branch -- unless the ask
        // said what should run instead
        let tabs = match (host, start) {
            (_, Start::Nothing) => serde_json::json!([]),
            // Every tab in a folder on another machine is a terminal on that
            // machine, whatever its command says. Copying the faces from here
            // would put an AI's name on a plain shell, so it gets one terminal
            // named for the machine it is on
            (Some(h), _) => serde_json::json!([{ "name": h, "command": "sh" }]),
            (None, Start::Same) => like
                .and_then(|want| {
                    folders.iter().find(|g| {
                        g.get("cwd")
                            .and_then(|c| c.as_str())
                            .map(resolve_folder_cwd)
                            .is_some_and(|c| c == want)
                    })
                })
                .and_then(|g| g.get("tabs").cloned())
                .unwrap_or_else(|| serde_json::json!([])),
            (None, Start::One { name, command }) => serde_json::json!([{ "name": name, "command": command }]),
        };
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
        // The machine it is on. Without it the folder reads as one on this
        // machine: its tabs would start here, in a path that only exists there
        if let Some(h) = host {
            folder["host"] = serde_json::json!(h);
        }
        // What it would take to make this folder again, written now rather than
        // asked for later. Nobody remembers which branch a folder held six
        // weeks ago, and the label above cannot be turned back into one -- it
        // flattens `work/2` and `work-2` to the same word. Only here: a folder
        // on another machine is never made again from this one
        if let (None, Some(branch), Some(from)) = (host, name, like)
            && let Some(url) = crate::repo::remote_url_of(from) {
                folder["source"] = serde_json::to_value(SourceSpec::worktree(
                    &crate::folders::scrub(&url),
                    branch,
                    &crate::worktree::default_base(from),
                ))?;
            }
        // Beside the folders it belongs with. A branch of one project written
        // after an unrelated one reads as unrelated: the list is drawn in the
        // order this is written in, and a family that is not next to itself is
        // a family nobody can see. A path on another machine cannot be asked
        // about here, so there the family is the one of the folder it was cut from
        let family = match host {
            Some(_) => like.and_then(crate::repo::family_of),
            None => crate::repo::family_of(cwd),
        };
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
pub fn rename_folder(desk_name: &str, cwd: &Path, name: &str) -> Result<()> {
    with_folders(&config_file_path(), desk_name, |folders| {
        let Some(g) = find_folder(folders, cwd) else {
            return Ok(());
        };
        match name.trim() {
            "" => {
                if let Some(o) = g.as_object_mut() {
                    o.shift_remove("name");
                }
            }
            n => g["name"] = serde_json::json!(n),
        }
        Ok(())
    })
}

/// Writes down where a folder came from, so no machine has to ask again.
///
/// Called at the moment a folder is made, and once more for a folder that was
/// written by hand and had to be asked about. Both write the same thing, so
/// "answered once" and "made by us" leave the settings in the same state and
/// every machine after this one is silent.
pub fn set_folder_source(desk_name: &str, cwd: &Path, source: &SourceSpec) -> Result<()> {
    set_folder_source_at(&config_file_path(), desk_name, cwd, source)
}

/// The same, told which settings file to edit.
pub fn set_folder_source_at(
    path: &Path,
    desk_name: &str,
    cwd: &Path,
    source: &SourceSpec,
) -> Result<()> {
    with_folders(path, desk_name, |folders| {
        let Some(g) = find_folder(folders, cwd) else {
            return Ok(());
        };
        g["source"] = serde_json::to_value(source)?;
        Ok(())
    })
}

/// Takes a folder out of the list, and its tabs with it.
///
/// The folder on disk is not touched. Closing a thing on screen and deleting
/// somebody's work are different acts, and only one of them can be undone by
/// opening it again.
pub fn remove_folder(desk_name: &str, cwd: &Path) -> Result<()> {
    with_folders(&config_file_path(), desk_name, |folders| {
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

/// Which tab a line in the settings has to be, to be the one meant.
///
/// A tab is found by where it stands, and then checked: the file is a person's
/// own and can have changed since it was read, and taking out whatever now
/// stands in that place would be taking out somebody else
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabMark {
    /// The name automation calls it, as settled on reading (see
    /// `settle_tab_ids`) -- so it is known even when the file never wrote one
    pub id: Option<String>,
    pub name: Option<String>,
    pub argv: Vec<String>,
    /// The folder it works in. Copies of one folder's tabs carry the same name
    /// and the same command, and only this tells them apart
    pub folder: Option<std::path::PathBuf>,
}

impl TabMark {
    pub fn of(desk: &Desk, t: &FlatTab) -> Self {
        Self {
            id: t.cfg.id.clone(),
            name: t.cfg.name.clone(),
            argv: t.cfg.command.argv(),
            folder: desk.cwd_of(t),
        }
    }

    /// Whether this folder is the one it works in
    fn works_in(&self, group: &serde_json::Value) -> bool {
        let here = group
            .get("cwd")
            .and_then(|c| c.as_str())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(resolve_folder_cwd);
        here == self.folder
    }

    /// Whether this line is that tab. A line that wrote its own id is that id
    /// and nothing else; one that did not is its name and its command
    fn fits(&self, line: &serde_json::Value) -> bool {
        let written = line.get("id").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
        match written {
            Some(id) => self.id.as_deref() == Some(id),
            None => {
                let name = line.get("name").and_then(|v| v.as_str()).map(str::to_string);
                let argv = line
                    .get("command")
                    .and_then(|c| serde_json::from_value::<CommandSpec>(c.clone()).ok())
                    .map(|c| c.argv())
                    .unwrap_or_default();
                name == self.name && argv == self.argv
            }
        }
    }
}

/// A tab taken out of the settings, with what it takes to put it back where it
/// stood.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct TakenTab {
    /// The line itself, as the person wrote it, without its children -- they
    /// stay behind (see `take_tab_at`)
    pub line: serde_json::Value,
    /// The folder it was working in, as the settings spell it. Absent for the
    /// folder that names none
    #[serde(default)]
    pub folder: Option<String>,
    /// Where in that folder: its place in the list, then in its parent's
    /// children, outermost first
    pub path: Vec<usize>,
    /// What it answered to when it was taken, so the same name can be given
    /// back to a line that never wrote one
    #[serde(default)]
    pub id: Option<String>,
}

/// Takes one tab out of a desk's settings.
///
/// `written` is its position among the desk's tabs as they are read
/// (`Desk::tabs`), and `mark` says what should be standing there. The line
/// goes; its children do not. They move up into its place, in their order,
/// because closing one tab is not closing the tabs drawn under it -- that
/// indent is only how they are shown.
pub fn take_tab(desk_name: &str, written: usize, mark: &TabMark) -> Result<TakenTab> {
    take_tab_at(&config_file_path(), desk_name, written, mark)
}

/// The same, told which settings file to edit.
pub fn take_tab_at(path: &Path, desk_name: &str, written: usize, mark: &TabMark) -> Result<TakenTab> {
    let mut taken = None;
    with_folders(path, desk_name, |folders| {
        // Every line, in the order the desk reads them: folder by folder, each
        // tab followed by its children
        fn walk(list: &[serde_json::Value], at: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
            for (i, line) in list.iter().enumerate() {
                at.push(i);
                out.push(at.clone());
                if let Some(kids) = line.get("children").and_then(|c| c.as_array()) {
                    walk(kids, at, out);
                }
                at.pop();
            }
        }
        let mut spots: Vec<(usize, Vec<usize>)> = Vec::new();
        for (fi, g) in folders.iter().enumerate() {
            let mut paths = Vec::new();
            if let Some(list) = g.get("tabs").and_then(|t| t.as_array()) {
                walk(list, &mut Vec::new(), &mut paths);
            }
            spots.extend(paths.into_iter().map(|p| (fi, p)));
        }
        let line_at = |fi: usize, p: &[usize]| -> Option<&serde_json::Value> {
            let mut list = folders.get(fi)?.get("tabs")?.as_array()?;
            let (last, up) = p.split_last()?;
            for i in up {
                list = list.get(*i)?.get("children")?.as_array()?;
            }
            list.get(*last)
        };
        // Where it should be, and failing that, wherever it is now -- the file
        // may have gained or lost a tab above it since it was read (two tabs
        // closed in a row are exactly that)
        let is_it = |(fi, p): &&(usize, Vec<usize>)| {
            mark.works_in(&folders[*fi]) && line_at(*fi, p).is_some_and(|l| mark.fits(l))
        };
        let (fi, p) = spots
            .get(written)
            .filter(is_it)
            .or_else(|| spots.iter().find(is_it))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.tab.not_in_settings")))?;
        let folder = folders[fi].get("cwd").and_then(|c| c.as_str()).map(str::to_string);
        let mut list = folders[fi]["tabs"].as_array_mut().expect("walked just above");
        let (last, up) = p.split_last().expect("a spot always has a place");
        for i in up {
            list = list[*i]["children"].as_array_mut().expect("walked just above");
        }
        let mut line = list.remove(*last);
        let kids = line
            .as_object_mut()
            .and_then(|o| o.shift_remove("children"))
            .and_then(|c| match c {
                serde_json::Value::Array(a) => Some(a),
                _ => None,
            })
            .unwrap_or_default();
        for (n, kid) in kids.into_iter().enumerate() {
            list.insert(*last + n, kid);
        }
        taken = Some(TakenTab { line, folder, path: p, id: mark.id.clone() });
        Ok(())
    })?;
    taken.ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.tab.not_in_settings")))
}

/// Puts a tab taken out of the settings back where it stood. Returns the name
/// automation will call it.
///
/// Its folder is found by path; a folder that has been closed since is opened
/// again, since that is where the tab works. Its place is kept as far as the
/// list still reaches -- a parent that has gone leaves it at the level that is
/// still there. And it is given a name nobody else is using: two tabs answering
/// to one name would hand the other one's work to whichever came first, and
/// reading the settings settles such a clash by renaming whichever stands later
/// -- which could be the tab that never left.
pub fn put_tab_back(desk_name: &str, taken: &TakenTab) -> Result<String> {
    put_tab_back_at(&config_file_path(), desk_name, taken)
}

/// The same, told which settings file to edit.
pub fn put_tab_back_at(path: &Path, desk_name: &str, taken: &TakenTab) -> Result<String> {
    let mut given = String::new();
    with_folders(path, desk_name, |folders| {
        fn ids(list: &[serde_json::Value], out: &mut std::collections::HashSet<String>) {
            for line in list {
                if let Some(id) = line.get("id").and_then(|v| v.as_str()).map(str::trim)
                    && !id.is_empty()
                {
                    out.insert(id.to_string());
                }
                if let Some(kids) = line.get("children").and_then(|c| c.as_array()) {
                    ids(kids, out);
                }
            }
        }
        let mut used = std::collections::HashSet::new();
        for g in folders.iter() {
            if let Some(list) = g.get("tabs").and_then(|t| t.as_array()) {
                ids(list, &mut used);
            }
        }
        let mut line = taken.line.clone();
        if !line.is_object() {
            anyhow::bail!(crate::i18n::t("err.tab.not_in_settings"));
        }
        let written = line.get("id").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
        let base = written.map(str::to_string).or_else(|| taken.id.clone()).unwrap_or_else(|| {
            let name = line.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            match slug_id(&name).is_empty() {
                false => slug_id(&name),
                true => "tab".into(),
            }
        });
        given = unique_id(&base, &used);
        line["id"] = serde_json::json!(given);

        let home = taken.folder.as_deref().map(resolve_folder_cwd);
        let at = folders.iter().position(|g| {
            g.get("cwd").and_then(|c| c.as_str()).map(resolve_folder_cwd) == home
        });
        let fi = match (at, &taken.folder) {
            (Some(i), _) => i,
            (None, Some(c)) => {
                folders.push(serde_json::json!({ "cwd": c, "tabs": [] }));
                folders.len() - 1
            }
            // The folder that names no path is the first one, and there is
            // always one to put it in
            (None, None) => {
                if folders.is_empty() {
                    folders.push(serde_json::json!({ "tabs": [] }));
                }
                0
            }
        };
        if !folders[fi].get("tabs").is_some_and(|t| t.is_array()) {
            folders[fi]["tabs"] = serde_json::json!([]);
        }
        let mut list = folders[fi]["tabs"].as_array_mut().expect("made just above");
        let (last, up) = taken.path.split_last().map(|(l, u)| (*l, u)).unwrap_or((usize::MAX, &[]));
        for i in up {
            if list.get(*i).is_none_or(|l| !l.is_object()) {
                break;
            }
            if !list[*i].get("children").is_some_and(|c| c.is_array()) {
                list[*i]["children"] = serde_json::json!([]);
            }
            list = list[*i]["children"].as_array_mut().expect("made just above");
        }
        let at = last.min(list.len());
        list.insert(at, line);
        Ok(())
    })?;
    Ok(given)
}

/// Takes a page out of a desk's `browsers` list -- the older way of opening
/// one beside the desk, still read. Returns where it stood and the line itself.
pub fn take_browser(desk_name: &str, id: &str) -> Result<(usize, serde_json::Value)> {
    take_browser_at(&config_file_path(), desk_name, id)
}

pub fn take_browser_at(path: &Path, desk_name: &str, id: &str) -> Result<(usize, serde_json::Value)> {
    with_listed_desk(path, desk_name, |w| {
        let list = w
            .get_mut("browsers")
            .and_then(|b| b.as_array_mut())
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.tab.not_in_settings")))?;
        let at = list
            .iter()
            .position(|b| b.get("id").and_then(|v| v.as_str()) == Some(id))
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.tab.not_in_settings")))?;
        Ok((at, list.remove(at)))
    })
}

/// Puts such a page back where it stood.
pub fn put_browser_back(desk_name: &str, at: usize, line: &serde_json::Value) -> Result<()> {
    put_browser_back_at(&config_file_path(), desk_name, at, line)
}

pub fn put_browser_back_at(path: &Path, desk_name: &str, at: usize, line: &serde_json::Value) -> Result<()> {
    with_listed_desk(path, desk_name, |w| {
        if !w.get("browsers").is_some_and(|b| b.is_array()) {
            w["browsers"] = serde_json::json!([]);
        }
        let list = w["browsers"].as_array_mut().expect("made just above");
        list.insert(at.min(list.len()), line.clone());
        Ok(())
    })
}

/// One entry of the settings' `desks` list, handed over to be changed and
/// written back. Only for what lives on that entry itself rather than in the
/// file it may name -- the folders go through `with_folders`
fn with_listed_desk<T>(
    path: &Path,
    desk_name: &str,
    edit: impl FnOnce(&mut serde_json::Value) -> Result<T>,
) -> Result<T> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into());
    let mut root: serde_json::Value = serde_json::from_str(without_bom(&text)).with_context(|| {
        crate::i18n::tp("err.config.json_invalid", &[("path", &path.display().to_string())])
    })?;
    let w = root
        .get_mut("desks")
        .and_then(|w| w.as_array_mut())
        .and_then(|a| a.iter_mut().find(|w| w.get("name").and_then(|n| n.as_str()) == Some(desk_name)))
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.worktree.no_desk")))?;
    let out = edit(w)?;
    crate::crypto::write_atomic(path, &serde_json::to_string_pretty(&root)?)?;
    Ok(out)
}

/// Opens a desk's groups, hands them over to be changed, and writes the
/// result back where it came from.
///
/// One way in, because there are three things that change a group and each of
/// them would otherwise carry its own copy of "find the desk, follow it
/// to the file it lives in, fold the loose tabs, write it out atomically".
fn with_folders(
    path: &Path,
    desk_name: &str,
    edit: impl FnOnce(&mut Vec<serde_json::Value>) -> Result<()>,
) -> Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into());
    // A file that is there and cannot be read is a person's settings halfway
    // through an edit, not an empty file: writing an answer over it would
    // throw away everything else in it
    let mut root: serde_json::Value = serde_json::from_str(without_bom(&text)).with_context(|| {
        crate::i18n::tp("err.config.json_invalid", &[("path", &path.display().to_string())])
    })?;

    // A desk kept in a file of its own is edited there; the entry in the
    // settings only names it
    let mut file_at: Option<std::path::PathBuf> = None;
    {
        let list = root
            .get("desks")
            .and_then(|w| w.as_array())
            .map(|a| a.to_vec())
            .unwrap_or_default();
        for w in list {
            if w.get("name").and_then(|n| n.as_str()) == Some(desk_name) {
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

    // The object that holds the groups: the desk's own, the file it names,
    // or the settings themselves when no desk was ever made. That last one is
    // read as a desk called DEFAULT and asked for by that name, so a file with
    // no list of desks answers to any name at all
    let unlisted = root
        .get("desks")
        .and_then(|w| w.as_array())
        .is_none_or(|a| a.is_empty());
    let holder: &mut serde_json::Value = match (&mut side, desk_name.is_empty() || unlisted) {
        (Some(v), _) => v,
        (None, true) => &mut root,
        (None, false) => root
            .get_mut("desks")
            .and_then(|w| w.as_array_mut())
            .and_then(|a| {
                a.iter_mut()
                    .find(|w| w.get("name").and_then(|n| n.as_str()) == Some(desk_name))
            })
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.worktree.no_desk")))?,
    };
    ensure_folders(holder);

    let folders = holder
        .get_mut("folders")
        .and_then(|g| g.as_array_mut())
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.worktree.no_desk")))?;
    edit(folders)?;

    let out = |v: &serde_json::Value| serde_json::to_string_pretty(v).unwrap_or_default();
    match (&side, &file_at) {
        (Some(v), Some(p)) => crate::crypto::write_atomic(p, &out(v))?,
        _ => crate::crypto::write_atomic(path, &out(&root))?,
    }
    Ok(())
}

/// Makes sure there is a list of folders to put something in.
///
/// Tabs written the old way -- beside the folders rather than inside one --
/// are read as the first folder's (`foldered_with`). They are written into
/// it here, before anything is added, so that a folder added after them does
/// not become "the first" and take them: adding an empty folder to a
/// desk written the old way used to move every tab it had into it.
pub(crate) fn ensure_folders(holder: &mut serde_json::Value) {
    if !holder.get("folders").map(|f| f.is_array()).unwrap_or(false) {
        holder["folders"] = serde_json::json!([]);
    }
    let legacy = holder
        .as_object_mut()
        .and_then(|o| o.shift_remove("tabs"))
        .and_then(|t| t.as_array().cloned())
        .filter(|t| !t.is_empty());
    if let Some(mut legacy) = legacy {
        let folders = holder["folders"].as_array_mut().expect("made just above");
        if folders.is_empty() {
            folders.push(serde_json::json!({ "tabs": [] }));
        }
        match folders[0].get_mut("tabs").and_then(|t| t.as_array_mut()) {
            Some(had) => had.append(&mut legacy),
            None => folders[0]["tabs"] = serde_json::Value::Array(legacy),
        }
    }
}

/// The tabs of the working folder at this path, making one if there is none.
/// One answer to "where does a tab go", used by everything that adds one
fn folder_tabs_at<'a>(holder: &'a mut serde_json::Value, cwd: Option<&Path>) -> &'a mut Vec<serde_json::Value> {
    ensure_folders(holder);
    let folders = holder["folders"].as_array_mut().expect("made just above");
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
    folders[at]["tabs"].as_array_mut().expect("tabs is an array")
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

/// Write down which issue or pull request a folder was made for
pub fn set_folder_work_item(desk_name: &str, cwd: &Path, item: &str) -> Result<()> {
    with_folders(&config_file_path(), desk_name, |folders| {
        let found = folders.iter_mut().find(|g| {
            g.get("cwd")
                .and_then(|c| c.as_str())
                .map(resolve_folder_cwd)
                .is_some_and(|c| crate::uistate::same_folder(&c, cwd))
        });
        if let Some(obj) = found.and_then(|g| g.as_object_mut()) {
            obj.insert("work_item".into(), serde_json::Value::String(item.to_string()));
        }
        Ok(())
    })
}

/// A group's folder as an absolute path, the same way launching resolves it.
pub fn resolve_folder_cwd(c: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(c.trim());
    match p.is_absolute() {
        true => p,
        false => root_dir().join(p),
    }
}

/// The names one written path may be stored under, best first.
///
/// The folder was called `projects` before it was called `desks`, and
/// settings written under either name are still out there, so a path naming
/// one is also looked for under the other. Kept apart from the looking so it
/// can be checked without a folder to look in -- the check used to change the
/// process's own working folder to make one, which every other test running
/// beside it then saw
fn data_path_candidates(p: &str) -> Vec<String> {
    let mut out = vec![p.to_string()];
    if let Some(rest) = p.strip_prefix("projects/") {
        out.push(format!("desks/{rest}"));
    } else if let Some(rest) = p.strip_prefix("desks/") {
        out.push(format!("projects/{rest}"));
    }
    out
}

/// Where a relative path written in the settings -- an automation folder, a
/// desk file -- is looked for, in order:
///
/// 1. the layout root (`root_dir`): the person's own folder, where the
///    settings screen writes and where a carried-over `scripts\` lands;
/// 2. beside the exe: what ships with the program, such as the examples.
///    The same place as 1 for the download, and the read-only package
///    folder for the Store copy;
/// 3. the working folder, for a path typed relative to wherever the program
///    was started from.
///
/// Nothing found means a place to make it, and that is the root: the one of
/// the three that is always writable. The Store copy used to look only in 2
/// and 3, so a script under `%LOCALAPPDATA%\SHIKISHA-TERM\scripts` was never
/// found, and a folder the settings screen made went to the working folder
/// -- `C:\Windows\System32` when started from the Start menu.
///
/// Configs pointing at the old projects/ name also fall back to desks/ (compat)
pub fn resolve_data_path(p: &str) -> std::path::PathBuf {
    let candidates = data_path_candidates(p);
    let root = root_dir();
    let mut dirs = vec![root.clone()];
    let exe = exe_dir();
    if exe != root {
        dirs.push(exe);
    }
    for cand in &candidates {
        for dir in &dirs {
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
    root.join(p)
}

impl Config {
    /// Resolve the desk definitions.
    /// If desks isn't defined, inline tabs are treated as a single unnamed desk
    pub fn resolve_desks(&self) -> (Vec<Desk>, Vec<String>) {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        if self.desks.is_empty() {
            // Tabs written the old way, with no folder around them, are still
            // a screenful of work somebody arranged
            if !self.folders.is_empty() || !self.tabs.is_empty() {
                let git = GitSpec::default();
                let (folders, tabs, moved) =
                    resolve_folders(&foldered_with(&self.folders, &self.tabs), &git.protected(), &self.hosts);
                errors.extend(moved_note("DEFAULT", &moved));
                out.push(Desk {
                    name: "DEFAULT".into(),
                    id: String::new(),
                    folders,
                    tabs,
                    automation: None,
                    browsers: Vec::new(),
                    secrets_allow: Vec::new(),
                    secrets_allow_all: false,
                    stops: Vec::new(),
                    discuss: None,
                    // A screenful written before desks existed has nothing of
                    // its own registered, and there is no app answer to lend it
                    notify: Default::default(),
                    primary_notify: None,
                    providers: Default::default(),
                    capabilities: Default::default(),
                    automation_permissions: Default::default(),
                    projects: Vec::new(),
                    git,
                    git_accounts: Vec::new(),
                    send_pictures_to: None,
                });
            }
            return (out, errors);
        }
        for desk in &self.desks {
            #[allow(clippy::type_complexity)]
            #[allow(clippy::type_complexity)]
            let (folder_defs, file_name, file_lua, file_secrets, file_stops, file_discuss): (
                Vec<FolderConfig>,
                Option<String>,
                Option<String>,
                (Vec<String>, bool),
                Vec<StopCond>,
                Option<DiscussSpec>,
            ) = match &desk.file {
                Some(f) => match read_json::<DeskFile>(&resolve_data_path(f)) {
                    Ok(p) => (
                        foldered_with(&p.folders, &p.tabs),
                        p.name,
                        p.automation.or(p.lua),
                        (p.secrets_allow, p.secrets_allow_all),
                        p.stops,
                        p.discuss,
                    ),
                    Err(e) => {
                        errors.push(format!("{}: {e:#}", desk.name));
                        continue;
                    }
                },
                None => (
                    foldered_with(&desk.folders, &desk.tabs),
                    None,
                    None,
                    (Vec::new(), false),
                    Vec::new(),
                    None,
                ),
            };
            // This desk's git settings. Its protected branches go to the
            // folders here, so a folder still has the one answer it has always
            // had -- its own, or the one handed down to it by its desk
            let git = desk.git.clone();
            let (folders, tabs, moved) = resolve_folders(&folder_defs, &git.protected(), &self.hosts);
            // Prefer the display name from config; fall back to the definition file's name if empty
            let name = if desk.name.is_empty() {
                file_name.unwrap_or_else(|| "UNNAMED".into())
            } else {
                desk.name.clone()
            };
            errors.extend(moved_note(&name, &moved));
            out.push(Desk {
                name,
                id: desk.id.clone().unwrap_or_default(),
                folders,
                tabs,
                // Prefer config's setting; fall back to the definition file's if absent
                automation: desk.automation.clone().or_else(|| desk.lua.clone()).or(file_lua),
                browsers: desk.browsers.clone(),
                // Prefer config's setting; fall back to the definition file's if absent
                secrets_allow: if desk.secrets_allow.is_empty() {
                    file_secrets.0
                } else {
                    desk.secrets_allow.clone()
                },
                secrets_allow_all: desk.secrets_allow_all || file_secrets.1,
                // Prefer config's setting; fall back to the definition file's if absent
                stops: if desk.stops.is_empty() { file_stops } else { desk.stops.clone() },
                discuss: desk.discuss.clone().or(file_discuss),
                notify: desk.notify.clone(),
                primary_notify: desk.primary_notify.as_deref().and_then(one_name),
                providers: desk.providers.clone(),
                capabilities: desk.capabilities.clone(),
                automation_permissions: desk.automation_permissions.clone(),
                git,
                git_accounts: desk.git_accounts.clone(),
                projects: desk.projects.clone(),
                send_pictures_to: desk.send_pictures_to.as_deref().and_then(one_name),
            });
        }
        errors.extend(settle_desk_ids(&mut out));
        (out, errors)
    }
}

/// Give every desk a name that is not the one on screen, and make sure no
/// two are the same.
///
/// The display name is a label a person is free to change and free to reuse --
/// two desks called "production" is nobody's mistake. What a desk's secrets
/// are filed under cannot work that way, so it is settled here: unique across
/// the settings, inferred from the display name when nothing was written, and
/// left alone once it exists. Renaming the desk on screen after that costs
/// nothing; changing this is what costs a re-entry of its passwords.
///
/// Returns a note for each one that had to be moved aside
fn settle_desk_ids(list: &mut [Desk]) -> Vec<String> {
    let mut used: std::collections::HashSet<String> = list
        .iter()
        .map(|w| w.id.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut notes = Vec::new();
    for w in list.iter_mut() {
        let written = w.id.trim().to_string();
        if !written.is_empty() && seen.insert(written.clone()) {
            w.id = written;
            continue;
        }
        let base = match written.is_empty() {
            false => written.clone(),
            true => match slug_id(&w.name) {
                s if s.is_empty() => "desk".into(),
                s => s,
            },
        };
        let id = unique_id(&base, &used);
        if !written.is_empty() {
            notes.push(crate::i18n::tp(
                "err.desk.duplicate_ids",
                &[("name", &w.name), ("old", &written), ("new", &id)],
            ));
        }
        used.insert(id.clone());
        seen.insert(id.clone());
        w.id = id;
    }
    notes
}

/// What to say when two tabs in one desk claimed the same automation name
fn moved_note(desk: &str, moved: &[String]) -> Vec<String> {
    match moved.is_empty() {
        true => Vec::new(),
        false => vec![crate::i18n::tp(
            "err.desk.duplicate_ids.tabs",
            &[("desk", desk), ("names", &moved.join(", "))],
        )],
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
#[cfg(not(windows))]
pub fn packaged() -> bool {
    // There is no Store to be installed from here
    false
}

#[cfg(windows)]
pub fn packaged() -> bool {
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
/// data, logs, desks.
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
///
/// A copy installed on Linux is in the same position and answered the same
/// way: see [`unpacked_root`].
pub fn root_dir() -> std::path::PathBuf {
    use std::sync::OnceLock;
    static ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        if packaged()
            && let Some(local) = std::env::var_os("LOCALAPPDATA")
        {
            return std::path::PathBuf::from(local).join("SHIKISHA-TERM");
        }
        #[cfg(unix)]
        {
            return unpacked_root(&exe_dir(), |k| std::env::var_os(k));
        }
        #[allow(unreachable_code)]
        exe_dir()
    })
    .clone()
}

/// The same question on Linux, where "beside the program" is often nowhere to
/// write.
///
/// A folder someone unpacked and runs out of keeps the portable promise: its
/// settings are the ones sitting beside it. A copy installed by `install.sh`
/// is at `/usr/local/bin/shikisha-serve`, which belongs to root, so its things
/// go where a person's things go on this system -- `XDG_DATA_HOME`, and
/// `~/.local/share` when that is not set. `SHIKISHA_HOME` overrides both, for
/// running several boxes on one machine.
///
/// Taking the environment as an argument so this can be asked what it would
/// answer, rather than only what it answers here.
#[cfg(unix)]
fn unpacked_root(
    beside: &std::path::Path,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> std::path::PathBuf {
    if let Some(told) = env("SHIKISHA_HOME").filter(|s| !s.is_empty()) {
        return std::path::PathBuf::from(told);
    }
    if beside.join("config").is_dir() || beside.join("config.json").is_file() {
        return beside.to_path_buf();
    }
    let data = env("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| env("HOME").map(|h| std::path::PathBuf::from(h).join(".local").join("share")));
    match data {
        Some(d) => d.join("shikisha"),
        // No home to speak of. Beside the program is where it has always been,
        // and a failure to write there is at least a failure in one place
        None => beside.to_path_buf(),
    }
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
    // The folder holding desk definition files is named after the desk, and the
    // desk used to be called something else. Moved rather than read from both
    // places: two folders that mean the same thing is how one of them quietly
    // stops being the one the app looks in
    let (was, now) = (root.join("workspaces"), root.join("desks"));
    if was.is_dir() && !now.exists() {
        let _ = std::fs::rename(&was, &now);
    }
    // The desk that was open last is remembered under the same word, and a
    // start that cannot find it opens the first desk instead of the one the
    // person left
    let data = root.join("data");
    let (was, now) = (data.join("last-workspace"), data.join("last-desk"));
    if was.is_file() && !now.exists() {
        let _ = std::fs::rename(&was, &now);
    }
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
    if old_s.exists() && !new_s.exists()
        && std::fs::rename(&old_s, &new_s).is_err() && std::fs::copy(&old_s, &new_s).is_ok() {
            let _ = std::fs::remove_file(&old_s);
        }
}

/// Path to the config file the web GUI edits.
/// Returns the existing file's path if present, otherwise the path where a new one would be created beside the exe.
/// Home for state files (ones a human doesn't edit), gathered under the root's data folder.
/// The exe is the only file directly at the root (folders are config / data / logs / lang / desks / scripts)
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

/// Home for the name of the last-open desk.
///
/// Not written back into config.json -- that would interrupt a user mid-edit,
/// and the change-watcher would react to its own write and trigger a reload
fn last_desk_path() -> std::path::PathBuf {
    state_path("last-desk")
}

/// Name of the last-open desk
pub fn load_last_desk() -> Option<String> {
    let s = std::fs::read_to_string(last_desk_path()).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Remember the name of the currently open desk. Fails silently if it can't
/// (being unable to remember it is no reason for things to stop working)
pub fn save_last_desk(name: &str) {
    let _ = crate::crypto::write_atomic(&last_desk_path(), name);
}

/// Write one appearance value back into the settings file, leaving the rest of
/// it exactly as the person wrote it.
///
/// Read-modify-write on the parsed JSON rather than serialising our own idea of
/// the config: a settings file is a person's own document, with their key order
/// and anything we do not know about still in it. Rewriting it wholesale to
/// record a font size would be a poor trade.
/// Add a tab to one desk and write the settings back.
///
/// The reopen the Vault performs: a resumed conversation becomes a real tab in
/// the desk, the way dragging a past session into a desk makes it a
/// member of it. Read-modify-write on the parsed JSON, like every other change
/// here, so the person's own file keeps its shape and its order -- the new tab
/// simply lands at the end of the desk it was reopened into.
///
/// Returns whether it was written. A desk that has vanished since the
/// page listed it is a false rather than a new tab in the wrong place.
pub fn append_tab(desk: &str, tab: serde_json::Value, cwd: Option<&Path>) -> bool {
    let path = config_file_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        crate::append_hook_log("could not reopen into a tab: settings are not readable");
        return false;
    };
    let Some(list) = doc.get_mut("desks").and_then(|w| w.as_array_mut()) else {
        return false;
    };
    let Some(desk) = list
        .iter_mut()
        .find(|w| w.get("name").and_then(|n| n.as_str()) == Some(desk))
    else {
        return false;
    };
    folder_tabs_at(desk, cwd).push(tab);
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
/// Walk every tab the settings file holds, and write the aim onto the one that
/// answers to `tab_name`.
///
/// Recursive because tabs nest: a desk holds working folders, a folder
/// holds tabs, and a tab holds children. Written as one walk over anything
/// called "tabs" rather than as a list of the places to look, because that
/// list was already out of date -- it knew the flat `tabs` and a desk's
/// own, and not the working folder that every tab made today lands in, so an
/// aim chosen on screen was never written down at all.
///
/// The tab is found by its automation name only, the same as everywhere else
/// (see `hooks::TabKey`): matching the name on screen as well would let an aim
/// land on a stranger that happens to be *called* what this one is *addressed* as
fn write_aim(v: &mut serde_json::Value, tab_name: &str, target: Option<&str>, written: &mut bool) {
    match v {
        serde_json::Value::Array(list) => {
            for item in list {
                write_aim(item, tab_name, target, written);
            }
        }
        serde_json::Value::Object(obj) => {
            let named =
                obj.get("id").and_then(|v| v.as_str()).map(str::trim) == Some(tab_name);
            if named {
                match target {
                    Some(t) => {
                        obj.insert("drives".into(), serde_json::Value::String(t.to_string()));
                    }
                    // Cleared aims leave nothing behind: an empty key in a
                    // person's file is a question they would have to answer
                    // for themselves
                    None => {
                        obj.shift_remove("drives");
                    }
                }
                *written = true;
            }
            for (k, child) in obj.iter_mut() {
                if matches!(k.as_str(), "tabs" | "children" | "folders" | "desks") {
                    write_aim(child, tab_name, target, written);
                }
            }
        }
        _ => {}
    }
}

/// Where a git account was chosen from: a git tab, by the name the board knows
/// it by, or the column beside a folder, which chooses for that folder's project
pub enum GitChoiceAt<'a> {
    Tab(&'a str),
    Folder(&'a Path),
}

/// Write down which git account a git tab or a project uses. Empty clears it.
///
/// A folder whose repository has no project written down yet gets one now,
/// named after its own checkout and pointed at it, which is what the settings
/// screen does the first time anything about such a project is changed.
/// Read-modify-write on the parsed JSON, like every other change here. Returns
/// whether it was written
pub fn save_git_account(desk_id: &str, at: GitChoiceAt, account: &str) -> bool {
    let Some(cfg) = load() else { return false };
    let (desks, _) = cfg.resolve_desks();
    let Some(resolved) = desks.iter().position(|d| d.id == desk_id) else {
        return false;
    };
    let path = config_file_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        crate::append_hook_log("could not record the git account: settings are not readable");
        return false;
    };
    if !write_git_choice(&mut doc, &desks[resolved], at, account) {
        return false;
    }
    match serde_json::to_string_pretty(&doc) {
        Ok(out) => crate::crypto::write_atomic(&path, &out).is_ok(),
        Err(_) => false,
    }
}

fn set_or_clear(obj: &mut serde_json::Map<String, serde_json::Value>, key: &str, value: &str) {
    match value.trim() {
        "" => {
            obj.shift_remove(key);
        }
        v => {
            obj.insert(key.into(), serde_json::Value::String(v.to_string()));
        }
    }
}

/// The same, on a parsed settings file, for the desk `desk` as it was read
fn write_git_choice(doc: &mut serde_json::Value, desk: &Desk, at: GitChoiceAt, account: &str) -> bool {
    type Obj = serde_json::Map<String, serde_json::Value>;
    // Where tabs are written: in the folders, in a tab's children, or -- the
    // older way -- straight on the desk
    const HOLDS_TABS: [&str; 3] = ["folders", "tabs", "children"];
    fn names(obj: &Obj, key: &str, by: &str) -> bool {
        obj.get(by).and_then(|n| n.as_str()).map(str::trim) == Some(key) && obj.get("command").is_some()
    }
    fn has(v: &serde_json::Value, key: &str, by: &str) -> bool {
        match v {
            serde_json::Value::Array(list) => list.iter().any(|i| has(i, key, by)),
            serde_json::Value::Object(obj) => {
                names(obj, key, by)
                    || obj.iter().any(|(k, child)| HOLDS_TABS.contains(&k.as_str()) && has(child, key, by))
            }
            _ => false,
        }
    }
    fn find<'a>(v: &'a mut serde_json::Value, key: &str, by: &str) -> Option<&'a mut Obj> {
        if v.as_object().is_some_and(|obj| names(obj, key, by)) {
            return v.as_object_mut();
        }
        match v {
            serde_json::Value::Array(list) => list.iter_mut().find_map(|i| find(i, key, by)),
            serde_json::Value::Object(obj) => obj
                .iter_mut()
                .filter(|(k, _)| HOLDS_TABS.contains(&k.as_str()))
                .find_map(|(_, child)| find(child, key, by)),
            _ => None,
        }
    }
    let Some(entry) = desk_entry_mut(doc, &desk.id) else {
        return false;
    };
    match at {
        GitChoiceAt::Tab(key) => {
            // By automation name first, then by the name on screen, which is
            // what a git tab with no id is known by
            let by = match entry.iter().any(|(k, v)| HOLDS_TABS.contains(&k.as_str()) && has(v, key, "id")) {
                true => "id",
                false => "name",
            };
            let tab = entry
                .iter_mut()
                .filter(|(k, _)| HOLDS_TABS.contains(&k.as_str()))
                .find_map(|(_, v)| find(v, key, by));
            match tab {
                Some(tab) => {
                    set_or_clear(tab, "git_account", account);
                    true
                }
                None => false,
            }
        }
        GitChoiceAt::Folder(cwd) => {
            let projects = entry
                .entry("projects")
                .or_insert_with(|| serde_json::Value::Array(Vec::new()));
            let Some(projects) = projects.as_array_mut() else { return false };
            match desk.project_of(cwd) {
                Some(p) => match projects
                    .iter_mut()
                    .find(|x| x.get("name").and_then(|n| n.as_str()) == Some(&p.name))
                    .and_then(|x| x.as_object_mut())
                {
                    Some(written) => {
                        set_or_clear(written, "git_account", account);
                        true
                    }
                    None => false,
                },
                // A repository with no project written down yet gets one, named
                // after its own checkout, the way the settings screen makes one
                None => {
                    if account.trim().is_empty() {
                        return true;
                    }
                    let Some(main) = crate::repo::main_checkout(cwd) else {
                        return false;
                    };
                    let base = main
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| "project".into());
                    let taken = |n: &str| desk.projects.iter().any(|p| p.name == n);
                    let name = match taken(&base) {
                        false => base,
                        true => (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).expect("endless"),
                    };
                    projects.push(serde_json::json!({
                        "name": name,
                        "at": main.display().to_string().replace('\\', "/"),
                        "git_account": account.trim(),
                    }));
                    true
                }
            }
        }
    }
}

/// Read-modify-write on the parsed JSON, like every other change here, so the
/// person's own file keeps its shape and its order. The tab is found by its
/// automation name, wherever it is written: the flat `tabs` list, a
/// desk's own, or -- where every tab a person makes today ends up -- a
/// working folder inside one. Returns whether it was written.
pub fn save_tab_aim(tab_name: &str, target: Option<&str>) -> bool {
    let path = config_file_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        crate::append_hook_log("could not record the aim: settings are not readable");
        return false;
    };
    let mut written = false;
    write_aim(&mut doc, tab_name, target, &mut written);
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

/// Record one of a desk's own settings back into the settings file, the same
/// read-modify-write [`save_setting`] does. `None` removes the key. Returns
/// whether it was written.
///
/// For answers a person gives while *using* a desk rather than while editing
/// its settings -- agreeing, from a tool, to what the tool does. The desk is
/// found by the id everything else calls it by
pub fn save_desk_setting(desk_id: &str, key: &str, value: Option<serde_json::Value>) -> bool {
    let path = config_file_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        crate::append_hook_log(&format!("could not record {key} for a desk: settings are not readable"));
        return false;
    };
    if !write_desk_value(&mut doc, desk_id, key, value) {
        return false;
    }
    match serde_json::to_string_pretty(&doc) {
        Ok(out) => crate::crypto::write_atomic(&path, &out).is_ok(),
        Err(_) => false,
    }
}

/// The desk entry called `desk_id` in a parsed settings file, with `key` set
/// to `value` (removed for `None`).
///
/// An entry that never had its id written is still the desk of that id -- it
/// was given one from its name when read (see [`settle_desk_ids`]), and it is
/// settled the same way here, over the same list in the same order. That id is
/// written into the entry as well, so the answer stays with this desk when
/// the name on screen changes
fn write_desk_value(
    doc: &mut serde_json::Value,
    desk_id: &str,
    key: &str,
    value: Option<serde_json::Value>,
) -> bool {
    let Some(entry) = desk_entry_mut(doc, desk_id) else {
        return false;
    };
    match value {
        Some(v) => {
            entry.insert(key.to_string(), v);
        }
        None => {
            entry.remove(key);
        }
    }
    true
}

/// The desk entry called `desk_id` in a parsed settings file, with that id
/// written into it (see [`write_desk_value`])
fn desk_entry_mut<'a>(
    doc: &'a mut serde_json::Value,
    desk_id: &str,
) -> Option<&'a mut serde_json::Map<String, serde_json::Value>> {
    let want = desk_id.trim();
    let list = doc.get_mut("desks").and_then(|d| d.as_array_mut())?;
    if want.is_empty() {
        return None;
    }
    let mut named: Vec<Desk> = list
        .iter()
        .map(|e| Desk {
            name: e.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            id: e.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            ..Default::default()
        })
        .collect();
    settle_desk_ids(&mut named);
    let at = named.iter().position(|d| d.id == want)?;
    let entry = list[at].as_object_mut()?;
    let written = entry.get("id").and_then(|v| v.as_str()).unwrap_or_default().trim().to_string();
    if written.is_empty() {
        entry.insert("id".into(), serde_json::Value::String(want.to_string()));
    }
    Some(entry)
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
            Ok(c) if !c.folders.is_empty() || !c.desks.is_empty() => return Some(c),
            _ => continue,
        }
    }
    None
}

#[cfg(test)]
mod tests {

    fn accounts_desk() -> super::Desk {
        let spec = |name: &str, host: Option<&str>, owners: &[&str]| super::GitAccountSpec {
            name: name.into(),
            host: host.map(str::to_string),
            owners: owners.iter().map(|o| o.to_string()).collect(),
            ..Default::default()
        };
        super::Desk {
            id: "work".into(),
            git_accounts: vec![
                spec("home", None, &["me"]),
                spec("lab", Some("gitlab.example.com"), &["acme"]),
                spec("company", None, &["Acme"]),
                spec("key", None, &[]),
            ],
            ..Default::default()
        }
    }

    /// A choice is read as written, and nothing is chosen for anybody: no
    /// choice is "nobody chose", not the first account and not this PC's git
    #[test]
    fn a_git_choice_is_looked_up_and_never_made_up() {
        use super::GitUse;
        let mut desk = accounts_desk();
        desk.git_accounts[3].method = Some("ssh".into());
        desk.git_accounts[3].key = Some("Z:/no/such/key".into());
        assert_eq!(desk.git_use(None), GitUse::Unset);
        assert_eq!(desk.git_use(Some("  ")), GitUse::Unset);
        assert_eq!(desk.git_use(Some(super::THIS_PC)), GitUse::Pc);
        assert_eq!(desk.git_use(Some("gone")), GitUse::Missing("gone".into()));
        let home = desk.git_use(Some("home"));
        assert!(matches!(&home, GitUse::Account { desk, spec } if desk == "work" && spec.name == "home"));

        let nothing = |_: &str| None;
        // Nothing chosen: talking to a server is refused, a commit is not
        assert!(GitUse::Unset.to_git(true, &nothing).is_err());
        assert!(matches!(GitUse::Unset.to_git(false, &nothing).map(|a| a.auth), Ok(crate::git::Auth::Sealed)));
        // A name that has gone is refused either way: its commits were meant
        // to carry that account's name
        assert!(GitUse::Missing("gone".into()).to_git(false, &nothing).is_err());
        // An account with no token stored cannot sign in, and says which
        let err = home.to_git(true, &nothing).err().unwrap_or_default();
        assert!(err.contains("home"), "{err}");
        // With one, the token goes to its own server only
        let stored = |k: &str| (k == "git/work/home").then(|| " tok ".to_string());
        match home.to_git(true, &stored).map(|a| a.auth) {
            Ok(crate::git::Auth::Token { host, login, token }) => {
                assert_eq!((host.as_str(), login.as_str(), token.as_str()), ("github.com", "x-access-token", "tok"));
            }
            _ => panic!("a token account did not sign in with its token"),
        }
        // A key file that is not there is said before git is started
        assert!(desk.git_use(Some("key")).to_git(true, &nothing).is_err());
        // The PC's own git is a choice like any other, and asks nothing of the store
        assert!(matches!(GitUse::Pc.to_git(true, &nothing).map(|a| a.auth), Ok(crate::git::Auth::Own)));

        // Pull request numbers are read as the same account, on GitHub only
        assert_eq!(GitUse::Pc.pr_account().as_deref(), Some(super::THIS_PC));
        assert_eq!(home.pr_account().as_deref(), Some("home"));
        assert_eq!(desk.git_use(Some("lab")).pr_account(), None);
        assert_eq!(GitUse::Unset.pr_account(), None);
    }

    /// The accounts that say they are for a repository's owner are offered
    /// first -- in any case of the owner's name -- then that server's, then
    /// the rest, each group as written
    #[test]
    fn accounts_meant_for_the_owner_come_first() {
        let desk = accounts_desk();
        let order = |host, owner| {
            super::git_accounts_for(&desk.git_accounts, host, owner)
                .into_iter()
                .map(|(a, fits)| format!("{}{}", a.name, if fits { "*" } else { "" }))
                .collect::<Vec<_>>()
        };
        assert_eq!(order(Some("github.com"), Some("acme")), ["company*", "home", "key", "lab"]);
        assert_eq!(order(Some("github.com"), Some("me")), ["home*", "company", "key", "lab"]);
        // An account of another server is not for this repository whatever
        // owners it names
        assert_eq!(order(None, None), ["home", "lab", "company", "key"]);
    }

    /// A choice made at the git column lands on the git tab it was made on, or
    /// on the project of the folder it was made beside -- and nowhere else
    #[test]
    fn a_git_choice_is_written_where_it_belongs() {
        let here = std::env::current_dir().unwrap();
        let mut doc = serde_json::json!({
            "desks": [
                {"name": "Other", "id": "other", "folders": [{"cwd": "C:/elsewhere", "tabs": [{"name": "Git", "command": "git"}]}]},
                {"name": "Work", "id": "work", "folders": [
                    {"cwd": "C:/elsewhere", "tabs": [
                        {"name": "Git", "command": "git"},
                        {"name": "Second", "id": "g2", "command": "git"}
                    ]}
                ]}
            ]
        });
        let desks = |doc: &serde_json::Value| {
            serde_json::from_value::<super::Config>(doc.clone()).unwrap().resolve_desks().0
        };
        let work = desks(&doc).into_iter().find(|d| d.id == "work").unwrap();
        assert!(super::write_git_choice(&mut doc, &work, super::GitChoiceAt::Tab("Git"), "home"));
        assert_eq!(doc["desks"][1]["folders"][0]["tabs"][0]["git_account"], "home");
        assert!(doc["desks"][0]["folders"][0]["tabs"][0].get("git_account").is_none(), "it reached another desk");
        assert!(super::write_git_choice(&mut doc, &work, super::GitChoiceAt::Tab("g2"), "@pc"));
        assert_eq!(doc["desks"][1]["folders"][0]["tabs"][1]["git_account"], "@pc");
        assert!(super::write_git_choice(&mut doc, &work, super::GitChoiceAt::Tab("Git"), ""));
        assert!(doc["desks"][1]["folders"][0]["tabs"][0].get("git_account").is_none(), "clearing left a key behind");
        assert!(!super::write_git_choice(&mut doc, &work, super::GitChoiceAt::Tab("nobody"), "home"));

        // Beside a folder: its repository has no project yet, so one is made
        // for it, pointed at its own checkout
        let Some(main) = crate::repo::main_checkout(&here) else { return }; // built outside a checkout
        assert!(super::write_git_choice(&mut doc, &work, super::GitChoiceAt::Folder(&here), "home"));
        let made = doc["desks"][1]["projects"][0].clone();
        assert_eq!(made["git_account"], "home");
        assert_eq!(made["at"], main.display().to_string().replace('\\', "/"));
        // ...and from then on the choice is that project's, changed in place
        let work = desks(&doc).into_iter().find(|d| d.id == "work").unwrap();
        assert_eq!(work.git_use_of_folder(&here).1.as_deref(), made["name"].as_str());
        assert!(super::write_git_choice(&mut doc, &work, super::GitChoiceAt::Folder(&here), "@pc"));
        assert_eq!(doc["desks"][1]["projects"].as_array().unwrap().len(), 1, "a second project was made");
        assert_eq!(doc["desks"][1]["projects"][0]["git_account"], "@pc");
    }

    /// A git account's token belongs to the account, and is left over when
    /// the account is gone
    #[test]
    fn a_git_token_without_its_account_is_left_over() {
        let cfg: super::Config = serde_json::from_value(serde_json::json!({
            "desks": [{"name": "Work", "id": "work", "git_accounts": [{"name": "home"}]}]
        }))
        .unwrap();
        let keys = ["git/work/home".to_string(), "git/work/gone".to_string(), "git/nobody/home".to_string()];
        assert_eq!(super::orphan_secrets(&cfg, &keys), ["git/work/gone".to_string(), "git/nobody/home".to_string()]);
    }

    /// An answer given from a tool lands on the desk it was given for, found
    /// by its id -- including a desk whose id was never written and comes from
    /// its name -- and nowhere else
    #[test]
    fn a_desk_answer_lands_on_that_desk() {
        let mut doc = serde_json::json!({
            "language": "ja",
            "desks": [
                {"name": "Work", "id": "work"},
                {"name": "Home Stuff"},
            ]
        });
        assert!(super::write_desk_value(&mut doc, "work", "send_pictures_to", Some("claude".into())));
        assert_eq!(doc["desks"][0]["send_pictures_to"], "claude");
        assert!(doc["desks"][1].get("send_pictures_to").is_none(), "it was written to the next desk");

        let settled = super::slug_id("Home Stuff");
        assert!(super::write_desk_value(&mut doc, &settled, "send_pictures_to", Some("codex".into())));
        assert_eq!(doc["desks"][1]["send_pictures_to"], "codex");
        assert_eq!(doc["desks"][1]["id"], settled.as_str(), "the call-name settled from the name was not written down");

        assert!(super::write_desk_value(&mut doc, "work", "send_pictures_to", None));
        assert!(doc["desks"][0].get("send_pictures_to").is_none(), "the removal did not go away");
        assert!(!super::write_desk_value(&mut doc, "nobody", "send_pictures_to", Some("x".into())));
        assert!(!super::write_desk_value(&mut doc, "", "send_pictures_to", Some("x".into())));
        assert_eq!(doc["language"], "ja", "other settings changed");
    }

    /// What a desk agreed to is read back as the name it was agreed for, and
    /// a blank is no answer
    #[test]
    fn a_desk_reads_the_ai_it_agreed_to() {
        let cfg: super::Config = serde_json::from_str(
            r#"{"desks":[{"name":"A","id":"a","send_pictures_to":" gemini "},{"name":"B","id":"b","send_pictures_to":""}]}"#,
        )
        .unwrap();
        let (desks, _) = cfg.resolve_desks();
        assert_eq!(desks[0].send_pictures_to.as_deref(), Some("gemini"));
        assert_eq!(desks[1].send_pictures_to, None);
    }

    /// The screen offers an automation name while you type; the app settles on
    /// one when it reads a file nobody typed into. They have to be the same
    /// name, or a desk would arrive under one spelling and be filed under
    /// another. These are the settings screen's own answers (`slugId` in
    /// webui.rs, run in node), so a change on either side breaks this test.
    #[test]
    fn an_inferred_name_matches_the_one_the_settings_screen_offers() {
        use super::slug_id;
        for (name, want) in [
            ("My Tab", "my-tab"),
            ("claude", "claude"),
            ("a", "a"),
            // Nothing latin to make a slug from: a short hash stands in
            ("実装", "orgq9"),
            ("検査", "z3cio"),
            ("本番サーバー", "0iwye"),
            ("レビュー担当", "bqqvx"),
            ("日本語テスト", "83iht"),
            // One latin letter is still a slug, hash or no hash
            ("デスクA", "a"),
            ("", ""),
        ] {
            assert_eq!(slug_id(name), want, "the automation name for {name}");
        }
    }

    /// Automation reaches a tab by its id and by nothing else, so every tab
    /// needs one and no two may share one -- including tabs in different
    /// working folders, which automation does not distinguish
    #[test]
    fn every_tab_ends_up_with_a_name_automation_can_say() {
        let cfg: super::Config = serde_json::from_str(
            r#"{"desks":[{"name":"project","folders":[
                 {"tabs":[{"name":"実装","command":"claude"},
                          {"name":"My Tab","command":"codex"},
                          {"command":"bash"}]},
                 {"tabs":[{"name":"実装","command":"claude"},
                          {"id":"rev","name":"検査","command":"codex"},
                          {"id":"rev","name":"検査2","command":"codex"}]}]}]}"#,
        )
        .unwrap();
        let (desk, errs) = cfg.resolve_desks();
        let ids: Vec<String> = desk[0].tabs.iter().map(|t| t.cfg.id.clone().unwrap()).collect();
        assert_eq!(
            ids,
            [
                "orgq9",   // a name in Japanese, with no ASCII to make an id from
                "my-tab",  // My Tab
                "bash",    // with no name, from the command
                "orgq9-2", // the same Japanese name again. A different folder does not make it reusable
                "rev",
                "rev-2", // a duplicate written by hand is shifted
            ]
        );
        assert!(
            errs.iter().any(|e| e.contains("rev")),
            "it does not keep quiet about shifting it: {errs:?}"
        );
    }

    /// The name on screen is a label -- two desks may be called the same
    /// thing -- so what secrets and automation are filed under is settled apart
    /// from it, once, and left alone afterwards
    #[test]
    fn every_desk_ends_up_with_a_name_of_its_own() {
        let cfg: super::Config = serde_json::from_str(
            r#"{"desks":[{"name":"本番"},
                              {"name":"本番"},
                              {"name":"Blog","id":"written"},
                              {"name":"Other","id":"written"}]}"#,
        )
        .unwrap();
        let (desk, errs) = cfg.resolve_desks();
        let ids: Vec<&str> = desk.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["fazcj", "fazcj-2", "written", "written-2"]);
        // The display names are left exactly as the person wrote them
        assert_eq!(desk[0].name, "本番");
        assert_eq!(desk[1].name, "本番");
        assert!(
            errs.iter().any(|e| e.contains("written")),
            "it does not keep quiet about shifting it: {errs:?}"
        );
    }

    /// The aim (🎯) is chosen on screen and has to survive the next start, so
    /// it is written back into the settings. It has to reach the tab wherever
    /// that tab is written: a working folder inside a desk is where every
    /// tab a person makes today lives, and an aim chosen on one used to be
    /// dropped on the floor -- the walk only knew the flat list and a
    /// desk's own
    #[test]
    fn an_aim_reaches_a_tab_wherever_it_is_written() {
        use serde_json::json;
        let mut doc = json!({
            "tabs": [{"id": "flat", "command": "sh"}],
            "desks": [{
                "name": "project",
                "tabs": [{"id": "old", "command": "sh"}],
                "folders": [{"tabs": [
                    {"id": "coder", "name": "実装", "command": "claude", "children": [
                        {"id": "deep", "command": "codex"}
                    ]}
                ]}]
            }]
        });
        let aim = |doc: &mut serde_json::Value, who: &str, at: Option<&str>| {
            let mut hit = false;
            super::write_aim(doc, who, at, &mut hit);
            hit
        };
        for who in ["flat", "old", "coder", "deep"] {
            assert!(aim(&mut doc, who, Some("page")), "it does not reach {who}");
        }
        assert_eq!(doc["desks"][0]["folders"][0]["tabs"][0]["drives"], "page");
        assert_eq!(
            doc["desks"][0]["folders"][0]["tabs"][0]["children"][0]["drives"],
            "page"
        );
        // Clearing takes the key away rather than leaving an empty one behind
        assert!(aim(&mut doc, "coder", None));
        assert!(doc["desks"][0]["folders"][0]["tabs"][0].get("drives").is_none());
        // The name on screen is not an address, here either: aiming at the tab's display name
        // must not land on the tab that merely displays that name
        assert!(!aim(&mut doc, "実装", Some("page")), "it was written using the name on screen");
    }

    /// A settings file outlives the version that wrote it.
    ///
    /// Tabs used to be written beside the folders rather than inside one. When
    /// that stopped being read, every tab in an older file stopped existing --
    /// no error, no warning, a desk that opened empty and a person with
    /// no way to tell why. They are read again, into the folder they would
    /// have been put in.
    #[test]
    fn tabs_written_the_old_way_are_still_someones_tabs() {
        let old: Config = serde_json::from_str(
            r#"{"desks":[{"name":"project","tabs":[
                 {"name":"coder","command":"claude"},
                 {"name":"reviewer","command":"codex"}]}]}"#,
        )
        .unwrap();
        let (desk, errs) = old.resolve_desks();
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(desk.len(), 1);
        let names: Vec<&str> = desk[0].tabs.iter().map(|t| t.cfg.name.as_deref().unwrap_or("")).collect();
        assert_eq!(names, vec!["coder", "reviewer"], "the old-style tabs are not gone");
        assert_eq!(desk[0].folders.len(), 1, "only one container folder is made");

        // Written both ways, the ones inside a folder come first and the older
        // ones follow: the file says where they sit, and the migration adds
        let both: Config = serde_json::from_str(
            r#"{"desks":[{"name":"p",
                 "folders":[{"cwd":"D:/a","tabs":[{"name":"inside","command":"cmd"}]}],
                 "tabs":[{"name":"outside","command":"cmd"}]}]}"#,
        )
        .unwrap();
        let (desk, _) = both.resolve_desks();
        let names: Vec<&str> = desk[0].tabs.iter().map(|t| t.cfg.name.as_deref().unwrap_or("")).collect();
        assert_eq!(names, vec!["inside", "outside"]);

        // The same shape without desks at all
        let flat: Config = serde_json::from_str(
            r#"{"tabs":[{"name":"only","command":"cmd"}]}"#,
        )
        .unwrap();
        let (desk, _) = flat.resolve_desks();
        assert_eq!(desk.len(), 1, "a settings file with only tabs still makes one screen");
        assert_eq!(desk[0].tabs.len(), 1);

        // And the current shape is untouched by any of this
        let now: Config = serde_json::from_str(
            r#"{"desks":[{"name":"p","folders":[{"cwd":"D:/a",
                 "tabs":[{"name":"one","command":"cmd"}]}]}]}"#,
        )
        .unwrap();
        let (desk, _) = now.resolve_desks();
        assert_eq!(desk[0].tabs.len(), 1);
        assert_eq!(desk[0].folders.len(), 1);
    }
    use super::*;

    /// Which branches refuse a direct commit is the project's answer, and a
    /// project that has not given one gets the app's.
    ///
    /// Two levels because both questions are real: one person alone turns the
    /// guard off everywhere, and a team that shares `develop` says so on the
    /// one folder that works that way.
    #[test]
    fn a_folder_guards_what_it_says_or_what_its_desk_says() {
        let read = |json: &str| {
            let cfg: Config = serde_json::from_str(json).expect("it reads as settings");
            let (desk, _) = cfg.resolve_desks();
            desk.into_iter()
                .next()
                .expect("there is one desk")
                .folders
                .into_iter()
                .map(|f| f.protect)
                .collect::<Vec<_>>()
        };

        // Nobody has said anything: the branches everybody shares
        let plain = read(r#"{"desks":[{"name":"W","folders":[{"cwd":"D:/a","tabs":[]}]}]}"#);
        assert_eq!(plain[0], vec!["main".to_string(), "master".to_string()]);

        // The desk's answer reaches the folders that have not given one, and
        // the folder that has keeps its own
        let mixed = read(
            r#"{"desks":[{"name":"W","git":{"protect":["develop"]},"folders":[
                {"cwd":"D:/a","tabs":[]},
                {"cwd":"D:/b","protect":["release/*"," "],"tabs":[]},
                {"cwd":"D:/c","protect":[],"tabs":[]}]}]}"#,
        );
        assert_eq!(mixed[0], vec!["develop".to_string()], "say nothing and the desk's answer applies");
        assert_eq!(mixed[1], vec!["release/*".to_string()], "say something and it is taken as said (spaces are not a name)");
        assert!(mixed[2].is_empty(), "an empty list is the answer 'guard nothing'");

        // Alone on your own repository: nothing is guarded anywhere in the desk
        let alone = read(
            r#"{"desks":[{"name":"W","git":{"protect":[]},"folders":[{"cwd":"D:/a","tabs":[]}]}]}"#,
        );
        assert!(alone[0].is_empty());
    }

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
            let spec = ProviderSpec {
                base_url: "http://localhost:11434/v1".into(),
                timeout_sec: secs,
                ..Default::default()
            };
            provider_conn(&spec, &|_| None).expect("it resolves").timeout
        };
        assert_eq!(
            resolved(None),
            Some(std::time::Duration::from_secs(PROVIDER_TIMEOUT_DEFAULT_SEC)),
            "without a value, the default wait"
        );
        assert_eq!(
            resolved(Some(600)),
            Some(std::time::Duration::from_secs(600)),
            "it waits the number of seconds written"
        );
        assert_eq!(resolved(Some(0)), None, "0 waits forever (no limit)");
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
        #[cfg(windows)]
        assert_eq!(root_dir(), exe_dir(), "the portable layout moved away from beside the exe");
        for p in [logs_dir(), state_path("x")] {
            assert!(p.starts_with(root_dir()), "{p:?} went outside the data folder");
        }
    }

    /// Where a copy on Linux keeps things, in each of the four situations it
    /// can be in. Asked of a made-up environment rather than this process's
    /// own, so the answer can be checked without the machine deciding it.
    #[test]
    #[cfg(unix)]
    fn an_installed_copy_writes_where_a_person_writes() {
        let unpacked = std::env::temp_dir().join(format!("shikisha-root-{}", crate::random_hex(6)));
        std::fs::create_dir_all(unpacked.join("config")).unwrap();
        let nothing = |_: &str| None;
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| std::ffi::OsString::from(*v))
            }
        };

        // Unpacked and run from its own folder: the settings beside it are its
        // own, which is the promise the download makes
        assert_eq!(unpacked_root(&unpacked, nothing), unpacked);

        // Installed: the program is root's and the person's things are not
        let bin = std::path::Path::new("/usr/local/bin");
        assert_eq!(
            unpacked_root(bin, env(&[("HOME", "/home/dev")])),
            std::path::PathBuf::from("/home/dev/.local/share/shikisha")
        );
        assert_eq!(
            unpacked_root(bin, env(&[("HOME", "/home/dev"), ("XDG_DATA_HOME", "/srv/things")])),
            std::path::PathBuf::from("/srv/things/shikisha"),
            "it ignored this machine's convention"
        );

        // Told outright, which is how two boxes run on one machine
        assert_eq!(
            unpacked_root(bin, env(&[("HOME", "/home/dev"), ("SHIKISHA_HOME", "/srv/box-2")])),
            std::path::PathBuf::from("/srv/box-2")
        );
        // And it wins even where a folder of settings is sitting right there
        assert_eq!(
            unpacked_root(&unpacked, env(&[("SHIKISHA_HOME", "/srv/box-2")])),
            std::path::PathBuf::from("/srv/box-2")
        );

        let _ = std::fs::remove_dir_all(&unpacked);
    }

    #[test]
    fn a_group_is_the_only_thing_that_says_where_work_happens() {
        let work = crate::local_path("D:/work/proj");
        let cfg: Config = serde_json::from_str(
            &r#"{"folders": [
                 {"name": "main", "cwd": "<work>",
                  "tabs": [{"name": "実装", "command": "claude"},
                           {"name": "レビュー", "command": "codex"}]},
                 {"name": "feature/login", "cwd": "scripts",
                  "tabs": [{"name": "実装", "command": "claude"}]}]}"#
                .replace("<work>", &work),
        )
        .unwrap();
        let desk = &cfg.resolve_desks().0[0];
        assert_eq!(desk.folders.len(), 2);
        assert_eq!(desk.tabs.len(), 3);
        // Everyone in a group works in the one folder -- the whole point, since
        // a reviewer pointed somewhere else reviews nothing
        assert_eq!(desk.cwd_of(&desk.tabs[0]), Some(work.clone().into()));
        assert_eq!(desk.cwd_of(&desk.tabs[1]), desk.cwd_of(&desk.tabs[0]));
        // Relative stays relative to the settings, so a folder of them travels
        assert_eq!(desk.cwd_of(&desk.tabs[2]), Some(root_dir().join("scripts")));
    }

    /// A desk written the old way, tabs beside the folders, keeps them
    /// when a folder is added: they go into the first folder, which is where
    /// launching already read them from, and the new folder comes after.
    #[test]
    fn adding_a_folder_to_the_old_shape_does_not_take_its_tabs() {
        let dir = std::env::temp_dir().join(format!("shikisha-legacy-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        std::fs::write(
            &file,
            r#"{"desks": [{"name": "orion", "tabs": [
                 {"name": "backend", "command": "claude"},
                 {"name": "frontend", "command": "codex"}]}]}"#,
        )
        .unwrap();

        let fresh = crate::local_path("D:/work/fresh");
        append_folder_at(&file, "orion", None, Path::new(&fresh), None, &Start::Same, None).unwrap();

        let text = std::fs::read_to_string(&file).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(raw["desks"][0].get("tabs").is_none(), "the old location is still there: {text}");
        let cfg: Config = serde_json::from_str(&text).unwrap();
        let desk = &cfg.resolve_desks().0[0];
        assert_eq!(desk.folders.len(), 2, "the original tabs' container, plus the one added: {text}");
        assert_eq!(desk.folders[0].cwd, None, "the original tabs stay in the app's folder");
        assert_eq!(desk.folders[1].cwd.as_deref(), Some(Path::new(&fresh)));
        let in_folder = |g: usize| desk.tabs.iter().filter(|t| t.folder == g).count();
        assert_eq!(in_folder(0), 2, "the original tabs are not in their original container: {text}");
        assert_eq!(in_folder(1), 0, "tabs moved into the added folder: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Writing down "work on another branch too", and reading it back the way
    /// launching would.
    #[test]
    fn another_folder_is_written_with_the_same_faces() {
        let dir = std::env::temp_dir().join(format!("shikisha-append-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        let proj = crate::local_path("D:/work/proj");
        let branch = crate::local_path("D:/work/proj.worktrees/feature/login");
        std::fs::write(
            &file,
            r#"{"max_chain": 7, "desks": [{"name": "Demo", "secrets_allow": ["x"],
                 "folders": [{"cwd": "<proj>", "tabs": [
                   {"name": "実装", "id": "coder", "command": "claude"},
                   {"name": "レビュー", "id": "rev", "command": "codex"}]}]}]}"#
                .replace("<proj>", &proj),
        )
        .unwrap();

        append_folder_at(
            &file,
            "Demo",
            Some(Path::new(&proj)),
            Path::new(&branch),
            Some("feature/login"),
            &Start::Same,
            None,
        )
        .unwrap();

        let cfg: Config = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        // Nothing else in the file was disturbed, including a key nobody read
        assert_eq!(cfg.max_chain, Some(7));
        assert_eq!(cfg.desks[0].secrets_allow, ["x"]);

        let desk = &cfg.resolve_desks().0[0];
        assert_eq!(desk.folders.len(), 2, "the original one, plus the one added");
        assert_eq!(desk.folders[0].cwd.as_deref(), Some(Path::new(&proj)));
        assert_eq!(desk.folders[1].name.as_deref(), Some("feature/login"));
        assert_eq!(desk.folders[1].cwd.as_deref(), Some(Path::new(&branch)));
        // The same faces, working in the new folder
        let names = |g: usize| {
            desk.tabs
                .iter()
                .filter(|t| t.folder == g)
                .map(|t| t.cfg.name.clone().unwrap_or_default())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(1), names(0));
        assert_eq!(names(1), ["実装", "レビュー"]);
        // Automation still has one name per tab: the copies are marked with the
        // folder they went to, while what is on screen stays readable
        let ids = desk
            .tabs
            .iter()
            .filter(|t| t.folder == 1)
            .map(|t| t.cfg.id.clone().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["coder@feature-login", "rev@feature-login"]);
        // ...and no two of them are the same, so automation can address each
        let all: Vec<String> = desk.tabs.iter().filter_map(|t| t.cfg.id.clone()).collect();
        let unique: std::collections::HashSet<&String> = all.iter().collect();
        assert_eq!(all.len(), unique.len(), "the names automation uses do not collide");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder can be made to run one AI, or nothing, instead of copying
    /// whatever its project runs.
    #[test]
    fn a_new_folder_runs_what_it_was_told_to() {
        let dir = std::env::temp_dir().join(format!("shikisha-start-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        std::fs::write(
            &file,
            r#"{"desks": [{"name": "Demo", "folders": [
                {"cwd": "D:/work/proj", "tabs": [{"name": "実装", "id": "coder", "command": "claude"}]}]}]}"#,
        )
        .unwrap();
        append_folder_at(
            &file,
            "Demo",
            Some(Path::new("D:/work/proj")),
            Path::new("D:/work/proj.worktrees/a-codex"),
            Some("a-codex"),
            &Start::One { name: "codex".into(), command: "codex --flag".into() },
            None,
        )
        .unwrap();
        append_folder_at(
            &file,
            "Demo",
            Some(Path::new("D:/work/proj")),
            Path::new("D:/work/proj.worktrees/quiet"),
            Some("quiet"),
            &Start::Nothing,
            None,
        )
        .unwrap();
        let cfg: Config = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let desk = &cfg.resolve_desks().0[0];
        assert_eq!(desk.folders.len(), 3);
        let in_folder = |g: usize| desk.tabs.iter().filter(|t| t.folder == g).collect::<Vec<_>>();
        let one = in_folder(1);
        assert_eq!(one.len(), 1, "just one AI");
        assert_eq!(one[0].cfg.name.as_deref(), Some("codex"));
        assert_eq!(one[0].cfg.command.argv(), ["codex", "--flag"]);
        assert!(in_folder(2).is_empty(), "it should start nothing, but there are tabs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A settings file of its own, for the close-and-reopen tests
    fn tabs_file(tag: &str, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("shikisha-{tag}-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        std::fs::write(&file, body).unwrap();
        (dir, file)
    }

    fn read_desk(file: &Path) -> Desk {
        let cfg: Config = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        cfg.resolve_desks().0.remove(0)
    }

    /// A path as the settings file spells it, which is JSON
    fn json_path(p: &str) -> String {
        p.replace('\\', "\\\\")
    }

    /// Closing one tab takes its line out and leaves everything else as the
    /// person wrote it; opening it again puts the same line back where it stood
    #[test]
    fn a_closed_tab_goes_back_where_it_stood() {
        let proj = crate::local_path("D:/work/proj");
        let (dir, file) = tabs_file(
            "close",
            &r#"{"max_chain": 7, "desks": [{"name": "Demo", "folders": [{"cwd": "<proj>", "tabs": [
                {"name": "実装", "id": "coder", "command": "claude", "locked": true},
                {"name": "レビュー", "id": "rev", "command": "codex"},
                {"name": "git", "command": "git"}]}]}]}"#
                .replace("<proj>", &json_path(&proj)),
        );
        let desk = read_desk(&file);
        let taken = take_tab_at(&file, "Demo", 0, &TabMark::of(&desk, &desk.tabs[0])).unwrap();
        assert_eq!(taken.path, vec![0]);
        assert_eq!(taken.line["locked"], serde_json::json!(true), "what was written on the tab was not kept");
        let after = read_desk(&file);
        assert_eq!(
            after.tabs.iter().map(|t| t.cfg.id.clone().unwrap_or_default()).collect::<Vec<_>>(),
            ["rev", "git"],
            "a different tab was taken out"
        );
        let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(raw["max_chain"], serde_json::json!(7), "the rest of the settings were disturbed");

        let id = put_tab_back_at(&file, "Demo", &taken).unwrap();
        assert_eq!(id, "coder");
        let back = read_desk(&file);
        assert_eq!(back.tabs[0].cfg.id.as_deref(), Some("coder"), "it did not go back where it stood");
        assert!(back.tabs[0].cfg.locked);
        assert_eq!(back.tabs.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Copies of a folder's tabs share their names and commands. Closing the
    /// one in the second folder must not take the first folder's
    #[test]
    fn a_copy_in_another_folder_is_not_mistaken_for_the_one_closed() {
        let a = crate::local_path("D:/work/proj");
        let b = crate::local_path("D:/work/proj.worktrees/login");
        let (dir, file) = tabs_file(
            "close-copy",
            &r#"{"desks": [{"name": "Demo", "folders": [
                {"cwd": "<a>", "tabs": [{"name": "AI", "command": "claude"}]},
                {"cwd": "<b>", "tabs": [{"name": "AI", "command": "claude"}]}]}]}"#
                .replace("<a>", &json_path(&a))
                .replace("<b>", &json_path(&b)),
        );
        let desk = read_desk(&file);
        // Asked for by a position that has gone stale -- the file lost a line
        // above it since it was read. The folder still tells the two apart
        let taken = take_tab_at(&file, "Demo", 0, &TabMark::of(&desk, &desk.tabs[1])).unwrap();
        assert_eq!(taken.folder.as_deref(), Some(b.as_str()));
        let after = read_desk(&file);
        assert_eq!(after.tabs.len(), 1);
        assert_eq!(after.tabs[0].folder, 0, "the tab in the other folder was taken out");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tab's children stay when it closes. The indent is only how they are
    /// drawn, and closing one tab is not closing three
    #[test]
    fn closing_a_tab_leaves_its_children_in_its_place() {
        let (dir, file) = tabs_file(
            "close-kids",
            r#"{"desks": [{"name": "Demo", "folders": [{"tabs": [
                {"name": "親", "id": "lead", "command": "claude", "children": [
                    {"name": "子1", "id": "k1", "command": "codex"},
                    {"name": "子2", "id": "k2", "command": "codex"}]},
                {"name": "隣", "id": "next", "command": "claude"}]}]}]}"#,
        );
        let desk = read_desk(&file);
        let taken = take_tab_at(&file, "Demo", 0, &TabMark::of(&desk, &desk.tabs[0])).unwrap();
        assert!(taken.line.get("children").is_none(), "the children went with it");
        let after = read_desk(&file);
        assert_eq!(
            after.tabs.iter().map(|t| (t.cfg.id.clone().unwrap_or_default(), t.depth)).collect::<Vec<_>>(),
            [("k1".to_string(), 0), ("k2".to_string(), 0), ("next".to_string(), 0)],
            "the children did not move up into its place"
        );
        // A child closes from inside its parent, and goes back there
        let (dir2, file2) = tabs_file(
            "close-kid",
            r#"{"desks": [{"name": "Demo", "folders": [{"tabs": [
                {"name": "親", "id": "lead", "command": "claude", "children": [
                    {"name": "子1", "id": "k1", "command": "codex"},
                    {"name": "子2", "id": "k2", "command": "codex"}]}]}]}]}"#,
        );
        let desk = read_desk(&file2);
        let taken = take_tab_at(&file2, "Demo", 2, &TabMark::of(&desk, &desk.tabs[2])).unwrap();
        assert_eq!(taken.path, vec![0, 1]);
        put_tab_back_at(&file2, "Demo", &taken).unwrap();
        let back = read_desk(&file2);
        assert_eq!(back.tabs[2].cfg.id.as_deref(), Some("k2"));
        assert_eq!(back.tabs[2].depth, 1, "it did not go back under its parent");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    /// Opened again after another tab took its name: it comes back under a
    /// name of its own, and the tab that never left keeps the one it has
    #[test]
    fn a_reopened_tab_does_not_take_a_name_from_the_tab_that_stayed() {
        let (dir, file) = tabs_file(
            "reopen-name",
            r#"{"desks": [{"name": "Demo", "folders": [{"tabs": [
                {"name": "AI", "id": "coder", "command": "claude"}]}]}]}"#,
        );
        let desk = read_desk(&file);
        let taken = take_tab_at(&file, "Demo", 0, &TabMark::of(&desk, &desk.tabs[0])).unwrap();
        // Somebody adds a tab of the same name in the meantime
        let mut raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        raw["desks"][0]["folders"][0]["tabs"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"name": "AI", "id": "coder", "command": "claude"}));
        std::fs::write(&file, raw.to_string()).unwrap();

        let id = put_tab_back_at(&file, "Demo", &taken).unwrap();
        assert_eq!(id, "coder-2");
        let back = read_desk(&file);
        assert_eq!(back.tabs[0].cfg.id.as_deref(), Some("coder-2"));
        assert_eq!(back.tabs[1].cfg.id.as_deref(), Some("coder"), "the tab that stayed was renamed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tab whose folder was closed in the meantime brings the folder back
    #[test]
    fn a_reopened_tab_brings_its_folder_back() {
        let a = crate::local_path("D:/work/a");
        let b = crate::local_path("D:/work/b");
        let (dir, file) = tabs_file(
            "reopen-folder",
            &r#"{"desks": [{"name": "Demo", "folders": [
                {"cwd": "<a>", "tabs": [{"name": "A", "id": "a", "command": "claude"}]},
                {"cwd": "<b>", "tabs": [{"name": "B", "id": "b", "command": "claude"}]}]}]}"#
                .replace("<a>", &json_path(&a))
                .replace("<b>", &json_path(&b)),
        );
        let desk = read_desk(&file);
        let taken = take_tab_at(&file, "Demo", 1, &TabMark::of(&desk, &desk.tabs[1])).unwrap();
        with_folders(&file, "Demo", |folders| {
            folders.remove(1);
            Ok(())
        })
        .unwrap();
        put_tab_back_at(&file, "Demo", &taken).unwrap();
        let back = read_desk(&file);
        assert_eq!(back.folders.len(), 2, "the folder did not come back");
        assert_eq!(back.folders[1].cwd.as_deref(), Some(Path::new(&b)));
        assert_eq!(back.tabs[1].cfg.id.as_deref(), Some("b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Settings that never made a desk are read as one called DEFAULT, and a
    /// tab in them closes like any other
    #[test]
    fn a_tab_closes_in_settings_with_no_list_of_desks() {
        let (dir, file) = tabs_file(
            "close-flat",
            r#"{"tabs": [{"name": "one", "command": "claude"}, {"name": "two", "command": "codex"}]}"#,
        );
        let desk = read_desk(&file);
        assert_eq!(desk.name, "DEFAULT");
        take_tab_at(&file, &desk.name, 1, &TabMark::of(&desk, &desk.tabs[1])).unwrap();
        let after = read_desk(&file);
        assert_eq!(after.tabs.len(), 1);
        assert_eq!(after.tabs[0].cfg.name.as_deref(), Some("one"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A settings file halfway through an edit is not an empty one, and a
    /// change written over it would throw the rest of it away
    #[test]
    fn a_settings_file_that_cannot_be_read_is_not_written_over() {
        let broken = r#"{"desks": [{"name": "Demo","#;
        let (dir, file) = tabs_file("close-broken", broken);
        let mark = TabMark { id: Some("x".into()), name: None, argv: vec![], folder: None };
        assert!(take_tab_at(&file, "Demo", 0, &mark).is_err());
        assert!(append_folder_at(&file, "Demo", None, Path::new("x"), None, &Start::Nothing, None).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), broken, "the file was written over");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A worktree made on another machine is written down as being on it.
    ///
    /// It was written as a folder here: its tabs started on this machine, in a
    /// path that only exists over there, running the AIs copied from the
    /// folder it was cut from.
    #[test]
    fn a_folder_made_on_another_machine_is_written_as_being_there() {
        let dir = std::env::temp_dir().join(format!("shikisha-there-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        std::fs::write(
            &file,
            r#"{"hosts": [{"name": "bench", "at": "ssh://me@example.test:22", "project": "/srv/proj"}],
                "desks": [{"name": "Demo", "folders": [
                {"cwd": "D:/work/proj", "tabs": [{"name": "実装", "id": "coder", "command": "claude"}]}]}]}"#,
        )
        .unwrap();
        append_folder_at(
            &file,
            "Demo",
            Some(Path::new("D:/work/proj")),
            Path::new("/srv/proj.branches/login"),
            Some("login"),
            &Start::Same,
            Some("bench"),
        )
        .unwrap();
        append_folder_at(
            &file,
            "Demo",
            Some(Path::new("D:/work/proj")),
            Path::new("/srv/proj.branches/quiet"),
            Some("quiet"),
            &Start::Nothing,
            Some("bench"),
        )
        .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        let cfg: Config = serde_json::from_str(&text).unwrap();
        let desk = &cfg.resolve_desks().0[0];
        assert_eq!(desk.folders.len(), 3, "{text}");
        let there = &desk.folders[1];
        assert_eq!(there.host.as_ref().map(|h| h.name.as_str()), Some("bench"), "the machine is not written: {text}");
        assert!(matches!(there.source, Source::Unknown), "the far folder was given a source to rebuild it from here: {text}");
        let in_folder = |g: usize| desk.tabs.iter().filter(|t| t.folder == g).collect::<Vec<_>>();
        let one = in_folder(1);
        assert_eq!(one.len(), 1, "there should be one terminal: {text}");
        assert_eq!(one[0].cfg.name.as_deref(), Some("bench"), "the AI's name went on a plain terminal: {text}");
        assert!(in_folder(2).is_empty(), "it should start nothing, but there are tabs");
        let opts = crate::desk::tab_options(&one[0].cfg, Some(there));
        assert!(opts.remote.is_some(), "a tab in the far folder starts on this machine");
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
        let about = |desc: &str| SecretMeta { desc: desc.into(), ..Default::default() };

        // Register in plaintext -> the list shows what it is for, never the value
        upsert_secret(&path, None, "blog.diary", &about("日記SaaSのログイン"), "hunter2秘密").unwrap();
        let list = list_secrets(&path, None).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "blog.diary");
        assert_eq!(list[0].1.desc, "日記SaaSのログイン");
        assert!(!list[0].1.ai, "not open to the AI by default");
        assert!(list[0].1.urls.is_empty(), "not allowed into any site by default");
        // The value really is stored (retrievable via resolve_tokens)
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("hunter2秘密"), "the value was not saved");
        // But the value never appears in the listing API
        assert!(!format!("{list:?}").contains("hunter2"), "the value leaks into the list");

        // What it is for can be changed without typing the password again
        let opened = SecretMeta {
            ai: true,
            urls: vec!["https://github.com".into()],
            desc: "説明更新".into(),
            ..Default::default()
        };
        upsert_secret(&path, None, "blog.diary", &opened, "").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("hunter2秘密"), "the value was overwritten with nothing");
        let list = list_secrets(&path, None).unwrap();
        assert!(list[0].1.ai && list[0].1.urls == ["https://github.com"], "the use did not change");

        // ...but a name with nothing behind it is not filed at all
        assert!(upsert_secret(&path, None, "blog.nothing", &about("x"), "").is_err());

        upsert_secret(&path, None, "blog.github", &about("PAT"), "ghp_xxx").unwrap();
        let keys: Vec<String> = list_secrets(&path, None).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["blog.diary".to_string(), "blog.github".to_string()], "sorted");

        // Delete
        delete_secret(&path, None, "blog.github").unwrap();
        let keys: Vec<String> = list_secrets(&path, None).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["blog.diary".to_string()]);

        // With a master password set, it's saved encrypted
        upsert_secret(&path, Some("master"), "ssh/blog/prod/password", &about("暗号化テスト"), "topsecret")
            .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(crate::crypto::is_encrypted(&raw), "with a password, it is encrypted");
        assert!(!raw.contains("topsecret"), "after encryption the raw value is not visible");
        // With the correct password the list can be read; the value still doesn't appear
        let list = list_secrets(&path, Some("master")).unwrap();
        assert!(list.iter().any(|(k, _)| k == "ssh/blog/prod/password"));
        // A name that could be made to read as another one is refused
        for bad in ["../evil", ".hidden", "a..b", "trailing/", "sp ace", ""] {
            assert!(
                upsert_secret(&path, None, bad, &about("x"), "y").is_err(),
                "{bad} gets through"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each desk has its own notification destinations, model connections,
    /// automation doors, permission table and git settings -- and nothing of
    /// anybody else's underneath.
    ///
    /// The same keys written at the top of the file, where the app's own
    /// answer used to live, reach no desk at all: an answer a desk inherits
    /// until somebody thinks to take it away is how work's code ends up in a
    /// personal account.
    #[test]
    fn a_desk_has_its_own_and_nothing_of_the_apps() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "primary_notify": "mine",
                "notify": {"mine": {"type":"slack","webhook":"https://example.com/a"}},
                "providers": {"mine": {"base_url":"http://localhost:11434/v1"}},
                "capabilities": {"allow_hosts": ["example.com"]},
                "automation_permissions": {"lua": {"ai": true}},
                "git": {"protect": ["main"], "message_hint": "アプリの言い分"},
                "desks": [
                  {"name":"個人", "folders":[{"cwd":"."}]},
                  {"name":"会社",
                   "notify": {"work": {"type":"slack","webhook":"@notify/kaisha/work"}},
                   "primary_notify": " work ",
                   "providers": {"claude": {"base_url":"https://work.example/v1", "api_key":"@provider/kaisha/claude"}},
                   "capabilities": {"files": {"books": {"dir": "C:/books", "write": true}}},
                   "automation_permissions": {"write_path": {"ai": false}},
                   "git": {"protect": ["main", "release/*"]},
                   "folders":[{"cwd":"."}, {"cwd":".", "protect":["nothing-else"]}]}
                ]
              }"#,
        )
        .unwrap();
        let (spaces, errs) = cfg.resolve_desks();
        assert!(errs.is_empty(), "{errs:?}");

        // Said nothing: has nothing
        let bare = &spaces[0];
        assert!(bare.notify.is_empty() && bare.primary_notify.is_none(), "it inherited the app's notification destinations");
        assert!(bare.providers.is_empty(), "it inherited the app's connections");
        assert!(bare.capabilities.allow_hosts.is_empty(), "it inherited the app's allowed hosts");
        assert!(
            !crate::grants::Grants::new(bare.automation_permissions.clone()).allows("lua", crate::grants::Subject::Ai),
            "it inherited the app's permissions"
        );
        assert!(bare.git.message_hint.is_none(), "it inherited the app's git settings");

        // Said its own: exactly that
        let work = &spaces[1];
        assert_eq!(work.primary_notify.as_deref(), Some("work"));
        assert_eq!(work.capabilities.files.keys().collect::<Vec<_>>(), vec!["books"]);
        assert!(
            !crate::grants::Grants::new(work.automation_permissions.clone()).allows("write_path", crate::grants::Subject::Ai)
        );
        // Its protected branches reach its folders, and a folder with its own
        // answer still has the last word
        assert_eq!(work.folders[0].protect, vec!["main".to_string(), "release/*".to_string()]);
        assert_eq!(work.folders[1].protect, vec!["nothing-else".to_string()]);

        // Its secrets are read through the store's door, by the names it wrote
        let store = |k: &str| match k {
            "notify/kaisha/work" => Some("https://hooks.example/work".to_string()),
            "provider/kaisha/claude" => Some("sk-work".to_string()),
            _ => None,
        };
        let dests = desk_notify(work, &store);
        assert!(
            matches!(&dests["work"], crate::notify::Destination::Slack { webhook } if webhook == "https://hooks.example/work")
        );
        let conns = desk_providers(work, &store);
        assert_eq!(conns["claude"].headers.get("Authorization").map(String::as_str), Some("Bearer sk-work"));
        assert!(desk_providers(bare, &store).is_empty());
    }

    /// A desk written with none of these keys reads as a desk with none of
    /// these things.
    #[test]
    fn a_desk_without_the_keys_has_none_of_them() {
        let cfg: Config =
            serde_json::from_str(r#"{"desks":[{"name":"素","tabs":[]}]}"#).unwrap();
        let d = &cfg.desks[0];
        assert!(d.notify.is_empty() && d.primary_notify.is_none() && d.providers.is_empty());
        assert!(d.capabilities.files.is_empty() && d.automation_permissions.is_empty());
        assert_eq!(d.git.protected(), GitSpec::default().protected(), "git's default is not the built-in answer");
    }

    /// A secrets file written before names meant anything is brought forward
    /// without anybody losing a password: the program's own credentials move
    /// behind a `/`, and everything a desk was allowed to use turns up
    /// under that desk's name
    #[test]
    fn an_older_secrets_file_is_brought_forward() {
        let dir = std::env::temp_dir().join("shikisha-secrets-migrate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secrets.json");
        std::fs::write(
            &path,
            r#"{"tokens":{"provider_deepseek":"sk-1","github":"ghp_2","private":"p3"},
                "descriptions":{"github":"PAT"}}"#,
        )
        .unwrap();
        let desk = |id: &str, allow: &[&str], all: bool| Desk {
            name: id.into(),
            id: id.into(),
            folders: Vec::new(),
            tabs: Vec::new(),
            automation: None,
            browsers: Vec::new(),
            secrets_allow: allow.iter().map(|s| s.to_string()).collect(),
            secrets_allow_all: all,
            stops: Vec::new(),
            discuss: None,
            ..Default::default()
        };
        let spaces = [desk("blog", &["github"], false), desk("shop", &[], true)];
        assert!(migrate_secrets(&path, None, &spaces).unwrap(), "nothing moved");

        let now: std::collections::HashMap<String, SecretMeta> =
            list_secrets(&path, None).unwrap().into_iter().collect();
        let mut names: Vec<&String> = now.keys().collect();
        names.sort();
        assert_eq!(
            names,
            [
                "blog.github",   // it said it could be used, so it goes to that desk
                "github",        // the original stays (it may never have said whose it was)
                "private",       // what no desk could use is left as it is
                "provider/deepseek",
                "shop.github",   // it also goes to a desk that allows everything
                "shop.private",
            ]
        );
        // What it was allowed to do carries over; where it may be typed does not,
        // because nothing ever recorded that
        assert!(now["blog.github"].ai, "it was usable from the AI, and got closed");
        assert!(now["blog.github"].urls.is_empty());
        assert_eq!(now["blog.github"].desc, "PAT", "the description was not carried over");
        // Values follow their names
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["tokens"]["blog.github"], "ghp_2");
        assert_eq!(v["tokens"]["provider/deepseek"], "sk-1");
        assert!(v["tokens"].get("provider_deepseek").is_none(), "the old name is still there");

        // Running again changes nothing: the file says which shape it is in
        assert!(!migrate_secrets(&path, None, &spaces).unwrap(), "it ran a second time");
        let again = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw, again);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stored password goes into a page only where it belongs, and only over
    /// a connection that proves the page is who it says. Without the second
    /// half, pointing `github.com` at another machine (a hosts file will do)
    /// would be enough to be handed the password
    #[test]
    fn a_secret_is_typed_only_into_the_place_it_belongs_to() {
        let at = |urls: &[&str]| SecretMeta {
            urls: urls.iter().map(|u| (*u).into()).collect(),
            ..Default::default()
        };

        // A whole site
        let m = at(&["https://github.com", "https://api.github.com"]);
        assert!(m.may_fill("https://github.com/login"));
        assert!(m.may_fill("https://GitHub.com/login"), "upper and lower case are the same site");
        assert!(m.may_fill("https://github.com/?next=/x"), "it does not look past ?");
        assert!(m.may_fill("https://api.github.com/"));
        assert!(!m.may_fill("https://gist.github.com/"), "it gets into another host");
        assert!(!m.may_fill("https://github.com.evil.example/"), "a prefix match gets through");
        assert!(!m.may_fill("http://github.com/login"), "it gets onto a path without a certificate");
        assert!(!m.may_fill("about:blank"));
        assert!(!m.may_fill(""));

        // Nothing listed means nowhere, not everywhere
        assert!(!SecretMeta::default().may_fill("https://github.com/login"));

        // A path is a prefix, and it ends at a slash
        let deep = at(&["https://example.com/api"]);
        assert!(deep.may_fill("https://example.com/api"));
        assert!(deep.may_fill("https://example.com/api/keys?x=1"));
        assert!(!deep.may_fill("https://example.com/apiary"), "it cut in the middle of a word");
        assert!(!deep.may_fill("https://example.com/"), "it spread to the whole site");
        // ...and saying "the whole site" out loud is the same as not saying it
        assert!(at(&["https://example.com/*"]).may_fill("https://example.com/anything"));

        // A star at the front covers the site and everything under it
        let sub = at(&["https://*.example.com"]);
        assert!(sub.may_fill("https://example.com/"), "the original site is excluded");
        assert!(sub.may_fill("https://dev.example.com/"));
        assert!(sub.may_fill("https://a.b.example.com/"));
        assert!(!sub.may_fill("https://example.com.evil.test/"), "adding to the end gets through");
        assert!(!sub.may_fill("https://notexample.com/"), "the part before the dot does not match");

        // A port written down has to match; left out it is the scheme's own
        assert!(at(&["https://example.com:8443"]).may_fill("https://example.com:8443/x"));
        assert!(!at(&["https://example.com:8443"]).may_fill("https://example.com/x"));
        assert!(at(&["https://example.com"]).may_fill("https://example.com:443/x"));

        // A plain connection is a place like any other, once it is written out
        let inside = at(&["http://intranet.local", "https://github.com"]);
        assert!(inside.may_fill("http://intranet.local/login"));
        assert!(!inside.may_fill("https://intranet.local/login"), "it differs from the path written");
        assert!(inside.may_fill("https://github.com/login"));
        assert!(!inside.may_fill("http://github.com/login"), "it gets through downgraded to plain text");
    }

    /// What is offered for deletion is only what nothing claims. Getting this
    /// wrong deletes a password somebody still needs, so each shape is asked
    /// for by name and anything unrecognised is left alone
    #[test]
    fn only_what_nothing_claims_is_offered_for_tidying() {
        let cfg: Config = serde_json::from_str(
            r#"{
              "desks": [
                {"name":"Blog","id":"blog","tabs":[
                   {"name":"prod","id":"prod","command":"ssh://me@example.com"}],
                 "providers": {"deepseek": {"base_url": "https://api.deepseek.com/v1"}},
                 "notify": {"team": {"type":"slack","webhook":"@notify/blog/team"}}}
              ]
            }"#,
        )
        .unwrap();
        let keys: Vec<String> = [
            // claimed
            "blog.diary",
            "ssh/blog/prod/password",
            "ssh/blog/prod/passphrase",
            "provider/blog/deepseek",
            "notify/blog/team",
            // nobody's
            "gone.diary",
            "ssh/gone/prod/password",
            "ssh/blog/gone/password",
            "provider/blog/openai",
            "provider/gone/deepseek",
            "notify/old",
            // not this program's shape: never judged, never offered
            "something_a_person_made",
            "weird/shape/here",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();

        let mut left = orphan_secrets(&cfg, &keys);
        left.sort();
        assert_eq!(
            left,
            vec![
                "gone.diary",
                "notify/old",
                "provider/blog/openai",
                "provider/gone/deepseek",
                "ssh/blog/gone/password",
                "ssh/gone/prod/password",
            ]
        );
    }

    /// The screen and the store ask the same question of a line, so what the
    /// screen refused cannot arrive by another door
    #[test]
    fn a_line_that_reaches_too_far_is_refused() {
        let ok = |u: &str| assert!(url_fault(u).is_none(), "{u} was refused: {:?}", url_fault(u));
        let no = |u: &str, why: &str| assert_eq!(url_fault(u), Some(why), "{u}");

        ok("https://example.com");
        ok("https://example.com/api");
        ok("https://example.com:8443/api");
        ok("https://*.example.com");
        ok("http://internal-dev.local");

        no("", "err.secret_url.empty");
        no("example.com", "err.secret_url.scheme");
        no("ftp://example.com", "err.secret_url.scheme");
        no("https://ex*.com", "err.secret_url.star_place");
        no("https://*example.com", "err.secret_url.star_place");
        no("https://*.*.com", "err.secret_url.star_place");
        // The whole internet with a shape
        no("https://*.com", "err.secret_url.star_wide");
        no("https://*.co.jp", "err.secret_url.star_wide");
        // Credentials in an address hide which host is really being asked for
        no("https://user@example.com", "err.secret_url.unreadable");
        no("https://exa mple.com", "err.secret_url.unreadable");

        // And a refused line does not quietly work anyway
        let bad = SecretMeta { urls: vec!["https://*.com".into()], ..Default::default() };
        assert!(!bad.may_fill("https://anything.com/"), "a line that should have been refused takes effect");
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
        let (desk, errs) = cfg.resolve_desks();
        assert!(errs.is_empty());
        let tabs = &desk[0].tabs;
        assert_eq!(tabs.len(), 3, "parents and children are flattened");
        assert_eq!(tabs[0].depth, 0);
        assert_eq!(tabs[1].depth, 1);
        assert!(tabs[1].cfg.locked, "locked can be read");
        assert_eq!(tabs[2].cfg.name.as_deref(), Some("C"));
    }

    #[test]
    fn legacy_projects_path_falls_back_to_desks() {
        // An existing config pointing at the old name projects/ should still be
        // able to read desks/, and the other way round
        assert_eq!(
            data_path_candidates("projects/x.json"),
            ["projects/x.json", "desks/x.json"],
            "a projects/ path falls back to desks/"
        );
        assert_eq!(
            data_path_candidates("desks/x.json"),
            ["desks/x.json", "projects/x.json"]
        );
        // Anything else is only ever itself
        assert_eq!(data_path_candidates("scripts/x.lua"), ["scripts/x.lua"]);
    }

    /// A relative path in the settings is found under the layout root, and a
    /// path that exists nowhere yet is placed there too -- not in the working
    /// folder, which for an installed copy started from the Start menu is
    /// System32
    #[test]
    fn a_written_path_is_found_under_the_root_and_made_there() {
        let stamp = format!("resolve-test-{}", std::process::id());
        let dir = root_dir().join(&stamp);
        std::fs::create_dir_all(&dir).unwrap();
        let rel = format!("{stamp}/on_done.lua");
        std::fs::write(root_dir().join(&rel), "").unwrap();
        assert_eq!(resolve_data_path(&rel), root_dir().join(&rel), "it is under the root but was not found");
        let _ = std::fs::remove_dir_all(&dir);

        let missing = format!("{stamp}-missing/scripts/new");
        assert_eq!(
            resolve_data_path(&missing),
            root_dir().join(&missing),
            "where something missing would go is not under the root"
        );
    }

    #[test]
    fn inline_desks_are_resolved() {
        let cfg: Config = serde_json::from_str(
            r#"{"desks":[
                {"name":"X","tabs":[{"name":"a","command":"a"}]},
                {"name":"Y","tabs":[{"name":"b","command":"b"}]}
            ]}"#,
        )
        .unwrap();
        let (desk, _) = cfg.resolve_desks();
        assert_eq!(desk.len(), 2);
        assert_eq!(desk[1].name, "Y");
    }

    #[test]
    fn missing_desk_file_is_reported_not_fatal() {
        let cfg: Config = serde_json::from_str(
            r#"{"desks":[
                {"name":"Bad","file":"desks/does-not-exist.json"},
                {"name":"Good","tabs":[{"name":"a","command":"a"}]}
            ]}"#,
        )
        .unwrap();
        let (desk, errs) = cfg.resolve_desks();
        assert_eq!(desk.len(), 1, "a broken definition is skipped and it carries on");
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn operate_defaults_match_the_baked_in_safety_net() {
        // A config with no `operate` block falls back to the historical limits and
        // the safe "stop" policy, so behavior is unchanged until the user opts in.
        let cfg: Config = serde_json::from_str(r#"{"desks":[]}"#).unwrap();
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
            serde_json::from_str(r#"{"desks":[],"operate":{"max_rounds":0,"on_limit":"continue"}}"#)
                .unwrap();
        assert_eq!(cfg.operate.max_rounds, 0, "0 = unlimited rounds");
        assert_eq!(cfg.operate.on_limit, "continue");
        assert_eq!(cfg.operate.max_seconds, 900, "untouched field keeps its default");
    }
}

#[cfg(test)]
mod browser_kind_tests {
    use super::{Config, browser_url_of, is_git_panel, is_sftp_panel, sftp_endpoint, ssh_endpoint};

    #[test]
    fn the_git_panel_is_the_word_on_its_own() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_git_panel(&v(&["git"])));
        assert!(is_git_panel(&v(&["GIT"])), "upper case is the same thing");
        // Somebody wanting a terminal that runs git keeps their terminal
        assert!(!is_git_panel(&v(&["git", "status"])));
        assert!(!is_git_panel(&v(&["gitk"])));
        assert!(!is_git_panel(&[]));
    }

    /// What a server tab knows beyond its address is read back whole, and a
    /// tab that has none is not given an empty one
    #[test]
    fn a_server_tab_keeps_what_its_address_cannot_hold() {
        let cfg: Config = serde_json::from_str(
            r#"{"desks":[{"name":"W","id":"w","folders":[{"cwd":"D:/a","tabs":[
                 {"name":"prod","id":"prod","command":"ssh://rocky@example.com:22",
                  "server":{"key":"~/.ssh/id_ed25519","remote_dir":"/var/www",
                            "keepalive":30,"file_command":"sudo su -",
                            "jump":{"host":"gw.example.com","port":2222,"user":"jump"}}},
                 {"name":"plain","command":"ssh://a@b:22"}]}]}]}"#,
        )
        .unwrap();
        let desk = &cfg.desks[0];
        let tabs = &desk.folders[0].tabs;
        let sv = tabs[0].server.as_ref().expect("the connection settings were not read");
        assert_eq!(sv.key.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(sv.remote_dir.as_deref(), Some("/var/www"));
        assert_eq!(sv.keepalive, Some(30));
        assert_eq!(sv.file_command.as_deref(), Some("sudo su -"));
        let j = sv.jump.as_ref().expect("the jump host was not read");
        assert_eq!((j.host.as_str(), j.port, j.user.as_str()), ("gw.example.com", Some(2222), "jump"));
        assert!(tabs[1].server.is_none(), "something not written has appeared");
    }

    /// A file panel carries its own address, written the way the world writes
    /// one, and is told apart from a terminal only by the scheme
    #[test]
    fn a_file_panel_is_an_address_like_any_other() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            sftp_endpoint(&v(&["sftp://deploy@example.com:2222"])),
            Some(("example.com".into(), 2222, "deploy".into()))
        );
        assert_eq!(
            sftp_endpoint(&v(&["SFTP://deploy@example.com"])),
            Some(("example.com".into(), 22, "deploy".into())),
            "upper case is the same thing. With no port written, 22"
        );
        // Half-written is still a panel: it has to hold its place and say what
        // it needs, not be handed to the launcher as the name of a program
        assert!(is_sftp_panel(&v(&["sftp://"])));
        assert_eq!(sftp_endpoint(&v(&["sftp://"])), None);
        // Somebody's own sftp command line is a command line
        assert!(!is_sftp_panel(&v(&["sftp", "deploy@example.com"])));
        assert!(!is_sftp_panel(&v(&["sftp"])));
        assert!(!is_sftp_panel(&[]));
        // The two schemes do not answer for each other
        assert_eq!(sftp_endpoint(&v(&["ssh://a@b:22"])), None);
        assert_eq!(ssh_endpoint(&v(&["sftp://a@b:22"])), None);
    }


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
            "the spelling web gets through too"
        );
        assert_eq!(
            browser_url_of(&v(&["BROWSER", "https://a.example/"])).as_deref(),
            Some("https://a.example/"),
            "case does not matter"
        );

        // Things that are not a browser
        assert!(browser_url_of(&v(&["cmd.exe"])).is_none());
        assert!(browser_url_of(&v(&["claude"])).is_none());
        assert!(browser_url_of(&v(&["browser"])).is_none(), "there is no URL");
        assert!(browser_url_of(&v(&["browser", "  "])).is_none(), "only spaces");
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
        let nav = t.nav.expect("the top bar was not read");
        assert!(nav.reload && nav.url, "what was written does not show");
        assert!(!nav.back && !nav.forward, "even what was not written shows");
        let ask = t.ask.expect("the banner was not read");
        assert_eq!(ask.label, "解析する");

        // If nothing is written, neither is shown
        let bare: super::TabConfig =
            serde_json::from_str(r#"{"command": "browser https://example.com/"}"#).unwrap();
        assert!(bare.nav.is_none() && bare.ask.is_none());

        // The banner's mere presence means "show it" -- shown even with empty contents
        let empty: super::TabConfig =
            serde_json::from_str(r#"{"command": "browser https://x/", "ask": {}}"#).unwrap();
        assert!(empty.ask.is_some(), "an empty banner is treated as if there were none");
    }
}
