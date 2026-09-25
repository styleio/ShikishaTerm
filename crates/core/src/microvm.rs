//! A project on a MicroVM: the two things about one that take longer than a
//! frame.
//!
//! **Making its checkout.** A project added straight onto a MicroVM -- one that
//! is on no machine yet but the git server -- is a machine made for it and the
//! project cloned into it, which takes as long as the service and the network
//! do. [`Checkout`] does that on a thread of its own and is looked at every
//! turn, the way a clone onto this PC is.
//!
//! **Saying what it signs in as.** Before a MicroVM is made for a project, the
//! dialog says which account it will sign in to the git server as, and what
//! kind of token that is -- one that reaches a few repositories until a day
//! somebody chose, or every repository for as long as nobody takes it back.
//! Finding out asks git or GitHub CLI on this PC, which can take seconds, so
//! [`sign_in_note`] answers from what it last found and asks again on a thread.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How far making a project's checkout on a MicroVM has got, or how it ended
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Still going: the step it is on, as the dialog names it
    Running(&'static str),
    /// There: the machine, and where the project is on it
    Done { sandbox: String, at: String },
    Failed(String),
}

/// A project's checkout being made on a MicroVM.
#[derive(Clone)]
pub struct Checkout {
    outcome: Arc<Mutex<Outcome>>,
    stopping: Arc<std::sync::atomic::AtomicBool>,
}

impl Checkout {
    /// Makes a machine from `host`'s entry, marked as `project`'s, and clones
    /// `url` into it. A machine the project could not be put on is thrown
    /// away: it would be a machine nobody knows about, billed for nothing
    pub fn start(host: crate::config::HostSpec, url: &str, project: &str, sign_in: crate::config::FarSignIn) -> Checkout {
        let job = Checkout {
            outcome: Arc::new(Mutex::new(Outcome::Running("microvm.making"))),
            stopping: Default::default(),
        };
        let (outcome, stopping) = (job.outcome.clone(), job.stopping.clone());
        let url = crate::worktree::fetchable(url);
        let project = project.to_string();
        std::thread::spawn(move || {
            let set = |o: Outcome| *outcome.lock().unwrap_or_else(|e| e.into_inner()) = o;
            let stop = || stopping.load(std::sync::atomic::Ordering::Relaxed);
            let made = (|| -> Result<(String, String), String> {
                let key = crate::e2b::key().ok_or_else(|| crate::i18n::t("err.e2b.no_key"))?;
                let sign_in = sign_in.resolve()?;
                let asking = crate::e2b::Asking {
                    template: host.template_or_default().to_string(),
                    minutes: host.minutes_or_default(),
                    marks: crate::e2b::marks(&project),
                    sign_in,
                };
                let box_ = crate::e2b::create(&key, &asking).map_err(|e| format!("{e:#}"))?;
                if stop() {
                    let _ = crate::e2b::kill(&key, &box_.id);
                    return Err(String::new());
                }
                set(Outcome::Running("microvm.cloning"));
                let at = checkout_path(&project);
                let line = format!("git clone --quiet -- '{}' '{}'", url.replace('\'', "'\\''"), at);
                let ran = crate::e2b::exec(&box_, &line, None).map_err(|e| format!("{e:#}"));
                match ran {
                    Ok(r) if r.ok() && !stop() => Ok((box_.id, at)),
                    Ok(r) => {
                        let _ = crate::e2b::kill(&key, &box_.id);
                        match stop() {
                            true => Err(String::new()),
                            false => Err(crate::i18n::tp("err.worktree.failed", &[("said", &r.said()), ("command", &line)])),
                        }
                    }
                    Err(e) => {
                        let _ = crate::e2b::kill(&key, &box_.id);
                        Err(e)
                    }
                }
            })();
            set(match made {
                Ok((sandbox, at)) => Outcome::Done { sandbox, at },
                Err(e) => Outcome::Failed(e),
            });
        });
        job
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Asks it to stop. A machine already made is thrown away when it does
    pub fn stop(&self) {
        self.stopping.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The addresses a folder's MicroVM answers on from anywhere: each port
/// something listens on in there, with the public URL the service gives it.
/// Asked of the machine itself, which starts it if it was paused -- so this
/// is asked when somebody asks, and never on a timer
pub fn ports_of(host: &crate::config::HostSpec) -> Result<Vec<crate::uistate::FarPort>, String> {
    let machine = crate::e2b::machine(host).map_err(|e| format!("{e:#}"))?;
    let ran = crate::e2b::exec(&machine, LISTENING, None).map_err(|e| format!("{e:#}"))?;
    Ok(listening(&ran.out)
        .into_iter()
        .map(|port| crate::uistate::FarPort { port, url: crate::e2b::public_url(&machine.id, port) })
        .collect())
}

/// What lists the ports listened on, on every image the service has: `ss`,
/// and `netstat` where there is no `ss`. With the processes: `ss` names the
/// ones the machine's own user started, and only those (see [`listening`])
const LISTENING: &str = "ss -ltnpH 2>/dev/null || netstat -ltn 2>/dev/null";

/// The ports in a listing of listening sockets, each once, in order.
///
/// What somebody started in there: a server run in a terminal, or by the AI,
/// runs as the machine's own user, and `ss` names its process. What the image
/// runs as root -- its SSH server, its port mapper -- has no process `ss` can
/// name for that user, and is not the work's to offer. A listing that names
/// no process at all (`netstat`) is taken whole. The service's own agents in
/// there are never offered
fn listening(said: &str) -> Vec<u16> {
    let named = said.contains("users:(");
    let mut ports: Vec<u16> = said
        .lines()
        .filter(|l| l.contains("LISTEN") || !l.trim_start().starts_with(|c: char| c.is_ascii_alphabetic()))
        .filter(|l| !named || l.contains("users:("))
        .filter_map(|l| {
            let fields: Vec<&str> = l.split_whitespace().collect();
            // `ss`: State Recv-Q Send-Q Local Peer; `netstat`: Proto Recv-Q Send-Q Local Foreign State
            let local = fields.iter().find(|f| f.contains(':') && !f.ends_with(":*"))?;
            local.rsplit(':').next()?.parse::<u16>().ok()
        })
        .filter(|p| !crate::e2b::OWN_PORTS.contains(p))
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Where a project is checked out on a MicroVM made for it
pub fn checkout_path(project: &str) -> String {
    format!("{}/{}", crate::worktree::MICROVM_HOME, crate::worktree::folder_leaf(project, ""))
}

/// What a project is called when it is added from its address: the last part
/// of the address, without `.git`
pub fn name_of_url(url: &str) -> String {
    let tail = url.trim().trim_end_matches('/').rsplit(['/', ':']).next().unwrap_or_default();
    let name = tail.strip_suffix(".git").unwrap_or(tail).trim();
    match name.is_empty() {
        true => "project".into(),
        false => name.to_string(),
    }
}

/// The server and the owner an address names: `github.com` and `acme` for
/// `https://github.com/acme/site.git` and `git@github.com:acme/site`
pub fn host_and_owner(url: &str) -> Option<(String, String)> {
    let https = crate::worktree::fetchable(url);
    let rest = https.split_once("://").map(|(_, r)| r).unwrap_or(&https);
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let host = parts.next()?.to_ascii_lowercase();
    let owner = parts.next()?.to_string();
    parts.next()?;
    Some((host, owner))
}

/// How long what was found about a sign-in is taken as still true. A token
/// changed in the settings is read afresh at once (the key below changes with
/// it); one changed in git's own store is noticed within this
const FRESH: Duration = Duration::from_secs(60);

/// What was found, by the account and the way it is handed over, and which
/// of them are being asked about right now
#[derive(Default)]
struct Notes {
    found: std::collections::HashMap<String, (Instant, crate::uistate::SignInNote)>,
    asking: std::collections::HashSet<String>,
}

static NOTES: std::sync::OnceLock<Mutex<Notes>> = std::sync::OnceLock::new();

/// What a MicroVM of a project would sign in to its git server as: the
/// account `account` (as the project settings name it), handed over the way
/// `far` says. From what was last found, with a fresh look on its way when
/// that is old; None until the first look comes back. The token itself is
/// never kept here -- only what kind it is
pub fn sign_in_note(account: &str, far: &Result<crate::config::FarSignIn, String>) -> Option<crate::uistate::SignInNote> {
    let far = match far {
        Ok(f) => f.clone(),
        Err(e) => {
            return Some(crate::uistate::SignInNote { account: account.to_string(), kind: "none".into(), error: e.clone() });
        }
    };
    // A token in hand says what it is at once; nothing to ask
    if let crate::config::FarSignIn::Given(s) = &far {
        return Some(crate::uistate::SignInNote {
            account: account.to_string(),
            kind: crate::config::token_kind(&s.token).into(),
            error: String::new(),
        });
    }
    // Keyed by how it is handed over, which names the account and never holds
    // a token: a token in hand was answered above
    let key = format!("{account}\u{1f}{far:?}");
    let Ok(mut notes) = NOTES.get_or_init(Default::default).lock() else { return None };
    let known = notes.found.get(&key).cloned();
    // Kept while it is asked again, so the dialog does not blink
    if known.as_ref().is_none_or(|(at, _)| at.elapsed() > FRESH) && notes.asking.insert(key.clone()) {
        let account = account.to_string();
        std::thread::spawn(move || {
            let note = match far.resolve() {
                Ok(Some(s)) => crate::uistate::SignInNote { account, kind: crate::config::token_kind(&s.token).into(), error: String::new() },
                Ok(None) => crate::uistate::SignInNote { account, kind: "none".into(), error: String::new() },
                Err(e) => crate::uistate::SignInNote { account, kind: "none".into(), error: e },
            };
            if let Ok(mut n) = NOTES.get_or_init(Default::default).lock() {
                n.asking.remove(&key);
                n.found.insert(key, (Instant::now(), note));
            }
        });
    }
    known.map(|(_, note)| note)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project added from its address is named for it, the way a clone's
    /// folder is
    #[test]
    fn a_project_is_named_for_its_address() {
        assert_eq!(name_of_url("https://github.com/acme/site.git"), "site");
        assert_eq!(name_of_url("git@github.com:acme/api"), "api");
        assert_eq!(name_of_url("https://example.test/a/b/"), "b");
        assert_eq!(name_of_url(""), "project");
        assert_eq!(checkout_path("My App"), "/home/user/My-App");
    }

    /// The ports offered are the ones the machine's own user started a
    /// server on, each once -- not what the image runs as root, and not the
    /// service's own agent in there
    #[test]
    fn the_ports_offered_are_what_listens_in_there() {
        let ss = concat!(
            "LISTEN 0 4096 0.0.0.0:8000 0.0.0.0:* users:((\"python3\",pid=812,fd=3))\n",
            "LISTEN 0 128 [::]:8000 [::]:* users:((\"python3\",pid=812,fd=4))\n",
            "LISTEN 0 4096 *:49983 *:* users:((\"envd\",pid=1,fd=7))\n",
            "LISTEN 0 511 127.0.0.1:5173 0.0.0.0:* users:((\"node\",pid=900,fd=21))\n",
            "LISTEN 0 128 0.0.0.0:22 0.0.0.0:*\n",
            "LISTEN 0 4096 0.0.0.0:111 0.0.0.0:*\n",
        );
        assert_eq!(listening(ss), vec![5173, 8000]);
        let netstat = "Active Internet connections (only servers)\nProto Recv-Q Send-Q Local Address Foreign Address State\ntcp 0 0 0.0.0.0:3000 0.0.0.0:* LISTEN\n";
        assert_eq!(listening(netstat), vec![3000]);
        assert!(listening("").is_empty());
        assert_eq!(crate::e2b::public_url("isb1", 3000), "https://3000-isb1.e2b.app");
    }

    /// A MicroVM fetches over HTTPS, where the sign-in put on its requests
    /// reaches the server: an SSH address is spelled that way, and a
    /// credential written into an address is taken out
    #[test]
    fn a_microvm_fetches_over_https_with_nothing_written_in_the_address() {
        use crate::worktree::fetchable;
        assert_eq!(fetchable("git@github.com:acme/site.git"), "https://github.com/acme/site.git");
        assert_eq!(fetchable("ssh://git@github.com:22/acme/site.git"), "https://github.com/acme/site.git");
        assert_eq!(fetchable("https://me:secret@github.com/acme/site.git"), "https://github.com/acme/site.git");
        assert_eq!(fetchable("https://gitlab.example.com/a/b"), "https://gitlab.example.com/a/b");
        assert_eq!(host_and_owner("git@github.com:acme/site.git"), Some(("github.com".into(), "acme".into())));
        assert_eq!(host_and_owner("https://github.com/acme"), None, "an owner alone is no repository");
    }

    /// The sign-in goes on the server git talks to, and -- for GitHub -- on
    /// its API, each in the form it takes; and a sign-in printed for a log
    /// or a panic says whose it is and never what it is
    #[test]
    fn a_sign_in_is_put_on_requests_and_never_printed() {
        let s = crate::e2b::SignIn { host: "github.com".into(), login: "x-access-token".into(), token: "github_pat_SECRET".into() };
        let rules = crate::e2b::network_of(Some(&s));
        let git = rules["rules"]["github.com"][0]["transform"]["headers"]["Authorization"].as_str().unwrap();
        use base64::Engine as _;
        let said = base64::engine::general_purpose::STANDARD.decode(git.trim_start_matches("Basic ")).unwrap();
        assert_eq!(String::from_utf8(said).unwrap(), "x-access-token:github_pat_SECRET");
        assert_eq!(rules["rules"]["api.github.com"][0]["transform"]["headers"]["Authorization"], "Bearer github_pat_SECRET");
        let other = crate::e2b::SignIn { host: "gitlab.example.com".into(), ..s.clone() };
        assert!(crate::e2b::network_of(Some(&other))["rules"].get("api.github.com").is_none(), "GitHub's API is signed in to for another server");
        assert_eq!(crate::e2b::network_of(None), serde_json::json!({}));
        assert!(!format!("{s:?}").contains("SECRET"), "the token is printed: {s:?}");
        assert_eq!(crate::config::token_kind("github_pat_x"), "fine");
        assert_eq!(crate::config::token_kind("ghp_x"), "classic");
        assert_eq!(crate::config::token_kind("gho_x"), "oauth");
        assert_eq!(crate::config::token_kind("glpat-x"), "unknown");
    }

    /// A token in hand is said at once and by kind; one that could not be had
    /// says why. The token itself is in neither
    #[test]
    fn what_a_microvm_signs_in_as_is_said_by_kind() {
        let given = Ok(crate::config::FarSignIn::Given(crate::e2b::SignIn {
            host: "github.com".into(),
            login: "x-access-token".into(),
            token: "ghp_0123456789".into(),
        }));
        let note = sign_in_note("home", &given).expect("a token in hand is said at once");
        assert_eq!((note.account.as_str(), note.kind.as_str()), ("home", "classic"));
        assert!(!format!("{note:?}").contains("0123456789"), "the token is in what is said");
        let failed = sign_in_note("gone", &Err("no such account".into())).unwrap();
        assert_eq!((failed.kind.as_str(), failed.error.as_str()), ("none", "no such account"));
    }
}
