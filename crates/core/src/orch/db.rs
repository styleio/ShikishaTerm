//! The record of work handed between AI tabs, kept on disk.
//!
//! What is written down, each for a way a chain of work stops without anyone
//! noticing:
//!
//! * **jobs** -- a piece of work a person asked one AI tab to see through. That
//!   tab is the job's *lead*.
//! * **tasks** -- the pieces the lead cut it into, and what each waits on.
//! * **assignments** -- one try at one task by one tab. A task tried twice has
//!   two, and a report that arrives for the first after the second began is
//!   recognised for what it is.
//! * **mail** -- everything said between the lead and its workers, in order.
//!   Handed to a reader in **handovers**, handed again until the reader says it
//!   has dealt with them, so a reader cut off half way loses nothing.
//! * **questions** and **decisions** -- a worker waiting on its lead, and a
//!   task held until somebody (the lead, or the person) decides something.
//!
//! **Changing the tables.** The tables are made and changed by the files in
//! `migrations/`, one file a change, each applied once and in order ([`STEPS`]).
//! A change SQLite cannot make in place -- a column's type, a constraint -- is
//! a rebuild ([`rebuild`]), written once here the way SQLite documents it.
//! Before a record is brought up to a newer version it is copied beside itself,
//! so a change that goes wrong costs nothing. What all the steps add up to is
//! written out in `docs/design/orchestration-db.sql`, and a test keeps that
//! file and these steps saying the same thing.
//!
//! **One way to change a state.** Every change of state goes through
//! [`Store::shift`]: it writes only if the row is still in a state the change
//! expects, and says so when it is not. Two changes racing -- a report arriving
//! as the tab is closed -- are then a refusal, not a record that says two
//! things at once. The states themselves are plain text with no CHECK in the
//! tables, so a new one is a line of code and not a rebuilt table.
//!
//! **A tab is its uid.** A job's lead, the tab doing an assignment, a tab a
//! job opened and a tab's own mailbox (`tab:<uid>`) are kept by the tab's uid
//! (`config::TabConfig::uid`), never its name: a name goes back in the bag
//! when its tab closes, and the next tab to draw it was handed the first
//! one's jobs and mail. What each uid is called is kept in **tab_names**, so
//! what is said about a tab says its name ([`Store::name_of`]); who sent mail
//! and who made a decision keep the name they had then.
//!
//! **One writer.** Everything here runs on the app's main loop, one call at a
//! time. The transactions are for a program killed half way through a change,
//! not for another writer.

use std::path::Path;

use anyhow::{Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use serde_json::{Value, json};

pub use crate::sqlite::{Step, now_ms, rebuild, schema, version_of};

/// What the record is called where a refusal names it
const WHAT: &str = "the record of handed work";

/// The current table layout followed by its numbered upgrades. Existing
/// records at versions 2 and 3 keep their versions and rows; a fresh record
/// reaches the same layout without constructing intermediate tables.
pub const STEPS: &[(i64, &str, Step)] = &[
    (1, "tables", Step::Sql(include_str!("migrations/0001_tables.sql"))),
    (2, "table baseline", Step::Code(check_baseline)),
    (3, "tab uids", Step::Sql(include_str!("migrations/0003_tab_uids.sql"))),
];

// Also permits a fresh record interrupted after step 1 to resume. A file
// with an unsupported layout is refused without changing its tables.
fn check_baseline(tx: &Transaction) -> Result<()> {
    let current: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'jobs')",
        [], |r| r.get(0),
    )?;
    if !current {
        bail!("the record does not have a supported table layout");
    }
    Ok(())
}

/// What `meta` says once the rows written under names were rewritten under
/// uids ([`Store::adopt_uids`])
const ADOPTED: &str = "tab_uids";

/// The mailbox of a tab, by its uid
pub fn tab_box(uid: &str) -> String {
    format!("tab:{uid}")
}

/// The version the steps bring a record to
pub fn latest() -> i64 {
    STEPS.last().map(|s| s.0).unwrap_or(0)
}

/// How many mail a handover carries at most. Enough for what piles up while a
/// lead waits on a few workers; few enough that the answer stays something an
/// AI reads whole rather than skims
pub const HANDOVER_MOST: usize = 25;

/// A task whose tab is lost this many times is not tried again: once can be
/// bad luck, twice says something about the task
pub const LOSSES_ALLOWED: i64 = 2;

/// How long a closed job is kept before it is forgotten
const KEEP_CLOSED_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Why an assignment whose brief never went in ended
pub const NOT_HANDED: &str = "not handed";

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: i64,
    pub goal: String,
    /// The tab that leads it, by uid
    pub lead: String,
    /// What that tab is called ([`Store::name_of`])
    pub lead_name: String,
    /// open / closed
    pub state: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: i64,
    pub job: i64,
    pub title: String,
    pub body: String,
    pub waits_on: Vec<i64>,
    /// waiting / open / working / done / failed / held / dropped (see [`Store::task_drop`])
    pub state: String,
    pub result: Option<Value>,
    pub losses: i64,
    /// Why it is held or failed
    pub why: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub id: i64,
    pub job: i64,
    pub task: i64,
    /// The tab doing it, by uid
    pub tab: String,
    /// What that tab is called ([`Store::name_of`])
    pub tab_name: String,
    /// Which run of that tab's program was given it (`api::incarnation_of`)
    pub process: Option<i64>,
    pub depth: i64,
    /// handing / working / done / failed / stopped
    pub state: String,
    /// Whether the tab was seen starting on it (`None` while handing)
    pub seen_starting: Option<bool>,
    /// Why it ended, when it ended other than by a report
    pub end_reason: Option<String>,
    /// What became of the tab once it was over: released / kept / reused
    pub afterwards: Option<String>,
    pub ended_at: Option<i64>,
    pub created_at: i64,
}

impl Assignment {
    pub fn active(&self) -> bool {
        self.state == "handing" || self.state == "working"
    }
    pub fn over(&self) -> bool {
        !self.active()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mail {
    pub seq: i64,
    pub job: Option<i64>,
    pub box_: String,
    pub sender: String,
    /// report / question / answer / note / alert / decision / reply
    pub kind: String,
    pub subject: String,
    pub body: String,
    pub extra: Value,
    pub sent_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Handover {
    pub id: i64,
    pub box_: String,
    pub mail: Vec<Mail>,
    /// Handed before and not yet dealt with
    pub again: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub id: i64,
    pub job: i64,
    pub assignment: i64,
    pub question: String,
    pub choices: Vec<String>,
    /// asked / answered / closed
    pub state: String,
    pub answer: Option<String>,
    pub asked_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub id: i64,
    pub job: i64,
    pub task: i64,
    pub question: String,
    pub choices: Vec<String>,
    /// Who decides: lead / person
    pub decided_by: String,
    /// open / made
    pub state: String,
    pub choice: Option<String>,
    pub asked_at: i64,
}

/// A tab a job opened, and whether it is still the job's to close
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    /// The tab, by uid
    pub tab: String,
    /// What it is called ([`Store::name_of`])
    pub tab_name: String,
    pub job: i64,
    /// job / person / gone
    pub held_by: String,
    pub opened_at: i64,
}

/// A working folder a job made
#[derive(Debug, Clone, PartialEq)]
pub struct MadeFolder {
    pub job: i64,
    pub folder: String,
    pub branch: String,
    pub made_at: i64,
}

/// A change refused because the row was no longer where the change expected it
#[derive(Debug)]
pub struct Conflict {
    pub table: &'static str,
    pub id: i64,
    pub expected: Vec<&'static str>,
    pub found: Option<String>,
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} is {} (expected {})",
            self.table,
            self.id,
            self.found.as_deref().unwrap_or("gone"),
            self.expected.join(" or ")
        )
    }
}

impl std::error::Error for Conflict {}

fn list(v: &str) -> Vec<String> {
    serde_json::from_str(v).unwrap_or_default()
}

fn ids(v: &str) -> Vec<i64> {
    serde_json::from_str(v).unwrap_or_default()
}

// -- bringing a record up to date ---------------------------------------------

/// Run every step a record does not have yet (see [`crate::sqlite::migrate`])
pub fn migrate(conn: &mut Connection, steps: &[(i64, &str, Step)], backup: Option<&Path>) -> Result<()> {
    crate::sqlite::migrate(conn, steps, backup, WHAT)
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// The record at `path`, made if it is not there and brought up to date if
    /// an older version wrote it (copied beside itself first, as
    /// `<name>.v<version>.bak`)
    pub fn open(path: &Path) -> Result<Self> {
        Self::with(crate::sqlite::open(path, STEPS, WHAT)?)
    }

    /// A record that lives only as long as this value (tests)
    pub fn in_memory() -> Result<Self> {
        Self::with(crate::sqlite::in_memory(STEPS, WHAT)?)
    }

    fn with(conn: Connection) -> Result<Self> {
        let mut s = Self { conn };
        s.forget_old()?;
        Ok(s)
    }

    /// Closed jobs past their keeping, with everything that was theirs
    fn forget_old(&mut self) -> Result<()> {
        let before = now_ms() - KEEP_CLOSED_MS;
        self.conn
            .execute("DELETE FROM jobs WHERE state = 'closed' AND closed_at < ?1", params![before])?;
        self.conn
            .execute("DELETE FROM mail WHERE job_id IS NULL AND is_read = 1 AND sent_at < ?1", params![before])?;
        Ok(())
    }

    /// The one way a state changes: only from a state the change expects
    fn shift(tx: &Transaction, table: &'static str, id: i64, from: &[&'static str], to: &str) -> Result<()> {
        let marks = from.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("UPDATE {table} SET state = ? WHERE id = ? AND state IN ({marks})");
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![&to, &id];
        for f in from {
            values.push(f);
        }
        if tx.execute(&sql, values.as_slice())? == 1 {
            return Ok(());
        }
        let found: Option<String> = tx
            .query_row(&format!("SELECT state FROM {table} WHERE id = ?1"), params![id], |r| r.get(0))
            .optional()?;
        Err(Conflict { table, id, expected: from.to_vec(), found }.into())
    }

    // -- who a tab is --------------------------------------------------------

    /// `uid` is called `name`, seen so at `at`
    pub fn named(&mut self, uid: &str, name: &str, at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tab_names (uid, name, seen_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(uid) DO UPDATE SET name = excluded.name, seen_at = MAX(seen_at, excluded.seen_at)",
            params![uid, name, at],
        )?;
        Ok(())
    }

    /// What the tab `uid` is called, or was when it was last seen; the uid
    /// itself for one never seen called anything
    pub fn name_of(&self, uid: &str) -> Result<String> {
        Ok(self
            .conn
            .query_row("SELECT name FROM tab_names WHERE uid = ?1", params![uid], |r| r.get(0))
            .optional()?
            .unwrap_or_else(|| uid.to_string()))
    }

    /// Rewrite once what was written under names before tabs had uids.
    /// `tabs` is every tab of the settings, by name and uid. A name only one
    /// tab of the settings has is that tab; any other -- a tab closed since,
    /// a name two desks share -- is given the uid of a name nobody answers to
    /// (`convo::db::gone_uid`), and its work reads as nobody's now. `true`
    /// when it did the rewriting
    pub fn adopt_uids(&mut self, tabs: &[(String, String)]) -> Result<bool> {
        let done: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![ADOPTED], |r| r.get(0))
            .optional()?;
        if done.is_some() {
            return Ok(false);
        }
        let mut anywhere: std::collections::HashMap<&str, Vec<&str>> = Default::default();
        for (name, uid) in tabs {
            anywhere.entry(name.as_str()).or_default().push(uid.as_str());
        }
        let whose = |n: &str| -> String {
            match anywhere.get(n).map(Vec::as_slice) {
                Some([only]) => only.to_string(),
                _ => crate::convo::db::gone_uid("", n),
            }
        };
        let is_uid = crate::config::is_tab_uid;
        let tx = self.conn.transaction()?;
        for (name, uid) in tabs {
            tx.execute("INSERT OR IGNORE INTO tab_names (uid, name, seen_at) VALUES (?1, ?2, 1)", params![uid, name])?;
        }
        let rename = |table: &str, col: &str, prefix: &str| -> Result<()> {
            let olds: Vec<String> = tx
                .prepare(&format!("SELECT DISTINCT {col} FROM {table}"))?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for old in olds {
                let Some(name) = old.strip_prefix(prefix) else { continue };
                if is_uid(name) || name.is_empty() {
                    continue;
                }
                let uid = whose(name);
                tx.execute("INSERT OR IGNORE INTO tab_names (uid, name, seen_at) VALUES (?1, ?2, 0)", params![uid, name])?;
                tx.execute(
                    &format!("UPDATE OR IGNORE {table} SET {col} = ?2 WHERE {col} = ?1"),
                    params![old, format!("{prefix}{uid}")],
                )?;
                // What its tab already has under its uid is the same row twice
                tx.execute(&format!("DELETE FROM {table} WHERE {col} = ?1"), params![old])?;
            }
            Ok(())
        };
        rename("jobs", "lead", "")?;
        rename("assignments", "tab", "")?;
        rename("opened_tabs", "tab", "")?;
        rename("mail", "box", "tab:")?;
        rename("handovers", "box", "tab:")?;
        tx.execute("INSERT INTO meta (key, value) VALUES (?1, '1')", params![ADOPTED])?;
        tx.commit()?;
        Ok(true)
    }

    // -- jobs ----------------------------------------------------------------

    pub fn job_open(&mut self, lead: &str, goal: &str) -> Result<Job> {
        let at = now_ms();
        self.conn.execute(
            "INSERT INTO jobs (goal, lead, state, started_at) VALUES (?1, ?2, 'open', ?3)",
            params![goal, lead, at],
        )?;
        let id = self.conn.last_insert_rowid();
        self.job(id)?.ok_or_else(|| anyhow!("j{id} vanished"))
    }

    fn job_row(r: &rusqlite::Row) -> rusqlite::Result<Job> {
        Ok(Job {
            id: r.get(0)?,
            goal: r.get(1)?,
            lead: r.get(2)?,
            state: r.get(3)?,
            started_at: r.get(4)?,
            lead_name: r.get(5)?,
        })
    }

    const JOB_COLS: &'static str =
        "id, goal, lead, state, started_at, COALESCE((SELECT n.name FROM tab_names n WHERE n.uid = jobs.lead), lead)";

    pub fn job(&self, id: i64) -> Result<Option<Job>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {} FROM jobs WHERE id = ?1", Self::JOB_COLS), params![id], Self::job_row)
            .optional()?)
    }

    /// The open jobs a tab leads, newest first
    pub fn jobs_led_by(&self, lead: &str) -> Result<Vec<Job>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM jobs WHERE lead = ?1 AND state = 'open' ORDER BY id DESC",
            Self::JOB_COLS
        ))?;
        Ok(st.query_map(params![lead], Self::job_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn open_jobs(&self) -> Result<Vec<Job>> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {} FROM jobs WHERE state = 'open' ORDER BY id", Self::JOB_COLS))?;
        Ok(st.query_map([], Self::job_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn job_close(&mut self, id: i64, outcome: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        Self::shift(&tx, "jobs", id, &["open"], "closed")?;
        tx.execute(
            "UPDATE jobs SET outcome = ?2, closed_at = ?3 WHERE id = ?1",
            params![id, outcome, now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    }

    // -- tasks ---------------------------------------------------------------

    /// A task, open at once when everything it waits on is done. What it waits
    /// on must be of the same job and already written down, so a task can
    /// never end up waiting on itself through others
    pub fn task_add(&mut self, job: i64, title: &str, body: &str, waits_on: &[i64]) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let state: Option<String> = tx
            .query_row("SELECT state FROM jobs WHERE id = ?1", params![job], |r| r.get(0))
            .optional()?;
        match state.as_deref() {
            Some("open") => {}
            Some(_) => bail!("j{job} is closed"),
            None => bail!("there is no job j{job}"),
        }
        let mut all_done = true;
        for w in waits_on {
            let found: Option<(i64, String)> = tx
                .query_row("SELECT job_id, state FROM tasks WHERE id = ?1", params![w], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            match found {
                None => bail!("there is no task t{w} to wait on"),
                Some((j, _)) if j != job => bail!("t{w} belongs to another job (j{j})"),
                Some((_, s)) => all_done &= s == "done",
            }
        }
        let state = if all_done { "open" } else { "waiting" };
        tx.execute(
            "INSERT INTO tasks (job_id, title, body, waits_on, state, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![job, title, body, json!(waits_on).to_string(), state, now_ms()],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.task(id)?.ok_or_else(|| anyhow!("t{id} vanished"))
    }

    fn task_row(r: &rusqlite::Row) -> rusqlite::Result<Task> {
        let waits: String = r.get(4)?;
        let result: Option<String> = r.get(6)?;
        Ok(Task {
            id: r.get(0)?,
            job: r.get(1)?,
            title: r.get(2)?,
            body: r.get(3)?,
            waits_on: ids(&waits),
            state: r.get(5)?,
            result: result.and_then(|v| serde_json::from_str(&v).ok()),
            losses: r.get(7)?,
            why: r.get(8)?,
        })
    }

    const TASK_COLS: &'static str = "id, job_id, title, body, waits_on, state, result, losses, why";

    pub fn task(&self, id: i64) -> Result<Option<Task>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {} FROM tasks WHERE id = ?1", Self::TASK_COLS), params![id], Self::task_row)
            .optional()?)
    }

    pub fn tasks(&self, job: i64) -> Result<Vec<Task>> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {} FROM tasks WHERE job_id = ?1 ORDER BY id", Self::TASK_COLS))?;
        Ok(st.query_map(params![job], Self::task_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The waiting tasks that wait on nothing any more, opened -- in the same
    /// transaction that finished what they waited on
    fn open_what_waited(tx: &Transaction, job: i64) -> Result<()> {
        let mut st = tx.prepare("SELECT id, waits_on FROM tasks WHERE job_id = ?1 AND state = 'waiting'")?;
        let waiting: Vec<(i64, String)> = st
            .query_map(params![job], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(st);
        for (id, waits) in waiting {
            let mut ready = true;
            for w in ids(&waits) {
                let s: Option<String> = tx
                    .query_row("SELECT state FROM tasks WHERE id = ?1", params![w], |r| r.get(0))
                    .optional()?;
                ready &= s.as_deref() == Some("done");
            }
            if ready {
                Self::shift(tx, "tasks", id, &["waiting"], "open")?;
            }
        }
        Ok(())
    }

    /// A held or failed task given back to the lead's hands: open to be
    /// assigned again
    pub fn reopen(&mut self, task: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        Self::shift(&tx, "tasks", task, &["held", "failed"], "open")?;
        tx.execute("UPDATE tasks SET why = NULL WHERE id = ?1", params![task])?;
        tx.commit()?;
        Ok(())
    }

    /// A task the lead took out of the job on purpose, with why. Only one
    /// nobody is working on and that is not already done
    pub fn task_drop(&mut self, task: i64, why: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        Self::shift(&tx, "tasks", task, &["waiting", "open", "held", "failed"], "dropped")?;
        tx.execute(
            "UPDATE tasks SET why = ?2, finished_at = ?3 WHERE id = ?1",
            params![task, why, now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    }

    // -- assignments ---------------------------------------------------------

    fn assignment_row(r: &rusqlite::Row) -> rusqlite::Result<Assignment> {
        let seen: Option<i64> = r.get(7)?;
        Ok(Assignment {
            id: r.get(0)?,
            job: r.get(1)?,
            task: r.get(2)?,
            tab: r.get(3)?,
            process: r.get(4)?,
            depth: r.get(5)?,
            state: r.get(6)?,
            seen_starting: seen.map(|v| v != 0),
            end_reason: r.get(8)?,
            afterwards: r.get(9)?,
            ended_at: r.get(10)?,
            created_at: r.get(11)?,
            tab_name: r.get(12)?,
        })
    }

    const ASSIGNMENT_COLS: &'static str = "id, job_id, task_id, tab, process, depth, state, seen_starting, end_reason, \
         afterwards, ended_at, created_at, COALESCE((SELECT n.name FROM tab_names n WHERE n.uid = assignments.tab), tab)";

    pub fn assignment(&self, id: i64) -> Result<Option<Assignment>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {} FROM assignments WHERE id = ?1", Self::ASSIGNMENT_COLS),
                params![id],
                Self::assignment_row,
            )
            .optional()?)
    }

    pub fn assignments(&self, job: i64) -> Result<Vec<Assignment>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM assignments WHERE job_id = ?1 ORDER BY id",
            Self::ASSIGNMENT_COLS
        ))?;
        Ok(st.query_map(params![job], Self::assignment_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every assignment not yet over, in every job
    pub fn active_assignments(&self) -> Result<Vec<Assignment>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM assignments WHERE state IN ('handing','working') ORDER BY id",
            Self::ASSIGNMENT_COLS
        ))?;
        Ok(st.query_map([], Self::assignment_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The assignment a tab is on now, if any
    pub fn active_for_tab(&self, tab: &str) -> Result<Option<Assignment>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {} FROM assignments WHERE tab = ?1 AND state IN ('handing','working') ORDER BY id DESC LIMIT 1",
                    Self::ASSIGNMENT_COLS
                ),
                params![tab],
                Self::assignment_row,
            )
            .optional()?)
    }

    /// The last assignment a tab was given, over or not
    pub fn last_for_tab(&self, tab: &str) -> Result<Option<Assignment>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {} FROM assignments WHERE tab = ?1 ORDER BY id DESC LIMIT 1", Self::ASSIGNMENT_COLS),
                params![tab],
                Self::assignment_row,
            )
            .optional()?)
    }

    /// Hand a task to a tab: the task goes from open to working, and the tab is
    /// written down as doing it. Refused when the tab is already on something
    /// or the task is not open -- the caller says why and what to do instead
    pub fn assign(&mut self, task: i64, tab: &str, process: Option<i64>, depth: i64) -> Result<Assignment> {
        let tx = self.conn.transaction()?;
        let (job, state): (i64, String) = tx
            .query_row("SELECT job_id, state FROM tasks WHERE id = ?1", params![task], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?
            .ok_or_else(|| anyhow!("there is no task t{task}"))?;
        let busy: Option<i64> = tx
            .query_row(
                "SELECT id FROM assignments WHERE tab = ?1 AND state IN ('handing','working')",
                params![tab],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(a) = busy {
            let name = tx
                .query_row("SELECT name FROM tab_names WHERE uid = ?1", params![tab], |r| r.get::<_, String>(0))
                .optional()?
                .unwrap_or_else(|| tab.to_string());
            bail!("<@{name}> is already on a{a}");
        }
        if state != "open" {
            bail!("t{task} is {state}, not open");
        }
        Self::shift(&tx, "tasks", task, &["open"], "working")?;
        // The tab's last assignment, over and not yet decided for, is decided
        // for now: the tab is being given more work
        tx.execute(
            "UPDATE assignments SET afterwards = 'reused' WHERE tab = ?1 AND afterwards IS NULL AND state NOT IN ('handing','working')",
            params![tab],
        )?;
        tx.execute(
            "INSERT INTO assignments (job_id, task_id, tab, process, depth, state, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'handing', ?6)",
            params![job, task, tab, process, depth, now_ms()],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.assignment(id)?.ok_or_else(|| anyhow!("a{id} vanished"))
    }

    /// The brief went in, and the tab was seen starting on it (`seen`) or
    /// nothing could be seen either way
    pub fn started(&mut self, id: i64, seen: bool) -> Result<()> {
        let tx = self.conn.transaction()?;
        Self::shift(&tx, "assignments", id, &["handing"], "working")?;
        tx.execute("UPDATE assignments SET seen_starting = ?2 WHERE id = ?1", params![id, seen as i64])?;
        tx.commit()?;
        Ok(())
    }

    /// The brief never went in (the tab was waiting on a person, or closed):
    /// the assignment is forgotten and the task is open again. Not a loss --
    /// nothing was tried
    pub fn withdraw(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let task: i64 = tx.query_row("SELECT task_id FROM assignments WHERE id = ?1", params![id], |r| r.get(0))?;
        Self::shift(&tx, "assignments", id, &["handing"], "failed")?;
        tx.execute(
            "UPDATE assignments SET end_reason = ?2, afterwards = 'released', ended_at = ?3 WHERE id = ?1",
            params![id, NOT_HANDED, now_ms()],
        )?;
        Self::shift(&tx, "tasks", task, &["working"], "open")?;
        tx.commit()?;
        Ok(())
    }

    /// The worker's own report. Taken once: a second one for the same
    /// assignment answers "already" rather than changing anything
    pub fn report(&mut self, id: i64, done: bool, result: &Value) -> Result<Reported> {
        let tx = self.conn.transaction()?;
        let a = tx
            .query_row(
                &format!("SELECT {} FROM assignments WHERE id = ?1", Self::ASSIGNMENT_COLS),
                params![id],
                Self::assignment_row,
            )
            .optional()?
            .ok_or_else(|| anyhow!("there is no a{id}"))?;
        let want = if done { "done" } else { "failed" };
        if a.over() {
            return Ok(if a.state == want { Reported::Already } else { Reported::EndedAs(a.state) });
        }
        Self::shift(&tx, "assignments", id, &["handing", "working"], want)?;
        tx.execute("UPDATE assignments SET ended_at = ?2 WHERE id = ?1", params![id, now_ms()])?;
        Self::shift(&tx, "tasks", a.task, &["working"], want)?;
        tx.execute(
            "UPDATE tasks SET result = ?2, finished_at = ?3 WHERE id = ?1",
            params![a.task, result.to_string(), now_ms()],
        )?;
        tx.execute(
            "UPDATE questions SET state = 'closed' WHERE assignment_id = ?1 AND state = 'asked'",
            params![id],
        )?;
        if done {
            Self::open_what_waited(&tx, a.job)?;
        }
        tx.commit()?;
        Ok(Reported::Now)
    }

    /// Ended without a report: the tab's program ended, the tab was closed.
    /// Counted against the task, which is open to be tried again until it has
    /// been lost [`LOSSES_ALLOWED`] times. Answers whether that was the last
    pub fn lost(&mut self, id: i64, reason: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let task: i64 = tx.query_row("SELECT task_id FROM assignments WHERE id = ?1", params![id], |r| r.get(0))?;
        Self::shift(&tx, "assignments", id, &["handing", "working"], "failed")?;
        tx.execute(
            "UPDATE assignments SET end_reason = ?2, ended_at = ?3 WHERE id = ?1",
            params![id, reason, now_ms()],
        )?;
        tx.execute("UPDATE tasks SET losses = losses + 1 WHERE id = ?1", params![task])?;
        let losses: i64 = tx.query_row("SELECT losses FROM tasks WHERE id = ?1", params![task], |r| r.get(0))?;
        let last = losses >= LOSSES_ALLOWED;
        Self::shift(&tx, "tasks", task, &["working"], if last { "failed" } else { "open" })?;
        if last {
            tx.execute(
                "UPDATE tasks SET why = ?2, finished_at = ?3 WHERE id = ?1",
                params![task, format!("its tab was lost {losses} times; last: {reason}"), now_ms()],
            )?;
        }
        tx.execute(
            "UPDATE questions SET state = 'closed' WHERE assignment_id = ?1 AND state = 'asked'",
            params![id],
        )?;
        tx.commit()?;
        Ok(last)
    }

    /// Stopped on purpose, by the lead or the person: the task is held until
    /// the lead decides what to do with it
    pub fn stop(&mut self, id: i64, why: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        let task: i64 = tx.query_row("SELECT task_id FROM assignments WHERE id = ?1", params![id], |r| r.get(0))?;
        Self::shift(&tx, "assignments", id, &["handing", "working"], "stopped")?;
        tx.execute(
            "UPDATE assignments SET end_reason = ?2, ended_at = ?3 WHERE id = ?1",
            params![id, why, now_ms()],
        )?;
        Self::shift(&tx, "tasks", task, &["working"], "held")?;
        tx.execute("UPDATE tasks SET why = ?2 WHERE id = ?1", params![task, why])?;
        tx.execute(
            "UPDATE questions SET state = 'closed' WHERE assignment_id = ?1 AND state = 'asked'",
            params![id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// What became of the tab once its assignment was over
    pub fn afterwards(&mut self, id: i64, what: &str) -> Result<()> {
        let a = self.assignment(id)?.ok_or_else(|| anyhow!("there is no a{id}"))?;
        if a.active() {
            bail!("a{id} is still {}", a.state);
        }
        self.conn
            .execute("UPDATE assignments SET afterwards = ?2 WHERE id = ?1", params![id, what])?;
        Ok(())
    }

    // -- tabs and folders a job made ----------------------------------------

    pub fn opened(&mut self, job: i64, tab: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO opened_tabs (tab, job_id, held_by, opened_at) VALUES (?1, ?2, 'job', ?3) ON CONFLICT(tab, job_id) DO NOTHING",
            params![tab, job, now_ms()],
        )?;
        Ok(())
    }

    fn opened_row(r: &rusqlite::Row) -> rusqlite::Result<Opened> {
        Ok(Opened { tab: r.get(0)?, job: r.get(1)?, held_by: r.get(2)?, opened_at: r.get(3)?, tab_name: r.get(4)? })
    }

    const OPENED_COLS: &'static str =
        "tab, job_id, held_by, opened_at, COALESCE((SELECT n.name FROM tab_names n WHERE n.uid = opened_tabs.tab), tab)";

    pub fn opened_by(&self, job: i64) -> Result<Vec<Opened>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM opened_tabs WHERE job_id = ?1 ORDER BY opened_at",
            Self::OPENED_COLS
        ))?;
        Ok(st.query_map(params![job], Self::opened_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Which job opened a tab, if one did
    pub fn opener_of(&self, tab: &str) -> Result<Option<Opened>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {} FROM opened_tabs WHERE tab = ?1 ORDER BY opened_at DESC LIMIT 1", Self::OPENED_COLS),
                params![tab],
                Self::opened_row,
            )
            .optional()?)
    }

    /// A person typed into it: it is theirs now, and nothing here closes it
    pub fn person_took(&mut self, tab: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE opened_tabs SET held_by = 'person' WHERE tab = ?1 AND held_by = 'job'",
            params![tab],
        )?;
        Ok(())
    }

    /// The job closed it
    pub fn gone(&mut self, tab: &str) -> Result<()> {
        self.conn
            .execute("UPDATE opened_tabs SET held_by = 'gone' WHERE tab = ?1 AND held_by = 'job'", params![tab])?;
        Ok(())
    }

    pub fn folder_made(&mut self, job: i64, folder: &str, branch: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO made_folders (job_id, folder, branch, made_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(job_id, folder) DO NOTHING",
            params![job, folder, branch, now_ms()],
        )?;
        Ok(())
    }

    pub fn folders(&self, job: i64) -> Result<Vec<MadeFolder>> {
        let mut st = self.conn.prepare(
            "SELECT job_id, folder, branch, made_at FROM made_folders WHERE job_id = ?1 ORDER BY made_at",
        )?;
        let rows = st
            .query_map(params![job], |r| {
                Ok(MadeFolder { job: r.get(0)?, folder: r.get(1)?, branch: r.get(2)?, made_at: r.get(3)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // -- mail ----------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn post(
        &mut self,
        job: Option<i64>,
        box_: &str,
        sender: &str,
        kind: &str,
        subject: &str,
        body: &str,
        extra: &Value,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO mail (job_id, box, sender, kind, subject, body, extra, sent_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![job, box_, sender, kind, subject, body, extra.to_string(), now_ms()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn mail_row(r: &rusqlite::Row) -> rusqlite::Result<Mail> {
        let extra: String = r.get(7)?;
        Ok(Mail {
            seq: r.get(0)?,
            job: r.get(1)?,
            box_: r.get(2)?,
            sender: r.get(3)?,
            kind: r.get(4)?,
            subject: r.get(5)?,
            body: r.get(6)?,
            extra: serde_json::from_str(&extra).unwrap_or(Value::Null),
            sent_at: r.get(8)?,
        })
    }

    const MAIL_COLS: &'static str = "seq, job_id, box, sender, kind, subject, body, extra, sent_at";

    pub fn mail(&self, seq: i64) -> Result<Option<Mail>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {} FROM mail WHERE seq = ?1", Self::MAIL_COLS), params![seq], Self::mail_row)
            .optional()?)
    }

    /// Unread mail in a box, oldest first, without handing it over
    pub fn unread(&self, box_: &str) -> Result<Vec<Mail>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM mail WHERE box = ?1 AND is_read = 0 ORDER BY seq LIMIT 100",
            Self::MAIL_COLS
        ))?;
        Ok(st.query_map(params![box_], Self::mail_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The last mail of a job, newest first (the person's view)
    pub fn history(&self, job: i64, most: usize) -> Result<Vec<Mail>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM mail WHERE job_id = ?1 ORDER BY seq DESC LIMIT ?2",
            Self::MAIL_COLS
        ))?;
        Ok(st
            .query_map(params![job, most as i64], Self::mail_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Boxes with unread mail their tab has not been told about, and how much
    pub fn untold(&self) -> Result<Vec<(String, usize)>> {
        let mut st = self
            .conn
            .prepare("SELECT box, COUNT(*) FROM mail WHERE is_read = 0 AND is_told = 0 GROUP BY box")?;
        Ok(st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The box's tab has been told about everything unread in it: nothing said
    /// so far is pointed at again
    pub fn told(&mut self, box_: &str) -> Result<()> {
        self.conn
            .execute("UPDATE mail SET is_told = 1 WHERE box = ?1 AND is_read = 0", params![box_])?;
        Ok(())
    }

    /// The handover given and not yet dealt with, if there is one
    pub fn given(&self, box_: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM handovers WHERE box = ?1 AND state = 'given'",
                params![box_],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The oldest mail not yet dealt with: the handover given before and not
    /// dealt with, or a new one of the oldest unread. `wake` names the kinds
    /// worth making a new handover for; once made, it carries everything unread
    /// in order, whatever its kind. `None` when there is nothing
    pub fn hand_over(&mut self, box_: &str, wake: Option<&[String]>) -> Result<Option<Handover>> {
        let tx = self.conn.transaction()?;
        let given: Option<(i64, String)> = tx
            .query_row(
                "SELECT id, mail FROM handovers WHERE box = ?1 AND state = 'given'",
                params![box_],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, seqs)) = given {
            let mail = Self::mail_of(&tx, &ids(&seqs))?;
            tx.commit()?;
            return Ok(Some(Handover { id, box_: box_.to_string(), mail, again: true }));
        }
        let mut st = tx.prepare(&format!(
            "SELECT {} FROM mail WHERE box = ?1 AND is_read = 0 ORDER BY seq LIMIT {HANDOVER_MOST}",
            Self::MAIL_COLS
        ))?;
        let unread = st.query_map(params![box_], Self::mail_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(st);
        let woken = match wake {
            None => !unread.is_empty(),
            Some(kinds) => unread.iter().any(|m| kinds.iter().any(|k| *k == m.kind)),
        };
        if !woken {
            tx.commit()?;
            return Ok(None);
        }
        let seqs: Vec<i64> = unread.iter().map(|m| m.seq).collect();
        tx.execute(
            "INSERT INTO handovers (box, mail, state, given_at) VALUES (?1, ?2, 'given', ?3)",
            params![box_, json!(seqs).to_string(), now_ms()],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Some(Handover { id, box_: box_.to_string(), mail: unread, again: false }))
    }

    fn mail_of(tx: &Transaction, seqs: &[i64]) -> Result<Vec<Mail>> {
        let mut out = Vec::new();
        for s in seqs {
            if let Some(m) = tx
                .query_row(&format!("SELECT {} FROM mail WHERE seq = ?1", Self::MAIL_COLS), params![s], Self::mail_row)
                .optional()?
            {
                out.push(m);
            }
        }
        Ok(out)
    }

    /// The reader has dealt with a handover: its mail is read. Saying so twice
    /// is not an error
    pub fn dealt(&mut self, box_: &str, handover: i64) -> Result<Dealt> {
        let tx = self.conn.transaction()?;
        let row: Option<(String, String, String)> = tx
            .query_row(
                "SELECT box, mail, state FROM handovers WHERE id = ?1",
                params![handover],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((owner, seqs, state)) = row else {
            bail!("there is no handover {handover}");
        };
        if owner != box_ {
            bail!("handover {handover} is not this inbox's");
        }
        if state == "dealt" {
            return Ok(Dealt::Already);
        }
        for s in ids(&seqs) {
            tx.execute("UPDATE mail SET is_read = 1 WHERE seq = ?1", params![s])?;
        }
        tx.execute(
            "UPDATE handovers SET state = 'dealt', dealt_at = ?2 WHERE id = ?1",
            params![handover, now_ms()],
        )?;
        tx.commit()?;
        Ok(Dealt::Now)
    }

    /// Mail nobody has to deal with again
    pub fn mark_read(&mut self, seq: i64) -> Result<()> {
        self.conn.execute("UPDATE mail SET is_read = 1 WHERE seq = ?1", params![seq])?;
        Ok(())
    }

    // -- questions -----------------------------------------------------------

    pub fn ask(&mut self, job: i64, assignment: i64, from: &str, question: &str, choices: &[String]) -> Result<Question> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO mail (job_id, box, sender, kind, subject, body, extra, sent_at) VALUES (?1, ?2, ?3, 'question', 'Question', ?4, ?5, ?6)",
            params![
                job,
                format!("job:{job}"),
                from,
                question,
                json!({"assignment": format!("a{assignment}"), "choices": choices}).to_string(),
                now_ms()
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO questions (id, job_id, assignment_id, question, choices, state, asked_at) VALUES (?1, ?2, ?3, ?4, ?5, 'asked', ?6)",
            params![id, job, assignment, question, json!(choices).to_string(), now_ms()],
        )?;
        tx.commit()?;
        self.question(id)?.ok_or_else(|| anyhow!("q{id} vanished"))
    }

    fn question_row(r: &rusqlite::Row) -> rusqlite::Result<Question> {
        let choices: String = r.get(4)?;
        Ok(Question {
            id: r.get(0)?,
            job: r.get(1)?,
            assignment: r.get(2)?,
            question: r.get(3)?,
            choices: list(&choices),
            state: r.get(5)?,
            answer: r.get(6)?,
            asked_at: r.get(7)?,
        })
    }

    const QUESTION_COLS: &'static str = "id, job_id, assignment_id, question, choices, state, answer, asked_at";

    pub fn question(&self, id: i64) -> Result<Option<Question>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {} FROM questions WHERE id = ?1", Self::QUESTION_COLS),
                params![id],
                Self::question_row,
            )
            .optional()?)
    }

    pub fn open_questions(&self, job: i64) -> Result<Vec<Question>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM questions WHERE job_id = ?1 AND state = 'asked' ORDER BY id",
            Self::QUESTION_COLS
        ))?;
        Ok(st.query_map(params![job], Self::question_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Answer a question. The same answer twice is fine; a different second
    /// answer is refused, since the worker may already be acting on the first
    pub fn answer(&mut self, id: i64, text: &str) -> Result<Answered> {
        let q = self.question(id)?.ok_or_else(|| anyhow!("there is no question q{id}"))?;
        match q.state.as_str() {
            "closed" => bail!("q{id} is closed: the assignment that asked it is over"),
            "answered" if q.answer.as_deref() == Some(text) => return Ok(Answered::Already),
            "answered" => bail!("q{id} was already answered differently: {}", q.answer.unwrap_or_default()),
            _ => {}
        }
        let tx = self.conn.transaction()?;
        Self::shift(&tx, "questions", id, &["asked"], "answered")?;
        tx.execute(
            "UPDATE questions SET answer = ?2, answered_at = ?3 WHERE id = ?1",
            params![id, text, now_ms()],
        )?;
        // The question itself is dealt with
        tx.execute("UPDATE mail SET is_read = 1 WHERE seq = ?1", params![id])?;
        tx.commit()?;
        Ok(Answered::Now)
    }

    // -- decisions -----------------------------------------------------------

    pub fn decision_open(&mut self, task: i64, question: &str, choices: &[String], decided_by: &str) -> Result<Decision> {
        let tx = self.conn.transaction()?;
        let (job, state): (i64, String) = tx
            .query_row("SELECT job_id, state FROM tasks WHERE id = ?1", params![task], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?
            .ok_or_else(|| anyhow!("there is no task t{task}"))?;
        let job_state: String = tx.query_row("SELECT state FROM jobs WHERE id = ?1", params![job], |r| r.get(0))?;
        if job_state != "open" {
            bail!("j{job} is closed; nothing more is decided in it");
        }
        match state.as_str() {
            "working" => bail!("t{task} is being worked on; a decision is asked for before or after, not during"),
            "done" => bail!("t{task} is already done"),
            "dropped" => bail!("t{task} was dropped from the job"),
            _ => {}
        }
        tx.execute(
            "INSERT INTO decisions (job_id, task_id, question, choices, decided_by, state, asked_at) VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6)",
            params![job, task, question, json!(choices).to_string(), decided_by, now_ms()],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE tasks SET state = 'held', why = ?2 WHERE id = ?1",
            params![task, format!("waiting for a decision (d{id})")],
        )?;
        tx.commit()?;
        self.decision(id)?.ok_or_else(|| anyhow!("d{id} vanished"))
    }

    fn decision_row(r: &rusqlite::Row) -> rusqlite::Result<Decision> {
        let choices: String = r.get(4)?;
        Ok(Decision {
            id: r.get(0)?,
            job: r.get(1)?,
            task: r.get(2)?,
            question: r.get(3)?,
            choices: list(&choices),
            decided_by: r.get(5)?,
            state: r.get(6)?,
            choice: r.get(7)?,
            asked_at: r.get(8)?,
        })
    }

    const DECISION_COLS: &'static str = "id, job_id, task_id, question, choices, decided_by, state, choice, asked_at";

    pub fn decision(&self, id: i64) -> Result<Option<Decision>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {} FROM decisions WHERE id = ?1", Self::DECISION_COLS),
                params![id],
                Self::decision_row,
            )
            .optional()?)
    }

    pub fn decisions(&self, job: i64) -> Result<Vec<Decision>> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {} FROM decisions WHERE job_id = ?1 ORDER BY id", Self::DECISION_COLS))?;
        Ok(st.query_map(params![job], Self::decision_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Decide. Once nothing more is to be decided for it, the task goes back
    /// to where it would be without the decision -- open, or waiting when what
    /// it waits on is not done yet -- whatever was chosen: what the choice
    /// means is for the lead to act on
    pub fn decision_make(&mut self, id: i64, choice: &str, by: &str) -> Result<Decision> {
        let d = self.decision(id)?.ok_or_else(|| anyhow!("there is no decision d{id}"))?;
        if !d.choices.is_empty() && !d.choices.iter().any(|o| o == choice) {
            bail!("d{id} is decided with one of: {}", d.choices.join(", "));
        }
        let tx = self.conn.transaction()?;
        Self::shift(&tx, "decisions", id, &["open"], "made")?;
        tx.execute(
            "UPDATE decisions SET choice = ?2, made_by = ?3, made_at = ?4 WHERE id = ?1",
            params![id, choice, by, now_ms()],
        )?;
        let still: i64 = tx.query_row(
            "SELECT COUNT(*) FROM decisions WHERE task_id = ?1 AND state = 'open'",
            params![d.task],
            |r| r.get(0),
        )?;
        if still == 0 {
            let waits: String = tx.query_row("SELECT waits_on FROM tasks WHERE id = ?1", params![d.task], |r| r.get(0))?;
            let mut ready = true;
            for w in ids(&waits) {
                let s: Option<String> = tx
                    .query_row("SELECT state FROM tasks WHERE id = ?1", params![w], |r| r.get(0))
                    .optional()?;
                ready &= s.as_deref() == Some("done");
            }
            tx.execute(
                "UPDATE tasks SET state = ?2, why = NULL WHERE id = ?1 AND state = 'held'",
                params![d.task, if ready { "open" } else { "waiting" }],
            )?;
        }
        tx.commit()?;
        self.decision(id)?.ok_or_else(|| anyhow!("d{id} vanished"))
    }

    // -- what the watcher raised ---------------------------------------------

    /// Raise `key` unless it was raised before; answers whether it was raised
    /// now. What the watcher says about a thing left undone, it says once
    pub fn raise_once(&mut self, key: &str) -> Result<bool> {
        let n = self.conn.execute(
            "INSERT INTO raised (key, raised_at) VALUES (?1, ?2) ON CONFLICT(key) DO NOTHING",
            params![key, now_ms()],
        )?;
        Ok(n == 1)
    }

    pub fn lower(&mut self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM raised WHERE key = ?1", params![key])?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reported {
    Now,
    Already,
    /// It had already ended some other way (the named state)
    EndedAs(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Dealt {
    Now,
    Already,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Answered {
    Now,
    Already,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::in_memory().unwrap()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("orch-{name}-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// What a version before uids wrote under names goes to the tab of the
    /// settings called that, when only one is; a closed tab's to nobody's --
    /// still read with its name -- and the rewriting happens once
    #[test]
    fn work_written_under_names_goes_to_the_tab_the_settings_call_that() {
        let mut s = store();
        let j = s.job_open("lead", "ship it").unwrap();
        let t = s.task_add(j.id, "build", "build it", &[]).unwrap();
        let a = s.assign(t.id, "heron", Some(1), 1).unwrap();
        s.opened(j.id, "heron").unwrap();
        s.post(None, "tab:lead", "codex", "reply", "r", "a reply", &json!({})).unwrap();
        s.post(Some(j.id), &format!("assignment:{}", a.id), "lead", "note", "n", "a note", &json!({})).unwrap();
        let lead = "11111111-1111-4111-8111-111111111111".to_string();
        let tabs = vec![("lead".to_string(), lead.clone())];
        assert!(s.adopt_uids(&tabs).unwrap());
        assert!(!s.adopt_uids(&tabs).unwrap(), "it rewrote the record twice");
        let job = s.job(j.id).unwrap().unwrap();
        assert_eq!((job.lead.as_str(), job.lead_name.as_str()), (lead.as_str(), "lead"));
        assert_eq!(s.jobs_led_by(&lead).unwrap().len(), 1);
        assert!(s.jobs_led_by("lead").unwrap().is_empty(), "the name still leads it");
        let heron = crate::convo::db::gone_uid("", "heron");
        let a = s.assignment(a.id).unwrap().unwrap();
        assert_eq!((a.tab.as_str(), a.tab_name.as_str()), (heron.as_str(), "heron"));
        assert_eq!(s.opener_of(&heron).unwrap().map(|o| o.tab_name), Some("heron".to_string()));
        assert_eq!(s.unread(&tab_box(&lead)).unwrap().len(), 1, "the lead's own mail is still its");
        assert_eq!(s.unread("assignment:1").unwrap().len(), 1, "a box that is not a tab's is left alone");
        // A name the record has never seen is its uid, said as it is
        assert_eq!(s.name_of("22222222-2222-4222-8222-222222222222").unwrap(), "22222222-2222-4222-8222-222222222222");
    }

    #[test]
    fn a_task_opens_only_once_what_it_waits_on_is_done() {
        let mut s = store();
        let j = s.job_open("lead", "ship it").unwrap();
        let a = s.task_add(j.id, "build", "build it", &[]).unwrap();
        let b = s.task_add(j.id, "review", "review it", &[a.id]).unwrap();
        assert_eq!(a.state, "open");
        assert_eq!(b.state, "waiting");
        let x = s.assign(a.id, "claude", Some(1), 1).unwrap();
        s.started(x.id, true).unwrap();
        assert_eq!(s.report(x.id, true, &json!({"did": "built"})).unwrap(), Reported::Now);
        assert_eq!(s.task(b.id).unwrap().unwrap().state, "open");
    }

    #[test]
    fn a_failed_task_does_not_open_what_waits_on_it() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let a = s.task_add(j.id, "a", "a", &[]).unwrap();
        let b = s.task_add(j.id, "b", "b", &[a.id]).unwrap();
        let x = s.assign(a.id, "w", None, 1).unwrap();
        s.report(x.id, false, &json!({})).unwrap();
        assert_eq!(s.task(a.id).unwrap().unwrap().state, "failed");
        assert_eq!(s.task(b.id).unwrap().unwrap().state, "waiting");
    }

    #[test]
    fn a_report_counts_once() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let a = s.task_add(j.id, "a", "a", &[]).unwrap();
        let x = s.assign(a.id, "w", None, 1).unwrap();
        assert_eq!(s.report(x.id, true, &json!({})).unwrap(), Reported::Now);
        assert_eq!(s.report(x.id, true, &json!({})).unwrap(), Reported::Already);
        assert_eq!(s.report(x.id, false, &json!({})).unwrap(), Reported::EndedAs("done".into()));
    }

    #[test]
    fn a_tab_is_on_one_thing_at_a_time_and_a_task_is_assigned_once() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let a = s.task_add(j.id, "a", "a", &[]).unwrap();
        let b = s.task_add(j.id, "b", "b", &[]).unwrap();
        s.assign(a.id, "w", None, 1).unwrap();
        assert!(s.assign(b.id, "w", None, 1).is_err());
        assert!(s.assign(a.id, "v", None, 1).is_err());
    }

    #[test]
    fn a_task_that_keeps_losing_its_tab_stops_being_tried() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let a = s.task_add(j.id, "a", "a", &[]).unwrap();
        for n in 1..=LOSSES_ALLOWED {
            let x = s.assign(a.id, "w", None, 1).unwrap();
            assert_eq!(s.lost(x.id, "exited").unwrap(), n == LOSSES_ALLOWED);
        }
        assert_eq!(s.task(a.id).unwrap().unwrap().state, "failed");
        assert!(s.assign(a.id, "w", None, 1).is_err());
        s.reopen(a.id).unwrap();
        assert_eq!(s.task(a.id).unwrap().unwrap().state, "open");
    }

    #[test]
    fn a_withdrawn_brief_is_not_a_loss() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let a = s.task_add(j.id, "a", "a", &[]).unwrap();
        let x = s.assign(a.id, "w", None, 1).unwrap();
        s.withdraw(x.id).unwrap();
        let t = s.task(a.id).unwrap().unwrap();
        assert_eq!((t.state.as_str(), t.losses), ("open", 0));
    }

    #[test]
    fn a_handover_comes_back_until_it_is_dealt_with() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let b = format!("job:{}", j.id);
        s.post(Some(j.id), &b, "w", "note", "hi", "one", &json!({})).unwrap();
        assert!(
            s.hand_over(&b, Some(&["report".to_string()])).unwrap().is_none(),
            "a note does not wake a wait for reports"
        );
        s.post(Some(j.id), &b, "w", "report", "done", "two", &json!({})).unwrap();
        let h = s.hand_over(&b, Some(&["report".to_string()])).unwrap().unwrap();
        assert_eq!(h.mail.len(), 2, "it carries everything unread, in order");
        assert!(!h.again);
        let again = s.hand_over(&b, None).unwrap().unwrap();
        assert_eq!((again.id, again.again), (h.id, true));
        assert_eq!(s.dealt(&b, h.id).unwrap(), Dealt::Now);
        assert_eq!(s.dealt(&b, h.id).unwrap(), Dealt::Already);
        assert!(s.hand_over(&b, None).unwrap().is_none());
        assert!(s.dealt("job:999", h.id).is_err(), "another inbox cannot deal with it");
    }

    #[test]
    fn a_handover_is_a_read_not_a_flood() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let b = format!("job:{}", j.id);
        for n in 0..HANDOVER_MOST + 5 {
            s.post(Some(j.id), &b, "w", "note", "n", &n.to_string(), &json!({})).unwrap();
        }
        let h = s.hand_over(&b, None).unwrap().unwrap();
        assert_eq!(h.mail.len(), HANDOVER_MOST);
        s.dealt(&b, h.id).unwrap();
        assert_eq!(s.hand_over(&b, None).unwrap().unwrap().mail.len(), 5, "the rest comes next");
    }

    #[test]
    fn a_question_is_answered_once_and_closed_with_its_assignment() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let a = s.task_add(j.id, "a", "a", &[]).unwrap();
        let x = s.assign(a.id, "w", None, 1).unwrap();
        let q = s.ask(j.id, x.id, "w", "which one?", &["x".into(), "y".into()]).unwrap();
        assert_eq!(s.answer(q.id, "x").unwrap(), Answered::Now);
        assert_eq!(s.answer(q.id, "x").unwrap(), Answered::Already);
        assert!(s.answer(q.id, "y").is_err());
        let q2 = s.ask(j.id, x.id, "w", "and?", &[]).unwrap();
        s.report(x.id, true, &json!({})).unwrap();
        assert_eq!(s.question(q2.id).unwrap().unwrap().state, "closed");
    }

    #[test]
    fn a_decision_holds_its_task_until_it_is_made() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let a = s.task_add(j.id, "merge", "merge it", &[]).unwrap();
        let d = s.decision_open(a.id, "merge into main?", &["yes".into(), "no".into()], "person").unwrap();
        assert_eq!(s.task(a.id).unwrap().unwrap().state, "held");
        assert!(s.decision_make(d.id, "maybe", "person").is_err());
        s.decision_make(d.id, "yes", "person").unwrap();
        assert_eq!(s.task(a.id).unwrap().unwrap().state, "open");
        assert!(s.decision_make(d.id, "no", "person").is_err(), "decided once");
    }

    #[test]
    fn a_decision_does_not_open_a_task_ahead_of_what_it_waits_on() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let a = s.task_add(j.id, "build", "build", &[]).unwrap();
        let b = s.task_add(j.id, "merge", "merge", &[a.id]).unwrap();
        let d = s.decision_open(b.id, "merge?", &[], "person").unwrap();
        s.decision_make(d.id, "yes", "person").unwrap();
        assert_eq!(s.task(b.id).unwrap().unwrap().state, "waiting", "t1 is not done yet");
        let x = s.assign(a.id, "w", None, 1).unwrap();
        s.report(x.id, true, &json!({})).unwrap();
        assert_eq!(s.task(b.id).unwrap().unwrap().state, "open");
    }

    #[test]
    fn nothing_is_decided_in_a_closed_job_or_for_a_dropped_task() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let a = s.task_add(j.id, "a", "a", &[]).unwrap();
        let b = s.task_add(j.id, "b", "b", &[]).unwrap();
        s.task_drop(a.id, "not needed").unwrap();
        assert!(s.decision_open(a.id, "?", &[], "person").is_err());
        s.task_drop(b.id, "not needed").unwrap();
        s.job_close(j.id, "done").unwrap();
        let c = s.job_open("lead", "y").unwrap();
        let t = s.task_add(c.id, "t", "t", &[]).unwrap();
        s.job_close(c.id, "stopped").unwrap();
        assert!(s.decision_open(t.id, "?", &[], "person").unwrap_err().to_string().contains("closed"));
    }

    #[test]
    fn a_new_assignment_decides_for_the_last_one_on_the_same_tab() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        let a = s.task_add(j.id, "a", "a", &[]).unwrap();
        let b = s.task_add(j.id, "b", "b", &[]).unwrap();
        let x1 = s.assign(a.id, "w", None, 1).unwrap();
        s.report(x1.id, true, &json!({})).unwrap();
        s.assign(b.id, "w", None, 1).unwrap();
        assert_eq!(s.assignment(x1.id).unwrap().unwrap().afterwards.as_deref(), Some("reused"));
    }

    #[test]
    fn nothing_waits_on_a_task_of_another_job() {
        let mut s = store();
        let j1 = s.job_open("lead", "x").unwrap();
        let j2 = s.job_open("lead", "y").unwrap();
        let a = s.task_add(j1.id, "a", "a", &[]).unwrap();
        assert!(s.task_add(j2.id, "b", "b", &[a.id]).is_err());
        assert!(s.task_add(j2.id, "b", "b", &[9999]).is_err());
    }

    #[test]
    fn the_record_survives_being_opened_again() {
        let dir = scratch("again");
        let path = dir.join("orchestration.db");
        {
            let mut s = Store::open(&path).unwrap();
            let j = s.job_open("lead", "keep me").unwrap();
            s.task_add(j.id, "a", "a", &[]).unwrap();
        }
        let s = Store::open(&path).unwrap();
        let jobs = s.open_jobs().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].goal, "keep me");
        drop(s);
        assert!(
            !dir.join("orchestration.db.v0.bak").exists() && !dir.join("orchestration.db.v1.bak").exists(),
            "nothing is copied when nothing changes"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_change_from_the_wrong_state_is_refused_and_says_what_it_found() {
        let mut s = store();
        let j = s.job_open("lead", "x").unwrap();
        s.job_close(j.id, "done").unwrap();
        let e = s.job_close(j.id, "again").unwrap_err();
        assert!(e.to_string().contains("closed"), "{e}");
        assert!(s.task_add(j.id, "late", "late", &[]).is_err());
    }

    // -- changing the tables ---------------------------------------------------

    /// The steps as they are, and one more after them
    fn with_one_more(name: &'static str, step: Step) -> Vec<(i64, &'static str, Step)> {
        let mut steps: Vec<(i64, &'static str, Step)> = STEPS
            .iter()
            .map(|(n, name, s)| {
                let s = match s {
                    Step::Sql(sql) => Step::Sql(sql),
                    Step::Code(f) => Step::Code(*f),
                };
                (*n, *name, s)
            })
            .collect();
        steps.push((latest() + 1, name, step));
        steps
    }

    /// A later version that renames a column, adds one with a default, and
    /// changes a table SQLite cannot change in place (a rebuild) -- the kinds
    /// of change a real step makes
    fn next_version() -> Vec<(i64, &'static str, Step)> {
        fn reshape(tx: &Transaction) -> Result<()> {
            tx.execute_batch(
                "ALTER TABLE jobs RENAME COLUMN goal TO aim; \
                 ALTER TABLE jobs ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;",
            )?;
            rebuild(
                tx,
                "raised",
                "CREATE TABLE {table} (key TEXT PRIMARY KEY, raised_at INTEGER NOT NULL, times INTEGER NOT NULL DEFAULT 1)",
                "key, raised_at",
                &[],
            )
        }
        with_one_more("reshape", Step::Code(reshape))
    }

    #[test]
    fn a_new_record_is_made_by_the_steps_alone() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, STEPS, None).unwrap();
        assert_eq!(version_of(&conn).unwrap(), latest());
        let before = schema(&conn).unwrap();
        migrate(&mut conn, STEPS, None).unwrap();
        assert_eq!(schema(&conn).unwrap(), before, "running them again changes nothing");
    }

    #[test]
    fn an_unsupported_table_layout_is_left_untouched() {
        let mut conn = Connection::open_in_memory().unwrap();
        version_of(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO meta (key, value) VALUES ('schema', '1');
             CREATE TABLE saved_work (body TEXT);
             INSERT INTO saved_work (body) VALUES ('keep this work');",
        ).unwrap();
        let before = schema(&conn).unwrap();
        let error = migrate(&mut conn, STEPS, None).unwrap_err();
        assert!(format!("{error:#}").contains("supported table layout"));
        assert_eq!(version_of(&conn).unwrap(), 1);
        assert_eq!(schema(&conn).unwrap(), before);
        let body: String = conn.query_row("SELECT body FROM saved_work", [], |r| r.get(0)).unwrap();
        assert_eq!(body, "keep this work");
    }

    #[test]
    fn a_record_resumes_after_the_first_or_second_step_without_losing_work() {
        for completed in [1, 2] {
            let dir = scratch(&format!("resume-{completed}"));
            let path = dir.join("orchestration.db");
            {
                let mut conn = Connection::open(&path).unwrap();
                migrate(&mut conn, &STEPS[..completed], None).unwrap();
                conn.execute_batch(
                    "INSERT INTO jobs (id, goal, lead, state, started_at) VALUES (7, 'keep this job', 'lead', 'open', 1);
                     INSERT INTO tasks (id, job_id, title, body, state, created_at) VALUES (9, 7, 'work', 'keep this task', 'working', 2);
                     INSERT INTO assignments (id, job_id, task_id, tab, depth, state, created_at)
                       VALUES (11, 7, 9, 'worker', 1, 'working', 3);
                     INSERT INTO mail (seq, job_id, box, sender, kind, subject, body, sent_at)
                       VALUES (13, 7, 'job:7', 'worker', 'note', 'progress', 'keep this message', 4);",
                ).unwrap();
            }
            let mut s = Store::open(&path).unwrap();
            assert_eq!(version_of(&s.conn).unwrap(), latest());
            assert_eq!(s.job(7).unwrap().unwrap().goal, "keep this job");
            assert_eq!(s.task(9).unwrap().unwrap().state, "working");
            assert_eq!(s.assignment(11).unwrap().unwrap().task, 9);
            assert_eq!(s.unread("job:7").unwrap()[0].body, "keep this message");
            assert_eq!(schema(&s.conn).unwrap(), schema(&Store::in_memory().unwrap().conn).unwrap());
            let lead_uid = "11111111-1111-4111-8111-111111111111";
            let worker_uid = "22222222-2222-4222-8222-222222222222";
            s.adopt_uids(&[("lead".into(), lead_uid.into()), ("worker".into(), worker_uid.into())]).unwrap();
            assert_eq!(s.job(7).unwrap().unwrap().lead, lead_uid);
            assert_eq!(s.assignment(11).unwrap().unwrap().tab, worker_uid);
            drop(s);
            let backup = Connection::open(dir.join(format!("orchestration.db.v{completed}.bak"))).unwrap();
            assert_eq!(version_of(&backup).unwrap(), completed as i64);
            let old_lead: String = backup.query_row("SELECT lead FROM jobs WHERE id = 7", [], |r| r.get(0)).unwrap();
            assert_eq!(old_lead, "lead");
            drop(backup);
            let reopened = Store::open(&path).unwrap();
            assert_eq!(reopened.assignment(11).unwrap().unwrap().tab, worker_uid);
            drop(reopened);
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn an_older_record_is_copied_then_brought_up_to_date_with_its_rows() {
        let dir = scratch("upgrade");
        let path = dir.join("orchestration.db");
        {
            let mut s = Store::open(&path).unwrap();
            let j = s.job_open("lead", "keep me").unwrap();
            let t = s.task_add(j.id, "a", "a", &[]).unwrap();
            s.assign(t.id, "w", None, 1).unwrap();
            s.raise_once("quiet:1").unwrap();
        }
        let now = latest();
        let backup = dir.join(format!("orchestration.db.v{now}.bak"));
        let mut conn = Connection::open(&path).unwrap();
        migrate(&mut conn, &next_version(), Some(&backup)).unwrap();
        assert_eq!(version_of(&conn).unwrap(), now + 1);
        let (aim, priority): (String, i64) = conn
            .query_row("SELECT aim, priority FROM jobs WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((aim.as_str(), priority), ("keep me", 0));
        let times: i64 = conn.query_row("SELECT times FROM raised WHERE key = 'quiet:1'", [], |r| r.get(0)).unwrap();
        assert_eq!(times, 1, "the rebuilt table kept its rows");
        let pointing: i64 = conn
            .query_row("SELECT COUNT(*) FROM assignments WHERE job_id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pointing, 1, "what pointed at the job still does");
        let on: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(on, 1, "foreign keys are back on");
        drop(conn);
        let old = Connection::open(&backup).unwrap();
        assert_eq!(version_of(&old).unwrap(), now, "the copy is the record as it was");
        let goal: String = old.query_row("SELECT goal FROM jobs WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(goal, "keep me");
        drop(old);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_step_that_fails_leaves_the_record_as_it_was() {
        fn broken(tx: &Transaction) -> Result<()> {
            tx.execute_batch("ALTER TABLE jobs ADD COLUMN half INTEGER;")?;
            bail!("the second half went wrong")
        }
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, STEPS, None).unwrap();
        let before = schema(&conn).unwrap();
        let e = migrate(&mut conn, &with_one_more("broken", Step::Code(broken)), None).unwrap_err();
        assert!(format!("{e:#}").contains("(broken)"), "{e:#}");
        assert_eq!(version_of(&conn).unwrap(), latest());
        assert_eq!(schema(&conn).unwrap(), before, "nothing of the step is left");
    }

    #[test]
    fn a_record_from_a_newer_version_is_left_alone() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, &next_version(), None).unwrap();
        let e = migrate(&mut conn, STEPS, None).unwrap_err();
        assert!(e.to_string().contains("newer version"), "{e}");
        assert_eq!(version_of(&conn).unwrap(), latest() + 1);
    }

    #[test]
    fn a_step_that_leaves_rows_pointing_at_nothing_is_refused() {
        fn orphan(tx: &Transaction) -> Result<()> {
            tx.execute_batch(
                "INSERT INTO tasks (job_id, title, body, state, created_at) VALUES (999, 't', 'b', 'open', 0);",
            )?;
            Ok(())
        }
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, STEPS, None).unwrap();
        let e = migrate(&mut conn, &with_one_more("orphan", Step::Code(orphan)), None).unwrap_err();
        assert!(e.to_string().contains("pointing at nothing"), "{e}");
        assert_eq!(version_of(&conn).unwrap(), latest());
    }

    #[test]
    fn the_steps_are_numbered_in_order_and_so_are_their_files() {
        crate::sqlite::check_numbering(STEPS, "crates/core/src/orch/migrations");
    }

    /// The tables as the steps make them, written out for anyone reading the
    /// repository. Change a step and this fails until it is written again:
    ///
    ///     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core orchestration_db
    #[test]
    fn the_orchestration_db_design_is_what_the_steps_make() {
        let header = format!(
            "-- The record of work handed between AI tabs (orchestration.db), as the\n\
             -- steps in crates/core/src/orch/migrations/ leave it at version {}.\n\
             --\n\
             -- Written by a test; do not edit. Change the tables by adding a step (see\n\
             -- orchestration-db.md), then write this again:\n\
             --     SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core orchestration_db",
            latest()
        );
        crate::sqlite::check_design(STEPS, WHAT, &header, "docs/design/orchestration-db.sql", "orchestration_db");
    }
}
