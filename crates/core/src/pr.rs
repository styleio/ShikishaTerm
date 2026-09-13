//! The pull request a branch is on, when there is one.
//!
//! The third thing you would otherwise go and look up, after the branch and
//! the ports. With several agents each on their own branch, "has that one been
//! merged yet" is a question about every row at once, and answering it means
//! leaving the terminal entirely.
//!
//! **Nothing has to be set up for this.** Where a desk has been given a
//! token of its own it uses that one; otherwise it uses what the person already
//! has -- `GITHUB_TOKEN` in their environment, or whatever their own `gh` is
//! signed in as. Asking someone to paste a token in so a terminal can show them
//! a number they can already see on a website is not a trade worth offering.
//!
//! A desk's own token is worth offering, though, and it is the reason this
//! is not one machine-wide answer any more: the repositories somebody works on
//! for a company and the ones they work on for themselves are reached with
//! different accounts, and whichever account answered first was the one every
//! row used. Where there is no token there is no PR line, and the settings say
//! so -- which token, whose it is, and how long it has left -- rather than
//! leaving it a mystery.
//!
//! The asking happens on a thread of its own and the answers are left where
//! the window can pick them up. A window that stops drawing because GitHub is
//! slow would be a poor way to learn that a branch has been merged.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long an answer is good for. A pull request opens, gets reviewed and
/// merges over hours; asking every minute is already generous
const FRESH: Duration = Duration::from_secs(60);

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
}

/// One branch of one repository.
type Key = (String, String);

struct Slot {
    asked: Instant,
    pr: Option<Pr>,
}

/// Somewhere to ask, that never keeps the window waiting.
pub struct Watch {
    ask: Sender<Key>,
    known: Arc<Mutex<HashMap<Key, Slot>>>,
    /// The token in use, which changes when the desk does. Held here
    /// rather than handed to the thread once, because the thread outlives any
    /// one desk. Never handed back out
    token: Arc<Mutex<Option<String>>>,
}

impl Watch {
    pub fn start() -> Watch {
        let known: Arc<Mutex<HashMap<Key, Slot>>> = Arc::new(Mutex::new(HashMap::new()));
        let token: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let (ask, inbox) = channel::<Key>();
        std::thread::spawn({
            let (known, token) = (Arc::clone(&known), Arc::clone(&token));
            move || serve(inbox, known, token)
        });
        Watch { ask, known, token }
    }

    /// The token the desk now on screen asks with.
    ///
    /// What is already known is thrown away, because it was learned with
    /// somebody else's account: a repository one token can see is a repository
    /// the other may not, and "no pull request" and "not allowed to look" would
    /// then be the same empty line
    pub fn use_token(&self, token: Option<String>) {
        if let Ok(mut held) = self.token.lock() {
            if *held == token {
                return;
            }
            *held = token;
        }
        if let Ok(mut known) = self.known.lock() {
            known.clear();
        }
    }

    /// Whether there is a token at all. Not the token -- nothing here hands
    /// that back out
    pub fn can_ask(&self) -> bool {
        self.token.lock().is_ok_and(|t| t.is_some())
    }

    /// What is known about this branch, and a nudge to find out if it is time.
    ///
    /// Returning what we have and asking in the background is the whole shape
    /// of this: the row draws now with whatever is known, including nothing
    pub fn of(&self, repo: &str, branch: &str) -> Option<Pr> {
        let key = (repo.to_string(), branch.to_string());
        let mut known = self.known.lock().ok()?;
        match known.get(&key) {
            Some(slot) if slot.asked.elapsed() < FRESH => slot.pr,
            found => {
                let had = found.and_then(|s| s.pr);
                // Marked as asked before the answer arrives, so a slow reply
                // does not turn into one request per frame
                known.insert(key.clone(), Slot { asked: Instant::now(), pr: had });
                drop(known);
                let _ = self.ask.send(key);
                had
            }
        }
    }
}

fn serve(
    inbox: Receiver<Key>,
    known: Arc<Mutex<HashMap<Key, Slot>>>,
    token: Arc<Mutex<Option<String>>>,
) {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .new_agent();
    while let Ok((repo, branch)) = inbox.recv() {
        // Read for each question rather than once: the desk, and with it
        // the account, can have changed since the last one
        let Some(now) = token.lock().ok().and_then(|t| t.clone()) else {
            continue;
        };
        let pr = look_up(&agent, &now, &repo, &branch);
        if let Ok(mut k) = known.lock() {
            k.insert((repo, branch), Slot { asked: Instant::now(), pr });
        }
    }
}

fn look_up(agent: &ureq::Agent, token: &str, repo: &str, branch: &str) -> Option<Pr> {
    let (owner, _) = repo.split_once('/')?;
    // Newest first, and only one: a branch can have had several pull requests
    // over its life, and the one that matters is the last one
    let url = format!(
        "https://api.github.com/repos/{repo}/pulls\
         ?head={owner}:{branch}&state=all&sort=created&direction=desc&per_page=1"
    );
    let mut resp = agent
        .get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header(
            "User-Agent",
            concat!("shikisha-term/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .ok()?;
    let v: serde_json::Value = resp.body_mut().read_json().ok()?;
    read_one(v.as_array()?.first()?)
}

/// One pull request, as GitHub describes it.
fn read_one(v: &serde_json::Value) -> Option<Pr> {
    let number = v.get("number")?.as_u64()?;
    // Merged is not one of the states GitHub reports; a merged one is closed
    // with a date on it. Saying "closed" to someone whose work went in would
    // be the most misleading thing this line could do
    let merged = v.get("merged_at").is_some_and(|m| !m.is_null());
    let draft = v.get("draft").and_then(|d| d.as_bool()).unwrap_or(false);
    let closed = v.get("state").and_then(|s| s.as_str()) == Some("closed");
    Some(Pr {
        number,
        state: match (merged, closed, draft) {
            (true, _, _) => State::Merged,
            (_, true, _) => State::Closed,
            (_, _, true) => State::Draft,
            _ => State::Open,
        },
    })
}

/// Whose token is being used.
///
/// Said on screen, because the three are different promises: one this desk
/// was given, one sitting in the environment of whoever started the app, and
/// whatever the person's own `gh` happens to be signed in as
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The secret `<desk>.github`
    Desk,
    /// `GITHUB_TOKEN` or `GH_TOKEN`
    Env,
    /// `gh auth token`
    Gh,
}

impl Source {
    /// The word the screen looks its sentence up by.
    pub fn id(self) -> &'static str {
        match self {
            Source::Desk => "desk",
            Source::Env => "env",
            Source::Gh => "gh",
        }
    }
}

/// The token to ask with, and where it came from.
///
/// Never written anywhere, never logged, and sent to nowhere but GitHub's own
/// API. Read in the order of how particular each one is: the token this
/// desk was given, then something the person put in their environment on
/// purpose, then whatever their own GitHub tool is signed in as.
///
/// `own` is the desk's own token, already looked up by whoever knows which
/// desk is being asked about -- this module never reaches into the secrets
pub fn find(own: Option<String>) -> Option<(String, Source)> {
    if let Some(t) = own.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        return Some((t, Source::Desk));
    }
    for name in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(name) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some((v, Source::Env));
            }
        }
    }
    from_gh().map(|t| (t, Source::Gh))
}

/// What the person's own GitHub tool will hand over, asked rather than read.
///
/// `gh auth token --hostname github.com`. Reading gh's own file is what this
/// used to do, and it was wrong twice over: a current `gh` on Windows keeps the
/// token in the credential manager, so the file has no `oauth_token:` line to
/// find at all, and a file that does have one can describe several accounts --
/// the first one under `github.com` being somebody else's is an ordinary state,
/// not a corrupt file. Only github.com is asked for: the rest of what gh knows
/// may be an enterprise server this app knows nothing about, and sending a token
/// to the wrong host is not a small mistake
fn from_gh() -> Option<String> {
    let mut cmd = std::process::Command::new("gh");
    cmd.args(["auth", "token", "--hostname", "github.com"])
        // It must answer or fail, never wait for somebody to read a question:
        // there is no console in front of this process to read one in
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let out = crate::detach_console(&mut cmd).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
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
    let Some((token, source)) = find(own) else {
        return Probe::default();
    };
    let mut said = Probe {
        source: Some(source.id()),
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

    #[test]
    fn merged_is_not_reported_as_closed() {
        // GitHub has no "merged" state: a merged one is closed with a date on
        // it. Telling someone their work was closed when it went in would be
        // the most misleading thing this line could say
        let merged = serde_json::json!({
            "number": 7, "state": "closed", "draft": false,
            "merged_at": "2026-08-25T00:00:00Z"
        });
        assert_eq!(read_one(&merged).unwrap().state, State::Merged);
        let closed = serde_json::json!({
            "number": 8, "state": "closed", "draft": false, "merged_at": null
        });
        assert_eq!(read_one(&closed).unwrap().state, State::Closed);
        let draft = serde_json::json!({
            "number": 9, "state": "open", "draft": true, "merged_at": null
        });
        assert_eq!(read_one(&draft).unwrap().state, State::Draft);
    }

    #[test]
    fn a_reply_that_is_not_a_pull_request_is_not_guessed_at() {
        assert!(read_one(&serde_json::json!({})).is_none());
        assert!(read_one(&serde_json::json!({"state": "open"})).is_none());
    }

    /// The desk's own token comes first, and nothing about the machine
    /// changes that.
    ///
    /// This is the whole point of the change: one machine, two accounts, and
    /// until now whichever one answered first was the one every row used. The
    /// environment is still read for a desk that has been given nothing,
    /// because that is what every setup so far relies on
    #[test]
    fn the_desk_is_asked_before_the_machine() {
        // SAFETY: this process's own environment, in a test that puts it back
        unsafe {
            std::env::set_var("GITHUB_TOKEN", "from_the_environment");
        }
        let (token, source) = find(Some("  ours  ".into())).expect("自分のトークンがある");
        assert_eq!(token, "ours", "前後の空白が値に入っている");
        assert_eq!(source, Source::Desk);

        let (token, source) = find(None).expect("環境変数が読まれていない");
        assert_eq!(token, "from_the_environment");
        assert_eq!(source, Source::Env);

        // A desk that was given an empty one has been given nothing
        assert_eq!(find(Some("   ".into())).unwrap().1, Source::Env);
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
        assert_eq!(days_until(&stamp(-3)), Some(-3), "切れたものは負の日数で出る");

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
        assert!(!w.can_ask());
        assert_eq!(w.of("owner/name", "main"), None);
        // Asking twice must not queue twice; the slot is marked before the
        // answer arrives so a slow reply is not one request per frame
        assert_eq!(w.of("owner/name", "main"), None);
        assert_eq!(w.known.lock().unwrap().len(), 1);
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
            panic!("SHIKISHA_GITHUB_PROBE にトークンを入れて実行してください");
        };
        let said = probe(Some(token));
        println!(
            "source={:?} ok={} status={} login={:?} expires_at={:?} days={:?}",
            said.source, said.ok, said.status, said.login, said.expires_at, said.expires_in_days
        );
        assert_eq!(said.source, Some("desk"));
        assert!(said.ok, "GitHub が受け付けませんでした: status={}", said.status);
        assert!(said.login.is_some(), "アカウント名が読めていない");
        // An expiry is not guaranteed -- a token can be made without one -- but
        // when the header is there it has to become a number of days
        if said.expires_at.is_some() {
            assert!(said.expires_in_days.is_some(), "期限の日付が日数にならない");
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
        assert_eq!(w.of("owner/name", "main"), None);
        assert_eq!(w.known.lock().unwrap().len(), 1);
        w.use_token(Some("ours".into()));
        assert!(w.can_ask());
        assert!(w.known.lock().unwrap().is_empty(), "前のアカウントの答えが残っている");
        // Saying the same thing twice is not a change, and must not throw away
        // answers that are still this token's
        assert_eq!(w.of("owner/name", "main"), None);
        w.use_token(Some("ours".into()));
        assert_eq!(w.known.lock().unwrap().len(), 1);
    }
}
