//! What every SQLite file the app keeps shares: how it is opened, and how its
//! tables are made and changed by numbered steps. The rules this carries out
//! are written down in `docs/design/sqlite.md`.
//!
//! Written once so a second record cannot drift from the first: the record of
//! handed work (`orch::db`) and the record of conversations (`convo`) both open
//! their files here and bring them up to date here, and a rule fixed here is
//! fixed for both.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};

/// One change to the tables
pub enum Step {
    /// Statements run as they are
    Sql(&'static str),
    /// A change that has to be worked out: a rebuild with its rows moved
    /// across ([`rebuild`]), a value reshaped
    Code(fn(&Transaction) -> Result<()>),
}

/// The version a record says it is at (0: nothing made yet)
pub fn version_of(conn: &Connection) -> Result<i64> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| r.get::<_, String>(0))
        .optional()?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

/// Run every step a record does not have yet, each in a transaction of its own
/// and recorded as it completes, so a program killed half way resumes at the
/// step it was on. A record at a version this program has never heard of is
/// refused rather than changed. `backup`, when given, is where a record that
/// already has tables is copied before anything is run on it. `what` names the
/// record in the refusal ("the record of handed work")
pub fn migrate(conn: &mut Connection, steps: &[(i64, &str, Step)], backup: Option<&Path>, what: &str) -> Result<()> {
    let have = version_of(conn)?;
    let newest = steps.last().map(|s| s.0).unwrap_or(0);
    if have > newest {
        bail!(
            "{what} was written by a newer version of this app \
             (version {have}; this one knows up to {newest})"
        );
    }
    let todo: Vec<&(i64, &str, Step)> = steps.iter().filter(|s| s.0 > have).collect();
    if todo.is_empty() {
        return Ok(());
    }
    if have > 0
        && let Some(copy) = backup
    {
        let _ = std::fs::remove_file(copy);
        conn.execute("VACUUM INTO ?1", params![copy.to_string_lossy()])
            .with_context(|| format!("copying {what} to {} before changing it", copy.display()))?;
    }
    // Off while the steps run: a rebuild drops a table other tables point at,
    // and with this on SQLite would delete what points at it. Checked after
    // every step instead, and switched back on however this ends
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    let ran = run_steps(conn, &todo);
    conn.pragma_update(None, "foreign_keys", "ON")?;
    ran
}

fn run_steps(conn: &mut Connection, todo: &[&(i64, &str, Step)]) -> Result<()> {
    for (n, name, step) in todo.iter().map(|s| (s.0, s.1, &s.2)) {
        let tx = conn.transaction()?;
        let done: Result<()> = match step {
            Step::Sql(sql) => tx.execute_batch(sql).map_err(Into::into),
            Step::Code(f) => f(&tx),
        };
        done.with_context(|| format!("step {n} ({name})"))?;
        let broken: Option<String> = tx
            .query_row("SELECT \"table\" FROM pragma_foreign_key_check LIMIT 1", [], |r| r.get(0))
            .optional()?;
        if let Some(t) = broken {
            bail!("step {n} ({name}) left rows in {t} pointing at nothing");
        }
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('schema', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![n.to_string()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

/// Change a table in a way SQLite cannot do in place -- a column's type, a
/// constraint, a column dropped or moved -- the way SQLite's own documentation
/// says to: make the new table beside the old, copy the rows across, drop the
/// old, give the new its name, and make its indexes again. `create` is the new
/// table's `CREATE TABLE` with `{table}` where its name goes; `columns` the
/// columns copied, named the same in both; `indexes` the table's indexes as
/// they should be afterwards. For a step, where foreign keys are off (see
/// [`migrate`])
pub fn rebuild(tx: &Transaction, table: &str, create: &str, columns: &str, indexes: &[&str]) -> Result<()> {
    let temp = format!("{table}_rebuilt");
    tx.execute_batch(&create.replace("{table}", &temp))?;
    tx.execute_batch(&format!("INSERT INTO {temp} ({columns}) SELECT {columns} FROM {table};"))?;
    tx.execute_batch(&format!("DROP TABLE {table}; ALTER TABLE {temp} RENAME TO {table};"))?;
    for i in indexes {
        tx.execute_batch(i)?;
    }
    Ok(())
}

/// The tables and indexes as SQLite holds them, in a fixed order: what the
/// design file a test writes (`docs/design/<feature>-db.sql`) shows
pub fn schema(conn: &Connection) -> Result<String> {
    let mut st = conn.prepare(
        "SELECT sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' AND name <> 'meta' \
         ORDER BY CASE type WHEN 'table' THEN 0 ELSE 1 END, name",
    )?;
    let rows = st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    // SQLite keeps each statement as it was written, so a step file checked out
    // with CRLF would read differently here than the same file with LF
    Ok(rows
        .into_iter()
        .map(|s| format!("{};\n", s.trim().replace("\r\n", "\n")))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The record at `path`, made if it is not there and brought up to date if an
/// older version wrote it (copied beside itself first, as
/// `<name>.v<version>.bak`), ready to use
pub fn open(path: &Path, steps: &[(i64, &str, Step)], what: &str) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    let have = version_of(&conn)?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let backup = path.with_file_name(format!("{name}.v{have}.bak"));
    ready(conn, steps, Some(&backup), what)
}

/// A record that lives only as long as the connection (tests)
pub fn in_memory(steps: &[(i64, &str, Step)], what: &str) -> Result<Connection> {
    ready(Connection::open_in_memory()?, steps, None, what)
}

/// What every connection is given before anything else is done with it
/// (`docs/design/sqlite.md` section 2)
fn ready(mut conn: Connection, steps: &[(i64, &str, Step)], backup: Option<&Path>, what: &str) -> Result<Connection> {
    // Readers do not block the writer
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // Safe with WAL, and much faster than FULL
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Wait for a lock instead of failing
    conn.busy_timeout(Duration::from_secs(5))?;
    migrate(&mut conn, steps, backup, what)?;
    // Off by default in SQLite, and per connection
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

/// Milliseconds since the epoch, UTC: how every `*_at` column is written
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The generated design file for a record (`docs/design/<feature>-db.sql`)
/// compared with what its steps make, or written when `SHIKISHA_WRITE_DOCS`
/// is set. `header` is the comment written above the tables
#[cfg(test)]
pub fn check_design(steps: &[(i64, &str, Step)], what: &str, header: &str, file: &str, test: &str) {
    let conn = in_memory(steps, what).unwrap();
    let want = format!("{header}\n\n{}", schema(&conn).unwrap());
    let path = crate::repo_root().join(file);
    if std::env::var("SHIKISHA_WRITE_DOCS").is_ok() {
        std::fs::write(&path, &want).expect("the design could be written");
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert_eq!(
        have,
        want,
        "{} is not what the steps make. Write it again:\n    \
         SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core {test}",
        path.display()
    );
}

/// Steps and their files numbered 1, 2, 3 ... with nothing skipped, and every
/// file in `dir` (relative to the repository) a step
#[cfg(test)]
pub fn check_numbering(steps: &[(i64, &str, Step)], dir: &str) {
    let mut last = 0;
    for (n, _, _) in steps {
        assert_eq!(*n, last + 1, "steps are numbered 1, 2, 3 ...");
        last = *n;
    }
    let dir = crate::repo_root().join(dir);
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .filter(|n| n.ends_with(".sql"))
        .collect();
    files.sort();
    for (i, f) in files.iter().enumerate() {
        assert!(f.starts_with(&format!("{:04}_", i + 1)), "{f} is out of order");
    }
    let sql_steps = steps.iter().filter(|s| matches!(s.2, Step::Sql(_))).count();
    assert_eq!(files.len(), sql_steps, "every file in the migrations folder is a step, and every SQL step a file");
}
