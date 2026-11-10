//! The pull request a branch is on, when there is one -- and what its review
//! and its checks are doing.
//!
//! The third thing you would otherwise go and look up, after the branch and
//! the ports. With several agents each on their own branch, "has that one been
//! merged yet" is a question about every row at once, and answering it means
//! leaving the terminal entirely. So is "has the review come back" after a
//! push: [`crate::prnews`] works that out from what is read here, and what it
//! says is worth telling comes out of [`Watch::take_told`].
//!
//! **It asks as the account somebody chose.** A folder's project chooses the git
//! account its column signs in with, and the pull request is read with that
//! same account: a token it was given, or -- when the choice was "the way git
//! on this PC signs in" -- what git's own credential store hands out for
//! GitHub. Nothing is chosen for anybody, so a row with no number is a row
//! whose project has no account chosen, and the settings say which.
//!
//! **One question per repository.** Every branch of a repository due to be
//! asked about is asked in one GraphQL question, and each is asked about as
//! often as it can change: every minute while a review or checks are being
//! waited for, every five otherwise, and every half hour once it is merged or
//! closed. A push this PC made is asked about at once. The account's hourly
//! allowance is shared with everything else that signs in as it, so nothing
//! is asked of GitHub's own accord once little of it is left
//! ([`crate::github::spare`]).
//!
//! The asking happens on a thread of its own and the answers are left where
//! the window can pick them up. A window that stops drawing because GitHub is
//! slow would be a poor way to learn that a branch has been merged.

use crate::prnews::{self, Ci, Said};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often a branch is asked about, by what its pull request is doing.
/// Waited on: a bot's review and a CI run each take a few minutes, and a
/// minute late is still before anybody would have gone to look
const WAITING_EVERY: Duration = Duration::from_secs(60);
/// Open, with nothing waited on: a push made elsewhere is noticed this late
/// at worst. A push from this PC is asked about at once (see [`Watch::of`])
const OPEN_EVERY: Duration = Duration::from_secs(300);
/// No pull request yet: one made on the website shows up this late at worst.
/// One made here is asked about at once
const NONE_EVERY: Duration = Duration::from_secs(120);
/// Merged or closed: only a new pull request from the same branch changes it
const DONE_EVERY: Duration = Duration::from_secs(1800);

/// How many branches go in one question. GitHub counts what a question costs
/// by how many things it could return; this many costs a few points of the
/// hour's five thousand
const BRANCHES_PER_QUESTION: usize = 25;

/// How long the asking thread waits for more branches before asking: the
/// rows of one desk are drawn together, and asked about together
const GATHER: Duration = Duration::from_millis(300);

/// How long what this PC's git hands out for GitHub is trusted before it is
/// asked again. Somebody signing in again should not wait for a restart
const PC_FRESH: Duration = Duration::from_secs(600);

/// What a branch's pull request is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    Draft,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pr {
    pub number: u64,
    pub state: State,
}

impl Pr {
    /// How it reads in a row eighteen columns wide.
    ///
    /// An open one says only its number: it is the ordinary case, and a word
    /// that appears on every row tells nobody anything. The other three are
    /// worth a word each, because each one means "stop waiting for this"
    pub fn short(&self) -> String {
        match self.state {
            State::Open => format!("#{}", self.number),
            State::Draft => format!("#{} draft", self.number),
            State::Merged => format!("#{} merged", self.number),
            State::Closed => format!("#{} closed", self.number),
        }
    }

    fn open(&self) -> bool {
        matches!(self.state, State::Open | State::Draft)
    }
}

/// What a row shows of an open pull request's review and checks, beside its
/// number. The words are the screen's; this is what they are about
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct News {
    pub number: u64,
    pub review: Said,
    /// With [`Said::Found`]: how many open threads have something not yet
    /// handed to an AI
    pub found: usize,
    pub ci: Ci,
    /// A push noticed a moment ago, whose review and checks may start any
    /// minute ([`prnews::settling`]): asked about as often as a wait is
    #[serde(skip)]
    pub settling: bool,
}

impl News {
    /// Whether there is anything to show
    pub fn any(&self) -> bool {
        self.review != Said::None || matches!(self.ci, Ci::Running | Ci::Failed)
    }
}

/// Something to tell somebody about a pull request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Told {
    pub account: String,
    pub repo: String,
    pub branch: String,
    /// The project the row that asked belongs to
    pub project: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    /// The commit it is about
    pub head: String,
    pub what: Tell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tell {
    /// A review came back with this many threads not yet handed to an AI
    Found(usize),
    /// A review came back with nothing to do
    Clean,
    CiPassed,
    /// The names of the checks that failed
    CiFailed(Vec<String>),
}

/// One branch of one repository, as one account sees it: (account, repo,
/// branch). The account is part of it because a repository one account can
/// see is one another may not
type Key = (String, String, String);

struct Slot {
    asked: Instant,
    /// How long until it is asked again, by what was last found
    every: Duration,
    pr: Option<Pr>,
    news: News,
    /// The project of the row that last asked: what was handed to an AI is
    /// remembered by it ([`crate::handed`])
    project: String,
    /// Where the branch has been seen on the server, as this PC's git knows
    /// it: a push from here moves it, and is asked about at once. Each of
    /// them, not only the last: two clones of one repository on one branch,
    /// fetched at different times, say two different commits, and asking
    /// whenever the two disagreed would be asking on every frame
    pushed: Vec<String>,
}

/// How many of those are kept per branch
const PUSHED_KEPT: usize = 8;

/// How long until a branch is asked about again
fn every(pr: Option<&Pr>, news: &News) -> Duration {
    match pr {
        None => NONE_EVERY,
        Some(p) if !p.open() => DONE_EVERY,
        _ if news.review == Said::Waiting || news.ci == Ci::Running || news.settling => WAITING_EVERY,
        _ => OPEN_EVERY,
    }
}

/// What the asking thread is handed: the branch, and the project of the row
/// that asked about it
type Ask = (Key, String);

/// Somewhere to ask, that never keeps the window waiting.
pub struct Watch {
    ask: Sender<Ask>,
    known: Arc<Mutex<HashMap<Key, Slot>>>,
    /// The tokens of the desk on screen, by account name. Held here rather
    /// than handed to the thread once, because the thread outlives any one
    /// desk. Never handed back out
    tokens: Arc<Mutex<HashMap<String, String>>>,
    told: Receiver<Told>,
}

impl Watch {
    pub fn start() -> Watch {
        let known: Arc<Mutex<HashMap<Key, Slot>>> = Arc::new(Mutex::new(HashMap::new()));
        let tokens: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let (ask, inbox) = channel::<Ask>();
        let (tell, told) = channel::<Told>();
        std::thread::spawn({
            let (known, tokens) = (Arc::clone(&known), Arc::clone(&tokens));
            move || serve(inbox, known, tokens, tell)
        });
        Watch { ask, known, tokens, told }
    }

    /// The tokens the desk now on screen asks with, by account.
    ///
    /// What is already known is thrown away when they change: the same account
    /// name in another desk is another account, and "no pull request" and "not
    /// allowed to look" would otherwise be the same empty line
    pub fn use_tokens(&self, tokens: HashMap<String, String>) {
        if let Ok(mut held) = self.tokens.lock() {
            if *held == tokens {
                return;
            }
            *held = tokens;
        }
        if let Ok(mut known) = self.known.lock() {
            known.clear();
        }
    }

    /// What is known about this branch as `account` sees it, and a nudge to
    /// find out if it is time.
    ///
    /// Returning what we have and asking in the background is the whole shape
    /// of this: the row draws now with whatever is known, including nothing.
    /// `project` is the project the row belongs to; `pushed` is the commit
    /// this PC's git last saw the branch at on the server, when it knows --
    /// a push from here changes it, and the branch is asked about at once
    pub fn of(&self, account: &str, repo: &str, branch: &str, project: &str, pushed: Option<&str>) -> (Option<Pr>, News) {
        let key = (account.to_string(), repo.to_string(), branch.to_string());
        let Ok(mut known) = self.known.lock() else { return (None, News::default()) };
        let pushed = pushed.unwrap_or_default();
        match known.get_mut(&key) {
            Some(slot) if slot.asked.elapsed() < slot.every && (pushed.is_empty() || slot.pushed.iter().any(|p| p == pushed)) => {
                slot.project = project.to_string();
                (slot.pr, slot.news)
            }
            found => {
                let (had, news, mut seen) = found.map(|s| (s.pr, s.news, std::mem::take(&mut s.pushed))).unwrap_or_default();
                if !pushed.is_empty() && !seen.iter().any(|p| p == pushed) {
                    seen.push(pushed.to_string());
                    if seen.len() > PUSHED_KEPT {
                        seen.remove(0);
                    }
                }
                // Marked as asked before the answer arrives, so a slow reply
                // does not turn into one request per frame
                known.insert(key.clone(), Slot {
                    asked: Instant::now(),
                    every: every(had.as_ref(), &news),
                    pr: had,
                    news,
                    project: project.to_string(),
                    pushed: seen,
                });
                drop(known);
                let _ = self.ask.send((key, project.to_string()));
                (had, news)
            }
        }
    }

    /// Ask about this branch again the next time a row draws it: something
    /// here just changed what GitHub would say (a pull request made, its
    /// review handed to an AI)
    /// (`None` for every repository with a branch of that name)
    pub fn again(&self, repo: Option<&str>, branch: &str) {
        if let Ok(mut known) = self.known.lock() {
            for ((_, r, b), slot) in known.iter_mut() {
                if repo.is_none_or(|repo| r.eq_ignore_ascii_case(repo)) && b == branch {
                    slot.every = Duration::ZERO;
                }
            }
        }
    }

    /// Ask about the pull request [`crate::handed::key`] names again the
    /// next time a row draws it: what was handed of it to an AI has changed,
    /// and with it how many findings its row says are left
    pub fn again_handed(&self, key: &str) {
        if let Ok(mut known) = self.known.lock() {
            for ((_, _, b), slot) in known.iter_mut() {
                let n = slot.pr.map(|p| p.number).unwrap_or(0);
                if n != 0 && crate::handed::key(&slot.project, n, b) == key {
                    slot.every = Duration::ZERO;
                }
            }
        }
    }

    /// What has happened since last time that somebody is to be told
    pub fn take_told(&self) -> Vec<Told> {
        self.told.try_iter().collect()
    }
}

/// What the asking thread keeps between questions
struct Memory {
    /// The review threads of a pull request last read, by its
    /// [`prnews::key`]: counted again against what was handed to an AI each
    /// time it is asked about, without reading them again
    threads: HashMap<String, Vec<Value>>,
}

fn serve(
    inbox: Receiver<Ask>,
    known: Arc<Mutex<HashMap<Key, Slot>>>,
    tokens: Arc<Mutex<HashMap<String, String>>>,
    tell: Sender<Told>,
) {
    // By how this PC's sign-ins were chosen: git's as it is or as one of its
    // accounts, or one of GitHub CLI's. Asked of the program that holds them,
    // and kept a while: neither answers instantly
    let mut pc: HashMap<String, (Instant, Option<String>)> = HashMap::new();
    let mut memory = Memory { threads: HashMap::new() };
    while let Ok(first) = inbox.recv() {
        std::thread::sleep(GATHER);
        let mut asks: Vec<Ask> = vec![first];
        asks.extend(inbox.try_iter());
        // By account and repository, each branch once
        let mut groups: HashMap<(String, String), Vec<(String, String)>> = HashMap::new();
        for ((account, repo, branch), project) in asks {
            let list = groups.entry((account, repo)).or_default();
            match list.iter_mut().find(|(b, _)| *b == branch) {
                Some(found) => found.1 = project,
                None => list.push((branch, project)),
            }
        }
        for ((account, repo), branches) in groups {
            // Read for each question rather than once: the desk, and with it
            // the accounts, can have changed since the last one
            let held: Option<Box<dyn Fn() -> Option<String>>> = match crate::config::pc_choice(&account) {
                Some(login) => {
                    let login = login.map(str::to_string);
                    Some(Box::new(move || pc_token(login.as_deref()).ok()))
                }
                None => crate::config::gh_choice(&account).map(|(host, login)| {
                    let (host, login) = (host.to_string(), login.to_string());
                    Box::new(move || gh_token_of(&host, &login)) as Box<dyn Fn() -> Option<String>>
                }),
            };
            let token = match held {
                Some(ask) => {
                    if pc.get(&account).is_none_or(|(at, _)| at.elapsed() > PC_FRESH) {
                        pc.insert(account.clone(), (Instant::now(), ask()));
                    }
                    pc.get(&account).and_then(|(_, t)| t.clone())
                }
                None => tokens.lock().ok().and_then(|t| t.get(&account).cloned()),
            };
            let Some(token) = token else { continue };
            // Little of the hour left, or told to wait: asked again when the
            // row next comes due, by which time it may be filled again
            if !crate::github::spare(&token, "graphql") {
                continue;
            }
            for part in branches.chunks(BRANCHES_PER_QUESTION) {
                look(&token, &account, &repo, part, &known, &tell, &mut memory);
            }
        }
    }
}

/// Ask about these branches of one repository, in one question, and leave
/// what was found where the rows read it
fn look(
    token: &str,
    account: &str,
    repo: &str,
    branches: &[(String, String)],
    known: &Mutex<HashMap<Key, Slot>>,
    tell: &Sender<Told>,
    memory: &mut Memory,
) {
    let Some(r) = crate::github::Repo::parse(repo) else { return };
    let hub = crate::github::Hub::new(token.to_string());
    let names: Vec<&str> = branches.iter().map(|(b, _)| b.as_str()).collect();
    let (query, mut vars) = question(&names);
    vars["o"] = json!(r.owner);
    vars["n"] = json!(r.name);
    // Not answered at all (the network, a refusal): what was known stays, and
    // is asked about again when it next comes due
    let Ok(answer) = hub.graphql(&query, vars) else { return };
    let file = prnews::path();
    let mut mems = prnews::read(&file);
    let handed = crate::handed::path();
    let now = prnews::now();
    for (i, (branch, project)) in branches.iter().enumerate() {
        // A repository this account cannot see answers with nothing at all,
        // which is the same "no pull request" a row shows for one not made
        let nodes = answer.pointer(&format!("/data/repository/b{i}/nodes")).and_then(|n| n.as_array());
        let read = nodes
            .into_iter()
            .flatten()
            .find(|n| {
                n.pointer("/headRepositoryOwner/login")
                    .and_then(|o| o.as_str())
                    .is_some_and(|o| o.eq_ignore_ascii_case(&r.owner))
            })
            .and_then(read_node);
        let mut news = News::default();
        let pr = read.as_ref().map(|(pr, _)| *pr);
        if let Some((pr, snap)) = &read {
            let k = prnews::key(repo, pr.number);
            let judged = prnews::judge(mems.get(&k).cloned().as_ref().unwrap_or(&Default::default()), snap, now);
            let m = judged.mem;
            let handed_key = crate::handed::key(project, pr.number, branch);
            // The threads are read again when a review came back, and while
            // any of them are still to be handed over: one resolved on
            // GitHub since is gone from them
            let had = memory.threads.get(&k).map(|t| crate::handed::new_threads(&handed, &handed_key, t, &[]).0.len());
            let read_again = judged.reviewed
                || (m.ended == prnews::Ended::Reviewed && had.is_none_or(|n| n > 0) && !project.is_empty());
            match read_again.then(|| hub.open_review_threads(&r, pr.number)) {
                Some(Ok(threads)) => {
                    memory.threads.insert(k.clone(), threads);
                }
                // A review came back and what it found could not be read:
                // nothing is told and nothing is remembered of this look, so
                // the next one reads it again rather than calling it clean
                Some(Err(_)) if judged.reviewed => {
                    if let Ok(mut k) = known.lock()
                        && let Some(slot) = k.get_mut(&(account.to_string(), repo.to_string(), branch.clone()))
                    {
                        slot.asked = Instant::now();
                        slot.pr = Some(*pr);
                        slot.every = WAITING_EVERY;
                    }
                    continue;
                }
                _ => {}
            }
            let fresh = memory
                .threads
                .get(&k)
                .map(|t| crate::handed::new_threads(&handed, &handed_key, t, &[]).0.len())
                .unwrap_or(0);
            news = News {
                number: pr.number,
                review: prnews::said(&m, fresh),
                found: fresh,
                ci: snap.ci,
                settling: prnews::settling(&m, now),
            };
            let say = |what: Tell| {
                let _ = tell.send(Told {
                    account: account.to_string(),
                    repo: repo.to_string(),
                    branch: branch.clone(),
                    project: project.clone(),
                    number: pr.number,
                    title: snap.title.clone(),
                    url: snap.url.clone(),
                    head: snap.head.clone(),
                    what,
                });
            };
            if judged.reviewed {
                say(if fresh > 0 { Tell::Found(fresh) } else { Tell::Clean });
            } else if judged.clean {
                say(Tell::Clean);
            }
            match judged.ci_ended {
                Some(Ci::Passed) => say(Tell::CiPassed),
                Some(Ci::Failed) => say(Tell::CiFailed(snap.failed.clone())),
                _ => {}
            }
            mems.insert(k, m);
        }
        if let Ok(mut k) = known.lock()
            && let Some(slot) = k.get_mut(&(account.to_string(), repo.to_string(), branch.clone()))
        {
            slot.asked = Instant::now();
            slot.pr = pr;
            slot.news = news;
            slot.every = every(pr.as_ref(), &news);
        }
    }
    prnews::write(&file, mems);
}

/// The question for these branches, and its variables but the repository's:
/// each branch's newest pull requests under a name of its own (`b0`, `b1`...),
/// with what [`read_node`] reads of each. A branch name goes in as a
/// variable, never into the text of the question
fn question(branches: &[&str]) -> (String, Value) {
    let mut params = String::from("$o:String!,$n:String!");
    let mut asks = String::new();
    let mut vars = json!({});
    for (i, b) in branches.iter().enumerate() {
        params.push_str(&format!(",$b{i}:String!"));
        // Newest first, and two of them: a branch can have had several pull
        // requests over its life and the newest is the one that matters, but
        // a fork's branch of the same name is one of them too
        asks.push_str(&format!(
            "b{i}:pullRequests(headRefName:$b{i},first:2,orderBy:{{field:CREATED_AT,direction:DESC}}){{nodes{{...P}}}} "
        ));
        vars[format!("b{i}")] = json!(b);
    }
    let query = format!(
        "query({params}){{repository(owner:$o,name:$n){{{asks}}}}}\
         fragment P on PullRequest{{number title url state isDraft headRefOid headRepositoryOwner{{login}} author{{login}} \
         commits(last:1){{nodes{{commit{{committedDate statusCheckRollup{{contexts(first:50){{nodes{{__typename \
         ...on CheckRun{{name status conclusion checkSuite{{workflowRun{{workflow{{name}}}}}}}} ...on StatusContext{{context state}}}}}}}}}}}}}} \
         reviews(last:20){{nodes{{id state body author{{login __typename}} submittedAt commit{{oid}} comments(first:1){{nodes{{replyTo{{id}}}}}}}}}} \
         reviewRequests(first:10){{nodes{{requestedReviewer{{__typename ...on User{{login}} ...on Bot{{login}} ...on Mannequin{{login}} ...on Team{{slug}}}}}}}} \
         reactions(last:20){{nodes{{content createdAt user{{login}}}}}} \
         comments(last:10){{nodes{{author{{login __typename}} updatedAt}}}} \
         early:comments(first:3){{nodes{{author{{login __typename}} updatedAt}}}}}}"
    );
    (query, vars)
}

/// One pull request, as the question above has GitHub describe it: what a row
/// shows of it, and what [`prnews::judge`] reads
fn read_node(v: &Value) -> Option<(Pr, prnews::Snapshot)> {
    let number = v.get("number")?.as_u64()?;
    let text = |p: &str| v.pointer(p).and_then(|s| s.as_str()).unwrap_or_default().to_string();
    let draft = v.get("isDraft").and_then(|d| d.as_bool()).unwrap_or(false);
    // GitHub says MERGED itself here, unlike its REST answer -- and a merged
    // one reported as closed would be the most misleading thing a row could say
    let state = match text("/state").as_str() {
        "MERGED" => State::Merged,
        "CLOSED" => State::Closed,
        _ if draft => State::Draft,
        _ => State::Open,
    };
    let pr = Pr { number, state };
    let list = |p: &str| v.pointer(p).and_then(|n| n.as_array()).cloned().unwrap_or_default();
    let commit = list("/commits/nodes").into_iter().next().unwrap_or(Value::Null);
    let mut failed = Vec::new();
    let (mut running, mut passed) = (false, false);
    for c in commit.pointer("/commit/statusCheckRollup/contexts/nodes").and_then(|n| n.as_array()).into_iter().flatten() {
        let s = |k: &str| c.get(k).and_then(|x| x.as_str()).unwrap_or_default();
        let (name, verdict) = match s("__typename") {
            "StatusContext" => (s("context").to_string(), crate::github::status_verdict(s("state"))),
            // Named as GitHub's own page names it, by its workflow first: a
            // job called "check" or "test" says nothing on its own
            _ => {
                let flow = c.pointer("/checkSuite/workflowRun/workflow/name").and_then(|x| x.as_str()).unwrap_or_default();
                let name = match flow {
                    "" => s("name").to_string(),
                    f if f == s("name") => f.to_string(),
                    f => format!("{f} / {}", s("name")),
                };
                (name, crate::github::check_verdict(s("status") == "COMPLETED", s("conclusion")))
            }
        };
        match verdict {
            "pending" => running = true,
            "failed" => failed.push(name),
            "passed" => passed = true,
            _ => {}
        }
    }
    // Ended only once every check has: one still running may yet fail
    let ci = match () {
        _ if running => Ci::Running,
        _ if !failed.is_empty() => Ci::Failed,
        _ if passed => Ci::Passed,
        _ => Ci::None,
    };
    let who = |n: &Value, p: &str| prnews::login(n.pointer(p).and_then(|s| s.as_str()).unwrap_or_default());
    let is_bot = |n: &Value, p: &str| n.pointer(p).and_then(|s| s.as_str()) == Some("Bot");
    let at = |n: &Value, k: &str| prnews::seconds(n.get(k).and_then(|s| s.as_str()).unwrap_or_default());
    let reviews = list("/reviews/nodes")
        .iter()
        // One being written and not yet sent is nobody's review yet
        .filter(|n| n.get("state").and_then(|s| s.as_str()) != Some("PENDING"))
        .map(|n| prnews::Review {
            id: n.get("id").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
            by: prnews::Act { who: who(n, "/author/login"), bot: is_bot(n, "/author/__typename"), at: at(n, "submittedAt") },
            commit: n.pointer("/commit/oid").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
            // An answer in a thread: no words of its own, and its comment
            // answers one already there
            reply: n.get("body").and_then(|b| b.as_str()).unwrap_or_default().trim().is_empty()
                && n.pointer("/comments/nodes/0/replyTo/id").is_some_and(|i| !i.is_null()),
        })
        .filter(|r| !r.id.is_empty())
        .collect();
    let requested = list("/reviewRequests/nodes")
        .iter()
        .filter_map(|n| {
            let r = n.get("requestedReviewer")?;
            r.get("login").or_else(|| r.get("slug")).and_then(|s| s.as_str()).map(prnews::login)
        })
        .collect();
    // A reaction names its giver as a user, a bot's with `[bot]` after it
    let reacted = |content: &str| {
        list("/reactions/nodes")
            .iter()
            .filter(|n| n.get("content").and_then(|c| c.as_str()) == Some(content))
            .map(|n| {
                let login = n.pointer("/user/login").and_then(|s| s.as_str()).unwrap_or_default();
                prnews::Act { who: prnews::login(login), bot: login.ends_with("[bot]"), at: at(n, "createdAt") }
            })
            .filter(|a| !a.who.is_empty())
            .collect::<Vec<_>>()
    };
    // The newest, and the first few: a reviewing bot's summary is usually
    // among the first comments, and on a busy pull request it is long out of
    // the newest by the time it is edited
    let comments = list("/early/nodes")
        .iter()
        .chain(list("/comments/nodes").iter())
        .map(|n| prnews::Act { who: who(n, "/author/login"), bot: is_bot(n, "/author/__typename"), at: at(n, "updatedAt") })
        .filter(|a| !a.who.is_empty())
        .collect();
    let snap = prnews::Snapshot {
        number,
        title: text("/title"),
        url: text("/url"),
        open: pr.open(),
        author: prnews::login(&text("/author/login")),
        head: text("/headRefOid"),
        committed: prnews::seconds(commit.pointer("/commit/committedDate").and_then(|s| s.as_str()).unwrap_or_default()),
        ci,
        failed,
        reviews,
        requested,
        thumbs: reacted("THUMBS_UP"),
        eyes: reacted("EYES"),
        comments,
    };
    Some((pr, snap))
}

/// Why git on this PC handed out no GitHub credential
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcSignIn {
    /// Nothing stored, or nothing that can be told apart from nothing
    None,
    /// More than one account stored, by these names. Git would ask which one,
    /// and nobody is there to answer
    Many(Vec<String>),
    /// The account asked for by name is not one of those stored
    Gone(String),
}

/// What git on this PC hands out for GitHub: the credential a push from a
/// terminal here would sign in with, or -- given `login` -- the one it holds
/// for that account. Asked of git's own credential store the way git asks it,
/// with nobody to prompt.
///
/// A store holding two accounts fails the same way as one holding none -- it
/// wants to ask which -- so on a failure the store is asked what it holds,
/// and the two are told apart
pub fn pc_token(login: Option<&str>) -> Result<String, PcSignIn> {
    let named = login.map(|l| format!("username={l}\n")).unwrap_or_default();
    let said = crate::git::run_as(
        &std::env::temp_dir(),
        &["-c", "credential.interactive=never", "credential", "fill"],
        &format!("protocol=https\nhost={}\n{named}\n", crate::config::GITHUB_HOST),
        STORE_LIMIT,
        &crate::git::As::default(),
    );
    let token = said.ok().and_then(|said| {
        said.lines()
            .find_map(|l| l.strip_prefix("password="))
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    });
    match (token, login) {
        (Some(t), _) => Ok(t),
        (None, Some(l)) => Err(PcSignIn::Gone(l.to_string())),
        (None, None) => match pc_accounts() {
            names if names.len() > 1 => Err(PcSignIn::Many(names)),
            _ => Err(PcSignIn::None),
        },
    }
}

/// How long git's credential store is given to answer one question. It answers
/// in a fraction of a second; a store that takes longer is waiting on
/// something that will not come
const STORE_LIMIT: Duration = Duration::from_secs(15);

/// The longest reading an account's sign-in can take, whichever kind it is:
/// this PC's git asked twice (for the token, then, when there was none, for
/// the accounts that say why), GitHub CLI once, a saved token at once
pub fn slowest_sign_in() -> Duration {
    (STORE_LIMIT * 2).max(GH_LIMIT)
}

/// How long the list of the PC's GitHub accounts is kept before it is read again
const PC_ACCOUNTS_FRESH: Duration = Duration::from_secs(30);

static PC_ACCOUNTS: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);
static PC_ACCOUNTS_ASKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The GitHub accounts Git Credential Manager holds on this PC, read now.
/// Empty when it holds none, or when the helper is another one that cannot
/// be asked this
pub fn pc_accounts() -> Vec<String> {
    let names = crate::git::run_as(
        &std::env::temp_dir(),
        &["credential-manager", "github", "list"],
        "",
        STORE_LIMIT,
        &crate::git::As::default(),
    )
    .map(|out| account_names(&out))
    .unwrap_or_default();
    if let Ok(mut kept) = PC_ACCOUNTS.lock() {
        *kept = Some((Instant::now(), names.clone()));
    }
    names
}

/// The same, as last read, for a screen that cannot wait on a program: what
/// is known now, and a fresh read on its way when that is old. Empty until
/// the first read comes back
pub fn pc_accounts_known() -> Vec<String> {
    let kept = PC_ACCOUNTS.lock().ok().and_then(|k| k.clone());
    let stale = kept.as_ref().is_none_or(|(at, _)| at.elapsed() > PC_ACCOUNTS_FRESH);
    if stale && !PC_ACCOUNTS_ASKING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        std::thread::spawn(|| {
            pc_accounts();
            PC_ACCOUNTS_ASKING.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }
    kept.map(|(_, names)| names).unwrap_or_default()
}

static GH_ACCOUNTS: Mutex<Option<(Instant, Vec<GhAccount>)>> = Mutex::new(None);
static GH_ACCOUNTS_ASKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The accounts GitHub CLI is signed in as, as last read (see
/// [`pc_accounts_known`] for the shape): what is known now, and a fresh read
/// on its way when that is old. Empty until the first read comes back, and
/// when gh is not on this PC
pub fn gh_accounts_known() -> Vec<GhAccount> {
    let kept = GH_ACCOUNTS.lock().ok().and_then(|k| k.clone());
    let stale = kept.as_ref().is_none_or(|(at, _)| at.elapsed() > PC_ACCOUNTS_FRESH);
    if stale && !GH_ACCOUNTS_ASKING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        std::thread::spawn(|| {
            let read = gh_accounts().unwrap_or_default();
            if let Ok(mut kept) = GH_ACCOUNTS.lock() {
                *kept = Some((Instant::now(), read));
            }
            GH_ACCOUNTS_ASKING.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }
    kept.map(|(_, a)| a).unwrap_or_default()
}

/// One account per line, as the credential manager lists them
fn account_names(listed: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in listed.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// The token GitHub CLI (`gh`) is signed in with for `host`, asked of `gh`
/// itself. None when it is not installed, not signed in to that server, or
/// does not answer in time -- nobody can see a prompt from here, so none is
/// allowed to appear.
///
/// Found through the same search a new terminal's PATH gets, so a `gh`
/// installed after this app started is found without starting it again
pub fn gh_token(host: &str) -> Option<String> {
    run_gh(&["auth", "token", "--hostname", host], "")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// The token GitHub CLI holds for one of its accounts on `host`, by login.
/// With two accounts signed in, `gh auth token` alone answers with the
/// active one; this asks for the one named
pub fn gh_token_of(host: &str, login: &str) -> Option<String> {
    run_gh(&["auth", "token", "--hostname", host, "--user", login], "")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// One account GitHub CLI is signed in as
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GhAccount {
    pub host: String,
    pub login: String,
    /// The one `gh` uses for that host when nothing names another
    pub active: bool,
}

/// The accounts GitHub CLI is signed in as on this PC, every host. None when
/// it is not installed; empty when it is signed in nowhere.
///
/// `gh auth status` answers in JSON when asked to, and answers with a failure
/// code as soon as one account's token no longer works -- the list is still
/// in what it printed, so it is read either way
pub fn gh_accounts() -> Option<Vec<GhAccount>> {
    let said = match run_gh_raw(&["auth", "status", "--json", "hosts"], "") {
        Ok(out) | Err(GhFailed::Said { out, .. }) => out,
        Err(GhFailed::Missing) => return None,
        Err(GhFailed::Silent) => String::new(),
    };
    Some(gh_accounts_in(&said))
}

/// The accounts in what `gh auth status --json hosts` printed
fn gh_accounts_in(said: &str) -> Vec<GhAccount> {
    let v: serde_json::Value = serde_json::from_str(said.trim()).unwrap_or_default();
    let Some(hosts) = v.get("hosts").and_then(|h| h.as_object()) else { return Vec::new() };
    let mut out = Vec::new();
    for (host, list) in hosts {
        for a in list.as_array().into_iter().flatten() {
            let Some(login) = a.get("login").and_then(|l| l.as_str()).map(str::trim).filter(|l| !l.is_empty()) else {
                continue;
            };
            out.push(GhAccount {
                host: host.clone(),
                login: login.to_string(),
                active: a.get("active").and_then(|b| b.as_bool()).unwrap_or(false),
            });
        }
    }
    out
}

/// Sign GitHub CLI in to `host` with a token, as `gh auth login --with-token`
/// does when the token is typed to it. What `gh` said when it would not
pub fn gh_sign_in(host: &str, token: &str) -> Result<(), String> {
    run_gh(&["auth", "login", "--hostname", host, "--with-token"], &format!("{}\n", token.trim())).map(|_| ())
}

/// Sign GitHub CLI out of one account on `host`. Its token stays valid on
/// GitHub, the same as `gh auth logout` leaves it
pub fn gh_sign_out(host: &str, login: &str) -> Result<(), String> {
    run_gh(&["auth", "logout", "--hostname", host, "--user", login], "").map(|_| ())
}

/// Why GitHub CLI did not answer
enum GhFailed {
    /// Not on this PC
    Missing,
    /// Started and took too long, or could not be read
    Silent,
    /// Finished with a failure, and this is what it printed and said
    Said { out: String, err: String },
}

/// Runs GitHub CLI with `args`, `input` on its standard input, and answers
/// with what it printed when it succeeded. Nobody can see a prompt from here,
/// so none is allowed to appear, and a `gh` that takes too long is killed.
///
/// Found through the same search a new terminal's PATH gets, so a `gh`
/// installed after this app started is found without starting it again
fn run_gh(args: &[&str], input: &str) -> Result<String, String> {
    match run_gh_raw(args, input) {
        Ok(out) => Ok(out),
        Err(GhFailed::Missing) => Err(crate::i18n::t("settings.gitacct.gh_missing")),
        Err(GhFailed::Silent) => Err(crate::i18n::t("settings.gitacct.gh_silent")),
        Err(GhFailed::Said { err, out }) => {
            let said = if err.trim().is_empty() { out } else { err };
            Err(said.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default().to_string())
        }
    }
}

fn run_gh_raw(args: &[&str], input: &str) -> Result<String, GhFailed> {
    use std::process::{Command, Stdio};
    let exe = crate::tab::resolve_command("gh").ok_or(GhFailed::Missing)?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(if input.is_empty() { Stdio::null() } else { Stdio::piped() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::detach_console(&mut cmd).spawn().map_err(|_| GhFailed::Missing)?;
    if !input.is_empty()
        && let Some(mut w) = child.stdin.take()
    {
        let _ = std::io::Write::write_all(&mut w, input.as_bytes());
    }
    // Both read on threads of their own, so a gh that fills one before the
    // other is read cannot stall on it
    fn drain<R: std::io::Read + Send + 'static>(from: Option<R>) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut f) = from {
                let _ = f.read_to_string(&mut s);
            }
            s
        })
    }
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|_| GhFailed::Silent)? {
            Some(s) => break s,
            None if started.elapsed() > GH_LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GhFailed::Silent);
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let out = out.join().map_err(|_| GhFailed::Silent)?;
    let err = err.join().map_err(|_| GhFailed::Silent)?;
    if status.success() { Ok(out) } else { Err(GhFailed::Said { out, err }) }
}

/// Whether a name can be a GitHub login, or a git server's host: letters,
/// digits and `-` `_` `.`, and nothing a command line or a credential line
/// could read as more than one word
pub fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Whether a token can be handed to git or gh on a line of its own
pub fn plain_token(s: &str) -> bool {
    !s.is_empty() && s.len() <= 512 && !s.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Store a GitHub token in the credential store git on this PC uses, the way
/// git itself does after a push that signed in: `git credential approve`, on
/// its standard input, so the token is on no command line. What git said
/// when it would not
pub fn pc_store(login: &str, token: &str) -> Result<(), String> {
    let line = format!("protocol=https\nhost={}\nusername={login}\npassword={}\n\n", crate::config::GITHUB_HOST, token.trim());
    crate::git::run_as(&std::env::temp_dir(), &["credential", "approve"], &line, Duration::from_secs(15), &crate::git::As::default())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Take one GitHub account out of the credential store git on this PC uses.
/// Its token stays valid on GitHub
pub fn pc_forget(login: &str) -> Result<(), String> {
    let line = format!("protocol=https\nhost={}\nusername={login}\n\n", crate::config::GITHUB_HOST);
    crate::git::run_as(&std::env::temp_dir(), &["credential", "reject"], &line, Duration::from_secs(15), &crate::git::As::default())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// How long `gh auth token` is given. It reads a file or the system's
/// credential store and answers at once; longer is something wrong
const GH_LIMIT: Duration = Duration::from_secs(10);

/// The token to ask with: the one the account was given, and nothing else.
///
/// Never written anywhere, never logged, and sent to nowhere but GitHub's own
/// API. The machine's `GITHUB_TOKEN` is not read: it is one account for every
/// desk, and nobody chose it.
///
/// `own` is the account's token, already looked up by whoever knows which
/// desk is being asked about -- this module never reaches into the secrets
pub fn find(own: Option<String>) -> Option<String> {
    own.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// What GitHub says about a token: whether it still works, whose it is, and
/// when it stops working.
///
/// The state and the date, never the value. A token that has run out is said
/// out loud -- a row that quietly stops showing pull request numbers looks
/// exactly like a branch that has none
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// Where the token came from, or `None` when there is no token at all
    pub source: Option<&'static str>,
    /// Whether GitHub answered the way it answers a token it accepts
    pub ok: bool,
    /// The account the token speaks as
    pub login: Option<String>,
    /// When it stops working, as GitHub wrote it, and in days from today.
    /// Both absent means a token that does not expire
    pub expires_at: Option<String>,
    pub expires_in_days: Option<i64>,
    /// What GitHub answered with, for the one case worth telling apart: 401 is
    /// a token that has run out or been taken away
    pub status: u16,
}

/// Ask GitHub about the token this desk would use.
///
/// Its own call rather than a side effect of asking about a branch: the
/// settings screen has to be able to say "this one works and has eleven days
/// left" before anybody is looking at a branch at all
pub fn probe(own: Option<String>) -> Probe {
    let Some(token) = find(own) else {
        return Probe::default();
    };
    let mut said = Probe {
        source: Some("desk"),
        ..Default::default()
    };
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .new_agent();
    let answer = agent
        .get("https://api.github.com/user")
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header(
            "User-Agent",
            concat!("shikisha-term/", env!("CARGO_PKG_VERSION")),
        )
        .call();
    let mut resp = match answer {
        Ok(r) => r,
        // A refusal comes back as an error in this client, and the status is
        // the whole point of asking
        Err(ureq::Error::StatusCode(code)) => {
            said.status = code;
            return said;
        }
        Err(_) => return said,
    };
    said.status = resp.status().as_u16();
    said.ok = said.status == 200;
    // GitHub puts the end date of a fine-grained token in a header of its own,
    // on every answer. Absent means one that does not expire
    if let Some(when) = resp
        .headers()
        .get("github-authentication-token-expiration")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        said.expires_in_days = days_until(when);
        said.expires_at = Some(when.to_string());
    }
    if let Ok(v) = resp.body_mut().read_json::<serde_json::Value>() {
        said.login = v
            .get("login")
            .and_then(|l| l.as_str())
            .map(str::to_string);
    }
    said
}

/// How many days from today until a date GitHub wrote down.
///
/// Its header reads `2026-10-04 12:00:00 UTC`, and only the day matters: "8
/// days left" is what a person acts on. Worked out here rather than with a date
/// library, because the whole of the arithmetic is turning two calendar days
/// into two numbers
fn days_until(stamp: &str) -> Option<i64> {
    // Its header reads `2026-10-04 12:00:00 UTC` today. The date is taken off
    // the front either way a time can be joined to it, because a header's
    // spelling is GitHub's to change and the day is all of it that is read
    let day = stamp.split([' ', 'T']).next()?;
    let mut part = day.split('-');
    let y: i64 = part.next()?.parse().ok()?;
    let m: i64 = part.next()?.parse().ok()?;
    let d: i64 = part.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64
        / 86_400;
    Some(days_from_epoch(y, m, d) - now)
}

/// Days from 1970-01-01 to a calendar day (Howard Hinnant's algorithm).
fn days_from_epoch(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_one_says_only_its_number() {
        // The ordinary case. A word on every row is a word that tells nobody
        // anything, and the row is eighteen columns wide
        assert_eq!(Pr { number: 12, state: State::Open }.short(), "#12");
        assert_eq!(Pr { number: 12, state: State::Draft }.short(), "#12 draft");
        assert_eq!(Pr { number: 12, state: State::Merged }.short(), "#12 merged");
        assert_eq!(Pr { number: 12, state: State::Closed }.short(), "#12 closed");
    }

    /// What `gh auth status --json hosts` prints is read as accounts, every
    /// host, with the one gh uses marked; a broken answer is no accounts
    #[test]
    fn gh_accounts_are_read_from_its_json() {
        let said = r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","login":"octo-cat","tokenSource":"keyring"},{"state":"error","active":false,"host":"github.com","login":"octo-dog"}],"ghe.example":[{"active":true,"login":"me"}]}}"#;
        let got = gh_accounts_in(said);
        assert_eq!(got.len(), 3);
        assert!(got.iter().any(|a| a.host == "github.com" && a.login == "octo-cat" && a.active));
        assert!(got.iter().any(|a| a.host == "github.com" && a.login == "octo-dog" && !a.active));
        assert!(got.iter().any(|a| a.host == "ghe.example" && a.login == "me"));
        assert!(gh_accounts_in("not json").is_empty());
        assert!(gh_accounts_in(r#"{"hosts":{}}"#).is_empty());
    }

    /// What is handed to git and gh on a line of its own has to be one word
    #[test]
    fn names_and_tokens_are_one_word() {
        assert!(plain_name("octo-cat"));
        assert!(plain_name("ghe.example.com"));
        assert!(!plain_name("octo cat"));
        assert!(!plain_name("a\nb"));
        assert!(!plain_name(""));
        assert!(plain_token("ghp_abc123"));
        assert!(!plain_token("ghp_abc 123"));
        assert!(!plain_token("ghp_abc\n"));
    }

    #[test]
    fn stored_accounts_are_read_one_per_line() {
        // Two accounts is what makes git ask which, so a name listed twice or a
        // blank line must not be counted as a second one
        assert_eq!(account_names("styleio\r\nx-access-token\r\n"), ["styleio", "x-access-token"]);
        assert_eq!(account_names("styleio\n\nstyleio\n"), ["styleio"]);
        assert!(account_names("").is_empty());
    }
    #[test]
    fn merged_is_not_reported_as_closed() {
        // Telling someone their work was closed when it went in would be the
        // most misleading thing this line could say
        let merged = json!({"number": 7, "state": "MERGED", "isDraft": false});
        assert_eq!(read_node(&merged).unwrap().0.state, State::Merged);
        let closed = json!({"number": 8, "state": "CLOSED", "isDraft": false});
        assert_eq!(read_node(&closed).unwrap().0.state, State::Closed);
        let draft = json!({"number": 9, "state": "OPEN", "isDraft": true});
        assert_eq!(read_node(&draft).unwrap().0.state, State::Draft);
        assert!(read_node(&draft).unwrap().1.open);
    }

    #[test]
    fn a_reply_that_is_not_a_pull_request_is_not_guessed_at() {
        assert!(read_node(&json!({})).is_none());
        assert!(read_node(&json!({"state": "OPEN"})).is_none());
    }

    /// A pull request as GitHub's GraphQL describes one a reviewing bot has
    /// been at: its review, its 👍 (named with `[bot]`), its summary comment,
    /// and checks of both kinds
    #[test]
    fn what_a_reviewing_bot_left_is_read() {
        let node = json!({
            "number": 115, "title": "Fix the login", "url": "https://github.com/o/r/pull/115",
            "state": "OPEN", "isDraft": false, "headRefOid": "f9b0", "author": {"login": "me"},
            "headRepositoryOwner": {"login": "o"},
            "commits": {"nodes": [{"commit": {"committedDate": "2026-11-09T05:11:26Z", "statusCheckRollup": {"contexts": {"nodes": [
                {"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "FAILURE",
                    "checkSuite": {"workflowRun": {"workflow": {"name": "ci"}}}},
                {"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "FAILURE",
                    "checkSuite": {"workflowRun": {"workflow": {"name": "build"}}}},
                {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"__typename": "StatusContext", "context": "deploy", "state": "SUCCESS"}
            ]}}}}]},
            "reviews": {"nodes": [{"id": "PRR_1", "author": {"login": "review-bot", "__typename": "Bot"},
                "submittedAt": "2026-11-09T05:10:24Z", "commit": {"oid": "425c"}}]},
            "reviewRequests": {"nodes": [{"requestedReviewer": {"__typename": "User", "login": "alice"}},
                {"requestedReviewer": {"__typename": "Team", "slug": "core"}}]},
            "reactions": {"nodes": [{"content": "THUMBS_UP", "createdAt": "2026-11-09T05:15:09Z", "user": {"login": "review-bot[bot]"}},
                {"content": "HEART", "createdAt": "2026-11-09T05:16:00Z", "user": {"login": "bob"}}]},
            "comments": {"nodes": [{"author": {"login": "review-bot", "__typename": "Bot"}, "updatedAt": "2026-11-09T05:15:06Z"}]}
        });
        let (pr, s) = read_node(&node).unwrap();
        assert_eq!((pr.number, pr.state), (115, State::Open));
        assert_eq!((s.head.as_str(), s.author.as_str(), s.committed), ("f9b0", "me", prnews::seconds("2026-11-09T05:11:26Z")));
        assert_eq!(s.ci, Ci::Failed);
        // Named by its workflow first, as GitHub's page does, and once when both are the same
        assert_eq!(s.failed, ["ci / test", "build"]);
        assert_eq!(s.reviews.len(), 1);
        assert!(s.reviews[0].by.bot);
        assert_eq!(s.reviews[0].commit, "425c");
        assert_eq!(s.requested, ["alice", "core"]);
        assert_eq!(s.thumbs.len(), 1);
        assert_eq!(s.thumbs[0].who, "review-bot");
        assert!(s.thumbs[0].bot);
        assert!(s.eyes.is_empty());
        assert_eq!(s.comments[0].at, prnews::seconds("2026-11-09T05:15:06Z"));
    }

    /// The checks have ended only when every one of them has
    #[test]
    fn checks_still_running_are_not_an_end() {
        let node = |runs: Value| json!({"number": 1, "state": "OPEN",
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"nodes": runs}}}}]}});
        let running = node(json!([
            {"__typename": "CheckRun", "name": "a", "status": "COMPLETED", "conclusion": "FAILURE"},
            {"__typename": "CheckRun", "name": "b", "status": "IN_PROGRESS", "conclusion": null}
        ]));
        assert_eq!(read_node(&running).unwrap().1.ci, Ci::Running);
        let waiting = node(json!([{"__typename": "StatusContext", "context": "c", "state": "PENDING"}]));
        assert_eq!(read_node(&waiting).unwrap().1.ci, Ci::Running);
        let passed = node(json!([{"__typename": "CheckRun", "name": "a", "status": "COMPLETED", "conclusion": "SKIPPED"}]));
        assert_eq!(read_node(&passed).unwrap().1.ci, Ci::Passed);
        assert_eq!(read_node(&node(json!([]))).unwrap().1.ci, Ci::None);
    }

    /// Each branch goes in as a variable of its own, never into the question's
    /// text, so a branch named like a piece of a query is only a name
    #[test]
    fn branch_names_are_variables() {
        let (q, vars) = question(&["fix/login", "x\"){evil}"]);
        assert!(q.contains("b0:pullRequests(headRefName:$b0"));
        assert!(q.contains("b1:pullRequests(headRefName:$b1"));
        assert!(!q.contains("evil"));
        assert_eq!(vars["b1"], "x\"){evil}");
    }

    #[test]
    fn a_row_is_asked_about_as_often_as_it_can_change() {
        let open = Pr { number: 1, state: State::Open };
        let waiting = News { number: 1, review: Said::Waiting, ..Default::default() };
        let running = News { number: 1, ci: Ci::Running, ..Default::default() };
        assert_eq!(every(Some(&open), &waiting), WAITING_EVERY);
        assert_eq!(every(Some(&open), &running), WAITING_EVERY);
        assert_eq!(every(Some(&open), &News::default()), OPEN_EVERY);
        assert_eq!(every(None, &News::default()), NONE_EVERY);
        assert_eq!(every(Some(&Pr { number: 1, state: State::Merged }), &waiting), DONE_EVERY);
    }

    /// Only the desk's own token is asked with. One machine, two accounts:
    /// a token in the machine's environment belongs to every desk at once,
    /// so a desk given none has none
    #[test]
    fn only_the_desk_own_token_is_used() {
        // SAFETY: this process's own environment, in a test that puts it back
        unsafe {
            std::env::set_var("GITHUB_TOKEN", "from_the_environment");
        }
        assert_eq!(find(Some("  ours  ".into())).as_deref(), Some("ours"), "surrounding spaces got into the value");
        assert_eq!(find(None), None, "the machine's token was used for a desk");
        assert_eq!(find(Some("   ".into())), None, "an empty token was used");
        unsafe {
            std::env::remove_var("GITHUB_TOKEN");
        }
    }

    /// "Eleven days left" is what a person acts on, so the date has to become a
    /// number of days -- in GitHub's own spelling of a date, including the
    /// shapes that are not a date at all
    #[test]
    fn a_date_becomes_the_days_that_are_left() {
        // The day count itself, independent of today
        assert_eq!(days_from_epoch(1970, 1, 1), 0);
        assert_eq!(days_from_epoch(1970, 1, 2), 1);
        assert_eq!(days_from_epoch(2000, 3, 1), 11017);
        // A leap day is a day
        assert_eq!(days_from_epoch(2024, 3, 1) - days_from_epoch(2024, 2, 28), 2);
        assert_eq!(days_from_epoch(2023, 3, 1) - days_from_epoch(2023, 2, 28), 1);

        // Today, through the same door the header comes in by
        let today = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            / 86_400;
        let stamp = |days: i64| {
            let d = today + days;
            // Turned back into a date the long way round, so the test is not
            // checking the arithmetic against itself
            let (mut y, mut rest) = (1970, d);
            loop {
                let len = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
                if rest < len {
                    break;
                }
                rest -= len;
                y += 1;
            }
            let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
            let lens = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
            let mut m = 1;
            for len in lens {
                if rest < len {
                    break;
                }
                rest -= len;
                m += 1;
            }
            format!("{y:04}-{m:02}-{:02} 12:00:00 UTC", rest + 1)
        };
        assert_eq!(days_until(&stamp(11)), Some(11));
        assert_eq!(days_until(&stamp(0)), Some(0));
        assert_eq!(days_until(&stamp(-3)), Some(-3), "an expired one comes out as negative days");

        // Not a date: nothing, rather than a number somebody would believe
        // The same day, with the time joined on the other way round
        assert_eq!(days_until(&stamp(11).replace(' ', "T")), Some(11));
        assert_eq!(days_until(""), None);
        assert_eq!(days_until("never"), None);
        assert_eq!(days_until("2026-13-01 00:00:00 UTC"), None);
    }

    #[test]
    fn with_no_token_nothing_is_asked_and_nothing_pretends_to_know() {
        // No token: every answer is "nothing known" rather than a made-up one
        let w = Watch::start();
        assert_eq!(w.of("work", "owner/name", "main", "proj", None).0, None);
        // Asking twice must not queue twice; the slot is marked before the
        // answer arrives so a slow reply is not one request per frame
        assert_eq!(w.of("work", "owner/name", "main", "proj", None).0, None);
        assert_eq!(w.known.lock().unwrap().len(), 1);
        // Another account asking about the same branch is another question
        assert_eq!(w.of("home", "owner/name", "main", "proj", None).0, None);
        assert_eq!(w.known.lock().unwrap().len(), 2);
    }

    /// The real thing, against the real GitHub, with a real token.
    ///
    /// Ignored by default: it needs the network and a token, and neither belongs
    /// in an ordinary run. Run it by hand when the shape of GitHub's answer is
    /// what is in doubt -- the expiry header especially, which is the one thing
    /// here that no fixture can vouch for:
    ///
    /// ```text
    /// SHIKISHA_GITHUB_PROBE=<a fine-grained token> cargo test -p shikisha-core \
    ///     -- --ignored --nocapture the_real_github
    /// ```
    ///
    /// It prints the state, the account and the days left. Never the token
    #[test]
    #[ignore = "needs the network and a token of your own"]
    fn the_real_github_answers_the_way_this_reads_it() {
        let Ok(token) = std::env::var("SHIKISHA_GITHUB_PROBE") else {
            panic!("run it with a token in SHIKISHA_GITHUB_PROBE");
        };
        let said = probe(Some(token));
        println!(
            "source={:?} ok={} status={} login={:?} expires_at={:?} days={:?}",
            said.source, said.ok, said.status, said.login, said.expires_at, said.expires_in_days
        );
        assert_eq!(said.source, Some("desk"));
        assert!(said.ok, "GitHub did not accept it: status={}", said.status);
        assert!(said.login.is_some(), "the account name was not read");
        // An expiry is not guaranteed -- a token can be made without one -- but
        // when the header is there it has to become a number of days
        if said.expires_at.is_some() {
            assert!(said.expires_in_days.is_some(), "the expiry date does not become a number of days");
        }
    }

    /// A new token means the old answers are somebody else's.
    ///
    /// A repository one account can see is one the other may not, so keeping
    /// what was learned would make "there is no pull request" and "you are not
    /// allowed to look" the same empty line
    #[test]
    fn changing_the_token_forgets_what_the_other_one_saw() {
        let w = Watch::start();
        assert_eq!(w.of("work", "owner/name", "main", "proj", None).0, None);
        assert_eq!(w.known.lock().unwrap().len(), 1);
        let ours = HashMap::from([("work".to_string(), "ours".to_string())]);
        w.use_tokens(ours.clone());
        assert!(w.known.lock().unwrap().is_empty(), "the previous account's answer is still there");
        // Saying the same thing twice is not a change, and must not throw away
        // answers that are still this token's
        assert_eq!(w.of("work", "owner/name", "main", "proj", None).0, None);
        w.use_tokens(ours);
        assert_eq!(w.known.lock().unwrap().len(), 1);
    }
}
