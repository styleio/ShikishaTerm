# Database access — design notes (WIP, before implementation)

> Status: agreed as of 2026-08-28. **Nothing here is implemented yet.** This is the record of a
> design discussion held before starting, not a description of the current code. It exists to
> say *why* each decision was made and *what was rejected*.
> Japanese version: [db-access.ja.md](db-access.ja.md).

## Background

This started from one request: a tab where you can see the contents of a database, add to it,
and edit it, in a GUI.

A terminal cannot show a table. Everyone knows the pain of `SELECT * FROM users` wrapping into
ruin at eighty columns. This is a place where a GUI structurally wins, and "you can already type
it in the terminal" is not an answer.

## Why this app should build one at all

General-purpose database clients are a saturated market and there is no point entering it.
**There is exactly one thing only this app can offer, and it is the whole reason to build:**

> **The AI can touch the database.**

An AI running unattended can be asked, in conversation, to seed test data or to go look at what
is in a table. This app is the only thing that knows which tab and which AI changed what. No
other database client can do that, in principle.

## The idea running through it

**Screen → Lua → database.** Database access is built as Lua primitives, and the management tab
is a thin GUI that calls them. No "database client mode" is built into Rust.

What that buys:

1. Automation scripts can reach the database
2. The management tab is composed from those primitives
3. **The AI can reach the database** (inside `run_scoped`)
4. Users can automate the management tab itself in Lua

What we build becomes a Lego brick in the user's hands. Far from breaking the primitives rule
(RULES, "設計の約束"), this is that rule applied plainly.

---

## 1. Connections ride the named-gateway scheme

This drops straight onto what `caps.rs` already does: nothing allowed by default, scripts cannot
assemble paths or URLs, they can only call registered names.

```lua
db.query("maindb", "SELECT * FROM users WHERE id = ?", { 42 })
```

The first argument is always **a name registered in the config**. No connection string and no
password ever reaches Lua, which makes it structurally impossible for AI-written Lua to
exfiltrate credentials. This is the same shape as `caps.http(name, body)` and
`caps.browser_click(name, sel)`.

The place to keep credentials already exists too (`secrets.json`, encryptable with Argon2id +
AES-256-GCM). **No new safety machinery has to be invented.**

The path where AI writes a `.lua` file and launders it through a human-triggered run is already
closed: `caps.rs`'s `is_forbidden` refuses writes to `.lua` and `.enc`, with the reason spelled
out in a comment ("prevents self-modification").

### Config shape

```jsonc
"db": {
  "maindb":  { "kind": "mysql",  "readonly": true,  "raw_write": "off"   },
  "scratch": { "kind": "sqlite", "readonly": false, "raw_write": "human" },
  "cache":   { "kind": "valkey", "readonly": true }
}
```

- `readonly` — defaults to **true**
- `raw_write` — whether raw SQL may **write**. `off` / `human` / `all`, defaulting to **off**.
  The `human` / `all` distinction is decided by `tab.is_model` (an existing field)
- **Raw SQL reads are always open** — §4 explains why that stays safe

The trade-off between convenience and safety is chosen **per connection**, by a person. There is
no need to pick one point for the whole app.

---

## 2. Layers — build the core, never the ORM

```
Layer 1  db.query(name, sql, params) / db.exec(name, sql, params)   raw; the foundation
Layer 2  db.tables(name) / db.columns(name, "users")                dialect差 lives only here
Layer 3  db.insert / db.update / db.delete / db.clause              sugar
──────────────────────────────────────────────────────────────────
Screen   a thin GUI that calls the above (one page in webui.rs)
```

**Layer 1 alone is enough to build the management tab.** Layer 3 is optional. Build layer 1
first and add layer 3 only when it is genuinely wanted — designing the sugar first bends the
foundation to fit it.

### The test

> **Safe as long as you can drop down to raw SQL. A landmine the moment you cannot.**

`db.query` is always there beside the sugar, and `db.insert{...}` is only a way of assembling
what it would run. While that holds, any amount of sugar is harmless. The moment someone says
"we have `db.insert`, so we don't need `db.query`", it has become an ORM.

### Explicitly not built

Model classes, relationship definitions, lazy loading, change tracking, migrations.

Those sell a *promise* — "you don't need to know which database this is" — and that promise
always breaks on joins, types, transactions and non-relational stores. When it breaks, raw SQL
is needed anyway, and **both the translation layer and the raw door have to be maintained.**

---

## 3. Writing `where` — do not invent operators

A `where` that only does equality is useless: not being able to write `created_at < ?`, LIKE, OR
or ranges is out of the question. But inventing an operator vocabulary grows without end.

**The answer: leave operators to SQL, and let the builder handle only the mechanical part.**

Three kinds of condition mix in one table:

```lua
db.clause({
  id = 12,                          -- column = value  → `id` = ?
  status = db.list({ 3, 4 }),       -- a list          → `status` IN (?, ?)
  { "created_at < ?", ts },         -- fragment + bound value
  { "name like ?", "%" .. q .. "%" },
})                                  -- AND by default; { operator = "OR" } for OR
```

That covers `created_at < ?`, LIKE, OR, BETWEEN and IN. **The expressive power is SQL's own.**

### Binding

- `?` binds a **value**
- `??` binds an **identifier** (table or column name) and quotes it for the dialect

There are effectively only three dialect differences here: identifier quoting
(`` `x` `` / `"x"` / `[x]`), how LIMIT is written, and whether RETURNING exists. Between MySQL
and SQLite the gap is very small.

### Helpers

`db.list{...}` / `db.raw("count + 1")` / `db.NULL`

### Raw fragments and SQL injection

Writing `{ "name like '%" .. user_input .. "%'" }` is an injection. That is **an unavoidable
property of having an escape hatch at all** — every builder of this kind has it.

The correct form is `{ "name like ?", "%" .. user_input .. "%" }` — **the wildcards belong in the
parameter, not in the SQL.**

Lua has no tagged templates (where interpolation automatically becomes a bound parameter), so
the language cannot close this for us. Therefore:

- **Do not make it the primary defence.** Banning quotes, or linting, is a *guess*, not a *ruling*
- As a **supplement**, warn at load time (the source is already compiled with `load()` for
  validation) when `..` appears inside a `db.*` argument
- **The real defence is §4.** Even a successful injection cannot exceed what the gateway allows

---

## 4. Where the guarantee lives (the core of this design)

### What has to hold

1. **No DDL or DML can ever get through the read door** — whatever string the user writes,
   however badly. It must be foolproof
2. **Ordinary conditions must be expressible.** Complex aggregates and multi-way joins may be
   given up, but not being able to write `created_at < ?`, LIKE, OR or ranges is out of the
   question

These two are compatible.

### The guarantee does not live in the engine

Every engine has some mechanism for enforcing read-only. But **a list of "A has this, B has
that" is not a design — it is a feature list.** Built that way:

- every new database can **silently** lower the guarantee
- when an upstream release changes behaviour, **nobody notices**
- there is no one sentence that states what the guarantee even is

### The guarantee lives in something we own

**The driver trait.**

```rust
trait Db {
    /// Open a connection that cannot write. The engine must enforce it.
    /// A driver that cannot implement this cannot be registered
    fn open_readonly(&self, cfg: &Cfg) -> Result<Conn>;
    fn open_readwrite(&self, cfg: &Cfg) -> Result<Conn>;
}
```

**`db.query` always runs on a connection from `open_readonly()`.**

This turns "remember to handle read-only when adding a database" from something a reviewer has
to catch into something that will not compile.

Each engine's mechanism is an **implementation of the contract, not the contract**:

| Database | Read-only enforcement | Statement/command classification |
|---|---|---|
| SQLite | opened read-only (a separate handle) | ask the engine whether a prepared statement writes; plus an authorization callback |
| MySQL | account privileges | column count in the prepare response (zero = returns no result set) |
| PostgreSQL | read-only transaction / dedicated role | (settled when the driver is written) |
| Valkey | ACL (`+@read -@write -@admin -@dangerous`, and `~key:*` narrows the key space) | the command's own readonly flag |

In every case **the engine does the classifying, not a regex of ours.** That is what makes
"no string can get through" true rather than hopeful.

### Only tests catch an upstream change

Not reasoning, not documentation: a **conformance suite** that every driver must pass, run
**against real database instances in CI.**

SQL family:

```
read door + "DROP TABLE t"                 → error; t still exists
read door + "INSERT INTO t ..."            → error; row count unchanged
read door + "UPDATE t SET ..."             → error
read door + "SELECT 1; DROP TABLE t"       → error (multi-statement never enabled)
read door + "CALL p()" (which writes)      → error
read door + a SELECT that writes a file    → error
read door + a complex SELECT (join/agg/CTE)→ succeeds
```

Key-value family:

```
read door + flush-everything      → error; key count unchanged
read door + delete / set          → error
read door + a script that writes  → error
read door + a read-only script    → succeeds
```

**When upstream changes, the suite goes red.** Only then does "can you guarantee it?" have a
checkable answer: yes, because these N tests are green for every driver.

### A cheap token check, backing up the engine

MySQL's column-count test is defeated by `CALL some_procedure()` — it returns a result set and
can write inside. So a **cheap check that the first token is `SELECT` or `WITH`** is layered on
top.

A token check is fragile on its own, but here it is **a supplement, not the primary defence**, so
its fragility does not matter. Conversely the engine check alone lets `CALL` through.
**Two independent checks** is the answer.

### Why this puts no limit on `where`

The read door gives up **none** of SQL's reading power. Joins, GROUP BY, CTEs, subqueries all
work. What is restricted is *writing*, not *how you write*.

That property is why requirement 1 and requirement 2 are compatible.

---

## 5. Preventing unintended mass updates and deletes

Checking for the presence of a `WHERE` is not the approach. `WHERE 1=1`, `WHERE id > 0` and
`WHERE id REGEXP '^[0-9]+$'` all match everything, and "does this condition mean all rows" is
undecidable in general. **Any syntactic check is a speed bump, not a wall** — requiring a WHERE
is properly called a *typo guard*, not a security control. It still earns its place, because
most accidents are typos.

**Use a database-independent method instead:**

```
BEGIN
  run the UPDATE / DELETE
  → read the affected row count
  too many / not confirmed → ROLLBACK
  confirmed                → COMMIT
```

- Behaves identically on MySQL and SQLite
- Counts **rows actually touched**, rather than guessing from a query plan
- Through the builder, the same condition can be sent as `SELECT COUNT(*)` first, so the
  confirmation appears **before** anything runs

Make it a **required trait method**. A driver that cannot offer transactions cannot be registered
as a writable connection (for instance, a deployment that does not support them is automatically
read-only — the type refuses, rather than a person remembering).

**A cheap insurance policy for SQLite:** it is one file, so a writable SQLite gateway can copy it
just before the session's first write. Nearly free, and it recovers from an accident that got
past every other defence. Having one "you can undo it" beats trying to make it unbreakable.

---

## 6. Non-SQL databases (Valkey and friends)

**Supported — but not on `db.*`.**

Valkey is not SQL; there are no tables, rows or columns. Forcing `db.query(name, sql)` over it
leaks on day one. It gets **its own primitive** (the same call applies to MongoDB).

```lua
kv.scan("cache", "user:*")                     -- keys
kv.type("cache", "user:42")                    -- string / hash / list / zset ...
kv.get("cache", "user:42")                     -- the value, per type
kv.ttl("cache", "user:42")
kv.command("cache", { "HGETALL", "user:42" })  -- raw command (read-only ones only)
```

**The contract (`trait Db` plus the conformance suite) is shared; only the surface differs.** One
more Lego brick, and none of the existing bricks bend.

Valkey satisfies the trait **more cleanly than MySQL does**: the ACL can be issued by the app
itself and can even narrow the key space, so it does not depend on whether the user remembered
to create a read-only account.

### The GUI skeleton carries over

| SQL tab | KV tab |
|---|---|
| table list | key list (a tree on `:`) |
| grid of rows | key + type + TTL table |
| cell value | per-type view (hash → table, list → table, zset → table with scores) |
| SQL box | command box |

Same skeleton — a list, one item's detail, and a free-form box — so the screen is reused.

---

## 7. Scope policy

> **The supported databases are chosen, not extensible.**

Only what implements `trait Db` and turns the whole conformance suite green gets in. There is no
"write a config and it connects to anything". **What the guarantee can cover is what is
supported.**

Order:

1. **SQLite** — no server, no auth, one file; read-only enforcement is the strictest here.
   Windows does not ship a SQLite CLI, so approaches that shell out cannot work
   (which is exactly why embedding pays here)
2. **MySQL** — a synchronous pure-Rust driver exists, so no async runtime is dragged in
3. **Valkey** — once the SQL side has settled, as `kv.*`
4. **PostgreSQL** — when its conformance run is green

The answer to "which databases are supported?" is written down as "MySQL / SQLite" from day one.
That is a support cost, not an implementation cost, and stating it plainly settles it.

---

## 8. Rejected, and why

### Embedding a general-purpose database client in Rust (the first proposal)

**Rejected.** Two of the original objections were **wrong and have been withdrawn**:

- "it drags in an async runtime" — wrong. Both SQLite and MySQL have **synchronous** pure-Rust
  drivers; no async runtime is needed, and even if one arrived it would stay behind a sync API
- "per-database maintenance grows forever" — with a fixed scope it is realistic. The real size is
  a few hundred lines plus one page

The **one surviving objection** was that it breaks the primitives rule by adding a built-out mode
in Rust — and **that was dissolved** by the screen → Lua → database structure. One primitive plus
a thin GUI does not break the rule.

### Pointing a browser tab at an existing web-based database admin tool

**Rejected.** The premise that "developers already run one of these" does not hold. Where the
application runs on a server and only editing happens locally, that runtime is not on the local
machine. For a different language paired with a different database, this amounts to telling
someone to install a runtime they do not use.

Only embedding satisfies "inside a tab" and "no new runtime" at the same time.

### Shelling out to each database's own CLI and rendering the TSV

**Rejected.** It avoided implementing drivers and left dialects to the CLI, but:

- **it does not work for SQLite** — Windows ships no SQLite CLI, so a machine using SQLite
  through another language does not have one
- TSV parsing has mines in it (BLOBs, NULL versus empty string, newlines inside values)

### Requiring a `WHERE`

**Rejected** (see §5). It is a typo guard, not a security control, and the affected-row-count
method replaces it.

### Making quote-banning or static analysis the primary defence against injection

**Rejected** (see §3). A guess, not a ruling. Useful only as a supplement.

### Letting "the engine's own feature" be the guarantee

**Rejected** (see §4). Every new database would silently lower it, and nobody would notice an
upstream change.

---

## 9. Open questions

- Whether to build layer 3 up front or after layer 1 settles → **the latter is recommended**
- Whether to allow editing directly in the grid, and if so how the affected-row confirmation is
  presented
- Whether connections can be added from the settings screen
  (`caps.rs` holds an existing position: not editable from the GUI, because a mistake there has a
  large blast radius)
- Whether a file-writing SELECT is caught by the prepare-response column count — confirm on a
  real server
- The concrete form of PostgreSQL's read-only enforcement, settled when that driver is written

---

## Notes

- Whatever SQL actually runs must be shown somewhere a person can read it (RULES,
  「走るものは見せる」). The display side must call the same assembly function the execution uses,
  never imitate it
- The settings screen says "create a read-only user and connect with it" as an instruction.
  It does not say "recommended" or "optional" (RULES and existing practice)
