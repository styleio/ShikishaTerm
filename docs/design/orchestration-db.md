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
| `tab_names` | what a tab is called, by its uid; kept after the tab closes | so what is said about a tab says its name, and a job's history still reads with it |

`meta` holds `schema`, the version the file is at, and `tab_uids` once the rows an older version
wrote under tabs' names have been rewritten under their uids.

**A tab is its uid.** `jobs.lead`, `assignments.tab`, `opened_tabs.tab` and a tab's own mailbox
(`tab:<uid>`) hold the tab's uid (`config::TabConfig::uid`), never its name: a name goes back in the
bag when its tab closes, and the next tab to draw it was handed the first one's jobs, its task and
its mail. Who sent mail and who made a decision keep the name they had then. On the first start
after this step, the rows written under names are rewritten once from the settings
(`Store::adopt_uids`): a name only one tab of the settings has goes to that tab, and any other -- a
closed tab, a name two desks share -- to a uid no tab has.

New files create the current tables directly in step **1**. Step **2** checks that baseline,
including when creation was interrupted after the first step. Step **3** adds `tab_names`.
Existing files at versions **2** and **3** retain their rows and version numbers. Earlier table
layouts are unsupported and are refused without changing their tables.

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
