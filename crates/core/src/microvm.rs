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

/// A project's checkout being made on a MicroVM.
#[derive(Clone)]
pub struct Checkout {
    outcome: Arc<Mutex<Outcome>>,
    stopping: Arc<std::sync::atomic::AtomicBool>,
}

impl Checkout {
    /// Makes a machine from `host`'s entry, marked as `project`'s, clones
    /// `url` into it, and prepares it as `preparing` says. A machine the
    /// project could not be put on is thrown away: it would be a machine
    /// nobody knows about, billed for nothing
    pub fn start(
        host: crate::config::HostSpec,
        url: &str,
        project: &str,
        sign_in: crate::config::FarSignIn,
        preparing: Preparing,
    ) -> Checkout {
        let job = Checkout {
            outcome: Arc::new(Mutex::new(Outcome::Running(PHASE_MAKING))),
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
                // Who commits made on the machine are by, told to git there
                // once: the AI in the terminal and the git panel both commit
                // there, and a machine that has never been told refuses both
                let identity = sign_in.as_ref().map(identity_steps).unwrap_or_default();
                let asking = crate::e2b::Asking {
                    template: host.template_or_default().to_string(),
                    minutes: host.minutes_or_default(),
                    marks: crate::e2b::marks(&project),
                    sign_in,
                };
                let box_ = crate::e2b::create(&key, &asking).map_err(|e| format!("{e:#}"))?;
                if stop() {
                    crate::e2b::throw_away(&key, &box_.id);
                    return Err(String::new());
                }
                set(Outcome::Running("microvm.cloning"));
                let at = checkout_path(&project);
                let clone = vec!["git".into(), "clone".into(), "--quiet".into(), "--".into(), url.clone(), at.clone()];
                // The clone, then what the machine is prepared with, each in
                // turn; the first that fails ends it, and the machine goes
                let mut steps = vec![(PHASE_CLONING, clone)];
                steps.extend(identity.into_iter().map(|a| (PHASE_CLONING, a)));
                match preparing.commands(&at) {
                    Ok(c) => steps.extend(c.into_iter().map(|a| (PHASE_PREPARING, a))),
                    Err(e) => {
                        crate::e2b::throw_away(&key, &box_.id);
                        return Err(e);
                    }
                }
                for (phase, argv) in steps {
                    set(Outcome::Running(phase));
                    let line = crate::worktree::for_a_shell(&argv);
                    let ran = crate::e2b::exec(&box_, &line, None).map_err(|e| format!("{e:#}"));
                    let failed = match ran {
                        Ok(r) if r.ok() && !stop() => None,
                        Ok(r) if !stop() => Some(crate::i18n::tp("err.worktree.failed", &[("said", &r.said()), ("command", &line)])),
                        Ok(_) => Some(String::new()),
                        Err(e) => Some(e),
                    };
                    if let Some(e) = failed {
                        crate::e2b::throw_away(&key, &box_.id);
                        return Err(e);
                    }
                }
                Ok((box_.id, at))
            })();
            set(match made {
                Ok((sandbox, at)) => Outcome::Done { sandbox, at, prepared: preparing.said() },
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

/// The git sign-in each MicroVM folder's machine has, kept current as the
/// settings change (the project-hosts plan, §6.4).
///
/// A machine is given its sign-in when it is made, and a copy carries the
/// one it was copied with. A token replaced later -- one that ran out, an
/// account chosen instead -- reached only the machines made after it, and
/// every worktree already open went on pushing with the old one until it
/// failed. Told to each machine here, on a thread, when what it would be
/// given changes.
///
/// What each machine was last given is kept in a file, as a fingerprint and
/// never as the token, so a token replaced while the app was closed is seen
/// the next time. A machine is told only while a terminal of it is open and
/// speaking: telling a paused one would start it, and it is told when it is
/// next looked at instead. One that could not be told keeps its old
/// fingerprint, and is tried again the next time round. A machine seen for the
/// first time is taken to have what it would be given. `folders` is
/// (machine, what it signs in as)
pub fn keep_sign_ins_current(folders: Vec<(String, crate::config::FarSignIn)>) {
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if folders.is_empty() || BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let mut given = given_sign_ins();
        let before = given.clone();
        for (id, far) in folders {
            // Only a sign-in that could be worked out: one that cannot says
            // nothing about what the machine should now be given
            let Ok(sign_in) = far.resolve() else { continue };
            let print = sign_in_print(sign_in.as_ref());
            match given.get(&id) {
                None => {
                    given.insert(id, print);
                }
                Some(was) if *was == print => {}
                Some(_) if !crate::e2b::awake(&id) => {}
                Some(_) => {
                    let Some(key) = crate::e2b::key() else { continue };
                    match crate::e2b::sign_in_as(&key, &id, sign_in.as_ref()) {
                        Ok(()) => {
                            crate::append_hook_log(&format!("machine {id} was given its new sign-in"));
                            given.insert(id, print);
                        }
                        Err(e) => crate::append_hook_log(&format!(
                            "could not give machine {id} its new sign-in (tried again later): {e:#}"
                        )),
                    }
                }
            }
        }
        if given != before {
            let text = serde_json::to_string_pretty(&given).unwrap_or_default();
            let _ = crate::crypto::write_atomic(&crate::config::state_path(GIVEN), &text);
        }
        BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// The file the fingerprints of what each machine was given are kept in
const GIVEN: &str = "microvm-sign-ins.json";

fn given_sign_ins() -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(crate::config::state_path(GIVEN))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// What a sign-in is, reduced to something that can be compared and kept in
/// memory without keeping the token itself
fn sign_in_print(s: Option<&crate::e2b::SignIn>) -> String {
    use sha2::Digest as _;
    let Some(s) = s else { return "nobody".into() };
    let mut h = sha2::Sha256::new();
    h.update(format!("{}\u{1f}{}\u{1f}{}", s.host, s.login, s.token).as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// What is cleared from a worktree's machine the moment it is copied from the
/// checkout's (the project-hosts plan, §6.4).
///
/// A copy is the machine as it was that instant, what was running in it
/// included: a server somebody tried in the checkout, the AI that was open
/// there, the shell's history of what was typed. None of that is the new
/// worktree's. Everything the terminals' account runs is ended -- save the
/// shell running this and the service's own agent, which the machine is
/// reached through. Hung up first, which ends a shell a person had open (it
/// ignores being asked to terminate) and has it write its history out; killed
/// if it is still there a second later; and only then are the histories
/// deleted, including what was just written. What was installed and signed in
/// to stays: that is what every worktree is copied to have
pub const AFTER_FORK: &str = "me=user; \
command -v ps >/dev/null 2>&1 || { echo 'ps is not on this machine' >&2; exit 3; }; \
list() { ps -u \"$me\" -o pid=,comm= 2>/dev/null | while read -r p c; do \
case \"$p\" in $$|$PPID) continue;; esac; \
case \"$c\" in envd*) continue;; esac; \
echo \"$p\"; done; }; \
for p in $(list); do kill -HUP \"$p\" 2>/dev/null; done; \
sleep 1; \
for p in $(list); do kill -KILL \"$p\" 2>/dev/null; done; \
rm -f /home/$me/.bash_history /home/$me/.zsh_history /home/$me/.python_history /home/$me/.node_repl_history /home/$me/.lesshst /home/$me/.viminfo; \
true";

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

/// What a project's checkout on a MicroVM is prepared with, once: the AI
/// chosen for it, and the lines of the project's machine setup. Every
/// worktree is a copy of that machine, so this is the one place anything is
/// installed for all of them
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preparing {
    /// The AI, by its command, when one was chosen
    pub ai: Option<String>,
    /// The machine setup, one line each, as written
    pub lines: Vec<String>,
}

impl Preparing {
    /// What a project says: its AI (`none` or nothing is none) and its
    /// machine setup
    pub fn of(ai: Option<&str>, setup: Option<&str>) -> Preparing {
        Preparing {
            ai: ai.map(str::trim).filter(|a| !a.is_empty() && !a.eq_ignore_ascii_case(NO_AI)).map(str::to_string),
            lines: setup.unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect(),
        }
    }

    /// The commands, in the order they run, each on screen before it does:
    /// the machine's own certificates made the ones its tools trust (see
    /// [`GROUND`]), the AI's install line as its maker writes it, then each
    /// setup line in the checkout. An AI with no install line is said as that
    /// rather than skipped in silence
    pub fn commands(&self, checkout: &str) -> Result<Vec<Vec<String>>, String> {
        let mut out = vec![vec!["sh".into(), "-lc".into(), GROUND.into()]];
        if let Some(ai) = &self.ai {
            let line = crate::profile::install_on_linux(ai)
                .ok_or_else(|| crate::i18n::tp("err.microvm.no_install", &[("ai", ai)]))?;
            // Once: a machine that has it is not made to install it again,
            // which on a small machine is what runs it out of memory
            out.push(vec!["sh".into(), "-lc".into(), format!("command -v {ai} >/dev/null 2>&1 || {{ {line}; }}")]);
        }
        for line in &self.lines {
            out.push(vec!["sh".into(), "-lc".into(), format!("cd {checkout} && {line}")]);
        }
        Ok(out)
    }

    /// The same, as one text: what is remembered as run on a checkout
    /// ([`crate::config::ProjectHome::prepared`]) and compared with what the
    /// project says now
    pub fn said(&self) -> String {
        let mut out = format!("ai: {}\n", self.ai.as_deref().unwrap_or(NO_AI));
        for l in &self.lines {
            out.push_str(l);
            out.push('\n');
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.ai.is_none() && self.lines.is_empty()
    }
}

/// What "no AI" is written as
pub const NO_AI: &str = "none";

/// GitHub asked, from the machine, who the sign-in is: the login and the
/// no-reply address GitHub keeps for it, told to git there where nothing is
/// set yet. The machine's requests to GitHub carry the sign-in, so this is
/// answered without a token on the machine. Nothing to say, nothing set:
/// a clone must not fail over an author
const GITHUB_IDENTITY: &str = r#"u=$(curl -sf https://api.github.com/user) || exit 0
n=$(printf '%s\n' "$u" | sed -n 's/^ *"login": *"\([^"]*\)".*/\1/p' | head -n1)
i=$(printf '%s\n' "$u" | sed -n 's/^ *"id": *\([0-9]*\).*/\1/p' | head -n1)
[ -n "$n" ] || exit 0
git config --global user.name >/dev/null 2>&1 || git config --global user.name "$n"
git config --global user.email >/dev/null 2>&1 || git config --global user.email "${i}+${n}@users.noreply.github.com"
"#;

/// What git on a new machine is told about who commits made there are by.
///
/// A commit needs an author, and a machine nobody has told refuses every
/// commit -- the AI's in the terminal and the panel's alike. The account's
/// own name and address, where it says them. What it leaves unsaid is, on
/// GitHub, asked of GitHub from the machine ([`GITHUB_IDENTITY`]): the
/// login and the no-reply address GitHub keeps for it, never one made up
/// here. On another server, the login and that server's no-reply form
pub fn identity_steps(sign_in: &crate::e2b::SignIn) -> Vec<Vec<String>> {
    let set = |key: &str, value: &str| {
        vec!["git".to_string(), "config".into(), "--global".into(), key.into(), value.to_string()]
    };
    let given = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let (name, email) = (given(&sign_in.name), given(&sign_in.email));
    let mut steps = Vec::new();
    if let Some(n) = &name {
        steps.push(set("user.name", n));
    }
    if let Some(e) = &email {
        steps.push(set("user.email", e));
    }
    if name.is_some() && email.is_some() {
        return steps;
    }
    if sign_in.host.eq_ignore_ascii_case(crate::config::GITHUB_HOST) {
        steps.push(vec!["sh".into(), "-c".into(), GITHUB_IDENTITY.into()]);
        return steps;
    }
    if name.is_none() {
        steps.push(set("user.name", &sign_in.login));
    }
    if email.is_none() {
        steps.push(set("user.email", &format!("{}@users.noreply.{}", sign_in.login, sign_in.host)));
    }
    steps
}

/// What a folder on a MicroVM opens with.
///
/// The AI the project's machines are given, when there is one: a checkout
/// prepared with Claude opens on Claude, and so does every worktree cut from
/// it, the way a worktree here opens on what its original runs. A machine
/// given no AI opens on its shell. An AI no profile knows is treated as none:
/// a command that fails on screen is worse than a prompt
pub fn start_with(ai: Option<&str>) -> crate::config::Start {
    match ai.map(str::trim).filter(|a| !a.is_empty() && *a != NO_AI) {
        Some(key) if crate::profile::machine_ais().iter().any(|a| a.key.eq_ignore_ascii_case(key)) => {
            crate::config::Start::One { name: key.to_string(), command: key.to_string() }
        }
        _ => crate::config::Start::Same,
    }
}

/// The ground every MicroVM checkout stands on, laid once before anything is
/// installed. Two things:
///
/// The machine's own certificate store made the one every tool on it trusts.
/// The sign-in to the git server is put on the machine's requests by the
/// service on the way out (see [`crate::e2b::SignIn`]), which it does by
/// answering for that server with a certificate of its own -- one the
/// machine's store holds. git, curl and Python read that store; Node and
/// installers built on uv carry lists of their own and refuse it, so an AI
/// written in Node could not reach GitHub, and an install that downloads from
/// there stopped. Told to read the store as well, they reach it as git does.
/// Written for every shell there, a login one and an interactive one.
///
/// A swap file on a small machine. The service's plain image has half a
/// gigabyte of memory, and an installer that unpacks a package in memory --
/// npm does -- is killed on it partway through, with "Killed" as the whole of
/// what is said. A gigabyte of swap on its disk lets such an install finish;
/// a machine with a gigabyte of memory or more is left as it is
pub const GROUND: &str = "printf '%s\\n' 'export NODE_EXTRA_CA_CERTS=/etc/ssl/certs/ca-certificates.crt' \
                          'export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt' 'export UV_NATIVE_TLS=1' \
                          | sudo tee /etc/profile.d/shikisha-certs.sh >/dev/null \
                          && { grep -q shikisha-certs /etc/bash.bashrc \
                          || echo '. /etc/profile.d/shikisha-certs.sh # shikisha-certs' | sudo tee -a /etc/bash.bashrc >/dev/null; } \
                          && { [ -e /swapfile ] || [ \"$(awk '/MemTotal/ {print $2}' /proc/meminfo)\" -ge 1000000 ] \
                          || { sudo fallocate -l 1G /swapfile && sudo chmod 600 /swapfile && sudo mkswap -q /swapfile && sudo swapon /swapfile; }; }";

/// Prepares a checkout's machine as `preparing` says: each command in turn,
/// `at_step` told the phase before each, stopping at the first that fails --
/// in git's and the installer's own words -- or when `stop` says so. Stopped,
/// the machine keeps what was done so far: an install half done is finished
/// by running it again, and nothing of the person's is in it to lose
pub fn prepare(
    host: &crate::config::HostSpec,
    checkout: &str,
    preparing: &Preparing,
    at_step: &dyn Fn(&'static str),
    stop: &dyn Fn() -> bool,
) -> Result<(), String> {
    let machine = crate::e2b::machine(host).map_err(|e| format!("{e:#}"))?;
    for argv in preparing.commands(checkout)? {
        if stop() {
            return Err(String::new());
        }
        at_step(PHASE_PREPARING);
        let line = argv.last().cloned().unwrap_or_default();
        let ran = crate::e2b::exec(&machine, &crate::worktree::for_a_shell(&argv), None).map_err(|e| format!("{e:#}"))?;
        if !ran.ok() {
            return Err(crate::i18n::tp("err.worktree.failed", &[("said", &ran.said()), ("command", &line)]));
        }
    }
    Ok(())
}

/// The phases a machine goes through on the board's row: made, the project
/// cloned into it, prepared with the AI and the machine setup. Each is a
/// stage of a row of the making kind (`tui.making.stage.<phase>`)
pub const PHASE_MAKING: &str = "vm_making";
pub const PHASE_CLONING: &str = "vm_cloning";
pub const PHASE_PREPARING: &str = "vm_preparing";

/// How far a job on a machine has got, or how it ended. An empty failure is
/// a job that was stopped
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Still going: the phase it is on
    Running(&'static str),
    /// There: the machine, where the project is on it, and what it was
    /// prepared with (see [`Preparing::said`])
    Done { sandbox: String, at: String, prepared: String },
    Failed(String),
}

/// A checkout's machine being prepared, on a thread of its own: an install
/// takes minutes, and the board shows a row for it meanwhile
#[derive(Clone)]
pub struct Prepare {
    outcome: Arc<Mutex<Outcome>>,
    stopping: Arc<std::sync::atomic::AtomicBool>,
}

impl Prepare {
    /// Prepares the machine `host` names (its instance) at `checkout`
    pub fn start(host: crate::config::HostSpec, checkout: &str, preparing: Preparing) -> Prepare {
        let job = Prepare { outcome: Arc::new(Mutex::new(Outcome::Running(PHASE_PREPARING))), stopping: Default::default() };
        let (outcome, stopping) = (job.outcome.clone(), job.stopping.clone());
        let checkout = checkout.to_string();
        std::thread::spawn(move || {
            let set = |o: Outcome| *outcome.lock().unwrap_or_else(|e| e.into_inner()) = o;
            let stop = || stopping.load(std::sync::atomic::Ordering::Relaxed);
            let sandbox = host.instance.clone().unwrap_or_default();
            let said = preparing.said();
            set(match prepare(&host, &checkout, &preparing, &|s| set(Outcome::Running(s)), &stop) {
                Ok(()) => Outcome::Done { sandbox, at: checkout, prepared: said },
                Err(e) => Outcome::Failed(e),
            });
        });
        job
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Asks it to stop after the command it is on
    pub fn stop(&self) {
        self.stopping.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A project's checkouts on MicroVMs, each with the machine it is on, as
/// the settings say now -- what preparing the project means
pub struct Targets {
    pub preparing: Preparing,
    /// Each checkout: the entry naming its machine, and the checkout itself
    pub homes: Vec<(crate::config::HostSpec, crate::config::ProjectHome)>,
}

/// What preparing `project` on desk `desk_id` means, from the saved
/// settings: the AI and the machine setup as written, and every checkout it
/// has on a MicroVM. A project with none is said as that
pub fn prepare_targets(desk_id: &str, project: &str) -> Result<Targets, String> {
    let cfg = crate::config::load().ok_or_else(|| crate::i18n::t("err.config.unreadable"))?;
    let (desks, _) = cfg.resolve_desks();
    let desk = desks.iter().find(|d| d.id == desk_id).ok_or_else(|| crate::i18n::tp("err.desk.missing", &[("name", desk_id)]))?;
    let p = desk.projects.iter().find(|p| p.name == project).ok_or_else(|| crate::i18n::tp("err.project.missing", &[("name", project)]))?;
    let homes: Vec<(crate::config::HostSpec, crate::config::ProjectHome)> = p
        .homes
        .iter()
        .filter_map(|home| {
            let host = cfg.hosts.iter().find(|h| h.name == home.host && h.is_made())?;
            let id = home.sandbox.as_deref()?;
            Some((host.with_instance(Some(id)), home.clone()))
        })
        .collect();
    if homes.is_empty() {
        return Err(crate::i18n::tp("err.microvm.no_checkout", &[("name", project)]));
    }
    Ok(Targets { preparing: Preparing::of(p.machine_ai.as_deref(), p.machine_setup.as_deref()), homes })
}

/// One line of a machine setup the assistant AI proposes, with why
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SetupLine {
    pub command: String,
    pub reason: String,
}

/// How long the assistant AI may take to answer
const ASK_TIMEOUT: Duration = Duration::from_secs(240);

/// The files a project says what it needs in, looked for at its top and one
/// folder down
const MANIFESTS: &[&str] = &[
    "composer.json", "package.json", "requirements.txt", "pyproject.toml", "Pipfile", "Gemfile", "go.mod",
    "Cargo.toml", "pom.xml", "build.gradle", "Dockerfile", "docker-compose.yml", "compose.yaml",
    ".devcontainer/devcontainer.json", ".tool-versions", ".nvmrc", ".python-version", "runtime.txt",
];

/// Where a project's files are read from: its checkout here, else its
/// checkout on a MicroVM
enum Source {
    Here(std::path::PathBuf),
    There { host: crate::config::HostSpec, at: String },
}

impl Source {
    /// A line run over there. Nothing is run here: this PC is not the
    /// machine the answer is for
    fn run(&self, line: &str) -> String {
        match self {
            Source::Here(_) => String::new(),
            Source::There { host, at } => crate::e2b::machine(host)
                .and_then(|m| crate::e2b::exec(&m, &format!("cd '{at}' && {line}"), None))
                .map(|r| r.out)
                .unwrap_or_default(),
        }
    }

    fn files(&self) -> Vec<String> {
        let listed = match self {
            Source::Here(at) => {
                let mut c = std::process::Command::new("git");
                c.arg("-C").arg(at).args(["ls-files"]);
                crate::detach_console(&mut c)
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .unwrap_or_default()
            }
            Source::There { .. } => self.run("git ls-files"),
        };
        listed.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
    }

    fn read(&self, file: &str) -> String {
        let text = match self {
            Source::Here(at) => std::fs::read_to_string(at.join(file)).unwrap_or_default(),
            Source::There { .. } => self.run(&format!("head -c 4000 '{file}'")),
        };
        text.chars().take(4000).collect()
    }
}

/// The assistant AI's proposal for a project's machine setup: read from its
/// files -- here, or on its MicroVM checkout -- and, where there is a
/// checkout on a MicroVM, from what that machine already has. Only a
/// proposal: the page shows it, and the person saves it or does not
pub fn propose_setup(desk_id: &str, project: &str, hint: &str, engine: Option<&str>) -> Result<Vec<SetupLine>, String> {
    let cfg = crate::config::load().ok_or_else(|| crate::i18n::t("err.config.unreadable"))?;
    let (desks, _) = cfg.resolve_desks();
    let desk = desks.iter().find(|d| d.id == desk_id).ok_or_else(|| crate::i18n::tp("err.desk.missing", &[("name", desk_id)]))?;
    let p = desk.projects.iter().find(|p| p.name == project).ok_or_else(|| crate::i18n::tp("err.project.missing", &[("name", project)]))?;
    let vm = p.homes.iter().find_map(|h| {
        let host = cfg.hosts.iter().find(|x| x.name == h.host && x.is_made())?;
        Some(Source::There { host: host.with_instance(Some(h.sandbox.as_deref()?)), at: h.at.clone() })
    });
    let here = p.at.as_deref().map(std::path::PathBuf::from).filter(|a| a.is_dir()).map(Source::Here);
    let source = here.or(vm).ok_or_else(|| crate::i18n::tp("err.microvm.no_checkout", &[("name", project)]))?;
    let files = source.files();
    if files.is_empty() {
        return Err(crate::i18n::t("err.microvm.no_files"));
    }
    let manifests: Vec<String> = files
        .iter()
        .filter(|f| {
            let depth = f.matches('/').count();
            MANIFESTS.iter().any(|m| f.as_str() == *m || (depth == 1 && f.ends_with(&format!("/{m}"))))
        })
        .take(8)
        .map(|f| format!("### {f}\n{}", source.read(f)))
        .collect();
    // What the machine has, asked of the machine when there is one
    let machine = match &source {
        Source::There { .. } => source.run(
            "head -2 /etc/os-release; for c in node npm php python3 ruby go java git curl; do printf '%s: ' $c; (command -v $c >/dev/null && $c --version 2>&1 | head -1) || echo missing; done",
        ),
        Source::Here(_) => {
            let first = p.homes.iter().find_map(|h| {
                let host = cfg.hosts.iter().find(|x| x.name == h.host && x.is_made())?;
                h.sandbox.as_deref().map(|id| host.with_instance(Some(id)))
            });
            match first {
                Some(h) => Source::There { host: h, at: "/home/user".into() }.run(
                    "head -2 /etc/os-release; for c in node npm php python3 ruby go java git curl; do printf '%s: ' $c; (command -v $c >/dev/null && $c --version 2>&1 | head -1) || echo missing; done",
                ),
                None => String::new(),
            }
        }
    };
    let machine = match machine.trim() {
        "" => "Not measured: no machine has been made for this project yet. It will be made from the MicroVM service's base image.".to_string(),
        m => m.to_string(),
    };
    let shown: Vec<&str> = files.iter().take(400).map(String::as_str).collect();
    let prompt = crate::i18n::fill(
        crate::asking::MACHINE_ASK,
        &[
            ("machine", &machine),
            ("files", &format!("{}{}", shown.join("\n"), if files.len() > shown.len() { format!("\n(and {} more)", files.len() - shown.len()) } else { String::new() })),
            ("manifests", &match manifests.is_empty() {
                true => "(none of the usual files)".to_string(),
                false => manifests.join("\n\n"),
            }),
            ("hint", match hint.trim() {
                "" => "(nothing)",
                said => said,
            }),
        ],
    );
    let system = format!(
        "{}\n\n{}\n\n{}",
        crate::asking::MACHINE_WHO,
        crate::asking::MACHINE_HOW,
        crate::asking::answer_in(&crate::i18n::language_name())
    );
    let shape = serde_json::json!({
        "type": "object",
        "properties": {
            "lines": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {"command": {"type": "string"}, "reason": {"type": "string"}},
                    "required": ["command", "reason"]
                }
            }
        },
        "required": ["lines"]
    });
    let said = crate::webui::ask_local_ai_shaped(&prompt, &system, &shape.to_string(), engine, ASK_TIMEOUT)
        .map_err(|e| format!("{e:#}"))?;
    Ok(read_setup(&said))
}

/// The lines of an answer, each once, kept to what can be run on one line
fn read_setup(said: &str) -> Vec<SetupLine> {
    let v: serde_json::Value = serde_json::from_str(said.trim())
        .or_else(|_| {
            let from = said.find('{').unwrap_or(0);
            let to = said.rfind('}').map(|i| i + 1).unwrap_or(said.len());
            serde_json::from_str(&said[from..to])
        })
        .unwrap_or_default();
    let mut out: Vec<SetupLine> = Vec::new();
    for l in v.get("lines").and_then(|l| l.as_array()).into_iter().flatten() {
        let command = l.get("command").and_then(|c| c.as_str()).unwrap_or_default().trim().to_string();
        if command.is_empty() || command.contains('\n') || out.iter().any(|o| o.command == command) {
            continue;
        }
        let reason = l.get("reason").and_then(|r| r.as_str()).unwrap_or_default().trim().to_string();
        out.push(SetupLine { command, reason });
    }
    out.truncate(30);
    out
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
pub const FRESH: Duration = Duration::from_secs(60);

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

/// What has been found out about the AI's sign-in on each checkout's machine
#[derive(Default)]
struct AiNotes {
    found: std::collections::HashMap<String, (Instant, crate::uistate::AiSignInNote)>,
    asking: std::collections::HashSet<String>,
}

static AI_NOTES: std::sync::OnceLock<Mutex<AiNotes>> = std::sync::OnceLock::new();

/// Whether the AI on a project's checkout machine is signed in, before a
/// worktree is copied from that machine.
///
/// A worktree on a MicroVM is a copy of the checkout's machine, sign-in and
/// all -- so one copied before the sign-in has none, and every one made after
/// it has it. That is the one thing worth saying in the dialog, so it is
/// asked of the machine itself: the line the AI's profile gives, run in a
/// login shell there (a key put in the profile is only there). Nothing for a
/// project with no AI, no checkout on that machine yet, or an AI whose
/// profile has no line. From what was last found, with a fresh look on its
/// way when that is old; "asking" until the first look comes back.
///
/// A server reached over SSH is asked the same question about its AI (the
/// one [`crate::serverai`] chose there). Its worktrees share the server and
/// its sign-in, so the answer is the same for all of them, and signing in
/// once in the checkout's tab is signing in for every one
pub fn ai_sign_in_note(
    host: &crate::config::HostSpec,
    home: Option<&crate::config::ProjectHome>,
    ai: Option<&str>,
    fresh: Duration,
) -> Option<crate::uistate::AiSignInNote> {
    let ai = ai.map(str::trim).filter(|a| !a.is_empty() && *a != NO_AI)?;
    let known_ai = crate::profile::machine_ai(ai)?;
    let line = known_ai.signed_in.clone()?;
    let set_up = known_ai.set_up.clone();
    let home = home?;
    // A MicroVM is asked by its machine; a server by its entry
    let sandbox = match host.is_made() {
        true => Some(home.sandbox.as_deref().map(str::trim).filter(|s| !s.is_empty())?.to_string()),
        false => None,
    };
    let blank = crate::uistate::AiSignInNote {
        ai: known_ai.key.clone(),
        name: known_ai.name.clone(),
        checkout: home.at.clone(),
        state: String::new(),
        error: String::new(),
        on: if host.is_made() { String::new() } else { "server".into() },
    };
    let note = |state: &str, error: String| crate::uistate::AiSignInNote { state: state.into(), error, ..blank.clone() };
    let key = match &sandbox {
        Some(sandbox) => format!("{sandbox}\u{1f}{}", known_ai.key),
        None => format!("{}\u{1f}{}\u{1f}{}", host.name.trim(), host.at.trim(), known_ai.key),
    };
    let Ok(mut notes) = AI_NOTES.get_or_init(Default::default).lock() else { return None };
    let known = notes.found.get(&key).cloned();
    if known.as_ref().is_none_or(|(at, _)| at.elapsed() > fresh) && notes.asking.insert(key.clone()) {
        let machine = host.with_instance(sandbox.as_deref());
        let (yes, no, finishing, failed) =
            (note("yes", String::new()), note("no", String::new()), note("finishing", String::new()), blank.clone());
        let failed = move |error: String| crate::uistate::AiSignInNote { state: "error".into(), error, ..failed };
        std::thread::spawn(move || {
            // A login shell, because a key set in the machine's profile is
            // what the AI would read, and a plain sh reads none of it
            let ask = |line: String| {
                let argv = ["bash".to_string(), "-lc".to_string(), line];
                match machine.is_made() {
                    true => crate::e2b::machine(&machine)
                        .and_then(|m| crate::e2b::exec(&m, &crate::worktree::for_a_shell(&argv), None)),
                    false => crate::elsewhere::Elsewhere::of(&machine)
                        .and_then(|at| crate::elsewhere::exec(&at, &crate::worktree::for_a_shell(&argv), 30_000)),
                }
            };
            let found = match ask(line) {
                // Signed in; and through its first run as well, where the CLI
                // says so -- until then a copy of the machine would start it
                // over from the beginning, sign-in included
                Ok(r) if r.ok() => match set_up {
                    None => yes,
                    Some(line) => match ask(line) {
                        Ok(r) if r.ok() => yes,
                        Ok(_) => finishing,
                        Err(e) => failed(format!("{e:#}")),
                    },
                },
                Ok(_) => no,
                Err(e) => failed(format!("{e:#}")),
            };
            if let Ok(mut n) = AI_NOTES.get_or_init(Default::default).lock() {
                n.asking.remove(&key);
                n.found.insert(key, (Instant::now(), found));
            }
        });
    }
    Some(known.map(|(_, n)| n).unwrap_or_else(|| note("asking", String::new())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new machine is told who commits there are by: the account's own
    /// name and address where it says them, else asked of GitHub from the
    /// machine, else the login and the server's no-reply form. Never a
    /// token, and never a clone that fails over an author
    #[test]
    fn a_new_machine_is_told_who_its_commits_are_by() {
        let s = crate::e2b::SignIn {
            host: "github.com".into(),
            login: "x-access-token".into(),
            token: "github_pat_SECRET".into(),
            name: None,
            email: None,
        };
        let steps = identity_steps(&s);
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert_eq!(&steps[0][..2], &["sh".to_string(), "-c".into()]);
        assert!(steps[0][2].contains("api.github.com/user") && steps[0][2].contains("users.noreply.github.com"));
        assert!(!steps[0][2].contains("SECRET") && !steps[0][2].contains("x-access-token"));
        let named = crate::e2b::SignIn { name: Some("Ada".into()), email: Some("ada@example.test".into()), ..s.clone() };
        assert_eq!(
            identity_steps(&named),
            vec![
                vec!["git", "config", "--global", "user.name", "Ada"].into_iter().map(String::from).collect::<Vec<_>>(),
                vec!["git", "config", "--global", "user.email", "ada@example.test"].into_iter().map(String::from).collect::<Vec<_>>(),
            ]
        );
        let half = crate::e2b::SignIn { name: Some("Ada".into()), ..s.clone() };
        let steps = identity_steps(&half);
        assert_eq!(steps.len(), 2, "the name given, the address asked of GitHub: {steps:?}");
        assert_eq!(steps[1][0], "sh");
        let elsewhere = crate::e2b::SignIn { host: "gitlab.example.com".into(), login: "ada".into(), ..s.clone() };
        let steps = identity_steps(&elsewhere);
        assert_eq!(steps[0][4], "ada");
        assert_eq!(steps[1][4], "ada@users.noreply.gitlab.example.com");
    }

    /// Every AI a MicroVM can be given says how to see whether it is signed
    /// in there, so the dialog is never silent about one of them. A line is
    /// one `sh` test or several joined by `||`, and never quotes the way a
    /// wrapping shell would trip on
    #[test]
    fn every_machine_ai_says_how_to_see_its_sign_in() {
        let ais = crate::profile::machine_ais();
        assert!(ais.len() >= 4, "the shipped AIs: {ais:?}");
        for a in &ais {
            let line = a.signed_in.as_deref().unwrap_or_else(|| panic!("{} has no sign-in line", a.name));
            assert!(line.starts_with("test "), "{}: {line}", a.name);
            assert!(!line.contains('\''), "{}: a single quote would end the wrapping shell's quoting: {line}", a.name);
        }
        assert_eq!(crate::profile::machine_ai("claude").map(|a| a.name), Some("Claude Code".into()));
        assert_eq!(crate::profile::machine_ai("nobody-ships-this"), None);
    }

    /// Nothing to say is nothing: no AI, an unknown one, no checkout there
    /// yet, or a checkout with no machine
    #[test]
    fn the_sign_in_is_asked_only_where_there_is_an_ai_and_a_machine() {
        let host = crate::config::HostSpec { name: "vm".into(), kind: Some("e2b".into()), ..Default::default() };
        let home = crate::config::ProjectHome {
            host: "vm".into(),
            at: "/home/user/proj".into(),
            placement: None,
            sandbox: None,
            prepared: None,
        };
        assert_eq!(ai_sign_in_note(&host, Some(&home), None, FRESH), None);
        assert_eq!(ai_sign_in_note(&host, Some(&home), Some(NO_AI), FRESH), None);
        assert_eq!(ai_sign_in_note(&host, Some(&home), Some("nobody-ships-this"), FRESH), None);
        assert_eq!(ai_sign_in_note(&host, None, Some("claude"), FRESH), None);
        assert_eq!(ai_sign_in_note(&host, Some(&home), Some("claude"), FRESH), None, "a checkout with no machine");
    }

    /// A folder on a MicroVM opens on the AI its machine was given, and on
    /// its shell when it was given none. A checkout prepared with Claude used
    /// to open on a bare prompt, with Claude installed and nothing running it
    #[test]
    fn a_folder_on_a_microvm_opens_on_the_ai_its_machine_was_given() {
        assert_eq!(
            start_with(Some("claude")),
            crate::config::Start::One { name: "claude".into(), command: "claude".into() }
        );
        assert_eq!(start_with(Some(NO_AI)), crate::config::Start::Same);
        assert_eq!(start_with(None), crate::config::Start::Same);
        assert_eq!(start_with(Some("  ")), crate::config::Start::Same);
        assert_eq!(start_with(Some("nobody-ships-this")), crate::config::Start::Same, "an AI no profile knows");
    }

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

    /// What a checkout is prepared with is the AI's install line, then each
    /// setup line in the checkout, all on screen; "none" is no AI; what is
    /// remembered as run is the same text however it was written
    #[test]
    fn a_checkout_is_prepared_with_its_ai_then_its_setup() {
        let p = Preparing::of(Some("claude"), Some("  sudo apt-get install -y php-cli \n\n composer --version"));
        let cmds = p.commands("/home/user/site").unwrap();
        // First the ground: the machine's tools told to trust its own
        // certificates, which the sign-in to the git server is carried on,
        // and a swap file for a small machine
        assert_eq!(cmds[0], ["sh", "-lc", GROUND]);
        assert!(GROUND.contains("NODE_EXTRA_CA_CERTS=/etc/ssl/certs/ca-certificates.crt") && GROUND.contains("/etc/bash.bashrc"));
        assert!(GROUND.contains("[ -e /swapfile ] ||") && GROUND.contains("swapon /swapfile"), "no swap for a small machine");
        // The AI as its maker installs it, and not again on a machine that has it
        assert_eq!(cmds[1], ["sh", "-lc", "command -v claude >/dev/null 2>&1 || { curl -fsSL https://claude.ai/install.sh | bash; }"]);
        assert_eq!(cmds[2], ["sh", "-lc", "cd /home/user/site && sudo apt-get install -y php-cli"]);
        assert_eq!(cmds.len(), 4);
        assert_eq!(p.said(), "ai: claude\nsudo apt-get install -y php-cli\ncomposer --version\n");
        let none = Preparing::of(Some("none"), None);
        assert!(none.is_empty() && none.commands("/x").unwrap() == [vec!["sh".to_string(), "-lc".into(), GROUND.into()]]);
        assert_eq!(none.said(), "ai: none\n");
        assert!(Preparing::of(Some("nobody-installs-this"), None).commands("/x").is_err(), "an AI with no install line was taken");
        // The page works out the same text to say whether it has been run
        let page = crate::webui::page();
        assert!(page.contains(r#"return "ai: " + (ai || "none") + "\n" + lines.map(l => l + "\n").join("");"#),
            "the settings page remembers what was run in another shape");
    }

    /// An answer is read line by line, each once, and a command that is more
    /// than one line is not taken for one
    #[test]
    fn a_proposed_setup_is_read_one_line_each() {
        let said = r#"Sure: {"lines":[{"command":"sudo apt-get install -y php-cli","reason":"PHP runs it"},{"command":"sudo apt-get install -y php-cli","reason":"again"},{"command":"a\nb","reason":"two"},{"command":"  ","reason":"empty"}]}"#;
        let lines = read_setup(said);
        assert_eq!(lines, vec![SetupLine { command: "sudo apt-get install -y php-cli".into(), reason: "PHP runs it".into() }]);
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
        let s = crate::e2b::SignIn { host: "github.com".into(), login: "x-access-token".into(), token: "github_pat_SECRET".into(), name: None, email: None };
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
            name: None,
            email: None,
        }));
        let note = sign_in_note("home", &given).expect("a token in hand is said at once");
        assert_eq!((note.account.as_str(), note.kind.as_str()), ("home", "classic"));
        assert!(!format!("{note:?}").contains("0123456789"), "the token is in what is said");
        let failed = sign_in_note("gone", &Err("no such account".into())).unwrap();
        assert_eq!((failed.kind.as_str(), failed.error.as_str()), ("none", "no such account"));
    }
}

#[cfg(test)]
mod after_fork_tests {
    /// The clearing ends what the terminals' account runs and nothing of the
    /// service's, says when it cannot see the processes at all, and deletes
    /// the histories only after the shells were hung up and wrote them out
    #[test]
    fn the_clearing_hangs_up_then_kills_and_only_then_deletes() {
        let s = super::AFTER_FORK;
        if let Ok(to) = std::env::var("SHIKISHA_AFTER_FORK_OUT") {
            std::fs::write(to, s).unwrap();
        }
        assert!(s.starts_with("me=user;"), "run as whoever the service's default is: {s}");
        assert!(s.contains("envd*) continue"), "the service's own agent is ended");
        assert!(s.contains("exit 3"), "a machine with no ps reads as cleared");
        let (hup, kill, rm) = (s.find("kill -HUP").unwrap(), s.find("kill -KILL").unwrap(), s.find("rm -f").unwrap());
        assert!(hup < kill && kill < rm, "the histories are deleted before the shells write them");
    }
}
