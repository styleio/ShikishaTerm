//! How many tokens the AIs on this PC have used, and what that would cost by
//! the token.
//!
//! Read from the conversation records Claude Code and Codex keep on this PC
//! (and the folders of the accounts added for them, `crate::accounts`), one
//! row per call to a model in `tokens.db`. Looked at when the settings card
//! asks, on a thread of its own: only what was added to a record since it
//! was last read is read again, so a look after the first is quick. The
//! rows outlive the records -- Claude Code forgets its own after 30 days --
//! until [`DAYS_KEPT`].

pub mod price;
pub mod read;

use std::io::{BufRead as _, Read as _, Seek as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension as _, params};
use sha2::{Digest as _, Sha256};

pub use crate::sqlite::Step;

/// What the record is called where a refusal names it
const WHAT: &str = "the record of tokens used";

/// Every change to the tables, in order. A step is never edited once it has
/// shipped
pub const STEPS: &[(i64, &str, Step)] = &[(1, "first", Step::Sql(include_str!("migrations/0001_first.sql")))];

/// How long a call is kept: a year, and a month over it, so the year view
/// always has a whole year to show
pub const DAYS_KEPT: u64 = 400;

/// How much of a record's start, and of the stretch just before where it was
/// read to, is compared to tell a record that only grew from one written
/// anew. Longer than the first line of every CLI's record, which carries the
/// conversation's own id: two records alike in this much are the same one
const SAMPLE: u64 = 512;

/// A look at the records is not repeated within this many seconds unless it
/// is asked for: the card asks every time it is drawn
const FRESH_SECS: u64 = 60;

/// Where the records of each CLI are: the folder whose files (in folders
/// under it too) are its records, by the name a profile gives its home
const PLACES: &[(&str, &str, &str)] = &[
    ("claude", "{home}/.claude/projects", ".jsonl"),
    ("codex", "{home}/.codex/sessions", ".jsonl"),
];

/// Where the record is kept
pub fn path() -> PathBuf {
    crate::config::state_path("tokens.db")
}

/// How the last look at the records went, and the one going on
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Looking {
    pub running: bool,
    /// Records read so far, and how many there are, while it runs
    pub done: usize,
    pub total: usize,
    /// When the last look ended, in seconds since 1970
    pub ended: u64,
    /// Why the last look failed, when it did
    pub error: Option<String>,
}

static LOOKING: Mutex<Looking> = Mutex::new(Looking { running: false, done: 0, total: 0, ended: 0, error: None });

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// How the look at the records stands
pub fn looking() -> Looking {
    LOOKING.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Look at the records again, on a thread of its own, unless a look is going
/// on or one ended a moment ago. `now_please`: asked for in so many words
/// (the card's "read again"), which a recent look does not hold back
pub fn look(now_please: bool) {
    {
        let mut l = LOOKING.lock().unwrap_or_else(|e| e.into_inner());
        if l.running || (!now_please && l.ended + FRESH_SECS > now()) {
            return;
        }
        *l = Looking { running: true, ended: l.ended, ..Default::default() };
    }
    std::thread::spawn(|| {
        let result = look_now(&path());
        let mut l = LOOKING.lock().unwrap_or_else(|e| e.into_inner());
        l.running = false;
        l.ended = now();
        l.error = result.err().map(|e| format!("{e:#}"));
    });
}

/// Every record of every CLI on this PC, each once: an account whose folder
/// links to the usual one would show the same records twice
fn records() -> Vec<(&'static str, PathBuf)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (cli, place, ending) in PLACES {
        for dir in crate::accounts::patterns(place) {
            for file in crate::sessionfind::walk(&crate::with_home(&dir), 0, std::time::UNIX_EPOCH) {
                if !file.to_string_lossy().ends_with(ending) {
                    continue;
                }
                let real = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
                if seen.insert(real) {
                    out.push((*cli, file));
                }
            }
        }
    }
    out
}

fn look_now(db: &Path) -> Result<()> {
    let mut conn = crate::sqlite::open(db, STEPS, WHAT)?;
    let files = records();
    LOOKING.lock().unwrap_or_else(|e| e.into_inner()).total = files.len();
    for (i, (cli, file)) in files.iter().enumerate() {
        if let Err(e) = read_one(&mut conn, cli, file) {
            crate::append_hook_log(&format!("tokens: {} could not be read: {e:#}", file.display()));
        }
        LOOKING.lock().unwrap_or_else(|e| e.into_inner()).done = i + 1;
    }
    let oldest = now().saturating_sub(DAYS_KEPT * 86_400) as i64 * 1000;
    conn.execute("DELETE FROM calls WHERE at < ?1", params![oldest])?;
    conn.execute(
        "DELETE FROM read_files WHERE read_at < ?1",
        params![oldest],
    )?;
    Ok(())
}

fn sample(file: &mut std::fs::File, from: u64, len: u64) -> Result<String> {
    file.seek(std::io::SeekFrom::Start(from))?;
    let mut buf = vec![0u8; len as usize];
    file.read_exact(&mut buf)?;
    Ok(Sha256::digest(&buf).iter().take(8).map(|b| format!("{b:02x}")).collect())
}

/// Read what was added to one record since it was last read -- or all of it,
/// when it is new or was written anew -- and keep its calls
fn read_one(conn: &mut Connection, cli: &'static str, path: &Path) -> Result<()> {
    let key = path.display().to_string();
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let was: Option<(u64, u64, String, String, String)> = conn
        .query_row(
            "SELECT size, read_to, head, edge, carry FROM read_files WHERE path = ?1",
            params![key],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let head = sample(&mut file, 0, size.min(SAMPLE))?;
    let mut from = 0u64;
    let mut carry = read::CodexCarry::default();
    if let Some((was_size, read_to, was_head, was_edge, was_carry)) = was {
        let same = was_head == head && read_to <= size && sample(&mut file, read_to - read_to.min(SAMPLE), read_to.min(SAMPLE))? == was_edge;
        if same && was_size == size && read_to == size {
            return Ok(());
        }
        if same {
            from = read_to;
            carry = serde_json::from_str(&was_carry).unwrap_or_default();
        }
    }
    file.seek(std::io::SeekFrom::Start(from))?;
    let mut lines = std::io::BufReader::new(file);
    let mut read_to = from;
    let mut line = Vec::new();
    let tx = conn.transaction()?;
    {
        let mut put = tx.prepare_cached(
            "INSERT INTO calls (key, cli, at, day, model, folder, session, fresh_in, cache_read, cache_write, cache_write_hour, out, reasoning, is_fast, is_us)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(key) DO UPDATE SET
               fresh_in = max(fresh_in, excluded.fresh_in), cache_read = max(cache_read, excluded.cache_read),
               cache_write = max(cache_write, excluded.cache_write), cache_write_hour = max(cache_write_hour, excluded.cache_write_hour),
               out = max(out, excluded.out), reasoning = max(reasoning, excluded.reasoning),
               model = CASE WHEN excluded.model <> '' THEN excluded.model ELSE model END",
        )?;
        loop {
            line.clear();
            let got = lines.read_until(b'\n', &mut line)?;
            // A last line not ended yet is still being written: it is read
            // next time, whole
            if got == 0 || line.last() != Some(&b'\n') {
                break;
            }
            read_to += got as u64;
            let call = match cli {
                "claude" => read::claude_line(&line),
                _ => read::codex_line(&line, &mut carry),
            };
            if let Some(c) = call {
                put.execute(params![
                    c.key,
                    c.cli,
                    c.at as i64 * 1000,
                    local_day(c.at),
                    c.model,
                    c.folder,
                    c.session,
                    c.fresh_in as i64,
                    c.cache_read as i64,
                    c.cache_write as i64,
                    c.cache_write_hour as i64,
                    c.out as i64,
                    c.reasoning as i64,
                    c.is_fast as i64,
                    c.is_us as i64,
                ])?;
            }
        }
    }
    let mut file = std::fs::File::open(path)?;
    let edge = sample(&mut file, read_to - read_to.min(SAMPLE), read_to.min(SAMPLE))?;
    tx.execute(
        "INSERT INTO read_files (path, cli, size, read_to, head, edge, carry, read_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(path) DO UPDATE SET size = excluded.size, read_to = excluded.read_to, head = excluded.head,
           edge = excluded.edge, carry = excluded.carry, read_at = excluded.read_at",
        params![key, cli, size as i64, read_to as i64, head, edge, serde_json::to_string(&carry)?, crate::sqlite::now_ms()],
    )?;
    tx.commit()?;
    Ok(())
}

/// The date a moment falls on by this PC's clock, as `YYYY-MM-DD`
pub fn local_day(epoch: u64) -> String {
    let (y, m, d, _, _) = crate::hooks::local_at(epoch);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Tokens of each kind, summed
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize)]
pub struct Sum {
    pub calls: u64,
    pub fresh_in: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub out: u64,
    pub reasoning: u64,
    /// What the priced calls would cost by the token, in US dollars
    pub cost: f64,
    /// Calls of a model with no price, which the cost leaves out
    pub unpriced: u64,
}

impl Sum {
    fn add(&mut self, o: &Sum) {
        self.calls += o.calls;
        self.fresh_in += o.fresh_in;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.out += o.out;
        self.reasoning += o.reasoning;
        self.cost += o.cost;
        self.unpriced += o.unpriced;
    }
}

/// One day's, one model's, one folder's or one conversation's use
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Line {
    /// The day, the model, the folder or the conversation
    pub name: String,
    pub cli: String,
    /// For a conversation: its folder and model, and when it began and last
    /// spoke (seconds since 1970)
    #[serde(skip_serializing_if = "String::is_empty")]
    pub folder: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<u64>,
    #[serde(flatten)]
    pub sum: Sum,
}

/// What a span of days used
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Report {
    /// The first day counted (`YYYY-MM-DD`) and the last
    pub from: String,
    pub to: String,
    pub total: Sum,
    /// Every day of the span, the empty ones too, oldest first
    pub days: Vec<Line>,
    /// Most used first
    pub models: Vec<Line>,
    pub folders: Vec<Line>,
    /// The conversations that spoke last, newest first
    pub sessions: Vec<Line>,
}

/// How many conversations the report lists
const SESSIONS_SHOWN: usize = 20;

/// What the last `days` days used (today among them), of `cli` alone when one
/// is named. Read over a connection of its own, beside a look going on
pub fn report(db: &Path, days: u32, cli: Option<&str>) -> Result<Report> {
    let today = now();
    let span: Vec<String> = (0..days.max(1) as u64).rev().map(|back| local_day(today.saturating_sub(back * 86_400))).collect();
    let mut out = Report { from: span[0].clone(), to: span[span.len() - 1].clone(), ..Default::default() };
    if !db.exists() {
        out.days = span.iter().map(|d| Line { name: d.clone(), ..Default::default() }).collect();
        return Ok(out);
    }
    let conn = crate::sqlite::open(db, STEPS, WHAT)?;
    let band = format!(
        "CASE WHEN fresh_in + cache_read + cache_write + cache_write_hour > {} THEN 2 WHEN fresh_in + cache_read + cache_write + cache_write_hour > {} THEN 1 ELSE 0 END",
        price::BANDS[1],
        price::BANDS[0]
    );
    let sums = "COUNT(*), SUM(fresh_in), SUM(cache_read), SUM(cache_write), SUM(cache_write_hour), SUM(out), SUM(reasoning)";
    // Grouped as finely as a price can differ, then priced and added up
    let sql = format!(
        "SELECT day, cli, model, folder, is_fast, is_us, {band}, {sums} FROM calls
         WHERE day >= ?1 AND (?2 = '' OR cli = ?2)
         GROUP BY day, cli, model, folder, is_fast, is_us, {band}"
    );
    let mut by_day: std::collections::BTreeMap<String, Sum> = span.iter().map(|d| (d.clone(), Sum::default())).collect();
    let mut by_model: std::collections::HashMap<(String, String), Sum> = Default::default();
    let mut by_folder: std::collections::HashMap<String, Sum> = Default::default();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![out.from, cli.unwrap_or("")])?;
    while let Some(r) = rows.next()? {
        let (day, who, model, folder): (String, String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
        let sum = priced(&model, r.get::<_, i64>(4)? != 0, r.get::<_, i64>(5)? != 0, r.get::<_, i64>(6)? as usize, |i| r.get::<_, i64>(7 + i).map(|v| v as u64))?;
        out.total.add(&sum);
        by_day.entry(day).or_default().add(&sum);
        by_model.entry((who, model)).or_default().add(&sum);
        by_folder.entry(folder).or_default().add(&sum);
    }
    out.days = by_day.into_iter().map(|(name, sum)| Line { name, sum, ..Default::default() }).collect();
    let most = |a: &Sum, b: &Sum| (b.cost, b.fresh_in + b.cache_read + b.out).partial_cmp(&(a.cost, a.fresh_in + a.cache_read + a.out)).unwrap_or(std::cmp::Ordering::Equal);
    out.models = by_model.into_iter().map(|((cli, name), sum)| Line { name, cli, sum, ..Default::default() }).collect();
    out.models.sort_by(|a, b| most(&a.sum, &b.sum));
    out.folders = by_folder.into_iter().map(|(name, sum)| Line { name, sum, ..Default::default() }).collect();
    out.folders.sort_by(|a, b| most(&a.sum, &b.sum));
    // The conversations: the newest ones, then what each used, priced the
    // same way
    let mut newest = conn.prepare(
        "SELECT session, MIN(at), MAX(at) FROM calls WHERE day >= ?1 AND (?2 = '' OR cli = ?2) AND session <> ''
         GROUP BY session ORDER BY MAX(at) DESC LIMIT ?3",
    )?;
    let picked: Vec<(String, i64, i64)> = newest
        .query_map(params![out.from, cli.unwrap_or(""), SESSIONS_SHOWN as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut of_session = conn.prepare(&format!(
        "SELECT cli, model, folder, is_fast, is_us, {band}, {sums}, MAX(at) FROM calls WHERE session = ?1 AND day >= ?2
         GROUP BY cli, model, folder, is_fast, is_us, {band} ORDER BY MAX(at)"
    ))?;
    for (session, first, last) in picked {
        let mut line = Line { name: session.clone(), first: Some(first as u64 / 1000), last: Some(last as u64 / 1000), ..Default::default() };
        let mut rows = of_session.query(params![session, out.from])?;
        while let Some(r) = rows.next()? {
            let model: String = r.get(1)?;
            let sum = priced(&model, r.get::<_, i64>(3)? != 0, r.get::<_, i64>(4)? != 0, r.get::<_, i64>(5)? as usize, |i| r.get::<_, i64>(6 + i).map(|v| v as u64))?;
            line.sum.add(&sum);
            // The latest model and folder it spoke with
            (line.cli, line.model, line.folder) = (r.get(0)?, model, r.get(2)?);
        }
        out.sessions.push(line);
    }
    Ok(out)
}

/// One group of calls of one model, priced. `col(i)` reads the i-th of the
/// sums in the order the queries give them
fn priced(model: &str, fast: bool, us: bool, band: usize, col: impl Fn(usize) -> rusqlite::Result<u64>) -> rusqlite::Result<Sum> {
    let (calls, fresh_in, cache_read, cache_write, cache_write_hour, out, reasoning) = (col(0)?, col(1)?, col(2)?, col(3)?, col(4)?, col(5)?, col(6)?);
    let mut sum = Sum { calls, fresh_in, cache_read, cache_write: cache_write + cache_write_hour, out, reasoning, cost: 0.0, unpriced: 0 };
    match price::of(model) {
        Some(p) => sum.cost = price::cost(&p, band, fast, us, fresh_in, cache_read, cache_write, cache_write_hour, out),
        None => sum.unpriced = calls,
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sk-tokens-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn claude_call(id: &str, at: &str, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","cwd":"/work/app","sessionId":"s1","requestId":"r-{id}","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":1000,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":{out}}}}}}}"#
        ) + "\n"
    }

    /// This moment, written the way a record writes one (in UTC): today, by
    /// this PC's clock, wherever the test runs
    fn today_at() -> String {
        let at = now();
        let secs = at % 86_400;
        let (y, m, d) = civil(at / 86_400);
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
    }

    fn civil(days: u64) -> (i64, u32, u32) {
        let z = days as i64 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (yoe + era * 400 + i64::from(m <= 2), m, d)
    }

    #[test]
    fn a_record_is_read_once_and_then_only_what_was_added() {
        let dir = scratch("grow");
        let db = dir.join("tokens.db");
        let rec = dir.join("a.jsonl");
        let at = today_at();
        // The same message twice, as it streamed: one call, at its largest
        std::fs::write(&rec, claude_call("m1", &at, 10) + &claude_call("m1", &at, 50)).unwrap();
        let mut conn = crate::sqlite::open(&db, STEPS, WHAT).unwrap();
        read_one(&mut conn, "claude", &rec).unwrap();
        let r = report(&db, 7, None).unwrap();
        assert_eq!((r.total.calls, r.total.out, r.total.fresh_in), (1, 50, 1000));
        // Grown by a call and half of another line still being written
        let mut more = std::fs::read_to_string(&rec).unwrap() + &claude_call("m2", &at, 7);
        more.push_str(r#"{"type":"assistant","#);
        std::fs::write(&rec, &more).unwrap();
        read_one(&mut conn, "claude", &rec).unwrap();
        let r = report(&db, 7, None).unwrap();
        assert_eq!((r.total.calls, r.total.out), (2, 57));
        let (read_to,): (i64,) = conn.query_row("SELECT read_to FROM read_files", [], |r| Ok((r.get(0)?,))).unwrap();
        assert_eq!(read_to as usize, more.rfind('\n').unwrap() + 1, "the half line is left for next time");
        // Written anew, shorter: read from the start, the calls already
        // counted kept once
        std::fs::write(&rec, claude_call("m3", &at, 1)).unwrap();
        read_one(&mut conn, "claude", &rec).unwrap();
        let r = report(&db, 7, None).unwrap();
        assert_eq!(r.total.calls, 3);
        // A copy in another record (a resumed conversation) is the same calls
        let copy = dir.join("b.jsonl");
        std::fs::write(&copy, claude_call("m1", &at, 50)).unwrap();
        read_one(&mut conn, "claude", &copy).unwrap();
        assert_eq!(report(&db, 7, None).unwrap().total.calls, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_report_prices_adds_up_and_lists_every_day() {
        let dir = scratch("report");
        let db = dir.join("tokens.db");
        let rec = dir.join("a.jsonl");
        let at = today_at();
        let mut text = claude_call("m1", &at, 1_000_000);
        text.push_str(&claude_call("m2", &at, 0).replace("claude-opus-5-5", "claude-unknown-9"));
        std::fs::write(&rec, text).unwrap();
        let mut conn = crate::sqlite::open(&db, STEPS, WHAT).unwrap();
        read_one(&mut conn, "claude", &rec).unwrap();
        let r = report(&db, 7, None).unwrap();
        assert_eq!(r.days.len(), 7, "every day of the span, the empty ones too");
        assert_eq!(r.days.last().unwrap().name, local_day(now()));
        // 1000 input at $4/M and 1M output at $20/M
        assert!((r.total.cost - 20.004).abs() < 1e-9, "{}", r.total.cost);
        assert_eq!(r.total.unpriced, 1, "the unknown model is counted and not priced");
        assert_eq!(r.models[0].name, "claude-opus-5-5");
        assert_eq!(r.folders[0].name, "/work/app");
        assert_eq!(r.sessions.len(), 1);
        assert_eq!(r.sessions[0].sum.calls, 2);
        assert_eq!(report(&db, 7, Some("codex")).unwrap().total.calls, 0, "one AI alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_steps_are_numbered_in_order_and_so_are_their_files() {
        crate::sqlite::check_numbering(STEPS, "crates/core/src/tokens/migrations");
    }

    /// The tables as the steps make them. Change a step and this fails until
    /// it is written again:
    ///
    ///     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core tokens_db
    #[test]
    fn tokens_db() {
        let header = format!(
            "-- The record of tokens used (data/tokens.db), version {}.\n\
             -- Written by a test from the steps in crates/core/src/tokens/migrations;\n\
             -- what each table is for is in tokens-db.md. Write it again with\n\
             --     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core tokens_db",
            STEPS.len()
        );
        crate::sqlite::check_design(STEPS, WHAT, &header, "docs/design/tokens-db.sql", "tokens_db");
    }
}

