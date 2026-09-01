# Working with git — design notes

> Status: agreed 2026-09-01. **Layer 1 (the primitives) is being built.**
> This file exists to record why each decision was made, and what was turned down.
> Japanese: [git-access.ja.md](git-access.ja.md).

## Background

It started as "the everyday things on screen, the advanced ones on the command line".

A git management GUI was turned down once (2026-08-28). Of the three reasons then, two
are still binding constraints and one has since been answered:

1. **`repo.rs` never launches git.** It reads `.git/HEAD` directly, so it can still
   answer while a rebase holds the lock. Launching git loses that → **still binding, see §6**
2. **It would put destructive buttons in a folder where an AI runs unattended** →
   **answered**. Automation permissions (a person / an AI, per command) now exist, and
   `git_*` is open to a person only by default
3. **Reinventing a general-purpose git client** → **avoided by scope**, see §3

## Why this app should build it at all

General-purpose git clients are a crowded market and there is no point joining it. Build
only what nothing else can:

> **The AI that did the work writes it up, and untangles it.**

The commit message is written by whoever wrote the change. A conflict is resolved by
whoever wrote one side of it — and lands **only after a person has looked at the diff**.
Other git clients can call an AI; none of them knows about *the AI in the next tab that
wrote this change*.

## The idea running through it

**Screen → Lua → git.** git work is built as Lua primitives, and the GUI is a thin layer
that calls them. No "git client mode" is built into Rust (the lego/primitives rule in
RULES).

Same shape as the database tab, chosen for the same reasons: automation can reach git,
the GUI is assembled from those parts, **an AI can reach git** (when allowed), and the
GUI itself can be automated.

---

## 1. Naming the repository — scripts do not build paths

The rule in `caps.rs` is that a script cannot assemble its own path or URL; it can only
call registered names. git follows the same rule, but **registering a gateway per
repository is too much ceremony** — working folders come and go, and every new workspace
would add one.

So use a name that already exists:

```lua
git_status(tab)      -- the repository the tab's working folder is in
git_status()         -- the tab that called
```

A tab is already something automation names (`TabRef`: number, display name, automation
id). **A repository is named by the tab sitting in it.** No raw path is accepted.

- A tab with no working folder, or a folder git does not track, answers with that
- Nothing outside the working folder is reachable: `-C <that folder>` is always passed

## 2. The layers — safe as long as you can drop to raw

```
Layer 1  read   git_status / git_diff / git_log / git_conflicts
Layer 1  write  git_stage / git_unstage / git_commit / git_checkout / git_branch / git_merge
Layer 1  raw    git_run(tab, "args…")        ← the floor; the sugar only assembles this
──────────────────────────────────────────────────────────────
Layer 2  screen a thin GUI (groups + rows + per-row actions + an input box)
```

**The test (same as the database's)**: the raw door is always next to the sugar, and
`git_commit` is only assembling it. While that holds, any amount of sugar is safe. The
moment "we have `git_commit`, so `git_run` can go" wins, the GUI becomes a ceiling.

**Not building**: a model of the repository (commit object types, a graph of refs, lazy
history). That sells a promise — "you never have to know git" — and the promise always
breaks on rebase, submodules, LFS and detached HEAD. When it breaks you need the raw
door anyway.

## 3. What goes on screen — shown, palette, or not registered at all

The SCM integration studied for this registers about **200 commands and puts a dozen or
so on screen**. The rest live in the command palette, and there is even a third tier:
registered but not shown by default. That split *is* "everyday on screen, advanced on the
command line", as implemented.

**Five things go on screen first**:

| Shown | Why |
|---|---|
| The list of changes (unstaged / staged / conflicted) | Beats reading `git status` in a terminal |
| Viewing a diff | Terminals lose here structurally |
| Stage / unstage | One click, once the list exists |
| Commit, with the message written by an AI (§5) | This is where the app's own value lands |

**Not shown**: pull, push, switching branches, stash, tag, rebase, cherry-pick, reset.
They exist as layer-1 primitives, callable from Lua and from the command line. **Whether
they earn a place on screen is decided after using it.** Ship them all at once and they
can never be taken away.

**Never shown**: `reset --hard`, force push, `clean -xdf`. These sit on the side where a
misclick cannot be walked back, and having to type them is the last confirmation.

## 4. Safety — offer another road rather than a wall

The best idea in the implementation studied: committing to a protected branch is not
refused, it is met with **"shall I make a new branch for this?"**. A ban sends people
around it; an alternative puts them on the right road.

- **`git_*` is open to a person only by default** in automation permissions. If an AI is
  to be let in, the reads (status / diff / log) are the place to start
- **`git_run` stays closed and off-screen.** Opening it is the same as handing an AI all
  of git, so it is one explicit row on the permissions screen
- **A protected branch** (default `main` / `master`) does not take a direct commit: the
  offer is to make a branch and commit there
- **Show what runs** (RULES). The git command line the GUI assembled is put somewhere a
  person can read, built by the same function that runs it

## 5. Written by an AI, untangled by an AI

**The commit message** — no new road. The path the automation editor already uses to have
Lua written (`ai_engine` / `suggest_with_local_ai`) is handed the diff and returns a
message. It shares that setting too.

**Resolving conflicts** — no "merge resolution mode" in Rust. It is written as a Lua
template, **reusing the rally** (handing a file over, and a judge):

```
git_conflicts(tab) lists the conflicted files
  → their contents go through exchange to the AI
  → what comes back is written down
  → a person reads the diff
  → git_stage
```

**One line holds: nothing an AI untangled is committed before a person has seen the
diff.** Proposing is the AI's job; deciding is the person's. Automate past that and the
day one side's changes quietly vanish, nobody can trace it.

## 6. Keep the line `repo.rs` drew

`repo.rs` and `pr.rs` **never launch git**; they read `.git/HEAD` directly. That is why the
sidebar can answer while a rebase holds `index.lock`. **That path stays exactly as it is.**

`git_*` does launch git — status and diff cannot be read correctly any other way. So the
app has two reading paths, which is not duplication but two different jobs:

| | Reads without launching (`repo.rs`) | Reads by launching (`git_*`) |
|---|---|---|
| For | What is always on screen (branch, PR, colour) | The list and the diffs, when opened |
| Always answers? | **Yes**, lock or no lock | Sometimes not, and that is fine |
| How often | Every tick | When a person opens it |

**Do not merge them.** Rebuilding the sidebar on `git_*` would make the branch name
disappear during a rebase: trading away a guarantee we already have, for tidiness.

## 7. Flat names (`git_status`, not `git.status`)

The database notes sketched `db.query(...)`. **Nested tables are not used.**

Being a **top-level function of the `shikisha` table** is load-bearing here:

- `primitive_names()` enumerates top-level functions only
- **Automation permissions** (`grants::CATALOG`) key on that name
- So does the external API's `method`, and the answer `list` gives

Nest it and it vanishes from all three at once — no row on the permissions screen, absent
from `list`, unreachable from the external API. **Write `git_status` and `db_query`, flat.**
The same goes for the database when its turn comes.

## 8. Turned down

### Read the repository ourselves instead of launching git
**No.** Reading `.git/HEAD` works *because* it is a branch name. Status is the index
format, diff is a diff algorithm, log is walking history — each one is reimplementing git.
**That does not raise the guarantee, it adds a path that is quietly wrong.**

### Register repositories as gateways, the way databases are
**No.** Database connections are few and long-lived; working folders come and go.
**Naming the tab that sits in one removes the registration step entirely.**

### Put everything on screen (pull, push, branches, stash, tags…)
**No** (§3). It can never be reduced again. The implementation studied shows a dozen of
its two hundred not because it lacks the rest, but because it decided to.

### Let an AI go from conflict to commit
**No** (§5). Proposing is the AI's; deciding is the person's.

### Put history editing (interactive rebase, cherry-pick, reset) in the GUI
**No.** The implementation studied keeps these out of the core too. **Having to type them
is the last confirmation.**

## 9. Still open

- Where the screen lives — a pane on the board, a fold in the sidebar, or the settings page
- How diffs are shown — whether staging by line is worth it (needs `git apply --cached`)
- Whether the protected-branch list should be a setting
- The way in to creating a worktree (a separate matter, not covered here)
- A ceiling on how much diff is handed to the message writer
