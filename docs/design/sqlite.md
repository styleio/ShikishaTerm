# SQLite in this app — rules for tables that have to change safely

A database file on a person's machine outlives every version of the code that wrote it. The
rules below exist so that any version of the app can open a file an older version left behind,
bring it up to date without losing a row, and refuse a file a newer version wrote instead of
damaging it.

What every file shares -- how it is opened, the numbered steps, the rebuild, the generated
design file and the tests of its numbering -- is written once in `crates/core/src/sqlite.rs`.
A feature's store calls it; it does not carry a copy. The worked examples are the record of
handed work (`crates/core/src/orch/db.rs`, [orchestration-db.md](orchestration-db.md)) and
the record of conversations (`crates/core/src/convo/db.rs`,
[conversations-db.md](conversations-db.md)). Follow them rather than inventing another way.

SQLite is bundled with `rusqlite` (`features = ["bundled"]`), so the version is known and the
same on every machine: **3.53.2** today (see `libsqlite3-sys` in `Cargo.lock`). Anything
described here as needing a newer SQLite is available.

---

## 1. Files

- **One file per feature.** SQLite has one writer at a time *per file*. Separate files keep
  one feature's writes from waiting on another's, let each carry its own version and its own
  migrations, and keep a damaged or deleted file from taking other features with it. Put two
  things in one file only when they need a JOIN, a foreign key or one transaction across them.
- **Where it lives:** `config::state_path("<feature>.db")`, which is `data/` beside the app.
  Never in the repository, never in a project folder a person works in.
- **Name it for what it holds**, not for the code that uses it: `orchestration.db`, not `db1.db`
  or `store.db`.
- **No secrets in it.** Tokens and keys belong in the secrets store. A database file is copied
  into backups, into `.bak` files (below) and into bug reports.

## 2. Opening a connection

Every connection, every time, before anything else. `sqlite::open(path, STEPS, what)` does
all of it (and `sqlite::in_memory` for tests):

```rust
conn.pragma_update(None, "journal_mode", "WAL")?;     // readers do not block the writer
conn.pragma_update(None, "synchronous", "NORMAL")?;   // safe with WAL, much faster than FULL
conn.busy_timeout(Duration::from_secs(5))?;          // wait for a lock instead of failing
migrate(&mut conn, STEPS, Some(&backup))?;            // see 4
conn.pragma_update(None, "foreign_keys", "ON")?;      // off by default in SQLite, per connection
```

- `foreign_keys` is **off by default and per connection**. A connection that forgets it
  silently stops enforcing every `REFERENCES` in the file.
- Keep **one writer**: do the writes from one thread (the app's main loop, for both records
  above). A thread that only reads opens its own read-only connection, which WAL lets run
  beside the writer; it does not run the steps (the writer did), and refuses a file at a
  version it does not know. Transactions then guard against a program killed half way, not against a
  second writer.

## 3. Designing tables

| Topic | Rule | Why |
|---|---|---|
| Table names | plural, `snake_case`, a word a reader understands: `jobs`, `made_folders` | the file is read by people debugging, and by the design doc |
| Keys | `id INTEGER PRIMARY KEY`; add `AUTOINCREMENT` only when an id must never be reused (ids shown to people or AIs, like `t3`) | a reused id can make an old reference point at a new row |
| Foreign keys | `<thing>_id INTEGER NOT NULL REFERENCES <things>(id) ON DELETE CASCADE` when the row means nothing without its parent | deleting a job then deletes its tasks and mail in the same statement |
| Times | `INTEGER` milliseconds since the epoch, UTC, named `*_at` (`created_at`, `ended_at`) | sortable, comparable, no time zones or formats to parse |
| Booleans | `INTEGER` 0/1, named `is_*` (`is_read`), `NOT NULL DEFAULT 0` | SQLite has no boolean type |
| Tri-state | a nullable `INTEGER` with the meaning of NULL written in a comment (`seen_starting`: 1, 0, or NULL while handing) | better than inventing a second column |
| States | `TEXT NOT NULL`, **no `CHECK`**, the allowed values in a comment | see below |
| Lists and loose data | `TEXT` holding JSON, `NOT NULL DEFAULT '[]'` or `'{}'` | read with `json_extract` in SQL, `serde_json` in Rust |
| Comments | a `--` comment on every table and every non-obvious column, **inside** the `CREATE TABLE` | SQLite keeps that text in `sqlite_master`, so the comments reach the generated design file |
| Indexes | named after what they serve (`mail_unread`, `assignments_by_tab`); a partial unique index for "at most one of these" (`one_handover_given ... WHERE state = 'given'`) | the name says why it exists when someone wonders whether it can go |

**States belong to the code, not to a CHECK.** A `CHECK (state IN (...))` can only be changed
by rebuilding the table (section 5). Keep the allowed values and the allowed transitions in
one function instead — in the example, `Store::shift(table, id, from, to)`, which writes only
when the row is still in a state the change expects and returns a conflict otherwise. A new
state is then a line of code. Keep `CHECK` for rules that can never change (`depth >= 1`), and
even then prefer the code.

**Things to avoid**

- `SELECT *` in code. Name the columns: a column added later must not shift what `row.get(3)`
  reads. Keep the column list in one constant per table (`TASK_COLS`) next to its row reader.
- Storing the same fact twice (a count that could be computed, a name copied from another
  table). One of the copies will be forgotten in a migration.
- Relying on `rowid` of a table without `INTEGER PRIMARY KEY`, or on `sqlite_sequence`: a
  rebuild renumbers the first and may reset the second.
- A column whose meaning depends on another column's value. Two columns, or a JSON field.

## 4. Migrations

**The only way a table is made or changed is a numbered step.**

```
crates/core/src/<feature>/migrations/
  0001_first.sql
  0002_own_words.sql
```

```rust
pub const STEPS: &[(i64, &str, Step)] = &[
    (1, "first", Step::Sql(include_str!("migrations/0001_first.sql"))),
    (2, "own words", Step::Sql(include_str!("migrations/0002_own_words.sql"))),
];
```

- **Number = version.** Steps are numbered 1, 2, 3 ... with no gaps, and the files carry the
  same number, four digits. A test checks both.
- **A shipped step is never edited.** Not even its comments. A file that already ran it will
  not run it again, and from then on the code and the files disagree. Fix a mistake with the
  next step. "Shipped" means it reached `main`: an installed app may have run it.
- **`Step::Sql`** for plain changes; **`Step::Code(fn(&Transaction) -> Result<()>)`** when the
  change has to be worked out: a rebuild, rows reshaped in Rust, a value parsed.
- **One transaction per step**, and the version recorded in the same transaction. A program
  killed half way resumes at the step it was on; a failing step leaves nothing behind.
- **Foreign keys are off while steps run**, because a rebuild drops a table others point at
  and with them on SQLite would delete the rows pointing at it. After every step,
  `pragma_foreign_key_check` must find nothing; if it does, the step is refused.
- **Copy before changing.** When a file that already has tables is about to run a step, it is
  first copied beside itself with `VACUUM INTO '<name>.v<version>.bak'`. Going back is putting
  that file in place.
- **Refuse a newer file.** A file at a version this build does not know was written by a newer
  app. It is refused, never "fixed": this build cannot know what the newer one meant.
- **The first layout is a step too.** When a feature's first tables have shipped, they stay
  step 1 forever, word for word; renaming them later is step 2 carrying the rows across (see
  `0002_own_words.sql`), not a new step 1.

### What SQLite can and cannot change in place

| Change | How |
|---|---|
| New table, new index | `CREATE TABLE` / `CREATE INDEX` in a step |
| New column | `ALTER TABLE t ADD COLUMN c ...` — only if it is nullable or has a constant `DEFAULT`, is not `PRIMARY KEY` or `UNIQUE`, and (with `REFERENCES`) defaults to NULL |
| Rename a table or a column | `ALTER TABLE ... RENAME TO` / `RENAME COLUMN` — references in other tables, indexes and triggers follow |
| Drop a column | `ALTER TABLE ... DROP COLUMN` — only if nothing indexes it, references it or constrains it; otherwise a rebuild |
| Change a type, a constraint, a default, `NOT NULL`, a `CHECK`, a foreign key; reorder columns | **Rebuild** (section 5) |
| Rename a value stored in rows (a state, a mailbox prefix) | `UPDATE ... SET x = CASE x WHEN 'old' THEN 'new' ... END` in a step |
| Drop a table | `DROP TABLE` in a step, after its rows have gone where they are needed |

## 5. Rebuilding a table

SQLite's own procedure (<https://www.sqlite.org/lang_altertable.html>, "Making Other Kinds Of
Table Schema Changes"), written once as `rebuild(tx, table, create, columns, indexes)`:

1. create the new table beside the old one under another name;
2. `INSERT INTO new (cols) SELECT cols FROM old` — reshaping values here if needed;
3. `DROP TABLE old`;
4. `ALTER TABLE new RENAME TO old`;
5. create the table's indexes again (and triggers and views that used it).

Run it inside a `Step::Code`, where foreign keys are already off and are checked afterwards.
Never rename the old table out of the way first when others reference it: `RENAME` would
rewrite their `REFERENCES` to follow it. (`0002_own_words.sql` does rename two tables aside,
which is safe only because every table referencing them is dropped in the same step.)

## 6. Changing data safely

- Old rows must still read after a change. When a column is added, decide what old rows mean
  and write it as the `DEFAULT` or as an `UPDATE` in the same step.
- A column that stops being used is left in place (and documented as unused) until a later
  step removes it on purpose. Code must not need it to exist.
- JSON columns change shape too: when the fields inside change, the step rewrites old rows
  (`json_object(...)`, `json_extract(...)`) or the reader accepts both shapes, with a comment
  saying until when.
- Text in steps is compared after `\r\n` becomes `\n`: step files are checked out with CRLF on
  Windows, and SQLite stores statements as written.

## 7. Keeping the design visible

- `docs/design/<feature>-db.sql` is **written by a test** from a fresh file after every step
  (`sqlite_master`, tables first, then indexes, ordered by name). The test fails until it is
  written again:

  ```
  SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core <feature>_db
  ```

- `docs/design/<feature>-db.md` says what each table is for — one row is what, and why it is
  kept — and how to add a step. The code says how; this says why.
- Review a migration by reading the diff of the generated `.sql` file: it shows exactly what
  a fresh file will look like afterwards.

## 8. Tests every feature's database has

| Test | What it proves |
|---|---|
| a new file is made by the steps alone, and running them again changes nothing | the steps are the only source of the tables |
| an older file (with rows in every table) is copied, then brought up to date with its rows | a real upgrade loses nothing; the `.bak` holds the old version |
| a file the first shipped version wrote is carried into today's layout, row by row | every state, prefix and JSON shape was mapped |
| a step that fails leaves the file exactly as it was | the transaction per step works |
| a step that leaves rows pointing at nothing is refused | the foreign key check works |
| a file from a newer version is refused and left alone | no downgrade damage |
| steps and files are numbered 1, 2, 3 ... and match | nobody inserted or skipped a number |
| the generated design file matches the steps | the design doc is never stale |

## 9. Keeping files small

- Decide at design time how long rows are kept and delete them on open (the record of handed
  work forgets closed jobs after 30 days, with everything that was theirs through `ON DELETE
  CASCADE`).
- Do not `VACUUM` on every start: it rewrites the whole file. Deleting rows is enough; the
  space is reused.

## 10. Checklists

**Adding a table or a column**

1. Write `NNNN_<what>.sql` with the next number (comments inside the `CREATE TABLE`).
2. Add the line to `STEPS`.
3. Add the column to the table's column constant and row reader.
4. Write the generated design file again; read its diff.
5. Update `<feature>-db.md` if a table's purpose changed.
6. Run the tests in section 8.

**Changing a column's type or constraint** — the same, with a `Step::Code` that calls
`rebuild`, and a test that fills the old table with rows first.

**Adding a state** — no step. Add it to the transitions in `Store::shift`'s callers and to the
comment on the column. If the state replaces an old one, add a step that updates the rows.

**Removing something** — stop using it in one release; remove it with a step in a later one.
