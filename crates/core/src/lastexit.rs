//! How the last run ended, and how to find out why.
//!
//! A program that vanishes without a word is the worst thing it can do to
//! somebody. This one did exactly that twice in one afternoon: the machine ran
//! out of memory, Windows stopped the process the way it stops a process that
//! cannot allocate, and nothing was written anywhere the person would look.
//! The panic log stayed empty, because a failed allocation is not a panic and
//! never reaches the panic hook.
//!
//! So: leave a mark while running and take it away on the way out. A mark
//! still there at the next start means the run before this one did not finish,
//! and Windows recorded why in its own log -- which is where the answer to
//! "what just happened to my terminal" actually lives.

use serde::{Deserialize, Serialize};

/// The file that says a run is in progress
const MARK: &str = "running.json";

/// What a run wrote about itself while it was alive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mark {
    pub pid: u32,
    /// The version that was running, so an answer can say whether the build
    /// has changed since
    #[serde(default)]
    pub version: String,
    /// When it started, as seconds since the epoch
    #[serde(default)]
    pub started: u64,
}

/// What happened to a run that did not finish
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ended {
    /// The process that went, so an answer is about the right one
    pub pid: u32,
    /// When Windows recorded it, in its own words. Empty when nothing was
    /// found -- the run may have been ended by a power cut or a task manager,
    /// and neither of those writes anything
    #[serde(default)]
    pub when: String,
    /// The exception, as Windows spells it (`0xc0000409`). Empty when unknown
    #[serde(default)]
    pub code: String,
    /// Everything found, as it was found, for handing to somebody who can
    /// read it. Never shown raw on screen
    #[serde(default)]
    pub evidence: String,
}

impl Ended {
    /// Whether there is anything here worth telling the person about
    pub fn worth_saying(&self) -> bool {
        self.pid != 0
    }
}

fn mark_path() -> std::path::PathBuf {
    crate::config::state_path(MARK)
}

/// What the last run's ending turned out to be, and what somebody made of it.
///
/// Held here rather than threaded through the screen's state, because it is a
/// fact about the program and not about anything on screen: found once, on a
/// thread, and read by whatever draws next
static TOLD: std::sync::Mutex<Option<Told>> = std::sync::Mutex::new(None);

/// A run that ended badly, everything known about it in one place
#[derive(Debug, Clone)]
pub struct Told {
    /// What the run said about itself while it was alive
    pub mark: Mark,
    /// What the machine recorded about its ending
    pub ended: Ended,
    /// What somebody made of it, once asked
    pub why: Option<String>,
}

/// Write down what happened, once it is known
pub fn remember(mark: Mark, ended: Ended) {
    if !ended.worth_saying() {
        return;
    }
    crate::append_hook_log(&format!(
        "the last run ended at {} with {}",
        if ended.when.is_empty() { "an unrecorded time" } else { &ended.when },
        if ended.code.is_empty() { "no code the machine kept" } else { &ended.code },
    ));
    if let Ok(mut g) = TOLD.lock() {
        *g = Some(Told { mark, ended, why: None });
    }
}

/// Everything known about the last run's ending, explanation and all
pub fn told() -> Option<Told> {
    TOLD.lock().ok().and_then(|g| g.clone())
}

/// Put an explanation beside it
pub fn explained(said: String) {
    if let Ok(mut g) = TOLD.lock()
        && let Some(t) = g.as_mut() {
            t.why = Some(said);
        }
}

/// The person has read it. Nothing is kept: the next start says nothing
/// unless there is something new to say
pub fn dismiss() {
    if let Ok(mut g) = TOLD.lock() {
        *g = None;
    }
}

/// Say that a run is in progress. Called once, at the start, **after**
/// [`left_behind`] has looked at what the run before left
pub fn mark_running() {
    let mark = Mark {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        started: now_secs(),
    };
    if let Ok(text) = serde_json::to_string(&mark) {
        let _ = std::fs::write(mark_path(), text);
    }
}

/// Say that this run finished properly. Anything that leaves the mark behind
/// -- a crash, a power cut, being stopped from outside -- is what the next
/// start reports
pub fn mark_closed() {
    let _ = std::fs::remove_file(mark_path());
}

/// The mark the previous run left behind, if it left one.
///
/// A mark whose process is still alive belongs to another copy running right
/// now, not to a run that died, so it is left alone
pub fn left_behind() -> Option<Mark> {
    let text = std::fs::read_to_string(mark_path()).ok()?;
    let mark: Mark = serde_json::from_str(&text).ok()?;
    if mark.pid == std::process::id() || still_running(mark.pid) {
        return None;
    }
    Some(mark)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether a process with this id is still about.
///
/// Only ever used to *not* report something: an answer of "yes" means the mark
/// belongs to a copy that is running, and a wrong "yes" costs a report nobody
/// gets. Erring that way on purpose
#[cfg(windows)]
fn still_running(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist");
    let mut out = out;
    crate::detach_console(&mut out);
    out.args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn still_running(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Ask the machine what happened to that run.
///
/// Slow enough to be worth doing off the main path (it starts a program and
/// reads a system log), which is why it is a function of its own rather than
/// part of the start
pub fn why(mark: &Mark) -> Ended {
    match ask_the_system(mark) {
        Some(raw) => pick(mark, &serde_json::from_str(&raw).unwrap_or_default()),
        // Nothing to read, which is still an answer: the run ended without
        // finishing and the machine kept no account of it
        None => Ended { pid: mark.pid, ..Default::default() },
    }
}

/// Which record is about this run, and what else the machine said at that
/// moment.
///
/// Separate from [`why`] because this half is the judgement and the other
/// half is a program call: this one can be put in front of the records from a
/// real afternoon and checked, on any machine
fn pick(mark: &Mark, found: &serde_json::Value) -> Ended {
    let mut ended = Ended { pid: mark.pid, ..Default::default() };
    let crashes = as_list(found.get("crashes"));
    let exhaustion = as_list(found.get("exhaustion"));
    // The record about *this* process, by the number Windows filed it under.
    //
    // A record's *sentence* is written in the language the machine is set to
    // -- this one says the whole thing in Japanese -- so matching on the
    // words in it works on the machine it was written on and nowhere else.
    // The fields underneath carry no language at all, which is why they are
    // what is read: the process id, and the code, exactly as recorded
    let Some(mine) = crashes.iter().find(|c| {
        c.get("pid").and_then(serde_json::Value::as_u64) == Some(u64::from(mark.pid))
    }) else {
        return ended;
    };
    ended.when = text_of(mine, "when");
    ended.code = match text_of(mine, "code") {
        // Recorded without the prefix everybody reads it with
        code if code.is_empty() => String::new(),
        code if code.starts_with("0x") => code,
        code => format!("0x{code}"),
    };
    // Each record goes over stamped with when it was written. Two records
    // from the same second are the same event seen twice, and without the
    // stamps nobody reading them can tell -- the first real answer this got
    // said so itself: that it could not connect the two
    let stamped = |v: &serde_json::Value| {
        format!("[{}] {}", text_of(v, "when"), text_of(v, "message"))
    };
    let mut evidence = vec![stamped(mine)];
    // What else the machine was doing at that second. A program does not
    // usually die on its own, and the thing that took it down says so here --
    // Windows writes a low-memory diagnosis naming the programs that took it
    for e in &exhaustion {
        if near(&ended.when, &text_of(e, "when")) {
            evidence.push(stamped(e));
        }
    }
    ended.evidence = evidence.join("\n\n");
    ended
}

fn as_list(v: Option<&serde_json::Value>) -> Vec<serde_json::Value> {
    match v {
        // One record comes back as itself rather than as a list of one
        Some(serde_json::Value::Array(a)) => a.clone(),
        Some(serde_json::Value::Object(o)) => vec![serde_json::Value::Object(o.clone())],
        _ => Vec::new(),
    }
}

fn text_of(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_string()
}

/// Whether two `2026-09-22T14:19:27`-shaped stamps are the same moment.
///
/// The same second, because these two records are written by the same event:
/// Windows diagnoses the shortage and the program dies of it together
fn near(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && a[..a.len().min(19)] == b[..b.len().min(19)]
}

/// The machine's own account of the last day, as JSON.
///
/// Windows keeps this in a log that needs a program to read it, so one is
/// started -- with no console, or a black window would flash at every start
/// that followed a crash
#[cfg(windows)]
fn ask_the_system(_mark: &Mark) -> Option<String> {
    // Two logs: what stopped, and what the machine said about itself at the
    // time. Asked for at once, because starting this program twice costs
    // twice as much as the reading does
    //
    // The fields of a crash record rather than its sentence: a record reads
    // in whatever language the machine is set to, and `Properties` carries
    // the same facts in none. The sentence comes too, but only as something
    // for a person or an AI to read -- nothing is decided by it
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$since = (Get-Date).AddHours(-24)
$crashes = @(Get-WinEvent -FilterHashtable @{LogName='Application';Id=1000;StartTime=$since} |
  Where-Object { "$($_.Properties[0].Value)" -like 'SHIKISHA*' } | Select-Object -First 5 |
  ForEach-Object {
    @{ when = $_.TimeCreated.ToString('s')
       pid = [int]"$($_.Properties[8].Value)"
       code = "$($_.Properties[6].Value)"
       path = "$($_.Properties[10].Value)"
       message = $_.Message } })
$exhaustion = @(Get-WinEvent -FilterHashtable @{LogName='System';Id=2004;StartTime=$since} |
  Select-Object -First 5 |
  ForEach-Object { @{ when = $_.TimeCreated.ToString('s'); message = $_.Message } })
@{ crashes = $crashes; exhaustion = $exhaustion } | ConvertTo-Json -Depth 4 -Compress
"#;
    // Handed over already encoded, so nothing in it has to survive being
    // quoted twice on the way to a shell
    let wide: Vec<u16> = SCRIPT.encode_utf16().collect();
    let mut bytes = Vec::with_capacity(wide.len() * 2);
    for unit in wide {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mut cmd = std::process::Command::new("powershell.exe");
    crate::detach_console(&mut cmd);
    let out = cmd
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .output()
        .ok()?;
    // Asked for in UTF-8 above, so it is read as UTF-8 -- the console's own
    // encoding is whatever the machine was set up with, and a record read
    // through it comes back as nonsense in every language but English
    let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!said.is_empty()).then_some(said)
}

#[cfg(not(windows))]
fn ask_the_system(_mark: &Mark) -> Option<String> {
    // Every other machine keeps this somewhere else, in a shape of its own.
    // Saying nothing is better than guessing at one
    None
}

/// What to put in front of whoever is going to explain this: the job, and
/// the records to do it on.
///
/// Two pieces because they are two different things -- what is being asked
/// for, and what there is to look at -- and because the asking half belongs
/// in the system prompt, where it is not mistaken for something to act on.
///
/// Facts only in the second half, and named as facts: what the machine
/// recorded, what was running, and nothing this side has decided
pub fn question(ended: &Ended, mark: &Mark, language: &str) -> (String, String) {
    let job = format!("{ASKING}\n\nAnswer in {language}.");
    let mut out = vec![
        format!("- version: {}", mark.version),
        format!("- process id: {}", mark.pid),
    ];
    if !ended.when.is_empty() {
        out.push(format!("- stopped at: {}", ended.when));
    }
    if !ended.code.is_empty() {
        out.push(format!("- exception code: {}", ended.code));
    }
    out.push(String::new());
    out.push("What the machine recorded:".into());
    out.push(if ended.evidence.is_empty() {
        "(nothing was recorded -- it may have been ended by a power cut, or stopped from outside)".into()
    } else {
        ended.evidence.clone()
    });
    // What the program wrote about itself, when it managed to. A panic leaves
    // a line here; a failed allocation leaves none, and that absence is itself
    // worth knowing
    if let Some(tail) = crash_log_tail() {
        out.push(String::new());
        out.push(
            "The last lines the application itself wrote as it fell over (empty is meaningful: \
             a failed memory allocation writes nothing here):"
                .into(),
        );
        out.push(tail);
    }
    (job, out.join("\n"))
}

/// What whoever explains this is being asked to do.
///
/// In English, and in the code rather than among the words on screen. It is
/// not something anybody reads: it is an instruction to another program, and
/// one written once cannot come apart from its translation. Which language
/// the *answer* comes back in is a line added to the end of it, so a new
/// language costs nothing here -- see `i18n::language_name`
const ASKING: &str = "You are reading records, not doing work. Change nothing, run nothing, \
open nothing, write no files: everything you need is in this message. \
A terminal application stopped without closing properly. Explain in plain words what most likely \
happened and what the person should do about it. If the records show another program caused it, \
say which one. Do not guess beyond what the records support; say plainly what cannot be known \
from them. Write plain sentences in short paragraphs, with no markdown, no headings and no \
asterisks -- this is shown in a narrow strip, not on a page.";

/// The last few lines the program itself wrote when it fell over
fn crash_log_tail() -> Option<String> {
    let text = std::fs::read_to_string(crate::config::logs_dir().join("crash.log")).ok()?;
    let tail: Vec<&str> = text.lines().rev().take(6).collect();
    (!tail.is_empty()).then(|| tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crashed(when: &str, pid: u32, code: &str, message: &str) -> serde_json::Value {
        serde_json::json!({ "when": when, "pid": pid, "code": code, "message": message })
    }

    fn said(when: &str, message: &str) -> serde_json::Value {
        serde_json::json!({ "when": when, "message": message })
    }

    /// The record picked is the one about the process that actually went.
    ///
    /// A machine that ran two copies has two records, and telling somebody
    /// about the wrong one is worse than telling them nothing: the code would
    /// be real, the explanation would be about somebody else's crash
    #[test]
    fn the_record_is_matched_to_the_process_that_went() {
        let mark = Mark { pid: 19504, version: "0.18.0".into(), started: 0 };
        // Written the way this machine really answered: the sentence in the
        // language the machine is set to, and the fields underneath in none
        let found = serde_json::json!({
            "crashes": [
                crashed("2026-09-22T12:22:41", 26712, "c0000005", "障害が発生しているアプリケーション名: SHIKISHA-TERM.exe"),
                crashed("2026-09-22T14:19:27", 19504, "c0000409", "障害が発生しているアプリケーション名: SHIKISHA-TERM.exe"),
            ],
            "exhaustion": [
                said("2026-09-22T14:19:27", "Windows が仮想メモリ不足状態を診断しました。python.exe (11164)"),
                said("2026-09-21T03:00:00", "An older one, about a different day."),
            ],
        });
        let ended = pick(&mark, &found);
        assert_eq!(ended.code, "0xc0000409", "the code belongs to our own process");
        assert_eq!(ended.when, "2026-09-22T14:19:27");
        assert!(
            ended.evidence.contains("仮想メモリ不足"),
            "what the machine said at that second comes along: {}",
            ended.evidence
        );
        assert_eq!(
            ended.evidence.matches("[2026-09-22T14:19:27]").count(),
            2,
            "both records carry the moment they were written, so they can be connected: {}",
            ended.evidence
        );
        assert!(
            !ended.evidence.contains("older one"),
            "and what it said on another day does not: {}",
            ended.evidence
        );
    }

    /// Nothing recorded is an answer too, and not the same answer as a crash
    #[test]
    fn a_run_with_no_record_says_so_rather_than_inventing_one() {
        let mark = Mark { pid: 999, version: "0.18.0".into(), started: 0 };
        let ended = pick(&mark, &serde_json::json!({ "crashes": [], "exhaustion": [] }));
        assert!(ended.code.is_empty());
        assert!(ended.when.is_empty());
        // Still worth saying: the run did end without finishing, which is the
        // thing the person noticed
        assert!(ended.worth_saying());
    }

    /// Against this machine's own log, for a process that really did stop.
    ///
    /// Not run by default: it needs a machine that has one to find. What it
    /// proves is the half the other tests cannot -- that the program started
    /// to read the log runs, answers in the shape expected, and that a real
    /// record is recognised as being about the process asked about.
    ///
    ///   SHIKISHA_PROBE_PID=19504 cargo test -p shikisha-core last_run_for_real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn the_last_run_for_real() {
        let pid: u32 = std::env::var("SHIKISHA_PROBE_PID")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .expect("SHIKISHA_PROBE_PID is the process this should find");
        let mark = Mark { pid, version: env!("CARGO_PKG_VERSION").into(), started: 0 };
        let began = std::time::Instant::now();
        let ended = why(&mark);
        println!("asked the machine in {}ms", began.elapsed().as_millis());
        println!("when: {:?}\ncode: {:?}", ended.when, ended.code);
        println!("--- what it found ---\n{}", ended.evidence);
        assert!(ended.worth_saying());
        assert!(!ended.code.is_empty(), "a record was found but no code was read out of it");
    }

    /// The code is recorded without the prefix everybody reads it with, and
    /// is put back the way a person would look it up
    #[test]
    fn the_code_is_spelled_the_way_it_is_looked_up() {
        let mark = Mark { pid: 7, version: String::new(), started: 0 };
        let one = |code: &str| {
            pick(
                &mark,
                &serde_json::json!({ "crashes": [{ "when": "x", "pid": 7, "code": code }] }),
            )
            .code
        };
        assert_eq!(one("c0000409"), "0xc0000409");
        assert_eq!(one("0xc0000409"), "0xc0000409", "one already spelled out is left alone");
        assert_eq!(one(""), "", "and nothing recorded stays nothing");
    }

    /// The same second is the same event; a minute apart is a coincidence
    #[test]
    fn only_what_happened_at_the_same_moment_is_brought_along() {
        assert!(near("2026-09-22T14:19:27", "2026-09-22T14:19:27"));
        assert!(!near("2026-09-22T14:19:27", "2026-09-22T14:20:27"));
        assert!(!near("", "2026-09-22T14:19:27"));
    }
}
