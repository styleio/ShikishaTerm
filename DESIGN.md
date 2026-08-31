# SHIKISHA-TERM — Design

> 🌐 **English** (this page) · [日本語](DESIGN.ja.md)

Concept: a portable, multi-session AI-orchestration desktop app — "PuTTY for high-powered AIs."

A portable Windows tool that monitors, drives and connects CLI agents (Claude Code,
Codex, etc.) and LLM APIs (KIMI, DeepSeek, Ollama, etc.) across several tabs, from one
cyberpunk-styled window.

> Note: this document tracks the current implementation. A few things moved since the
> original plan — the UI is rendered in WebView2 (not a terminal TUI), the local HTTP
> server is `tiny_http`, and the phone view pushes over a WebSocket. Those sections are
> written as built. The design rationale (state detection, profiles, the Lua sandbox,
> the security model, workspace portability) is unchanged.

---

## 0. Terminology

| Term | Meaning |
|---|---|
| **Session** | One child process (Claude Code / SSH / a shell, …) and its screen. One tab = one session |
| **Tab** | The unit that shows and drives a session. Listed vertically in the left bar |
| **Workspace** | The unit you switch between — like a virtual desktop, a set of tabs grouped together (e.g. ProjectX, Chores) |
| **Workspace definition file** | A workspace's contents (its tab definitions) externalized as JSON: `workspaces/*.json`. The unit you copy and share |
| **Profile** | Per-tool state-detection rules: `profiles/*.json` |
| **Hook** | A Lua script fired on a state transition: `scripts/*.lua` |
| **Chain depth** | A counter of how many times auto-send has chained. Reset to 0 by manual human input |

`workspaces` in `config.json` is the **index of workspaces**; each entry can either point
at its contents with `file` or inline them with `tabs` (the two are equivalent).

## 1. System overview

- Runs several AI sessions (CLI agents / API chats) in parallel, as tabs
- Moves data between tabs, builds automatic pipelines, and reshapes text with Lua
- Fully portable: unzip from Google Drive or a USB stick and run — no install
- A "hacker movie" visualization (CRT / neon)

## 2. Environment & distribution

| Item | Detail |
|---|---|
| OS | Windows 10 (1809+) / 11 (64-bit) — ConPTY is required |
| Form | A single executable `SHIKISHA-TERM.exe` plus config / script folders |
| Requirements | No install, no admin rights, no runtime dependencies |
| Portability | Uses only paths relative to the exe; all data stays inside the folder |
| Assumption | The CLI agents it wraps (Claude Code, etc.) are already installed on the machine — the same relationship PuTTY has with an SSH server |

## 3. Tech stack (Rust)

| Area | Choice | Notes |
|---|---|---|
| Language | Rust | A single, statically linked exe (~18 MB), no runtime deps |
| UI | wry + tao (WebView2) | The interface is HTML/JS rendered in an OS WebView2 window |
| Terminal parsing | vt100 | Parses each child's output into a screen buffer for display + detection |
| PTY | portable-pty (from WezTerm) | Windows ConPTY support |
| Scripting | mlua (Lua 5.4, vendored & statically linked) | Sandboxed execution |
| Local HTTP | tiny_http | The built-in settings server and the phone relay |
| HTTP client | ureq + rustls | For LLM APIs / notifications; no OpenSSL dependency |
| Config | serde / serde_json (`config.json`) | |
| Encryption | argon2 + aes-gcm | Protects API keys / tokens |
| Character width | unicode-width | Full-width (CJK) correctness |

Rationale: rather than write a terminal emulator from scratch, reuse the proven WezTerm
PTY crate and the `vt100` parser, and render the interface as HTML/JS in a WebView2 window
so the whole thing ships as one dependency-free exe.

## 4. Session architecture

**Design principle: vendor-neutral. Anything that runs in a terminal must work.**
It does not depend on any one tool's headless API (stream-json, etc.).

### 4.0 Connection settings (the PuTTY-equivalent)

Connections are made by launching the real SSH client (OpenSSH) directly. Ports, keys,
port-forwarding, jump hosts, agent forwarding — all of it can be expressed as command-line
arguments, and complex setups belong in `~/.ssh/config` (the equivalent of PuTTY's saved
sessions).

For beginners, a GUI offers a structured form (host, port, user, key file, port forwarding,
jump host, keep-alive). It generates a command string from the form, and parses an existing
command string back into the form — the command string is the single source of truth, and
power users can just write it.

The terminal-side settings (the part PuTTY keeps for itself) are held per tab:

| Setting | Key | Default |
|---|---|---|
| Working folder | `cwd` | The app's own location (relative paths are resolved from the config file) |
| Encoding | `encoding` | UTF-8 (`shift_jis` / `euc-jp` etc. can be set) |
| Scrollback lines | `scrollback` | 5000 |
| Session log | `log` | Off (when on: `logs/<tab>-<date>.log`) |

`cwd` matters because it decides which project the AI CLI sees. Pointing it at a folder that
does not exist does not fail the launch — it falls back to the app's location.

**Folders inside Docker / WSL** can't be set via `cwd` (which is Windows-side), so they go
on the command instead. The GUI provides input helpers:

| Kind | Command |
|---|---|
| Docker | `docker exec -it -w /app <container> bash` |
| WSL | `wsl -d Ubuntu --cd /home/me/proj -- bash` |

Not supported: serial / Telnet, `.ppk` keys (convert to OpenSSH format with `puttygen`),
and per-tab color/font overrides.

### 4.1 Terminal tabs (the core)

- `portable-pty` launches any CLI AI (claude / codex / gemini / aider / `ollama run` /
  some unknown new tool) and its terminal is shown and driven inside the tab
- State detection, response capture and automation are handled by the detection engine (4.2)

### 4.2 State-detection engine (the heart of the system)

It layers several independent signals and runs a state machine to decide a tab's state
(BUSY / DONE / QUESTION / WAIT / ERROR):

| Signal | What it is | Confidence |
|---|---|---|
| Screen patterns | Profile-defined regexes against the vt100 screen buffer (e.g. "esc to interrupt" = BUSY; a choice list like "❯ 1." = QUESTION) | High (depends on the rule) |
| Terminal control sequences | The bell (BEL), the window title (OSC 0/2 — the marks a CLI writes there while a turn runs, listed per profile as `title_busy`), alt-screen switches, cursor show/hide | Medium |
| Output-silence timer | No output for N seconds + the cursor at an input position → assume it's waiting | Medium (generic fallback) |
| The program's own word | The CLI's own hooks, reporting the turn it just started, the dialog it just opened, the turn it just finished (`--hook state:BUSY`, installed per CLI from Settings) | Certain, where a CLI has them |
| Process exit | The child's exit code | Certain |

The program's own word outranks the rest whenever it is there. It is not a reading of anything: the CLI
fires it the instant a turn begins or ends, in any language, under any theme, whether or not
it happened to draw a word this app knows — which is what removes the two-second silence
timer's habit of ending a turn that had only paused. What no CLI reports is the *answer* to a
permission dialog: approving, refusing and pressing Ctrl+C are things a person does to a CLI,
not things a CLI does. So the rising edge comes from the hook and the falling edge from the
screen, and nothing is allowed to depend on an event that never comes. A question found on
screen still outranks everything, because not every question a CLI asks is one it reports, and
a tab claiming to be busy while it waits for a person is the one mistake nobody goes back to
check.

The title sits between the two. Below the word, because anything running in that tab can write
a title while the hook is the CLI itself; above the screen, because a CLI that is thinking
draws nothing, and a screen that has not moved for two seconds was being read as a turn that
had ended — which handed the work on while the AI was still doing it. It can only ever say
that a turn *is* running: which kind of rest a tab is in — finished, at a prompt, waiting on a
person — is what a hand-over depends on, and no CLI puts that in its title.

Because this is heuristic (versus the "100% certain" of a headless JSON integration), the
whole thing is built so a misdetection can't cause an accident: the auto-run budget (7.5)
and a "when in doubt, stop and hand back to the human" default (§9) are assumed throughout.

### 4.3 Agent profiles (`./profiles/*.json`)

Per-tool detection and driving rules are declared externally, so a new tool works without
touching the code:

```jsonc
// profiles/claude.json (illustrative)
{
  "name": "Claude Code",
  "launch": "claude {args}",              // launch command is user-editable
  "busy_patterns":  ["esc to interrupt"],
  "question_patterns": ["Do you want", "❯ \\d+\\."],
  "done_signals":   ["bell", "silence:2s"],
  "answer": { "style": "number_enter" },  // how to answer a choice (number+Enter / arrows+Enter)
  "capture_cleanup": ["spinner_lines"],   // cleanup rules when capturing a response
  "detector_lua": null                    // complex logic can defer to Lua: detect(screen)
}
```

- Bundled profiles: claude / codex / gemini / aider / ollama (user-editable, carry on Drive)
- If a tool changes its UI and detection breaks, only the profile needs fixing
- An unknown tool with no profile still runs on the generic heuristic (silence timer + BEL + exit)
- Each tool's auto-approve flag (`--full-auto`, etc.) is just written into `launch` by the
  user; the app never needs to understand its meaning (consistent with vendor-neutrality)

### 4.4 Headless adapter (an optional accuracy upgrade)

The major CLIs have official headless modes that still use your subscription auth
(Claude Code `claude -p`, Codex `codex exec`, Gemini CLI). Only for tools whose profile
defines `headless`, a pipeline / auto-hook may run headless, making state detection and
response capture 100% accurate.

- Strictly optional. Unsupported tools always run on PTY + the detection engine (the
  generality principle is invariant)
- The app knows nothing of each vendor's format; the parse rules live in the profile
- Caution: if `ANTHROPIC_API_KEY` is set, Claude Code bills the API instead of the
  subscription (OAuth). The child process's environment is controlled explicitly to prevent that

### 4.5 API chat tabs (auxiliary)

There are also chat tabs that hit an OpenAI-compatible API directly (KIMI / DeepSeek /
`ollama serve`, …). Skipping the CLI makes state detection exact — good for lightweight
sub-sessions.

### 4.6 The Session abstraction

```
trait Session {
    fn send(&mut self, input: Input);
    fn events(&mut self) -> EventStream;  // StateChanged / Question / Done / Error ...
}
```

Implementations: `PtySession` (core) / `HeadlessSession` (optional) / `ApiChatSession` (auxiliary).

## 5. Layout & UI (cyber / hacker theme)

The interface is HTML/JS rendered in a WebView2 window — CRT-style neon (green / yellow /
blue / black), tabs down the left. It is not a terminal TUI; it's a real UI that happens to
look like one. The conceptual layout:

```
┌─────────────────┬────────────────────────────────────────────────────────┐
│ [≡] 0. INDEX    │  [ACTIVE SESSION MAP & HOST CONNECTIVITY]              │
├─────────────────┼────────────────────────────────────────────────────────┤
│ [●] 1. Claude   │  User: refactor this using tab 1's output              │
│ [●] 2. Codex    │  AI  : Got it. Tidying the structure...                │
│ [●] 3. Local-Q4 │                                                        │
│                 │  >>>[RESPONSE COMPLETE]                                │
├─────────────────┴────────────────────────────────────────────────────────┤
│ KERNEL ACCESS GRANTED... PORTABLE_MODE_ON...                    [READY]  │
└──────────────────────────────────────────────────────────────────────────┘
```

### Status indicators
- `0. INDEX`: the whole-system dashboard (pinned at the top)
- 🟡 BUSY: spinner (⠋⠙⠹) — receiving an AI response
- 🟢 DONE: response received / work complete
- 🔵 WAIT: waiting to hand off / waiting on a human decision (including when auto-answer held back)
- `A` mark: shown on tabs with autopilot (auto-YES) enabled

### Key routing
On a terminal tab, keystrokes pass through to the child process; app actions (switching tabs,
etc.) are separated behind a prefix key (tmux-style, e.g. `Ctrl+B`).

## 6. INDEX (dashboard)

- A list of every tab: model / role / status / autopilot state
- Reachability of connected hosts (registered APIs, local LLMs)
- The remaining auto-run budget
- Emergency-stop key: halts every tab's automation at once
- Opens the settings web GUI (the `e` key)

## 7. Pipelines & auto-hooks

### 7.1 Manual pipe (one-shot)
Immediate transfer via a prompt-line syntax:
- `@tab2 summarize this log` → send to tab 2
- `@tab1 | @tab2` → send to tab 1, then chain the result to tab 2

### 7.2 Auto-hooks (standing, event-driven rules)
`config.json` binds "a `scripts/*.lua` to tab N's events." On a tab's completion (DONE), it
can reshape → transfer → auto-run, unattended.

### 7.3 Capturing the response text (the pipeline's input)
A send-boundary marker scheme, so only the *latest* response is taken:

- On the BUSY transition (right after sending), the scrollback position is recorded as a marker
- On the DONE transition, only the lines after the marker are extracted and kept as `tab.output`
  (older responses and history are before the marker, so they can't leak in)
- The profile's `capture_cleanup` is applied (strip spinner lines, prompt echoes, …)
- A full-screen TUI (alt-screen) doesn't scroll, so a snapshot of the visible screen is used
  instead (UI frames can bleed in; correct with `string.match` on the Lua side, or use the
  headless adapter (4.4) when precision matters)
- `Ctrl+B c` copies the latest capture to the clipboard (for checking / manual use)
- With the headless adapter on: taken exactly from structured output
- API chat tabs: the API response is used directly (exact)

### 7.4 Startup automation (Expect)
Per-tab auto-input so that "just launching the app restores yesterday's working state."
Example: SSH login → cd into the working folder → `claude --resume` → pick the top session.

Tabs are defined in `config.json`, and `startup` lists "wait for the screen → send" steps:

```jsonc
{
  "tabs": [
    {
      "name": "dev-server",
      "command": "ssh root@example.com",
      "profile": "claude",          // manually pin the detection profile (for the AI on the far side)
      "startup": [
        { "wait_for": "\\$ $",            "send": "cd /srv/myproj\r" },
        { "wait_for": "\\$ $",            "send": "claude --resume\r" },
        { "wait_for": "Select a session", "send": "\r" }
      ]
    }
  ]
}
```

More complex branching (no `--resume` on the first run, etc.) goes in a Lua startup script:

```jsonc
{ "startup_lua": "scripts/resume_work.lua" }
```

```lua
function on_start(tab)
  if not shikisha.wait(tab, "\\$ $", 15000) then return end  -- ms; false on timeout
  shikisha.send(tab, "cd /srv/myproj\r")
  shikisha.wait(tab, "\\$ $", 5000)
  shikisha.send(tab, "claude --resume\r")
  if shikisha.wait(tab, "Select a session", 5000) then
    shikisha.send(tab, "\r")                                 -- pick the top session
  end
end
```

- `wait_for` is a regex against the detection engine's screen buffer
- Each step has a timeout (default 10 s). On overrun, auto-input aborts and hands off to a
  human as a blue WAIT (the same "when in doubt, stop" rule, so a misfire can't run away)
- During auto-input the tab shows a dedicated indicator, and any keystroke lets the human
  take over instantly
- Embedding a plaintext password is discouraged (prefer SSH key auth). If it's truly needed,
  reference it from the encrypted store (§10) — never in plaintext `config.json`

### 7.5 Runaway protection (the auto-run budget)
Auto-hook chains, auto-YES and API calls per unit time are all managed under one budget:
- Cycle detection at pipeline-definition time (a DAG is enforced) + a runtime max chain depth (default 10)
- A cap on consecutive auto-answers (default 10; over it, drop to blue WAIT and hand to a human)
- The emergency-stop key halts all automation at once
- The caps and defaults are changeable in `config.json` (at your own risk)

## 8. Lua scripting (the hook engine)

Lua is the one and only execution engine for hooks. Template mode (`{{ .TabA.Output }}`
etc.) is beginner sugar and expands to the equivalent Lua internally.

### 8.1 Sandbox (the capability-injection pattern)
- mlua loads no `os` / `io` / raw sockets at all
- A memory cap, an instruction-count hook and a timeout cut off infinite loops
- Only safe functions the Rust side implemented are injected. Arbitrary-URL traffic and file
  operations are impossible

### 8.2 Lua API
| API | What it does |
|---|---|
| `tab.output` / `tab.model` / `tab.name` | Info about the tab that fired the event |
| `shikisha.send_to_tab(tab, text)` | Send to another tab + run it. `tab` is a **tab name** or number (name preferred: reordering won't break it) |
| `shikisha.draft_to_tab(tab, text)` | Place `text` in the tab's input **without sending** — a person reviews and sends it. An AI CLI only; never a shell |
| `shikisha.notify(dest, text)` | Notify a registered Slack/Telegram (registered targets only) |
| `shikisha.log(text)` | Write to `logs/` |
| `shikisha.get_var(k)` / `set_var(k, v)` | Variables shared across hooks |
| `shikisha.wait(tab, pattern, timeout_ms)` | Wait until a regex appears on the screen (Expect) |
| `shikisha.send(tab, text)` | Send keystrokes to a tab |
| `shikisha.sleep(ms)` | Wait |

### 8.3 Hook events (driven by the detection engine's state transitions)

| Hook | Fires when | Example use |
|---|---|---|
| `on_start(tab)` | Right after a tab launches | resume automation (§7.4) |
| `on_question(tab, screen)` | On QUESTION | Auto-approve. Return a key string to send it, `nil` to hand to a human |
| `on_busy(tab)` | On BUSY (a response starts) | Start log, elapsed timer |
| `on_done(tab)` | On BUSY→DONE | Notify / reshape `tab.output` / transfer to another tab |
| `on_exit(tab, code)` | When the child exits | Auto-reconnect a dropped SSH, notify on abnormal exit |

There is no dedicated "periodic" hook; recurring work is expressed with a **loop inside a hook
+ `shikisha.sleep()`** (so the interval is the user's to choose, and the detection-tick
implementation isn't exposed). Use `shikisha.state(tab)` (the current value) as the loop's exit
condition. Pending coroutines are discarded on a tab's exit, restart, or an emergency stop.

**Naming**: the user-facing term is "**automation**." The word "Lua" appears only on the screen
where you actually write code (users needn't think about the language until they want to).

**File layout**: if the target is a folder, it's the "one file per event" scheme. The file
name *is* the event name, and the file contains **just the body** (`function ... end` is wrapped
by the Rust side). A `.lua` file target uses the classic function-definition style.

```
scripts/projectx/reviewer/
  ├── on_start.lua      when it launches
  ├── on_done.lua       when a response completes
  ├── on_question.lua   when it asks for confirmation
  ├── on_exit.lua       when it exits
  └── _shared.lua       shared helpers (read first, shared within the folder)
```

**Attachment levels**: automation can be bound at any of three levels.

| Level | Where it's written | Purpose |
|---|---|---|
| Tab | the tab definition's `"automation"` | Behavior specific to that tab (e.g. a reviewer) |
| Workspace | the workspace definition's `"automation"` | A pipeline specific to that project |
| Global | `config.json`'s `"automation"` | A fallback shared by all tabs |

The GUI auto-names the path by convention (`scripts/<workspace>/<tab>`) and **saves the result
into the config**. The user never thinks about the path, renaming won't break it, and pointing a
different tab at the same path shares the automation. The old key `"lua"` is still read.

Resolution: **the more specific wins** (Tab > Workspace > Global). If a tab-level script doesn't
define a given hook, it falls back to the level above — never both. This removes the need to
branch on `tab.index` and lets scripts be reused.

Execution model: hooks run as Lua coroutines, and `shikisha.wait()` doesn't block the UI — it
waits for its condition on the detection tick (looks synchronous, is actually async). There is one
Lua environment **per workspace**. Each script is read in its own namespace, so defining `on_done`
in several files doesn't clash, and shared variables (`get_var`/`set_var`) are shared within the
workspace (so an A⇔B loop's round-trip counter works).

Safety rules (all hooks):
- Auto-sends are counted against the auto-run budget (§7.5); over it, blue WAIT
- A human typing into a tab pauses its automation (so keystrokes don't cross); there's a resume key
- A `wait` timeout aborts the automation and hands off to a human

Pipeline-loop example — A (Claude implements) ⇔ B (Codex reviews) ⇔ C (a branch target):
```lua
function on_done(tab)
  local out = tab.output                    -- latest response only (the §7.3 marker scheme)
  if tab.index == 1 then                    -- A: the implementing tab
    local rounds = (shikisha.get_var("rounds") or 0)
    if out:match("LGTM") or rounds >= 5 then
      shikisha.notify("slack", "auto-loop done (" .. rounds .. " rounds)")
      return                                -- do nothing = stop the loop
    end
    shikisha.set_var("rounds", rounds + 1)
    shikisha.send_to_tab(2, "review this code:\n" .. out)
  elseif tab.index == 2 then                -- B: the review tab
    if out:match("NEEDS FIX") then
      shikisha.send_to_tab(1, "fix these points:\n" .. out)
    else
      shikisha.send_to_tab(3, "write docs for it:\n" .. out)  -- branch to C
    end
  end
end
```

### 8.5 File / network capabilities (off by default)
The sandbox exists to protect against **scripts the user did not write** — not against the user
themselves (AI-generated code, a shared workspace definition, an automation grabbed off the net).
So capabilities are **all disabled by default**, and only what's explicitly named in the config is
enabled. The GUI doesn't edit these (it's a power-user feature where a mistake is costly).

The scheme is "named gateways" (the same capability injection as notifications):

- `capabilities.files` … a name → folder mapping. `shikisha.write_file(gateway, name, string)`
- `capabilities.http`  … a name → URL + auth mapping. `shikisha.http(gateway, body)`
- A script can't assemble a path or a URL, and **can't see the credentials**
- Raw paths / raw URLs (`allow_dirs` / `allow_hosts`) exist for power users but default to empty

Invariants:
- Lua's `io` / `os` are never enabled (they have escape hatches like `io.popen`; only the Rust-side
  dedicated functions are injected)
- `config.json` / `secrets.json` / `.env` / `*.lua` / `*.enc` are always refused, even inside an
  allowed folder (prevents self-modification and credential exfiltration)
- Host matching is exact, `https` only. `user@host`-style URLs are refused
- Every file write / network call is recorded to `logs/hooks.log`

### 8.4 Notifications (Slack / Telegram)
- The real work is a Rust-side notifier. It only sends to a Slack Webhook / Telegram Bot API
  registered in `config.json`
- Without writing any Lua, a tab's "notify on done" checkbox turns it on (calling the same notifier
  internally)

## 9. Auto-YES (autopilot)

Target: when the AI presents a choice-style confirmation ("1: OK, 2: NG", etc.).

### Detection and answering (terminal tab, generic)
- The detection engine decides QUESTION from the profile's choice patterns
- The answer is sent as keystrokes per the profile's `answer.style` (number+Enter / arrows+Enter / y+Enter)
- On API chat tabs, a trailing choice pattern in the response (`1:` `2:` / `[Y/n]`, …) is detected and
  a configured canned phrase (default "Yes, please continue") is sent automatically

### Answer policy
1. Simple mode: auto-answer only when a positive choice (OK / Yes / continue) is unambiguous. If it
   can't tell, hand to a human as blue WAIT ("when in doubt, stop" is the default)
2. Lua mode: defer the decision to `on_question`. Return `nil` to hand to a human (e.g. a rule that
   routes only confirmations containing "delete" / "overwrite" to a person)

### Assistive means
- You can also write each tool's approve flag (`--full-auto`, `--permission-mode acceptEdits`, …) into
  the profile's launch command (the app doesn't know the flag's meaning — consistent with vendor-neutrality)
- The §7.5 auto-run budget applies in every mode

## 10. Security

### 10.1 API-key storage
- Standard: a master-password scheme (Argon2id derives the key → AES-GCM encrypts the key material).
  You enter the password at startup
- At your own risk: `"encryption": "none"` allows plaintext storage. Plaintext shows a warning at startup

### 10.2 The settings web server
- Binds `127.0.0.1` + a random port + a per-launch one-time token in the URL
  (`http://127.0.0.1:PORT/?token=...`)
- Blocks CSRF / DNS rebinding / another local process poking the settings API
- Binding to localhost also avoids the Windows Firewall pop-up

### 10.3 The GUI's shape
The top priority is that a beginner can do everything with just a mouse and keyboard.

**A sidebar + a detail pane** (the shape common to Apple System Settings / VS Code / Termius). The
list shows only "what exists"; editing focuses on the one thing you selected. Cramming buttons into
each row breaks down as features grow, so it isn't done.

```
┌ Global settings ─┬──────────────────────────┐
│ PROJECTX         │  Basic   display name / id     │
│  implement       │  Launch  kind / destination…   │
│  review          │  Automation  status per event  │
│  server          │  ▸ Advanced                    │
│  + add tab       │  Delete this tab               │
└──────────────────┴──────────────────────────┘
```

- Each list row is only **a name + the command (a subtitle to identify it)** — no buttons
- The display name and the id (its name in automation) are identity, so they sit **adjacent**
- Automation shows **each event's status (set / unset) as a list**, editable in place
- Low-frequency actions (reorder, re-parent, delete) live in the detail pane
- The palette is separate from the app's cyber look — a quiet, readable dark UI (accent color is
  reserved for the save button, etc.)
- **Start from a template** (one Claude / an implement↔review loop / an AI on the far side of SSH) —
  never from a blank page
- Automation is edited per event (on start / on done / …)
- **Generation from natural language**: run a local AI CLI (claude / codex / gemini) one-shot to write
  the Lua. No API key needed (it uses your subscription auth). Handing the AI `docs/AUTOMATION.md` as
  the spec makes it use the API correctly. The result is always shown and never saved until you approve it
- Registering notification targets (Slack Webhook / Telegram Bot)
- The GUI never edits JSON directly (power users edit the config files by hand)

### 10.4 Remote UI (monitor & instruct from a phone)
See tab status and send instructions from a phone or another PC's browser, over a private network
like Tailscale. "Carry on from yesterday" without RDP/VNC.

**It goes for "monitor + instruct," not a reproduction of the terminal screen.** Driving an 80-column
terminal on a phone isn't realistic; what you actually want is to check status and drop a one-line
instruction. It reuses the material already there (detection state, captured responses, screen text)
and serves it as JSON.

The screen is **pushed over a WebSocket** (`/ws-state`): the server sends the current state once on
connect, then pushes on change (screen updates are rate-limited to ~7 Hz so a burst of output can't
saturate a slow link; nothing is sent when idle). The client auto-reconnects and falls back to a slow
poll only while the socket is down. Browser (relay) tabs use the same discipline over `/ws`.

- The tab list shows state (working / done / waiting) by color; tap for detail
- Only while waiting does it show `1` `2` `Yes` `No` `Enter` answer buttons
- Automation can be stopped / resumed (you hold the emergency stop in your hand)
- Screen text drops trailing blank lines and is capped at the last ~200 lines

**Safety design** (stricter than the settings screen, since it can run arbitrary commands remotely):

| | |
|---|---|
| Default | **Off**. Listens only when `remote.enabled` is explicitly set |
| Bind | `auto` detects Tailscale → LAN, in that order. Anything but a private network is refused unless `allow_public` is set |
| Auth | A 32+ char token required (constant-time compare). Fixable via `remote_token` in `secrets.json`; generated per launch otherwise. **Rotating it cuts every connected session** |
| Path | On Tailscale it's already WireGuard-encrypted, so plain HTTP suffices. A LAN bind shows a warning |
| Display | While listening, the status bar shows `REMOTE:ON` at all times |
| Handling | Remote input is treated as **a human's action** (resets the auto-chain; a locked tab is refused) |

Connecting is done by QR code (no typing a URL and token by hand). It appears in the settings screen's
"Use from your phone" and via the TUI's `[i]`. The token is saved to `data\remote-token` and reused, so
a restart doesn't force you to re-pair.

**It's pointless if people can't use it**, so enabling is a single checkbox in the settings; saving it
starts the listener (no restart). Tailscale isn't required — the screen states plainly that it works on
the LAN without it, and what to watch out for then (explained, not forbidden; only direct exposure to
the internet requires an explicit setting in the config file).

## 10.5 Config hot-reload
Don't make the user restart on every change. The modified times of the config files, workspace
definitions, automation scripts and secrets are checked once a second, and a change triggers a reload.

| Change | Effect |
|---|---|
| Lock / auto-restart / profile / hierarchy / tab name | Immediate |
| Add / remove a tab | Immediate (add = launch, remove = terminate) |
| Automation script / notify target / capabilities / chain cap / tab-bar width | Immediate |
| Command / encoding / scrollback lines | **Deferred**. The tab shows a "⟳"; apply with `Ctrl+B r` |

Command changes don't auto-restart so a running AI session isn't cut without warning. The new settings
are held, and the tab is rebuilt at a time the user chooses.

## 10.6 Exporting / importing a workspace

A workspace isn't just a tab order. Its tabs point at automation scripts that live elsewhere, and
without those it won't run on the machine you handed it to. So the settings and the scripts go into one
file (`*.stws.json`).

```json
{
  "shikisha_workspace": 1,
  "workspace": { "name": "...", "automation": "...", "tabs": [...], "browsers": [...] },
  "roots":   ["scripts/ws1"],
  "scripts": { "scripts/ws1/on_start.lua": "...", "scripts/ws1/html/on_load.lua": "..." }
}
```

- `roots` is the **unit that gets re-pasted**. Folders inside move with their parent, so they aren't part of the unit
- A workspace split into a separate file (`file`) is expanded in place, so the one file is self-contained
- The subject is **the saved settings**. Exporting the mid-edit state produces settings only the recipient has

Rules on the import side:

| Situation | Behavior |
|---|---|
| The slot is taken | `scripts/ws1` → `scripts/ws1-2`; the reference is rewritten too |
| The display name exists | `editors` → `editors-2` |
| Points outside the config folder / non-Lua / overwrites an existing file | **Refuse**. If any one item is bad, nothing is written |
| An unknown version / format | Refuse (never a half import) |

What doesn't come in: notification targets, `secrets.json`, capabilities. Those are global settings, not
a workspace's belongings, and it would mean handing credentials around. Commands and working folders come
in as-is, so check the contents before you hand it over.

## 11. Portable operation & Google-Drive conflict handling

- Writes use "temp file → atomic rename"
- Logs are per-session append-only JSONL (`logs/`) — no merge needed even on a sync conflict
- A lock file at startup detects a second instance
- Every path is relative to the exe; config / logs / scripts stay inside the folder

## 12. Multibyte (CJK) support

- Full-width sizing via `unicode-width`; box-drawing layouts are tested with CJK content
- config / logs / scripts are all UTF-8
- Japanese IME input is documented as best on Windows Terminal (bare conhost has quirks)

## 13. Directory layout

```
[SHIKISHA-TERM Directory]
 ├── SHIKISHA-TERM.exe   # the app (single binary)
 ├── config.json            # global settings + the workspace index (relative paths)
 ├── secrets.json           # credentials (encryptable; never share)
 ├── workspaces/            # workspace definition files (shippable per project)
 │     ├── projectx.json
 │     └── chores.json
 ├── profiles/              # agent profiles (detection rules)
 │     ├── claude.json
 │     ├── codex.json
 │     └── gemini.json
 ├── scripts/               # user-written Lua hooks
 │     └── hooks.lua
 └── logs/                  # conversation history / hook logs
```

## 14. Development phases

| Phase | Content | Main risk checked |
|---|---|---|
| 1 | PTY spike: show and drive Claude Code in a single tab; verify CJK width | Rendering quality (kill the biggest risk first) |
| 2 | State-detection engine + profiles (claude / codex / gemini) | Detection accuracy, silence-timer tuning (the second risk) |
| 3 | Multi-tab + INDEX + status indicators | Key routing |
| 4 | Pipelines, response capture, Lua hooks, notifications, auto-YES, startup automation (Expect) | Capture quality, budget control |
| 5 | Headless adapter (optional), settings GUI, encryption, polish | Distribution & signing |

## 15. Known caveats

- An unsigned exe is easily flagged by SmartScreen / antivirus. Consider code signing for distribution
- A wrapped CLI's version bump can change its screen output / format, so detection rules live in the
  profile (an external file), decoupling fixes from a binary update
- If `ANTHROPIC_API_KEY` is set, Claude Code bills the API instead of the subscription (OAuth) — there
  are reports of large bills. When using the headless adapter, control the child's environment explicitly
