//! Work handed between AI tabs, seen through to the end.
//!
//! A person asks the AI in front of them to have other tabs do parts of a job
//! ("have <@claude> implement it and <@codex> review it until nothing is
//! left"). That AI becomes the job's **lead**; the tabs it hands tasks to are
//! its **workers**. None of it is a mode of this app: it is a handful of
//! commands -- open a job, add a task, assign it, report, read the inbox --
//! that the lead strings together as the request asks, and the record they
//! keep ([`db`]).
//!
//! **Why the record, and not the screen.** Work stops in ways nobody notices:
//! a worker ends its turn without saying whether it finished, asks the person
//! a question nobody is looking at, or its program dies; a lead is cut off in
//! the middle of reading what came back. Each of those has a counter here: a
//! report that must say done or failed, a question routed to the lead, a
//! watcher that raises what went quiet, and an inbox that hands the same mail
//! over again until the reader says it has dealt with it.
//!
//! **Why the commands, and not the words.** The lead is an AI, and a rule it
//! has to remember is one it will forget. So the rules live in the commands:
//! each refuses what cannot be right and says how to put it right, each answer
//! carries the exact command to run next (`next`), a report is taken once and
//! only from the tab and the process that was handed the work, and
//! `job_close` will not close a job that still has loose ends.
//!
//! **How it reaches the tabs.** This module decides; the runtime acts. Calls
//! arrive through the pipe ([`Call`]), a picture of the tabs arrives each tick
//! ([`Scene`]), and what should happen to a tab -- a brief typed in, a line
//! saying there is mail, an Esc, a close -- goes back as an [`Effect`] for the
//! runtime to carry out with the machinery every other hand-off uses.

pub mod db;
pub mod glue;
pub mod text;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::detect::TabState;
use db::{Assignment, Store};

/// The commands answered later rather than at once: the pipe holds their
/// line open while the loop decides (`worktree_add` is the runtime's, which
/// waits for the folder to be made)
pub const HELD: [&str; 4] = ["assign", "inbox", "ask_lead", "worktree_add"];

/// Every command this module answers
pub const METHODS: [&str; 17] = [
    "job_open",
    "job_status",
    "job_close",
    "task_add",
    "task_list",
    "task_drop",
    "assign",
    "report",
    "ask_lead",
    "answer",
    "tell",
    "inbox",
    "decision_open",
    "decision_make",
    "let_go",
    "keep",
    "stop",
];

/// How long a waiting command holds on when it names no time of its own.
/// Under the two minutes an AI's shell gives a command, so the answer "not
/// yet" arrives before the shell gives up on the line
pub const WAIT: Duration = Duration::from_secs(95);

/// How long a tab may stay busy before a brief meant for it is given up on.
/// With [`START_WAIT`] inside the time an `assign` holds its line, so the
/// lead hears how it went on the same call, not the next
const IDLE_WAIT: Duration = Duration::from_secs(45);

/// How long after a brief went in the tab has to be seen starting on it: a
/// CLI shows it is working within seconds of taking a prompt
const START_WAIT: Duration = Duration::from_secs(20);

/// How long a tab on another machine is given for its machine to wake and its
/// bridge to connect before the brief is given up on. Under the two minutes an
/// AI's shell gives the command, so the answer arrives; tried again, the
/// machine is awake by then and the bridge is quick
const BRIDGE_WAIT: Duration = Duration::from_secs(100);

/// How long something may be left undone before the watcher says so
const LEFT: Duration = Duration::from_secs(120);

/// How often the watcher looks
const WATCH_EVERY: Duration = Duration::from_secs(5);

/// How long a stopped tab has to stop before one this job opened is closed
const STOP_GRACE: Duration = Duration::from_secs(15);

/// How much of a task `task_list` shows when asked for the short form
const SHORT: usize = 120;

/// What a job the person stopped from its card is closed with
const STOPPED_BY_PERSON: &str = "stopped by the person";

/// One call from a tab, as the pipe delivered it
pub struct Call {
    /// The tab whose key made the call, by the name its key was minted under
    pub caller: Option<String>,
    pub incarnation: Option<u64>,
    pub method: String,
    pub params: Vec<Value>,
    pub reply: Sender<Result<Value, String>>,
}

/// One tab, as this module needs to know it
#[derive(Debug, Clone)]
pub struct TabFact {
    /// The id it is named by (`<@ID>`)
    pub id: String,
    /// Who it is (`Tab::uid`): what the record keeps it by, so a tab that
    /// draws the name of one closed before it is handed none of its work
    pub uid: String,
    /// The name its key is minted under (`Tab::called`)
    pub called: String,
    pub title: String,
    /// An AI CLI in a terminal: something that can run `shikisha`
    pub ai: bool,
    /// Which CLI (`claude`, `codex`, ...), for `workers:<cli>`
    pub cli: String,
    pub state: TabState,
    pub folder: Option<String>,
    /// Runs on another machine
    pub far: bool,
    /// ...and `shikisha` reaches this app from there
    pub reachable: bool,
    /// ...or will, once its machine is awake: the person agreed to the bridge
    /// there
    pub bridge: bool,
    /// Which run of its program holds a key now
    pub incarnation: Option<u64>,
    /// Its CLI follows pasted text only when a typed line asks it to
    pub typed_request: bool,
}

impl TabFact {
    fn turn_over(&self) -> bool {
        crate::asktab::turn_over(self.state)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub tabs: Vec<TabFact>,
}

impl Scene {
    /// The tab a name says (`<@ID>`, `assign t3 <tab>`): what an AI or a
    /// person calls it now
    fn by_id(&self, id: &str) -> Option<&TabFact> {
        self.tabs.iter().find(|t| t.id == id)
    }
    /// The tab a record means
    fn by_uid(&self, uid: &str) -> Option<&TabFact> {
        self.tabs.iter().find(|t| t.uid == uid)
    }
    fn by_called(&self, called: &str) -> Option<&TabFact> {
        self.tabs.iter().find(|t| t.called == called)
    }
}

/// The limits in force, read from the settings each time
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Assignments per job. 0: no limit
    pub max_assignments: u32,
    /// How deep hand-offs may nest (at least 1)
    pub max_depth: u32,
}

/// What the runtime should do to a tab
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Type `text` into the tab and submit it. `lead` goes first as typed
    /// words, and `text` as a paste
    Type { tab: String, lead: Option<String>, text: String },
    /// Press Esc in the tab
    Esc { tab: String },
    /// Close the tab
    Close { tab: String },
    /// Tell the person (a notification, and the phone's)
    Person { text: String },
    /// Open the tab's terminal, and so its machine, if it is still waiting to
    /// be looked at: a task handed to it is work for that machine to do
    Wake { tab: String },
}

struct Starting {
    assignment: i64,
    /// The tab, by uid
    tab: String,
    /// What it is called, for what is said about it
    name: String,
    reply: Option<Sender<Result<Value, String>>>,
    asked_at: Instant,
    sent_at: Option<Instant>,
    answer: Value,
    /// The tab's machine has been asked to open (a tab over there whose
    /// bridge is not up yet)
    woken: bool,
}

struct InboxWait {
    inbox: String,
    wake: Option<Vec<String>>,
    deadline: Instant,
    reply: Sender<Result<Value, String>>,
    /// Whether the command that says it is dealt with also waits again
    then_wait: bool,
}

struct AskWait {
    question: i64,
    deadline: Instant,
    reply: Sender<Result<Value, String>>,
}

struct Stopping {
    tab: String,
    job: i64,
    /// The assignment it was stopped on: a tab handed anything after it is
    /// busy with that, not with this
    assignment: i64,
    since: Instant,
}

pub struct Orchestra {
    store: Result<Store, String>,
    starting: Vec<Starting>,
    inbox_waits: Vec<InboxWait>,
    ask_waits: Vec<AskWait>,
    stopping: Vec<Stopping>,
    /// Since when each worker has sat with its turn over and no report
    quiet_since: HashMap<i64, Instant>,
    last_watch: Option<Instant>,
    /// What each tab was last written down as being called, by uid
    names: HashMap<String, String>,
    /// Bumped whenever something a person would see changes
    pub revision: u64,
}

/// `t3`, `3`, `"t3"` -- a number written the way people and AIs write it
fn number(v: &Value, prefix: char) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => {
            let s = s.trim();
            let s = s.strip_prefix(prefix).or_else(|| s.strip_prefix(prefix.to_ascii_uppercase())).unwrap_or(s);
            let s = s.strip_prefix('#').unwrap_or(s);
            s.parse().ok()
        }
        _ => None,
    }
}

/// A tab named any way an AI might name it: `<@ID>`, `@ID`, `ID`
fn tab_named(v: &Value) -> Option<String> {
    let s = v.as_str()?.trim();
    let s = s.trim_start_matches('<').trim_end_matches('>').trim_start_matches('@').trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn text_of(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Null => None,
        other if !other.is_object() => Some(other.to_string()),
        _ => None,
    }
}

/// The options object, wherever it was put: the last argument that is one
fn options(params: &[Value]) -> serde_json::Map<String, Value> {
    params
        .iter()
        .rev()
        .find_map(|p| p.as_object().cloned())
        .unwrap_or_default()
}

/// The first of `keys` the options have: the name the guide gives first, then
/// the others an AI is likely to reach for
fn option<'a>(o: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| o.get(*k))
}

/// A list given as a JSON list or as words separated by commas
fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_string())).collect(),
        Some(Value::String(s)) => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        _ => Vec::new(),
    }
}

fn words(params: &[Value]) -> Vec<&Value> {
    params.iter().filter(|p| !p.is_object()).collect()
}

fn wait_of(o: &serde_json::Map<String, Value>, default: Duration) -> Duration {
    o.get("wait_ms")
        .or_else(|| o.get("timeout_ms"))
        .and_then(Value::as_u64)
        .map(Duration::from_millis)
        .unwrap_or(default)
}

fn title_of(body: &str) -> String {
    let first = body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("task");
    let mut t: String = first.chars().take(72).collect();
    if first.chars().count() > 72 {
        t.push('…');
    }
    t
}

fn cmd(parts: &[&str]) -> String {
    let mut s = String::from("shikisha");
    for p in parts {
        s.push(' ');
        s.push_str(p);
    }
    s
}

/// How a report says it went: done, or failed, in the words an AI reaches for
fn outcome_of(word: &str) -> Option<bool> {
    match word.trim().trim_matches('"').to_lowercase().as_str() {
        "done" | "succeeded" | "success" | "completed" | "complete" | "finished" | "ok" => Some(true),
        "failed" | "fail" | "failure" | "not done" | "error" | "blocked" => Some(false),
        _ => None,
    }
}

/// The command a new AI tab of `ai` starts with: the CLI's own command, and
/// its "act without asking" flag only when the person's Yolo setting puts it
/// on every new AI tab. Never a command anyone wrote -- which is what makes
/// `open_ai_tab` safe to open to an AI
pub fn ai_command(ai: &str) -> Result<String, String> {
    let want = ai.trim().trim_start_matches('@').to_lowercase();
    let files = crate::profile::files();
    let head = files
        .iter()
        .find(|pf| {
            pf.command_match.first().is_some_and(|c| c.trim().eq_ignore_ascii_case(&want))
                || pf.name.to_lowercase() == want
        })
        .and_then(|pf| pf.command_match.first())
        .map(|c| c.trim().to_string())
        .ok_or_else(|| {
            let known: Vec<String> = files.iter().filter_map(|pf| pf.command_match.first().cloned()).collect();
            format!("{ai} is not an AI this app knows; one of: {}", known.join(", "))
        })?;
    let yolo = crate::config::load().map(|c| c.yolo).unwrap_or(false);
    Ok(match (yolo, crate::tab::bypass_flag(&head)) {
        (true, Some(flag)) => format!("{head} {flag}"),
        _ => head,
    })
}

impl Default for Orchestra {
    fn default() -> Self {
        Self::open(&crate::config::state_path("orchestration.db"))
    }
}

impl Orchestra {
    pub fn open(path: &std::path::Path) -> Self {
        let store = Store::open(path).map_err(|e| format!("{e:#}"));
        if let Err(e) = &store {
            crate::append_hook_log(&format!("orchestration: the record could not be opened: {e}"));
        }
        Self::with(store)
    }

    pub fn in_memory() -> Self {
        Self::with(Store::in_memory().map_err(|e| format!("{e:#}")))
    }

    fn with(store: Result<Store, String>) -> Self {
        Self {
            store,
            starting: Vec::new(),
            inbox_waits: Vec::new(),
            ask_waits: Vec::new(),
            stopping: Vec::new(),
            quiet_since: HashMap::new(),
            last_watch: None,
            names: HashMap::new(),
            revision: 0,
        }
    }

    /// Rewrite what an older version wrote under tabs' names, once, under
    /// their uids: `tabs` is every tab of the settings, by name and uid
    /// ([`Store::adopt_uids`]). Called before anything else is written
    pub fn adopt(&mut self, tabs: &[(String, String)]) {
        match self.store().and_then(|s| s.adopt_uids(tabs).map_err(|e| e.to_string())) {
            Ok(true) => crate::append_hook_log(&format!(
                "orchestration: the record now knows tabs by their uids ({} tabs in the settings)",
                tabs.len()
            )),
            Ok(false) => {}
            Err(e) => crate::append_hook_log(&format!("orchestration: the tabs' uids could not be written: {e}")),
        }
    }

    /// What every tab on the screen is called now, written down where it
    /// changed: what is said about a tab says the name it goes by
    fn name_tabs(&mut self, scene: &Scene) {
        let at = db::now_ms();
        for t in &scene.tabs {
            if self.names.get(&t.uid) == Some(&t.id) {
                continue;
            }
            let Ok(store) = self.store.as_mut() else { return };
            if store.named(&t.uid, &t.id, at).is_ok() {
                self.names.insert(t.uid.clone(), t.id.clone());
            }
        }
    }

    pub fn handles(method: &str) -> bool {
        METHODS.contains(&method)
    }

    fn store(&mut self) -> Result<&mut Store, String> {
        self.store
            .as_mut()
            .map_err(|e| format!("the record of handed work is unavailable: {e}"))
    }

    /// Something the person can see changed
    fn touched(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    // -- entry points --------------------------------------------------------

    /// Take one call. Answered now, or held and answered by a later [`tick`]
    pub fn call(&mut self, c: Call, scene: &Scene, named: &HashSet<String>, limits: Limits) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.name_tabs(scene);
        let me = c.caller.as_deref().and_then(|called| scene.by_called(called)).cloned();
        let result = match c.method.as_str() {
            "assign" => match self.assign(&c, me.as_ref(), scene, named, limits) {
                Ok(s) => {
                    self.starting.push(Starting { reply: Some(c.reply), ..s });
                    return effects;
                }
                Err(e) => Err(e),
            },
            "inbox" => match self.inbox(&c, me.as_ref()) {
                Ok(Held::Now(v)) => Ok(v),
                Ok(Held::Later(w)) => {
                    self.inbox_waits.push(InboxWait { reply: c.reply, ..w });
                    return effects;
                }
                Err(e) => Err(e),
            },
            "ask_lead" => match self.ask_lead(&c, me.as_ref()) {
                Ok(Held::Now(v)) => Ok(v),
                Ok(Held::Later(w)) => {
                    self.ask_waits.push(AskWait { reply: c.reply, ..w });
                    return effects;
                }
                Err(e) => Err(e),
            },
            "job_open" => self.job_open(&c, me.as_ref(), limits),
            "job_status" => self.job_status(&c, me.as_ref(), scene),
            "job_close" => self.job_close(&c, me.as_ref(), scene),
            "task_add" => self.task_add(&c, me.as_ref()),
            "task_list" => self.task_list(&c, me.as_ref(), scene),
            "task_drop" => self.task_drop(&c, me.as_ref(), scene),
            "report" => self.report(&c, me.as_ref()),
            "answer" => self.answer_call(&c, me.as_ref()),
            "tell" => self.tell(&c, me.as_ref(), scene),
            "decision_open" => self.decision_open(&c, me.as_ref(), &mut effects),
            "decision_make" => self.decision_make_call(&c, me.as_ref()),
            "let_go" => self.let_go(&c, me.as_ref(), scene, &mut effects),
            "keep" => self.keep(&c, me.as_ref(), scene),
            "stop" => self.stop(&c, me.as_ref(), scene, &mut effects),
            other => Err(format!("{other} is not a command of this app")),
        };
        match &result {
            Ok(_) => self.touched(),
            // Said to the AI that asked; written down for whoever looks into why
            // a job stopped
            Err(e) => crate::append_hook_log(&format!(
                "orchestration: {} refused for {}: {}",
                c.method,
                me.as_ref().map(|t| t.id.as_str()).unwrap_or("outside"),
                e.chars().take(200).collect::<String>()
            )),
        }
        let _ = c.reply.send(result);
        effects
    }

    // -- who is calling ------------------------------------------------------

    fn caller_id(me: Option<&TabFact>) -> Result<&TabFact, String> {
        me.ok_or_else(|| "only an AI tab in SHIKISHA-TERM can do this (run it inside one)".to_string())
    }

    /// The job a lead means: the one it named, or the newest it leads
    fn led_job(&mut self, me: &TabFact, o: &serde_json::Map<String, Value>) -> Result<db::Job, String> {
        let named = o.get("job").and_then(|v| number(v, 'j'));
        let store = self.store()?;
        let jobs = store.jobs_led_by(&me.uid).map_err(|e| e.to_string())?;
        match named {
            Some(id) => jobs
                .into_iter()
                .find(|j| j.id == id)
                .ok_or_else(|| format!("j{id} is not an open job led by this tab")),
            None => jobs.into_iter().next().ok_or_else(|| {
                format!(
                    "this tab leads no open job. Start one first: {}",
                    cmd(&["job_open", "\"<the whole job, in one sentence>\""])
                )
            }),
        }
    }

    /// The assignment a worker is on, checked against the process given it
    fn my_assignment(&mut self, me: &TabFact, incarnation: Option<u64>) -> Result<Option<Assignment>, String> {
        let store = self.store()?;
        let a = store.active_for_tab(&me.uid).map_err(|e| e.to_string())?;
        if let Some(a) = &a
            && let (Some(given), Some(now)) = (a.process, incarnation)
            && given as u64 != now
        {
            return Err(format!(
                "a{} was handed to an earlier run of this tab's program; this one was not given it",
                a.id
            ));
        }
        Ok(a)
    }

    fn depth_of(&mut self, me: &TabFact) -> i64 {
        self.store
            .as_ref()
            .ok()
            .and_then(|s| s.active_for_tab(&me.uid).ok().flatten())
            .map(|a| a.depth)
            .unwrap_or(0)
    }

    // -- jobs ----------------------------------------------------------------

    fn job_open(&mut self, c: &Call, me: Option<&TabFact>, limits: Limits) -> Result<Value, String> {
        let me = Self::caller_id(me)?;
        let goal = words(&c.params)
            .first()
            .and_then(|v| text_of(Some(v)))
            .ok_or_else(|| "job_open needs the job, in one sentence".to_string())?;
        let depth = self.depth_of(me);
        if depth >= i64::from(limits.max_depth) {
            return Err(format!(
                "this tab is itself a worker at depth {depth}, and work may nest {} deep: do this task yourself",
                limits.max_depth
            ));
        }
        let job = self.store()?.job_open(&me.uid, &goal).map_err(|e| e.to_string())?;
        Ok(json!({
            "job": format!("j{}", job.id),
            "goal": job.goal,
            "next": [cmd(&["task_add", "\"<task>\""])],
        }))
    }

    fn job_status(&mut self, c: &Call, me: Option<&TabFact>, scene: &Scene) -> Result<Value, String> {
        let me = Self::caller_id(me)?;
        let o = options(&c.params);
        let job = self.led_job(me, &o)?;
        self.status_of(job.id, scene)
    }

    /// Everything about a job, and what to do next, in one answer
    fn status_of(&mut self, job: i64, scene: &Scene) -> Result<Value, String> {
        let store = self.store()?;
        let j = store.job(job).map_err(|e| e.to_string())?.ok_or("no such job")?;
        let tasks = store.tasks(job).map_err(|e| e.to_string())?;
        let assignments = store.assignments(job).map_err(|e| e.to_string())?;
        let questions = store.open_questions(job).map_err(|e| e.to_string())?;
        let decisions = store.decisions(job).map_err(|e| e.to_string())?;
        let unread = store.unread(&format!("job:{job}")).map_err(|e| e.to_string())?;
        let task_rows: Vec<Value> = tasks
            .iter()
            .map(|t| {
                let last = assignments.iter().rev().find(|a| a.task == t.id);
                json!({
                    "task": format!("t{}", t.id),
                    "title": t.title,
                    "state": t.state,
                    "why": t.why,
                    "waits_on": t.waits_on.iter().map(|w| format!("t{w}")).collect::<Vec<_>>(),
                    "tab": last.map(|a| a.tab_name.clone()),
                    "assignment": last.map(|a| format!("a{}", a.id)),
                    "tries": assignments.iter().filter(|a| a.task == t.id).count(),
                })
            })
            .collect();
        let loose = self.loose_ends(job, scene)?;
        let mut next: Vec<String> = Vec::new();
        if !unread.is_empty() {
            next.push(cmd(&["inbox"]));
        }
        for q in &questions {
            next.push(cmd(&["answer", &format!("q{}", q.id), "\"<answer>\""]));
        }
        // Open tasks come through the loose ends, with everything else left
        for l in &loose {
            if let Some(n) = l.get("next").and_then(Value::as_str)
                && !next.iter().any(|x| x == n)
            {
                next.push(n.to_string());
            }
        }
        let working = assignments.iter().any(|a| a.active());
        if next.is_empty() && working {
            next.push(cmd(&["inbox", "wait"]));
        }
        if next.is_empty() && tasks.is_empty() {
            next.push(cmd(&["task_add", "\"<task>\""]));
        }
        if next.is_empty() && loose.is_empty() {
            next.push(cmd(&["job_close", "\"<what was done>\""]));
        }
        if next.is_empty() {
            next.push("Settle each of loose_ends above; each says how.".to_string());
        }
        Ok(json!({
            "job": format!("j{}", j.id),
            "goal": j.goal,
            "state": j.state,
            "tasks": task_rows,
            "working": assignments.iter().filter(|a| a.active()).map(|a| json!({
                "assignment": format!("a{}", a.id), "task": format!("t{}", a.task), "tab": a.tab_name,
                "state": scene.by_uid(&a.tab).map(|t| t.state.label()).unwrap_or("GONE"),
            })).collect::<Vec<_>>(),
            "questions": questions.iter().map(|q| json!({"question": format!("q{}", q.id), "asks": q.question})).collect::<Vec<_>>(),
            "decisions": decisions.iter().filter(|d| d.state == "open").map(|d| json!({
                "decision": format!("d{}", d.id), "task": format!("t{}", d.task), "asks": d.question, "who": d.decided_by,
            })).collect::<Vec<_>>(),
            "unread": unread.len(),
            "loose_ends": loose,
            "next": next,
        }))
    }

    /// What is left open in a job: each with why and the command that settles it
    fn loose_ends(&mut self, job: i64, scene: &Scene) -> Result<Vec<Value>, String> {
        let store = self.store()?;
        let mut out = Vec::new();
        let assignments = store.assignments(job).map_err(|e| e.to_string())?;
        for a in &assignments {
            if a.active() {
                out.push(json!({
                    "what": format!("a{} (t{}) is still being worked on by <@{}>", a.id, a.task, a.tab_name),
                    "next": cmd(&["inbox", "wait"]),
                }));
            } else if a.afterwards.is_none() && scene.by_uid(&a.tab).is_some() {
                out.push(json!({
                    "what": format!("a{} is over; nobody has said whether its tab <@{}> is let go or kept", a.id, a.tab_name),
                    "next": cmd(&["let_go", &format!("a{}", a.id)]),
                }));
            }
        }
        // A job is done when every task is: done, or dropped by the lead on
        // purpose. One not started, waiting or held is work still to do
        let deciding: HashSet<i64> = store
            .decisions(job)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|d| d.state == "open")
            .map(|d| d.task)
            .collect();
        let drop = |id: i64| cmd(&["task_drop", &format!("t{id}"), "\"<why it is not needed>\""]);
        for t in store.tasks(job).map_err(|e| e.to_string())? {
            let again = cmd(&["assign", &format!("t{}", t.id), "<tab>"]);
            match t.state.as_str() {
                "open" => out.push(json!({
                    "what": format!("t{} has not been handed to anyone ({})", t.id, t.title),
                    "next": again,
                })),
                "waiting" => out.push(json!({
                    "what": format!(
                        "t{} waits on {} and has not started; if it is no longer needed: {}",
                        t.id,
                        t.waits_on.iter().map(|w| format!("t{w}")).collect::<Vec<_>>().join(", "),
                        drop(t.id)
                    ),
                })),
                // Its decision is listed below
                "held" if deciding.contains(&t.id) => {}
                "held" | "failed" => out.push(json!({
                    "what": format!(
                        "t{} is {}{}; assign it again, or if it is no longer needed: {}",
                        t.id,
                        t.state,
                        t.why.as_deref().map(|w| format!(" ({w})")).unwrap_or_default(),
                        drop(t.id)
                    ),
                    "next": again,
                })),
                _ => {}
            }
        }
        for q in store.open_questions(job).map_err(|e| e.to_string())? {
            out.push(json!({
                "what": format!("q{} is unanswered: {}", q.id, q.question),
                "next": cmd(&["answer", &format!("q{}", q.id), "\"<answer>\""]),
            }));
        }
        for d in store.decisions(job).map_err(|e| e.to_string())? {
            if d.state == "open" {
                out.push(json!({
                    "what": format!("d{} waits for a decision ({}): {}", d.id, d.decided_by, d.question),
                    "next": cmd(&["inbox", "wait"]),
                }));
            }
        }
        let given: HashSet<&str> = assignments.iter().map(|a| a.tab.as_str()).collect();
        for o in store.opened_by(job).map_err(|e| e.to_string())? {
            if o.held_by == "job" && !given.contains(o.tab.as_str()) && scene.by_uid(&o.tab).is_some() {
                out.push(json!({
                    "what": format!("<@{}> was opened for this job and was never given a task", o.tab_name),
                    "next": cmd(&["assign", "<task>", &o.tab_name]),
                }));
            }
        }
        if store.given(&format!("job:{job}")).map_err(|e| e.to_string())?.is_some()
            || !store.unread(&format!("job:{job}")).map_err(|e| e.to_string())?.is_empty()
        {
            out.push(json!({
                "what": "the inbox has mail not yet dealt with",
                "next": cmd(&["inbox"]),
            }));
        }
        Ok(out)
    }

    fn job_close(&mut self, c: &Call, me: Option<&TabFact>, scene: &Scene) -> Result<Value, String> {
        let me = Self::caller_id(me)?;
        let o = options(&c.params);
        let job = self.led_job(me, &o)?;
        let outcome = words(&c.params).first().and_then(|v| text_of(Some(v))).unwrap_or_default();
        let loose = self.loose_ends(job.id, scene)?;
        if !loose.is_empty() {
            let list: Vec<String> = loose
                .iter()
                .map(|l| match l["next"].as_str() {
                    Some(n) => format!("- {} -> {n}", l["what"].as_str().unwrap_or_default()),
                    None => format!("- {}", l["what"].as_str().unwrap_or_default()),
                })
                .collect();
            return Err(format!("j{} still has loose ends:\n{}", job.id, list.join("\n")));
        }
        self.store()?.job_close(job.id, &outcome).map_err(|e| e.to_string())?;
        crate::append_hook_log(&format!("orchestration: j{} closed: {}", job.id, outcome));
        Ok(json!({"job": format!("j{}", job.id), "state": "closed", "next": ["Tell the person how it went."]}))
    }

    // -- tasks ---------------------------------------------------------------

    fn task_add(&mut self, c: &Call, me: Option<&TabFact>) -> Result<Value, String> {
        let me = Self::caller_id(me)?;
        let o = options(&c.params);
        let job = self.led_job(me, &o)?;
        let body = words(&c.params).first().and_then(|v| text_of(Some(v))).ok_or_else(|| {
            "task_add needs the task: where, what to end up with, what must not change, how to tell it is done"
                .to_string()
        })?;
        let mut waits_on = Vec::new();
        if let Some(list) = option(&o, &["waits_on", "after", "deps", "depends_on"]) {
            let items: Vec<Value> = match list {
                Value::Array(a) => a.clone(),
                Value::String(s) => s.split(',').map(|x| json!(x.trim())).collect(),
                other => vec![other.clone()],
            };
            for w in items {
                waits_on.push(number(&w, 't').ok_or_else(|| format!("{w} is not a task (write it t3)"))?);
            }
        }
        let title = o.get("title").and_then(|v| text_of(Some(v))).unwrap_or_else(|| title_of(&body));
        let t = self
            .store()?
            .task_add(job.id, &title, &body, &waits_on)
            .map_err(|e| e.to_string())?;
        crate::append_hook_log(&format!("orchestration: t{} added to j{}: {}", t.id, job.id, t.title));
        let next = if t.state == "open" {
            vec![cmd(&["assign", &format!("t{}", t.id), "<tab>"])]
        } else {
            vec![format!(
                "t{} opens once {} {} done; assign it then",
                t.id,
                waits_on.iter().map(|w| format!("t{w}")).collect::<Vec<_>>().join(", "),
                if waits_on.len() == 1 { "is" } else { "are" }
            )]
        };
        Ok(json!({"task": format!("t{}", t.id), "title": t.title, "state": t.state, "next": next}))
    }

    /// Take a task out of the job on purpose: it will not be done, and the job
    /// can close without it. Said with why, which the lead's own record keeps
    fn task_drop(&mut self, c: &Call, me: Option<&TabFact>, scene: &Scene) -> Result<Value, String> {
        let me = Self::caller_id(me)?;
        let w = words(&c.params);
        let task = w.first().and_then(|v| number(v, 't')).ok_or_else(|| {
            format!("task_drop needs the task and why: {}", cmd(&["task_drop", "t1", "\"<why it is not needed>\""]))
        })?;
        let why = w
            .get(1)
            .and_then(|v| text_of(Some(v)))
            .ok_or_else(|| format!("task_drop needs why t{task} is not needed, for the record"))?;
        let store = self.store()?;
        let t = store.task(task).map_err(|e| e.to_string())?.ok_or_else(|| format!("there is no task t{task}"))?;
        let job = store.job(t.job).map_err(|e| e.to_string())?.ok_or("no such job")?;
        if job.lead != me.uid {
            return Err(format!("t{task} belongs to <@{}>'s job", job.lead_name));
        }
        if t.state == "working" {
            let a = store.assignments(job.id).map_err(|e| e.to_string())?.into_iter().rev().find(|a| a.task == task && a.active());
            return Err(format!(
                "t{task} is being worked on; stop it first: {}",
                cmd(&["stop", &a.map(|a| format!("a{}", a.id)).unwrap_or_else(|| "<assignment>".into())])
            ));
        }
        store.task_drop(task, &why).map_err(|e| e.to_string())?;
        crate::append_hook_log(&format!("orchestration: t{task} dropped from j{}: {why}", job.id));
        let waiting: Vec<String> = store
            .tasks(job.id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|x| x.state == "waiting" && x.waits_on.contains(&task))
            .map(|x| format!("t{}", x.id))
            .collect();
        let mut v = json!({"task": format!("t{task}"), "state": "dropped"});
        if !waiting.is_empty() {
            v["note"] = json!(format!(
                "{} waited on t{task} and will not open now: drop them too, or add a task they can wait on instead",
                waiting.join(", ")
            ));
        }
        v["next"] = self.status_of(job.id, scene)?["next"].clone();
        Ok(v)
    }

    fn task_list(&mut self, c: &Call, me: Option<&TabFact>, scene: &Scene) -> Result<Value, String> {
        let me = Self::caller_id(me)?;
        let o = options(&c.params);
        let job = self.led_job(me, &o)?;
        let open_only = o.get("open").and_then(Value::as_bool).unwrap_or(false)
            || words(&c.params).iter().any(|w| w.as_str() == Some("open"));
        let short = o.get("short").and_then(Value::as_bool).unwrap_or(false)
            || words(&c.params).iter().any(|w| w.as_str() == Some("short"));
        let tasks = self.store()?.tasks(job.id).map_err(|e| e.to_string())?;
        let rows: Vec<Value> = tasks
            .into_iter()
            .filter(|t| !open_only || t.state == "open")
            .map(|t| {
                let body = if short {
                    let flat = t.body.split_whitespace().collect::<Vec<_>>().join(" ");
                    let cut: String = flat.chars().take(SHORT).collect();
                    json!({"text": cut, "cut": flat.chars().count() > SHORT})
                } else {
                    json!(t.body)
                };
                json!({
                    "task": format!("t{}", t.id), "title": t.title, "state": t.state,
                    "waits_on": t.waits_on.iter().map(|w| format!("t{w}")).collect::<Vec<_>>(),
                    "why": t.why, "body": body,
                })
            })
            .collect();
        let next = self.status_of(job.id, scene)?["next"].clone();
        Ok(json!({"job": format!("j{}", job.id), "tasks": rows, "next": next}))
    }

    // -- assign --------------------------------------------------------------

    fn assign(
        &mut self,
        c: &Call,
        me: Option<&TabFact>,
        scene: &Scene,
        named: &HashSet<String>,
        limits: Limits,
    ) -> Result<Starting, String> {
        let me = Self::caller_id(me)?;
        let w = words(&c.params);
        let task = w
            .first()
            .and_then(|v| number(v, 't'))
            .ok_or_else(|| format!("assign needs a task and a tab: {}", cmd(&["assign", "t1", "<tab>"])))?;
        let target = w.get(1).and_then(|v| tab_named(v)).ok_or_else(|| {
            format!("assign needs a tab to hand t{task} to: {}", cmd(&["assign", &format!("t{task}"), "<tab>"]))
        })?;
        let store = self.store()?;
        let t = store
            .task(task)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("there is no task t{task}"))?;
        let job = store
            .job(t.job)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("t{task} has no job"))?;
        if job.lead != me.uid {
            return Err(format!("t{task} belongs to j{}, led by <@{}>, not this tab", job.id, job.lead_name));
        }
        if job.state != "open" {
            return Err(format!("j{} is closed", job.id));
        }
        // The tab the name says now: what the record keeps it by is its uid
        let Some(tab) = scene.by_id(&target) else {
            return Err(format!(
                "there is no tab <@{target}> on this desk. Open one: {}",
                cmd(&["open_ai_tab", "claude"])
            ));
        };
        let uid = tab.uid.clone();
        // The same task to the same tab again, while it is on it: the
        // assignment in hand, not a second one
        if let Some(a) = store.active_for_tab(&uid).map_err(|e| e.to_string())?
            && a.task == task
        {
            return Err(format!(
                "a{} already has <@{target}> on t{task}; wait for its report: {}",
                a.id,
                cmd(&["inbox", "wait"])
            ));
        }
        if tab.uid == me.uid {
            return Err("a lead does not hand work to itself: do it, or assign it to another tab".into());
        }
        if !tab.ai {
            return Err(format!(
                "<@{target}> is not an AI tab: a terminal runs commands (shikisha tab_run), a page is driven (shikisha browser_do)"
            ));
        }
        if tab.far && !tab.reachable && !tab.bridge {
            return Err(format!(
                "<@{target}> runs on another machine whose bridge is not connected, so it could not report back. \
                 If the person has not put the bridge on that machine, ask them to: Settings > Where it runs > that machine > \
                 Put the bridge on this machine. Until then, use shikisha ask_tab"
            ));
        }
        let ours = store
            .opener_of(&uid)
            .map_err(|e| e.to_string())?
            .is_some_and(|o| o.job == job.id && o.held_by != "gone");
        // Named by who it is (`glue::named_key`): the tab the person named,
        // not another given its id since
        if !named.contains(&uid) && !ours {
            return Err(format!(
                "the person has not named <@{target}> in what they asked you, and this job did not open it. \
                 Ask the person to name it with @, or open a new tab for the job: {}",
                cmd(&["open_ai_tab", if tab.cli.is_empty() { "claude" } else { &tab.cli }])
            ));
        }
        // Held by a stop, or failed: assigning it again is the lead deciding to
        // try once more. Held for a decision is not the lead's to undo
        let deciding = store
            .decisions(job.id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|d| d.task == task && d.state == "open");
        let again = t.state == "failed" || (t.state == "held" && deciding.is_none());
        if !again && t.state != "open" {
            return Err(match t.state.as_str() {
                "waiting" => format!(
                    "t{task} waits on {} and is not open yet",
                    t.waits_on.iter().map(|w| format!("t{w}")).collect::<Vec<_>>().join(", ")
                ),
                "held" => format!(
                    "t{task} is held for a decision: {}",
                    deciding.map(|d| format!("d{} {}", d.id, d.question)).unwrap_or_default()
                ),
                "working" => format!("t{task} is already being worked on: {}", cmd(&["job_status"])),
                "done" => format!("t{task} is already done"),
                "dropped" => format!("t{task} was dropped from the job; add a new task instead"),
                other => format!("t{task} is {other}"),
            });
        }
        let depth = self.depth_of(me) + 1;
        if depth > i64::from(limits.max_depth) {
            return Err(format!(
                "this tab is itself a worker, and work may nest {} deep: do t{task} yourself",
                limits.max_depth
            ));
        }
        let store = self.store()?;
        let count = store.assignments(job.id).map_err(|e| e.to_string())?.len() as u32;
        if limits.max_assignments > 0 && count >= limits.max_assignments {
            return Err(format!(
                "j{} has used {} of its {} assignments (Settings > Limits on handing work): stop and report to the person",
                job.id, count, limits.max_assignments
            ));
        }
        if self.starting.iter().any(|s| s.tab == uid) {
            return Err(format!("<@{target}> is being handed another task right now"));
        }
        let store = self.store()?;
        // Reopened only now, once nothing above has refused it
        if again {
            store.reopen(task).map_err(|e| e.to_string())?;
        }
        let a = store
            .assign(task, &uid, tab.incarnation.map(|n| n as i64), depth)
            .map_err(|e| e.to_string())?;
        crate::append_hook_log(&format!("orchestration: a{} t{task} -> {target} (j{})", a.id, job.id));
        let answer = json!({
            "assignment": format!("a{}", a.id),
            "task": format!("t{task}"),
            "tab": target,
            "round": format!("{}/{}", count + 1, if limits.max_assignments == 0 { "∞".to_string() } else { limits.max_assignments.to_string() }),
            "next": [cmd(&["inbox", "wait"])],
        });
        Ok(Starting {
            assignment: a.id,
            tab: uid,
            name: target,
            reply: None,
            asked_at: Instant::now(),
            sent_at: None,
            answer,
            woken: false,
        })
    }

    // -- worker's side -------------------------------------------------------

    fn report(&mut self, c: &Call, me: Option<&TabFact>) -> Result<Value, String> {
        let me = Self::caller_id(me)?.clone();
        let w = words(&c.params);
        let o = options(&c.params);
        let how = || {
            format!(
                "report says done or failed, then what you did, what you found, and what is left: {}",
                cmd(&["report", "done", "\"<what you did>\"", "\"<what you found>\"", "\"<what is left, or: nothing>\""])
            )
        };
        let done = w
            .first()
            .and_then(|v| v.as_str())
            .or_else(|| option(&o, &["outcome", "result"]).and_then(Value::as_str))
            .and_then(outcome_of)
            .ok_or_else(how)?;
        let part = |i: usize, keys: &[&str]| w.get(i).and_then(|v| text_of(Some(v))).or_else(|| text_of(option(&o, keys)));
        let did = part(1, &["did", "summary"]).ok_or_else(how)?;
        let found = part(2, &["found"]);
        let left = part(3, &["left"]);
        let named = option(&o, &["assignment"]).and_then(|v| number(v, 'a'));
        let a = match self.my_assignment(&me, c.incarnation)? {
            Some(a) => a,
            None => {
                // Already reported, or never given anything
                let last = self.store()?.last_for_tab(&me.uid).map_err(|e| e.to_string())?;
                return match last {
                    Some(a) if a.over() => Ok(json!({
                        "assignment": format!("a{}", a.id), "state": "already reported",
                        "next": ["Stop here and wait at the prompt."],
                    })),
                    _ => Err("this tab has no assignment to report on".into()),
                };
            }
        };
        if let Some(n) = named
            && n != a.id
        {
            return Err(format!("this tab is on a{}, not a{n}", a.id));
        }
        let files = strings(o.get("files"));
        let outcome = if done { "done" } else { "failed" };
        let result = json!({
            "outcome": outcome,
            "did": did,
            "found": found,
            "left": left,
            "files": files,
            "report_path": o.get("report_path").cloned().unwrap_or(Value::Null),
        });
        let store = self.store()?;
        let reported = store.report(a.id, done, &result).map_err(|e| e.to_string())?;
        if reported == db::Reported::Now {
            let title = store.task(a.task).map_err(|e| e.to_string())?.map(|t| t.title).unwrap_or_default();
            let mut body = format!("Did: {did}");
            if let Some(f) = &found {
                body.push_str(&format!("\nFound: {f}"));
            }
            if let Some(l) = &left {
                body.push_str(&format!("\nLeft: {l}"));
            }
            store
                .post(
                    Some(a.job),
                    &format!("job:{}", a.job),
                    &me.id,
                    "report",
                    &format!("t{} {outcome}: {title}", a.task),
                    &body,
                    &json!({"assignment": format!("a{}", a.id), "task": format!("t{}", a.task), "outcome": outcome, "files": files}),
                )
                .map_err(|e| e.to_string())?;
            self.quiet_since.remove(&a.id);
            crate::append_hook_log(&format!("orchestration: a{} reported {outcome}", a.id));
        }
        Ok(json!({
            "assignment": format!("a{}", a.id),
            "state": if reported == db::Reported::Now { "reported" } else { "already reported" },
            "next": ["Stop here and wait at the prompt."],
        }))
    }

    fn ask_lead(&mut self, c: &Call, me: Option<&TabFact>) -> Result<Held<AskWait>, String> {
        let me = Self::caller_id(me)?.clone();
        let o = options(&c.params);
        let wait = wait_of(&o, WAIT);
        let a = self
            .my_assignment(&me, c.incarnation)?
            .ok_or("ask_lead is for a tab working on an assignment; this one has none")?;
        let q = if let Some(resume) = o.get("resume").and_then(|v| number(v, 'q')) {
            let q = self
                .store()?
                .question(resume)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("there is no question q{resume}"))?;
            if q.assignment != a.id {
                return Err(format!("q{resume} was not asked on this assignment"));
            }
            q
        } else {
            let question = words(&c.params)
                .first()
                .and_then(|v| text_of(Some(v)))
                .ok_or("ask_lead needs the question")?;
            let choices = strings(option(&o, &["choices", "options"]));
            self.store()?
                .ask(a.job, a.id, &me.id, &question, &choices)
                .map_err(|e| e.to_string())?
        };
        if let Some(v) = Self::answered(&q) {
            return Ok(Held::Now(v));
        }
        Ok(Held::Later(AskWait {
            question: q.id,
            deadline: Instant::now() + wait,
            reply: c.reply.clone(),
        }))
    }

    fn answered(q: &db::Question) -> Option<Value> {
        match q.state.as_str() {
            "answered" => Some(json!({
                "question": format!("q{}", q.id),
                "answer": q.answer,
                "next": ["Carry on with the task."],
            })),
            "closed" => Some(json!({
                "question": format!("q{}", q.id),
                "state": "closed",
                "why": "the assignment it was asked on is over",
                "next": ["Stop here and wait at the prompt."],
            })),
            _ => None,
        }
    }

    fn answer_call(&mut self, c: &Call, me: Option<&TabFact>) -> Result<Value, String> {
        let me = Self::caller_id(me)?.clone();
        let w = words(&c.params);
        let id = w
            .first()
            .and_then(|v| number(v, 'q'))
            .ok_or_else(|| format!("answer needs the question: {}", cmd(&["answer", "q1", "\"<answer>\""])))?;
        let text = w.get(1).and_then(|v| text_of(Some(v))).ok_or("answer needs the answer")?;
        let store = self.store()?;
        let q = store
            .question(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("there is no question q{id}"))?;
        let job = store.job(q.job).map_err(|e| e.to_string())?.ok_or("no such job")?;
        if job.lead != me.uid {
            return Err(format!("q{id} is for <@{}>, the lead of j{}", job.lead_name, job.id));
        }
        store.answer(id, &text).map_err(|e| e.to_string())?;
        // The worker hears it as the answer to its call if it is still
        // waiting, and in its inbox otherwise
        if !self.ask_waits.iter().any(|a| a.question == id) {
            self.store()?
                .post(
                    Some(q.job),
                    &format!("assignment:{}", q.assignment),
                    &me.id,
                    "answer",
                    &format!("Answer to q{id}"),
                    &text,
                    &json!({"question": format!("q{id}")}),
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(json!({"question": format!("q{id}"), "state": "answered", "next": [cmd(&["inbox", "wait"])]}))
    }

    fn tell(&mut self, c: &Call, me: Option<&TabFact>, scene: &Scene) -> Result<Value, String> {
        let me = Self::caller_id(me)?.clone();
        let w = words(&c.params);
        let to = w.first().and_then(|v| v.as_str()).map(str::trim).unwrap_or_default().to_string();
        let body = w.get(1).and_then(|v| text_of(Some(v))).ok_or("tell needs who it is for and the text")?;
        let o = options(&c.params);
        // A worker's note goes to its lead, whatever it named
        if let Some(a) = self.my_assignment(&me, c.incarnation)?
            && !to.starts_with("assignment:")
            && !to.starts_with('a')
            && !to.starts_with("workers")
        {
            self.store()?
                .post(
                    Some(a.job),
                    &format!("job:{}", a.job),
                    &me.id,
                    "note",
                    &format!("From a{}", a.id),
                    &body,
                    &json!({"assignment": format!("a{}", a.id)}),
                )
                .map_err(|e| e.to_string())?;
            return Ok(json!({"sent": format!("job:{}", a.job), "next": ["Carry on with the task."]}));
        }
        let job = self.led_job(&me, &o)?;
        let store = self.store()?;
        let active: Vec<Assignment> = store
            .assignments(job.id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|a| a.active())
            .collect();
        let targets: Vec<&Assignment> = if let Some(group) = to.strip_prefix("workers") {
            let group = group.trim_start_matches(':');
            active
                .iter()
                .filter(|a| match group {
                    "" => true,
                    "idle" => scene.by_uid(&a.tab).is_some_and(|t| t.turn_over()),
                    cli => scene.by_uid(&a.tab).is_some_and(|t| t.cli.eq_ignore_ascii_case(cli)),
                })
                .collect()
        } else {
            let id = number(&json!(to.trim_start_matches("assignment:")), 'a')
                .ok_or_else(|| format!("send it to a<N>, or to workers / workers:idle / workers:<cli> -- not {to}"))?;
            active.iter().filter(|a| a.id == id).collect()
        };
        if targets.is_empty() {
            return Err(format!("nobody working on j{} matches {to}", job.id));
        }
        let mut sent = Vec::new();
        for a in targets {
            store
                .post(Some(job.id), &format!("assignment:{}", a.id), &me.id, "note", "From the lead", &body, &json!({}))
                .map_err(|e| e.to_string())?;
            sent.push(format!("a{}", a.id));
        }
        Ok(json!({"sent": sent, "next": [cmd(&["inbox", "wait"])]}))
    }

    fn inbox(&mut self, c: &Call, me: Option<&TabFact>) -> Result<Held<InboxWait>, String> {
        let me = Self::caller_id(me)?.clone();
        let o = options(&c.params);
        let w = words(&c.params);
        let wants_wait = o.get("wait").and_then(Value::as_bool).unwrap_or(false)
            || o.contains_key("wait_ms")
            || w.iter().any(|v| v.as_str() == Some("wait"));
        let peek = o.get("peek").and_then(Value::as_bool).unwrap_or(false) || w.iter().any(|v| v.as_str() == Some("peek"));
        let wake: Option<Vec<String>> = option(&o, &["kinds", "types"]).map(|t| strings(Some(t)));
        let inbox = self.inbox_of(&me, c.incarnation, &o)?;
        let dealt = option(&o, &["dealt", "ack"]).and_then(|v| number(v, 'h'));
        let store = self.store()?;
        if let Some(h) = dealt {
            store.dealt(&inbox, h).map_err(|e| e.to_string())?;
        }
        if peek {
            let unread = store.unread(&inbox).map_err(|e| e.to_string())?;
            // A look, not a read: taking them is `inbox`, which hands them over
            let next = if unread.is_empty() {
                if inbox.starts_with("job:") { cmd(&["inbox", "wait"]) } else { "Carry on with the task.".to_string() }
            } else {
                cmd(&["inbox"])
            };
            return Ok(Held::Now(json!({
                "inbox": inbox,
                "unread": unread.iter().map(Self::shown).collect::<Vec<_>>(),
                "next": [next],
            })));
        }
        let then_wait = inbox.starts_with("job:");
        if let Some(h) = store.hand_over(&inbox, wake.as_deref()).map_err(|e| e.to_string())? {
            return Ok(Held::Now(Self::handed(&h, then_wait)));
        }
        if !wants_wait {
            return Ok(Held::Now(json!({
                "inbox": inbox, "mail": [], "state": "NOTHING YET",
                "next": [if then_wait { cmd(&["inbox", "wait"]) } else { "Carry on with the task.".to_string() }],
            })));
        }
        // A wait already there is one its caller let go of -- an AI's shell
        // gives up on a command and the AI asks again. The newest wait is the
        // one somebody is holding, so it takes over
        if let Some(i) = self.inbox_waits.iter().position(|x| x.inbox == inbox) {
            let old = self.inbox_waits.remove(i);
            let _ = old.reply.send(Err("another wait on this inbox took over".into()));
        }
        Ok(Held::Later(InboxWait {
            inbox,
            wake,
            deadline: Instant::now() + wait_of(&o, WAIT),
            reply: c.reply.clone(),
            then_wait,
        }))
    }

    /// Whose mail a caller reads: a lead its job's, a worker its assignment's,
    /// any other tab its own
    fn inbox_of(&mut self, me: &TabFact, incarnation: Option<u64>, o: &serde_json::Map<String, Value>) -> Result<String, String> {
        if let Some(a) = self.my_assignment(me, incarnation)? {
            // A worker that also leads a job of its own reads that one when it says so
            if o.contains_key("job") {
                return Ok(format!("job:{}", self.led_job(me, o)?.id));
            }
            return Ok(format!("assignment:{}", a.id));
        }
        if let Ok(job) = self.led_job(me, o) {
            return Ok(format!("job:{}", job.id));
        }
        Ok(db::tab_box(&me.uid))
    }

    fn shown(m: &db::Mail) -> Value {
        let mut v = json!({
            "from": m.sender,
            "kind": m.kind,
            "subject": m.subject,
            "text": m.body,
        });
        if let (Some(obj), Some(p)) = (v.as_object_mut(), m.extra.as_object()) {
            for (k, x) in p {
                obj.entry(k.clone()).or_insert(x.clone());
            }
        }
        if m.kind == "question" {
            v["question"] = json!(format!("q{}", m.seq));
        }
        v
    }

    fn handed(h: &db::Handover, then_wait: bool) -> Value {
        let dealt = format!("'{{\"dealt\":{}}}'", h.id);
        let mut next: Vec<String> = Vec::new();
        for m in &h.mail {
            if m.kind == "question" {
                next.push(cmd(&["answer", &format!("q{}", m.seq), "\"<answer>\""]));
            }
        }
        next.push(if then_wait {
            format!("{} -- once you have acted on every piece of mail above", cmd(&["inbox", "wait", &dealt]))
        } else {
            format!("{} -- once you have acted on these", cmd(&["inbox", &dealt]))
        });
        json!({
            "handover": h.id,
            "again": h.again,
            "mail": h.mail.iter().map(Self::shown).collect::<Vec<_>>(),
            "next": next,
        })
    }

    // -- decisions -----------------------------------------------------------

    fn decision_open(&mut self, c: &Call, me: Option<&TabFact>, effects: &mut Vec<Effect>) -> Result<Value, String> {
        let me = Self::caller_id(me)?.clone();
        let w = words(&c.params);
        let o = options(&c.params);
        let task = w.first().and_then(|v| number(v, 't')).ok_or("decision_open needs the task it holds")?;
        let question = w.get(1).and_then(|v| text_of(Some(v))).ok_or("decision_open needs the question")?;
        let choices = strings(option(&o, &["choices", "options"]));
        let decided_by = match option(&o, &["to", "by"]).and_then(Value::as_str) {
            Some("person") | Some("human") | Some("user") => "person",
            _ => "lead",
        };
        let store = self.store()?;
        let t = store.task(task).map_err(|e| e.to_string())?.ok_or_else(|| format!("there is no task t{task}"))?;
        let job = store.job(t.job).map_err(|e| e.to_string())?.ok_or("no such job")?;
        if job.lead != me.uid {
            return Err(format!("t{task} belongs to <@{}>'s job", job.lead_name));
        }
        let d = store
            .decision_open(task, &question, &choices, decided_by)
            .map_err(|e| e.to_string())?;
        if decided_by == "person" {
            effects.push(Effect::Person {
                text: crate::i18n::tp("orch.decide.notify", &[("question", &question)]),
            });
        }
        Ok(json!({
            "decision": format!("d{}", d.id),
            "task": format!("t{task}"),
            "who": decided_by,
            "next": [if decided_by == "person" { cmd(&["inbox", "wait"]) } else { cmd(&["decision_make", &format!("d{}", d.id), "<choice>"]) }],
        }))
    }

    fn decision_make_call(&mut self, c: &Call, me: Option<&TabFact>) -> Result<Value, String> {
        let me = Self::caller_id(me)?.clone();
        let w = words(&c.params);
        let id = w.first().and_then(|v| number(v, 'd')).ok_or("decision_make needs the decision (d<N>)")?;
        let choice = w.get(1).and_then(|v| text_of(Some(v))).ok_or("decision_make needs the choice")?;
        let store = self.store()?;
        let d = store
            .decision(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("there is no decision d{id}"))?;
        let job = store.job(d.job).map_err(|e| e.to_string())?.ok_or("no such job")?;
        if job.lead != me.uid {
            return Err(format!("d{id} belongs to <@{}>'s job", job.lead_name));
        }
        if d.decided_by == "person" {
            return Err(format!("d{id} is the person's to decide; wait for it: {}", cmd(&["inbox", "wait"])));
        }
        let d = self.make_decision(id, &choice, "lead")?;
        Ok(json!({"decision": format!("d{}", d.id), "choice": d.choice, "next": [cmd(&["assign", &format!("t{}", d.task), "<tab>"])]}))
    }

    /// A decision made -- by the lead through a command, or by the person on
    /// the screen. The lead hears of it in its inbox either way
    pub fn make_decision(&mut self, id: i64, choice: &str, by: &str) -> Result<db::Decision, String> {
        let store = self.store()?;
        let d = store.decision_make(id, choice, by).map_err(|e| e.to_string())?;
        store
            .post(
                Some(d.job),
                &format!("job:{}", d.job),
                by,
                "decision",
                &format!("d{} decided: {choice}", d.id),
                &format!("{} -> {choice}", d.question),
                &json!({"decision": format!("d{}", d.id), "task": format!("t{}", d.task), "choice": choice}),
            )
            .map_err(|e| e.to_string())?;
        // Shown as a card in the conference of the desk the lead is on
        if let Ok(Some(job)) = store.job(d.job) {
            crate::convo::note_agreed(&job.lead, &format!("{} → {choice}", d.question));
        }
        self.touched();
        Ok(d)
    }

    // -- tabs once the work is over -----------------------------------------

    fn assignment_named(&mut self, c: &Call, me: &TabFact, verb: &str) -> Result<(Assignment, db::Job), String> {
        let w = words(&c.params);
        let id = w
            .first()
            .and_then(|v| number(v, 'a'))
            .ok_or_else(|| format!("{verb} needs the assignment: {}", cmd(&[verb, "a1"])))?;
        let store = self.store()?;
        let a = store
            .assignment(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("there is no assignment a{id}"))?;
        let job = store.job(a.job).map_err(|e| e.to_string())?.ok_or("no such job")?;
        if job.lead != me.uid {
            return Err(format!("a{id} belongs to <@{}>'s job", job.lead_name));
        }
        Ok((a, job))
    }

    fn let_go(&mut self, c: &Call, me: Option<&TabFact>, scene: &Scene, effects: &mut Vec<Effect>) -> Result<Value, String> {
        let me = Self::caller_id(me)?.clone();
        let (a, job) = self.assignment_named(c, &me, "let_go")?;
        if a.active() {
            return Err(format!(
                "a{} is still being worked on; wait for its report, or stop it first: {}",
                a.id,
                cmd(&["stop", &format!("a{}", a.id)])
            ));
        }
        let store = self.store()?;
        let opener = store.opener_of(&a.tab).map_err(|e| e.to_string())?;
        let busy_elsewhere = store.active_for_tab(&a.tab).map_err(|e| e.to_string())?.is_some();
        let (closed, why) = match opener {
            _ if busy_elsewhere => (false, "it is working on another assignment"),
            Some(o) if o.job == job.id && o.held_by == "job" && scene.by_uid(&a.tab).is_some() => (true, ""),
            Some(o) if o.held_by == "person" => (false, "the person has typed into it, so it is theirs"),
            _ if scene.by_uid(&a.tab).is_none() => (false, "it is already closed"),
            _ => (false, "this job did not open it"),
        };
        if closed {
            store.gone(&a.tab).map_err(|e| e.to_string())?;
            effects.push(Effect::Close { tab: a.tab.clone() });
        }
        store
            .afterwards(a.id, if closed { "released" } else { "kept" })
            .map_err(|e| e.to_string())?;
        let next = self.status_of(job.id, scene)?["next"].clone();
        Ok(json!({
            "assignment": format!("a{}", a.id),
            "tab": a.tab_name,
            "closed": closed,
            "kept_because": if closed { Value::Null } else { json!(why) },
            "next": next,
        }))
    }

    fn keep(&mut self, c: &Call, me: Option<&TabFact>, scene: &Scene) -> Result<Value, String> {
        let me = Self::caller_id(me)?.clone();
        let (a, job) = self.assignment_named(c, &me, "keep")?;
        if a.active() {
            return Err(format!("a{} is still being worked on; wait for its report: {}", a.id, cmd(&["inbox", "wait"])));
        }
        self.store()?.afterwards(a.id, "kept").map_err(|e| e.to_string())?;
        let next = self.status_of(job.id, scene)?["next"].clone();
        Ok(json!({"assignment": format!("a{}", a.id), "tab": a.tab_name, "kept": true, "next": next}))
    }

    fn stop(&mut self, c: &Call, me: Option<&TabFact>, scene: &Scene, effects: &mut Vec<Effect>) -> Result<Value, String> {
        let me = Self::caller_id(me)?.clone();
        let all = words(&c.params).first().and_then(|v| v.as_str()) == Some("all");
        let targets: Vec<Assignment> = if all {
            let o = options(&c.params);
            let job = self.led_job(&me, &o)?;
            self.store()?
                .assignments(job.id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|a| a.active())
                .collect()
        } else {
            let (a, _) = self.assignment_named(c, &me, "stop")?;
            if !a.active() {
                return Err(format!("a{} is already over ({})", a.id, a.state));
            }
            vec![a]
        };
        let stopped = self.stop_these(&targets, "stopped by the lead", scene, effects)?;
        Ok(json!({"stopped": stopped, "next": [cmd(&["job_status"])]}))
    }

    /// Stop assignments: Esc now, and a tab this job opened is closed if it
    /// has not stopped a little later
    fn stop_these(&mut self, targets: &[Assignment], why: &str, scene: &Scene, effects: &mut Vec<Effect>) -> Result<Vec<String>, String> {
        let mut stopped = Vec::new();
        for a in targets {
            self.starting.retain(|s| {
                if s.assignment == a.id {
                    if let Some(r) = &s.reply {
                        let _ = r.send(Err("stopped before it started".into()));
                    }
                    false
                } else {
                    true
                }
            });
            self.store()?.stop(a.id, why).map_err(|e| e.to_string())?;
            if scene.by_uid(&a.tab).is_some() {
                effects.push(Effect::Esc { tab: a.tab.clone() });
                // Who stopped it, for the record of conversations: the lead,
                // or the person from the job's card
                let by = match why == STOPPED_BY_PERSON {
                    true => crate::convo::By::Person,
                    false => crate::convo::By::Lead,
                };
                let stop = crate::convo::Stop { by, device: None, how: "job", job: Some(a.job), why: Some(why.to_string()) };
                crate::convo::note_stopped(&a.tab, stop);
                self.stopping.push(Stopping {
                    tab: a.tab.clone(),
                    job: a.job,
                    assignment: a.id,
                    since: Instant::now(),
                });
            }
            self.quiet_since.remove(&a.id);
            stopped.push(format!("a{}", a.id));
        }
        self.touched();
        Ok(stopped)
    }

    /// The person stopped a whole job from the screen. Every worker stops, and
    /// the job ends there: closed, so nothing more can be assigned in it. The
    /// lead hears of it -- on the wait it is holding, or in its own mail, since
    /// the job's is gone with the job -- and a new request from the person is
    /// a new job
    pub fn stop_job(&mut self, job: i64, scene: &Scene) -> Vec<Effect> {
        let mut effects = Vec::new();
        let (targets, lead) = match self.store() {
            Ok(s) => match s.job(job) {
                Ok(Some(j)) if j.state == "open" => (
                    s.assignments(job).unwrap_or_default().into_iter().filter(|a| a.active()).collect::<Vec<_>>(),
                    j.lead,
                ),
                _ => return effects,
            },
            Err(_) => return effects,
        };
        if let Err(e) = self.stop_these(&targets, "stopped by the person", scene, &mut effects) {
            crate::append_hook_log(&format!("orchestration: stopping j{job}: {e}"));
        }
        let Ok(s) = self.store() else { return effects };
        if let Err(e) = s.job_close(job, STOPPED_BY_PERSON) {
            crate::append_hook_log(&format!("orchestration: closing j{job}: {e}"));
        }
        crate::append_hook_log(&format!("orchestration: j{job} closed: {STOPPED_BY_PERSON}"));
        let subject = format!("The person stopped j{job}");
        let body = "The person stopped this job: every tab working on it was stopped, and the job is closed. \
                    Hand out nothing more for it. If the person asks for more, start a new job.";
        let next = json!(["Tell the person the job was stopped, and what had been done by then."]);
        let inbox = format!("job:{job}");
        let mut told = false;
        self.inbox_waits.retain(|w| {
            if w.inbox != inbox {
                return true;
            }
            let _ = w.reply.send(Ok(json!({
                "inbox": inbox, "state": "STOPPED",
                "mail": [{"from": "person", "kind": "alert", "subject": subject, "text": body}],
                "next": next,
            })));
            told = true;
            false
        });
        if !told && let Ok(s) = self.store() {
            let _ = s.post(Some(job), &format!("tab:{lead}"), "person", "alert", &subject, body, &json!({"job": format!("j{job}")}));
        }
        self.touched();
        effects
    }

    // -- what the runtime tells this module ---------------------------------

    /// A tab this job opened (`open_ai_tab` or `open_tab`, called by a lead):
    /// `id` is what it is called, `uid` who it is
    pub fn opened(&mut self, caller: Option<&str>, scene: &Scene, id: &str, uid: &str) -> Option<Value> {
        let me = scene.by_called(caller?)?.clone();
        let job = self.led_job(&me, &serde_json::Map::new()).ok()?;
        let store = self.store().ok()?;
        store.named(uid, id, db::now_ms()).ok()?;
        store.opened(job.id, uid).ok()?;
        self.touched();
        let open: Vec<i64> = self
            .store()
            .ok()?
            .tasks(job.id)
            .ok()?
            .into_iter()
            .filter(|t| t.state == "open")
            .map(|t| t.id)
            .collect();
        let task = open.first().map(|t| format!("t{t}")).unwrap_or_else(|| "<task>".into());
        Some(json!([cmd(&["assign", &task, id])]))
    }

    /// A working folder this job made (`worktree_add`, called by a lead)
    pub fn worktree_made(&mut self, caller: Option<&str>, scene: &Scene, folder: &str, branch: &str) -> Option<Value> {
        let me = scene.by_called(caller?)?.clone();
        let job = self.led_job(&me, &serde_json::Map::new()).ok()?;
        self.store().ok()?.folder_made(job.id, folder, branch).ok()?;
        Some(json!([cmd(&["open_ai_tab", "<claude|codex|gemini>", &format!("'{}'", json!({"folder": folder}))])]))
    }

    /// A person typed into a tab (by uid): if a job opened it, it is theirs now
    pub fn person_typed(&mut self, tab: &str) {
        if let Ok(s) = self.store() {
            let _ = s.person_took(tab);
        }
    }

    /// Something to tell a tab (by uid) later, outside any job: an ask_tab
    /// reply that outlived the call it was asked on
    pub fn mail_tab(&mut self, tab: &str, from: &str, subject: &str, body: &str) {
        if let Ok(s) = self.store() {
            let _ = s.post(None, &db::tab_box(tab), from, "reply", subject, body, &json!({"from": from}));
        }
    }

    // -- the tick ------------------------------------------------------------

    /// Everything held, looked at again against the tabs as they are now
    pub fn tick(&mut self, scene: &Scene) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.name_tabs(scene);
        self.tend_starting(scene, &mut effects);
        self.tend_waits();
        self.tend_untold(scene, &mut effects);
        let due = self.last_watch.is_none_or(|t| t.elapsed() >= WATCH_EVERY);
        if due {
            self.last_watch = Some(Instant::now());
            self.watch(scene, &mut effects);
        }
        effects
    }

    fn tend_starting(&mut self, scene: &Scene, effects: &mut Vec<Effect>) {
        let now = Instant::now();
        let starting = std::mem::take(&mut self.starting);
        let mut kept = Vec::new();
        for mut s in starting {
            let tab = scene.by_uid(&s.tab);
            // Some(Ok((seen starting, what to add))) once the tab has it,
            // Some(Err(why)) when it never will, None while it is too early
            let outcome: Option<Result<(bool, Option<&str>), String>> = match (tab, s.sent_at) {
                (None, _) => Some(Err(format!("<@{}> was closed before the task went in", s.name))),
                // Over there, and its bridge not up yet: its machine is opened
                // (the task is work for it), and the brief waits for the line
                (Some(t), None) if t.far && !t.reachable => {
                    if !s.woken {
                        effects.push(Effect::Wake { tab: s.tab.clone() });
                        s.woken = true;
                    }
                    if now.duration_since(s.asked_at) >= BRIDGE_WAIT {
                        Some(Err(format!(
                            "the bridge on <@{}>'s machine has not connected yet; nothing was sent. Its machine is waking: assign again in a minute",
                            s.name
                        )))
                    } else {
                        None
                    }
                }
                (Some(t), None) => {
                    if matches!(t.state, TabState::Exited) {
                        Some(Err(format!("<@{}>'s program has ended", s.name)))
                    } else if matches!(t.state, TabState::Question) {
                        Some(Err(format!(
                            "<@{}> is waiting for the person to approve or choose something; nothing was sent. Tell the person",
                            s.name
                        )))
                    } else if t.turn_over() {
                        let brief = self.brief_for(s.assignment);
                        let lead = t.typed_request.then(|| text::LEAD_LINE.to_string());
                        // Whose words these are, for the record of
                        // conversations: the job's, handed out by its lead
                        let (job, by) = self.job_of_assignment(s.assignment);
                        let typed = lead.as_ref().map(|l| format!("{l} {brief}"));
                        let texts: Vec<&str> = typed.iter().map(String::as_str).chain([brief.as_str()]).collect();
                        crate::convo::note_sent(&s.tab, &texts, crate::convo::Origin::job(job, by.as_deref(), "brief"));
                        effects.push(Effect::Type {
                            tab: s.tab.clone(),
                            lead,
                            text: brief,
                        });
                        s.sent_at = Some(now);
                        None
                    } else if now.duration_since(s.asked_at) >= IDLE_WAIT {
                        Some(Err(format!("<@{}> stayed busy; nothing was sent. Try again when it is free", s.name)))
                    } else {
                        None
                    }
                }
                (Some(t), Some(sent)) => match t.state {
                    TabState::Busy | TabState::Background => Some(Ok((true, None))),
                    TabState::Question => Some(Ok((
                        true,
                        Some("it is waiting for the person to approve something; the person has been told"),
                    ))),
                    TabState::Exited => Some(Err(format!("<@{}>'s program ended as the task went in", s.name))),
                    _ if now.duration_since(sent) >= START_WAIT => Some(Ok((
                        false,
                        Some("the tab was not seen starting on it; its report still counts if it sends one"),
                    ))),
                    _ => None,
                },
            };
            // A quick worker can report before it is seen starting -- on
            // another machine, the report travels faster than its screen. The
            // work is done: that is the answer, whatever the screen shows now
            let over = self
                .store
                .as_ref()
                .ok()
                .and_then(|st| st.assignment(s.assignment).ok().flatten())
                .filter(|a| a.over() && a.end_reason.as_deref() != Some(db::NOT_HANDED));
            if let (Some(a), Some(_)) = (&over, &outcome) {
                let mut answer = s.answer.clone();
                answer["why"] = json!(format!("it has already reported: {}", a.state));
                if let Some(reply) = s.reply.take() {
                    let _ = reply.send(Ok(answer));
                }
                self.touched();
                continue;
            }
            match outcome {
                None => kept.push(s),
                Some(Ok((seen, why))) => {
                    let r = self.store().and_then(|st| st.started(s.assignment, seen).map_err(|e| e.to_string()));
                    let mut answer = s.answer.clone();
                    if let Some(n) = why {
                        answer["why"] = json!(n);
                    }
                    if let Some(reply) = s.reply.take() {
                        let _ = reply.send(r.map(|_| answer));
                    }
                    self.touched();
                }
                Some(Err(why)) => {
                    let failed = if s.sent_at.is_some() {
                        self.store().and_then(|st| st.lost(s.assignment, &why).map_err(|e| e.to_string())).map(|_| ())
                    } else {
                        self.store().and_then(|st| st.withdraw(s.assignment).map_err(|e| e.to_string()))
                    };
                    if let Err(e) = failed {
                        crate::append_hook_log(&format!("orchestration: a{}: {e}", s.assignment));
                    }
                    if let Some(reply) = s.reply.take() {
                        let _ = reply.send(Err(why));
                    }
                    self.touched();
                }
            }
        }
        self.starting = kept;
    }

    /// The job an assignment belongs to, and what the tab that leads it is
    /// called: who the brief is from, as the record of conversations says it
    fn job_of_assignment(&mut self, assignment: i64) -> (Option<i64>, Option<String>) {
        let Ok(store) = self.store() else { return (None, None) };
        let Some(a) = store.assignment(assignment).ok().flatten() else { return (None, None) };
        let lead = store.job(a.job).ok().flatten().map(|j| j.lead_name);
        (Some(a.job), lead)
    }

    fn brief_for(&mut self, assignment: i64) -> String {
        let Ok(store) = self.store() else { return String::new() };
        let Some(a) = store.assignment(assignment).ok().flatten() else { return String::new() };
        let body = store.task(a.task).ok().flatten().map(|t| t.body).unwrap_or_default();
        let nest = a.depth < i64::from(crate::config::operate().depth());
        text::brief(a.id, a.task, &body, nest)
    }

    fn tend_waits(&mut self) {
        let now = Instant::now();
        let waits = std::mem::take(&mut self.inbox_waits);
        let mut kept = Vec::new();
        for w in waits {
            let got = self.store().and_then(|s| s.hand_over(&w.inbox, w.wake.as_deref()).map_err(|e| e.to_string()));
            match got {
                Ok(Some(h)) => {
                    let _ = w.reply.send(Ok(Self::handed(&h, w.then_wait)));
                }
                Err(e) => {
                    let _ = w.reply.send(Err(e));
                }
                Ok(None) if now >= w.deadline => {
                    let _ = w.reply.send(Ok(json!({
                        "inbox": w.inbox, "mail": [], "state": "NOTHING YET",
                        "next": [format!("{} -- nothing came yet; that is not a failure, wait again", cmd(&["inbox", "wait"]))],
                    })));
                }
                Ok(None) => kept.push(w),
            }
        }
        self.inbox_waits = kept;
        let asks = std::mem::take(&mut self.ask_waits);
        let mut kept = Vec::new();
        for a in asks {
            let q = self.store().ok().and_then(|s| s.question(a.question).ok().flatten());
            match q.as_ref().and_then(Self::answered) {
                Some(v) => {
                    let _ = a.reply.send(Ok(v));
                }
                None if now >= a.deadline => {
                    let _ = a.reply.send(Ok(json!({
                        "question": format!("q{}", a.question),
                        "state": "NOTHING YET",
                        "next": [format!(
                            "{} -- the question stays asked; this waits for its answer again",
                            cmd(&["ask_lead", &format!("'{{\"resume\":\"q{}\"}}'", a.question)])
                        )],
                    })));
                }
                None => kept.push(a),
            }
        }
        self.ask_waits = kept;
    }

    /// Mail nobody has been told about, pointed out to a tab that is free to
    /// read it and not already waiting for it
    fn tend_untold(&mut self, scene: &Scene, effects: &mut Vec<Effect>) {
        let Ok(store) = self.store.as_mut() else { return };
        let Ok(boxes) = store.untold() else { return };
        for (inbox, count) in boxes {
            if self.inbox_waits.iter().any(|w| w.inbox == inbox) {
                continue;
            }
            // An answer the worker is waiting on arrives as the answer to its call
            if let Some(a) = inbox.strip_prefix("assignment:").and_then(|n| n.parse::<i64>().ok())
                && self
                    .ask_waits
                    .iter()
                    .any(|w| store.question(w.question).ok().flatten().is_some_and(|q| q.assignment == a))
            {
                continue;
            }
            if store.given(&inbox).ok().flatten().is_some() {
                continue;
            }
            let reader = if let Some(j) = inbox.strip_prefix("job:").and_then(|n| n.parse::<i64>().ok()) {
                store.job(j).ok().flatten().filter(|j| j.state == "open").map(|j| j.lead)
            } else if let Some(a) = inbox.strip_prefix("assignment:").and_then(|n| n.parse::<i64>().ok()) {
                store.assignment(a).ok().flatten().filter(|a| a.active()).map(|a| a.tab)
            } else {
                inbox.strip_prefix("tab:").map(str::to_string)
            };
            let Some(reader) = reader else {
                // Nobody left to read it: not pointed at forever
                let _ = store.told(&inbox);
                continue;
            };
            let Some(t) = scene.by_uid(&reader) else { continue };
            if !t.turn_over() || self.starting.iter().any(|s| s.tab == reader) {
                continue;
            }
            let line = text::mail_line(count);
            let job = match inbox.split_once(':') {
                Some(("job", n)) => n.parse::<i64>().ok(),
                Some(("assignment", n)) => n.parse::<i64>().ok().and_then(|a| store.assignment(a).ok().flatten()).map(|a| a.job),
                _ => None,
            };
            crate::convo::note_sent(&reader, &[&line], crate::convo::Origin::job(job, None, "mail"));
            effects.push(Effect::Type {
                tab: reader.clone(),
                lead: None,
                text: line,
            });
            let _ = store.told(&inbox);
        }
    }

    /// Things left undone, raised once each to the lead that can do them
    fn watch(&mut self, scene: &Scene, effects: &mut Vec<Effect>) {
        let now = Instant::now();
        // Stopped tabs this job opened, closed once they have had time to stop
        let stopping = std::mem::take(&mut self.stopping);
        for s in stopping {
            if now.duration_since(s.since) < STOP_GRACE {
                self.stopping.push(s);
                continue;
            }
            let busy = scene.by_uid(&s.tab).is_some_and(|t| matches!(t.state, TabState::Busy));
            let store = self.store.as_ref().ok();
            let ours = store
                .and_then(|st| st.opener_of(&s.tab).ok().flatten())
                .is_some_and(|o| o.job == s.job && o.held_by == "job");
            // Handed anything since it was stopped -- still at it or already
            // done with it -- and what it is busy with is that, not the work it
            // would not let go of
            let given_again = store
                .and_then(|st| st.last_for_tab(&s.tab).ok().flatten())
                .is_some_and(|a| a.id != s.assignment);
            if busy && ours && !given_again {
                effects.push(Effect::Close { tab: s.tab.clone() });
                if let Ok(st) = self.store() {
                    let _ = st.gone(&s.tab);
                }
            }
        }
        let Ok(store) = self.store.as_mut() else { return };
        let Ok(active) = store.active_assignments() else { return };
        let starting: HashSet<i64> = self.starting.iter().map(|s| s.assignment).collect();
        let asking: HashSet<i64> = self
            .ask_waits
            .iter()
            .filter_map(|w| store.question(w.question).ok().flatten().map(|q| q.assignment))
            .collect();
        let mut raised: Vec<(i64, String, String, String)> = Vec::new(); // job, key, subject, body
        let mut lost: Vec<(i64, String)> = Vec::new();
        for a in &active {
            if starting.contains(&a.id) {
                continue;
            }
            let job = a.job;
            match scene.by_uid(&a.tab) {
                None => lost.push((a.id, format!("the tab <@{}> was closed", a.tab_name))),
                Some(t) => match t.state {
                    TabState::Exited => lost.push((a.id, format!("<@{}>'s program ended", a.tab_name))),
                    _ if t.incarnation.is_some() && a.process.is_some() && t.incarnation.map(|n| n as i64) != a.process => {
                        lost.push((a.id, format!("<@{}> was restarted; the new run of it was never given a{}", a.tab_name, a.id)))
                    }
                    TabState::Question => {
                        raised.push((
                            job,
                            format!("approval:{}", a.id),
                            format!("<@{}> is waiting for the person", a.tab_name),
                            format!(
                                "a{} (t{}) stopped at a question for the person (an approval or a choice) on <@{}>'s screen. \
                                 The person has been told. Wait: {}",
                                a.id,
                                a.task,
                                a.tab_name,
                                cmd(&["inbox", "wait"])
                            ),
                        ));
                        self.quiet_since.remove(&a.id);
                    }
                    TabState::Limit => {
                        raised.push((
                            job,
                            format!("limit:{}", a.id),
                            format!("<@{}> hit its usage limit", a.tab_name),
                            format!(
                                "a{} (t{}) is held up: <@{}> reached its usage limit. Wait for it, or stop it and assign the task elsewhere: {}",
                                a.id,
                                a.task,
                                a.tab_name,
                                cmd(&["stop", &format!("a{}", a.id)])
                            ),
                        ));
                    }
                    _ if t.turn_over() && !asking.contains(&a.id) => {
                        let since = *self.quiet_since.entry(a.id).or_insert(now);
                        if now.duration_since(since) >= LEFT {
                            raised.push((
                                job,
                                format!("quiet:{}:{}", a.id, since.elapsed().as_secs() / 600),
                                format!("<@{}> stopped without reporting", a.tab_name),
                                format!(
                                    "<@{}> has been idle for {} minutes on a{} (t{}) and has not reported. \
                                     See what it said: {} Then tell it what to do: {}",
                                    a.tab_name,
                                    now.duration_since(since).as_secs() / 60,
                                    a.id,
                                    a.task,
                                    cmd(&["tab_conversation", &a.tab_name]),
                                    cmd(&["tell", &format!("a{}", a.id), "\"<report with shikisha report, or carry on>\""])
                                ),
                            ));
                        }
                    }
                    _ => {
                        self.quiet_since.remove(&a.id);
                        let _ = store.lower(&format!("approval:{}", a.id));
                        let _ = store.lower(&format!("limit:{}", a.id));
                    }
                },
            }
        }
        for (id, why) in lost {
            let Ok(Some(a)) = store.assignment(id) else { continue };
            let last = store.lost(id, &why).unwrap_or(false);
            let next = if last {
                format!(
                    "t{} has lost its tab {} times and will not be tried again: add a task that removes the cause, or ask the person",
                    a.task,
                    db::LOSSES_ALLOWED
                )
            } else {
                cmd(&["assign", &format!("t{}", a.task), "<tab>"])
            };
            let _ = store.post(
                Some(a.job),
                &format!("job:{}", a.job),
                "shikisha",
                "alert",
                &format!("a{} ended without a report", a.id),
                &format!("{why}. t{} is {}. Next: {next}", a.task, if last { "failed" } else { "open again" }),
                &json!({"assignment": format!("a{}", a.id), "task": format!("t{}", a.task)}),
            );
            self.quiet_since.remove(&id);
            self.revision = self.revision.wrapping_add(1);
        }
        // Loose ends of open jobs, older than a moment
        let Ok(jobs) = store.open_jobs() else { return };
        let old = |at: i64| db::now_ms() - at >= LEFT.as_millis() as i64;
        for job in &jobs {
            let assignments = store.assignments(job.id).unwrap_or_default();
            for a in &assignments {
                if a.over() && a.afterwards.is_none() && a.ended_at.is_some_and(old) && scene.by_uid(&a.tab).is_some() {
                    raised.push((
                        job.id,
                        format!("undecided:{}", a.id),
                        format!("a{} is over and <@{}> is still open", a.id, a.tab_name),
                        format!(
                            "Decide what <@{}> does next: give it another task ({}), keep it ({}), or let it go ({}).",
                            a.tab_name,
                            cmd(&["assign", "<task>", &a.tab_name]),
                            cmd(&["keep", &format!("a{}", a.id)]),
                            cmd(&["let_go", &format!("a{}", a.id)])
                        ),
                    ));
                }
            }
            let given: HashSet<&str> = assignments.iter().map(|a| a.tab.as_str()).collect();
            for o in store.opened_by(job.id).unwrap_or_default() {
                if o.held_by == "job" && !given.contains(o.tab.as_str()) && old(o.opened_at) && scene.by_uid(&o.tab).is_some() {
                    raised.push((
                        job.id,
                        format!("opened:{}:{}", job.id, o.tab),
                        format!("<@{}> was opened and never given a task", o.tab_name),
                        format!(
                            "Give it one ({}) or close it (shikisha close_tab {}).",
                            cmd(&["assign", "<task>", &o.tab_name]),
                            o.tab_name
                        ),
                    ));
                }
            }
            for f in store.folders(job.id).unwrap_or_default() {
                let used = scene.tabs.iter().any(|t| {
                    t.folder.as_deref().is_some_and(|x| {
                        crate::sessionfind::same_folder(std::path::Path::new(x), std::path::Path::new(&f.folder))
                    })
                });
                if !used && old(f.made_at) {
                    raised.push((
                        job.id,
                        format!("worktree:{}:{}", job.id, f.folder),
                        format!("The working folder for {} has no tab", f.branch),
                        format!(
                            "Open a tab there ({}) or tell the person it is not needed.",
                            cmd(&["open_ai_tab", "<claude|codex|gemini>", &format!("'{}'", json!({"folder": f.folder}))])
                        ),
                    ));
                }
            }
        }
        for (job, key, subject, body) in raised {
            if store.raise_once(&key).unwrap_or(false) {
                if key.starts_with("approval:") {
                    effects.push(Effect::Person { text: subject.clone() });
                }
                let _ = store.post(Some(job), &format!("job:{job}"), "shikisha", "alert", &subject, &body, &json!({}));
                self.revision = self.revision.wrapping_add(1);
            }
        }
    }

    // -- the person's view ---------------------------------------------------

    /// Every open job, as the screen draws it: what each task is doing, who is
    /// on it, and what waits for the person
    pub fn board(&self, scene: &Scene) -> Value {
        let Ok(store) = self.store.as_ref() else { return json!([]) };
        let Ok(jobs) = store.open_jobs() else { return json!([]) };
        let mut out = Vec::new();
        for j in jobs {
            let tasks = store.tasks(j.id).unwrap_or_default();
            let assignments = store.assignments(j.id).unwrap_or_default();
            let decisions = store.decisions(j.id).unwrap_or_default();
            let rows: Vec<Value> = tasks
                .iter()
                .map(|t| {
                    let tries: Vec<&Assignment> = assignments.iter().filter(|a| a.task == t.id).collect();
                    let last = tries.last();
                    // By uid: the board finds the tab on its rows by it
                    let tab_state = last.and_then(|a| scene.by_uid(&a.tab)).map(|x| x.state.label());
                    json!({
                        "id": t.id,
                        "title": t.title,
                        "state": t.state,
                        "why": t.why,
                        "tab": last.map(|a| a.tab.clone()),
                        "tab_state": tab_state,
                        "tries": tries.len(),
                        "since": last.map(|a| a.created_at),
                        "did": t.result.as_ref().and_then(|v| v.get("did")).cloned(),
                    })
                })
                .collect();
            let workers: Vec<String> = assignments.iter().fold(Vec::new(), |mut v, a| {
                if !v.contains(&a.tab) {
                    v.push(a.tab.clone());
                }
                v
            });
            out.push(json!({
                "id": j.id,
                "goal": j.goal,
                "lead": j.lead,
                "started_at": j.started_at,
                "rounds": assignments.len(),
                "tasks": rows,
                "workers": workers,
                "decisions": decisions.iter().filter(|d| d.state == "open").map(|d| json!({
                    "id": d.id, "task": d.task, "question": d.question, "choices": d.choices, "who": d.decided_by,
                })).collect::<Vec<_>>(),
                "working": assignments.iter().any(|a| a.active()),
            }));
        }
        json!(out)
    }

    /// The jobs a tab leads, and the tabs working for them, all by uid: for
    /// the tab bar, which draws a lead's workers beside it
    pub fn workers_of(&self) -> Vec<(String, Vec<String>)> {
        let Ok(store) = self.store.as_ref() else { return Vec::new() };
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for j in store.open_jobs().unwrap_or_default() {
            let tabs: Vec<String> = store
                .assignments(j.id)
                .unwrap_or_default()
                .into_iter()
                .map(|a| a.tab)
                .fold(Vec::new(), |mut v, t| {
                    if !v.contains(&t) {
                        v.push(t);
                    }
                    v
                });
            match out.iter_mut().find(|(l, _)| *l == j.lead) {
                Some((_, v)) => v.extend(tabs.into_iter().filter(|t| !v.contains(t)).collect::<Vec<_>>()),
                None => out.push((j.lead, tabs)),
            }
        }
        out
    }
}

enum Held<W> {
    Now(Value),
    Later(W),
}

#[cfg(test)]
mod tests;
