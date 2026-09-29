# The record of handed work (orchestration.db)

> Japanese version: [orchestration-db.ja.md](orchestration-db.ja.md).
> The rules every SQLite file in this app follows: [sqlite.md](sqlite.md).
> The tables as they are now: [orchestration-db.sql](orchestration-db.sql) (written by a test; do not edit).

When one AI tab hands parts of a job to other AI tabs (`shikisha job_open`, `assign`,
`report`, `inbox` ... -- see "Orchestration" in [AUTOMATION.md](../AUTOMATION.md)), the app
keeps what was handed to whom, what came back, and what is still open in a SQLite file:
`orchestration.db` in the app's state folder. The code is `crates/core/src/orch/db.rs`.

## What each table is for

| Table | One row is | Why it is written down |
|---|---|---|
| `jobs` | a job a person asked one AI tab (its lead) to see through | so the lead, cut off or summarised, can ask where it stands (`job_status`) |
| `tasks` | one piece of a job, and the pieces it waits on | so a piece opens only once what it waits on is done |
| `assignments` | one try at one task by one tab | so a report is taken only from the tab, and the run of its program, that was given the work; a second try has a row of its own |
| `opened_tabs` | a tab a job opened for itself | so the job closes only tabs it opened, and never one a person has typed into |
| `made_folders` | a working folder (git worktree) a job made | so a folder nobody works in is raised |
| `mail` | one thing said between a lead and its workers | so nothing said is lost while the reader is busy |
| `handovers` | mail handed to a reader, until it says it has dealt with it | so a reader cut off half way gets the same mail again |
| `questions` | a worker waiting on its lead | so the answer reaches the worker whether it is still waiting or not |
| `decisions` | a task held until the lead or the person decides | so a merge to main, say, waits for the person |
| `raised` | something the watcher has already said | so each is said once |

`meta` holds one row, `schema`: the version the file is at.

The steps so far: **1** is the layout the first version shipped with, in the words it used then
(runs, dispatches, messages, gates); **2** carries a file written by it, rows and all, into the
words used now. A new file runs both.

States (`state`, `held_by`, `afterwards` ...) are plain text. There is no CHECK on them in the
tables: which state may follow which is decided in one place in the code, `Store::shift`, which
writes only if the row is still in a state the change expects. A new state is then a line of
code, not a rebuilt table.

## Changing the tables

The tables are made and changed by numbered steps, never by editing what shipped.

1. **Add a step.** A plain change is a file `crates/core/src/orch/migrations/NNNN_what.sql`
   (the next number, four digits) and a line at the end of `STEPS` in `db.rs`:

   ```rust
   (3, "what", Step::Sql(include_str!("migrations/0003_what.sql"))),
   ```

   A change SQLite cannot make in place -- a column's type or constraint, a column dropped or
   moved -- is `Step::Code` with a function that calls `rebuild(...)`, which does it the way
   SQLite's documentation says (a new table beside the old, the rows copied, the old dropped,
   the new renamed, its indexes made again). A change to the rows themselves is `Step::Code`
   too.
2. **Never edit a step that has shipped.** A file that has already run it would not run it
   again, and the two would disagree for ever.
3. **Write the design again**, and look at what changed:

   ```
   SHIKISHA_WRITE_DOCS=1 cargo test -p shikisha-core orchestration_db
   ```

   Until then `the_orchestration_db_design_is_what_the_steps_make` fails.
4. **Test the step on a record with rows in it**, as
   `an_older_record_is_copied_then_brought_up_to_date_with_its_rows` does.

## What happens on a person's machine

- Each step runs in a transaction of its own and is recorded as it completes. A program killed
  half way starts again at the step it was on; a step that fails leaves nothing of itself.
- Before a file that already has tables is changed, it is copied beside itself as
  `orchestration.db.v<version>.bak` (`VACUUM INTO`). Going back is putting that file in its place.
- Foreign keys are off while the steps run (a rebuild drops a table others point at) and are
  checked after every step: a step that leaves rows pointing at nothing is refused.
- A file written by a newer version of the app is refused, not changed: an older app does not
  know what the newer one meant by it.
- Closed jobs are forgotten 30 days after they closed, with everything that was theirs.
