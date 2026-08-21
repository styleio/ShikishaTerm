# The convenience bar (a summoned sub-input bar) — design note

> Status: the record of the design discussion as it stood on 2026-08-20, before
> the work started. Most of it has since shipped as the composer; this file is
> kept as the reasoning behind those decisions, not as a description of the
> current code. The Japanese original is [convenience-bar.ja.md](convenience-bar.ja.md).

## Background and goal

Today the sub-input bar is mobile-only. It was born to work around the phone
keyboard covering the input line, and `castMode` switches its destination
between "browser relay" and "terminal". The goal is to **make it usable on the
desktop too, and to hang convenience features on it** — attachments, canned
text and macros, and driving another tab.

## The design ideas that run through all of it

- **Primitives × a thin GUI × dispatch by kind.** Don't build "modes" into the
  Rust side; lay a thin GUI over the primitives that already exist
  (`send_to_tab` / `browser_fill` / `browser_press` …). The only things that
  grow are the GUI and the primitives.
- **No merged functions.** Add independent primitives and let Lua compose them.
- When in doubt, look at **the reality at run time**, not at the declaration
  (a value saved in the settings).

---

## 0. The sub-input bar on the desktop — summoned, not permanent

**Decision: on the desktop it is summoned with a toggle / hotkey. It is not
permanent. Typing straight into the terminal stays the default.** On mobile it
stays permanent as before (the keyboard-covers-the-line problem justifies it).

Why a permanent desktop bar is a bad trade:

- It doubles the input path. On the desktop, typing straight into the terminal
  is the fastest and the most reliable. A permanent bar creates the "which one
  do I type into?" hesitation, plus a bar→PTY injection round trip (latency and
  drift). Injecting a pre-formatted block into a TUI (vim / less / a pager)
  breaks it.
- It eats vertical space. Unlike mobile, on the desktop that is pure cost.
- Focus and key conflicts (the Ctrl+B prefix, the mouse, the IME) — the same
  family of fight as the `if REMOTE return` (focus) case on mobile.
- Building it up on the Rust side would turn the bar itself into a "mode",
  against the design idea.

→ The bar is not the star; it is **a container for convenience features**.
Summon it, and the virtues of typing directly are preserved.

---

## 1. File attachments

### The premise (agreed)

A terminal CLI cannot "attach" the way a GUI chat does. The real idiom is to
**put the file somewhere and hand the AI its path (or `@path`)** — Claude
Code's `@file` and image-path reading, Aider's file arguments, and so on. So
"paste → save to a temp location → put the absolute path in the input line" is
the royal road, and it is right.

### Behaviour

Paste / drag / the attach button → save → insert a path the target can read
into the input line.

### Where it is saved

- **The rule: under the target tab's working directory (cwd),
  `<cwd>/.SHIKISHA/tmp/`.** Under the cwd the AI can reliably read it; from a
  central folder an AI's sandbox may refuse to read outside its cwd. That
  trade-off is what picks this.
- **Don't leave gitignore to the repository:** always drop a
  `.SHIKISHA/.gitignore` containing just `*` inside the `.SHIKISHA/` we create.
  Then everything under it is ignored whether or not the parent is a git repo
  (self-contained).
- Note: the cwd is arbitrary per tab. It is *not* "the folder SHIKISHA runs
  from".

### File names — we generate them, always

- **Don't sanitize the user's name; generate a random one on our side.**
  Sanitizing is a game of whack-a-mole; generating is the safe side. Multibyte
  names, collisions, and path traversal all disappear at once.
- The name can be **plain random** (there is no dedup use case, so no content
  hash is needed — which avoids reading big files end to end).
- The extension follows the sniffing below. If readability is wanted, prefixing
  a defanged version of the original name is optional.

### Type sniffing (magic bytes) — a narrow role; don't over-trust it

The point of sniffing is not "get the type right". The value sits in two
layers:

1. **Reject executable magic** (MZ / ELF / Mach-O / a shebang …) — unambiguous
   and reliably effective. "I thought it was an image, it was an executable"
   dies here.
2. A known type (PNG/JPEG/GIF/PDF) matches → **take the sniffed extension**
   (correcting an obvious mislabel).
3. **Unknown** (text-ish formats like SVG, minor formats) → since the magic
   isn't dangerous it is close to inert → **accept the original extension**.

"Falling back to the original extension when unknown" does not dilute the
sniffing. Its meaning lives in (1), the dangerous-magic rejection, not in the
guessing game of (2) — and (1) holds independently of the unknown case.

### On spoofing — explicitly against "block everything"

- **Saving is not executing.** Writing bytes makes nothing happen. This app
  saves the file and hands over a path string; it **never launches it**. The OS
  won't run it off its extension either.
- "Block every dangerous file at the door" is **not fully achievable** and
  breeds a false sense of safety (any image can be dangerous given a parser
  vulnerability — an extension can't defend that).
- The defence that actually works is the structure itself: **inert on disk**,
  and **the only thing that touches it is the AI the user picked** — an AI that
  can already reach the whole cwd, so the marginal cost is small.
- **The real threat of the AI era is prompt injection** (instructions planted
  in an image or a PDF that the AI then obeys). Making attachment easy widens
  that door. Don't break the premise that *a file handed to an AI is something
  you trust*.
- → Conclusion: not "block everything", but **save it inert + reject executable
  magic + never execute + clean up**.

### Settings

- A size limit (changeable in General settings).
- The starting set of accepted extensions (jpg/png/gif/pdf, say). An advanced
  user who accepts the risk can add more — acceptable, since we never execute.

### Cleaning up tmp

Consider a policy for deleting old files at startup, daily, or similar.

### The shape that "arrives" differs per kind (important)

What must be unified is the **concept** (hand over a path the target can read);
the **means** must differ per kind. "Unify on SCP for everything" is wrong —
local and WSL need no copy, and SCP would only add an sshd dependency,
re-authentication, and latency.

- **Local tab**: no copy. Put it under the cwd and use that path.
- **WSL tab**: no copy — **solved by path translation**. But hard-coding
  `/mnt/c` is dangerous (`[automount] root=` in `/etc/wsl.conf` can change it,
  and UNC paths aren't covered). **Delegate the translation to
  `wsl.exe -d <distro> wslpath -a -u "<winpath>"`** (it respects the automount
  setting and absorbs WSL1/2). Take the distro from the launch command or the
  process, defaulting to the default distro.
- **SSH tab**: a real upload is needed. **A later phase.** And not a plain SCP:
  - Auth: an `scp` in a separate process may re-authenticate (key, password,
    2FA) → avoid it with **OpenSSH's ControlMaster (connection reuse)**: launch
    ssh with `-o ControlMaster=auto -o ControlPath=…` and reuse that socket
    from scp/sftp. This needs the launch command to be extended.
  - The remote cwd is unknown → put files in a fixed place such as
    `~/.SHIKISHA/tmp/`.
  - Cleanup, permissions, and directory creation are needed too.

---

## 2. Actions (canned text / Lua macros)

### Decision: one concept and one UI, two payload kinds

- **The concept, the UI, the list, and the foldering are a single system** — an
  "action" is a registered item that fires on a tap.
- **The payload is either `text`** (inserted/sent as is; beginner-friendly,
  fast, nothing to escape) **or `lua`** (fires a script; advanced).
- **Turning a string into "Lua source" for storage is a landmine** (symbols and
  quotes break the Lua), so keep text as literal data.

### The advanced switch is per action

Not global: **each action independently** chooses text or lua (an "advanced
(Lua)" checkbox on the item changes the kind of that one item's body). A global
mode would be less coherent.

### What it is built on

A thin GUI in two sheets ("canned text" and "script") over the existing
`send_to_tab` and the automation layer (the scoped Lua sandbox).

---

## 3. Driving a target tab (AI-generated Lua) — hardest, last, opt-in

### The flow the user has in mind

1. In tab A's convenience bar, pick the tab to drive (B: a browser) from a
   dropdown.
2. Ask A's AI in plain language: "search Google in B and open the haiku
   Wikipedia page" → **A's AI emits Lua and drives B**.
3. Switch the target to C (another AI) → "do X in C" → A's AI drives C with
   Lua.

This is the generalization of the rally (the AI brain emits a ```lua fence in
its reply → extract → run scoped). The extraction and scoped execution already
exist.

### Important: the AI writes the Lua, not the user

The user only asks in plain language. The Lua is generated by the AI out of the
primitives that suit the target's kind.

### Handing over the state — "pre-read it on our side into a file, for the first turn"

- The naive version (write "read first, then act" into the prompt) **always
  wastes one round trip**.
- **The improvement (adopted): reading the state is a deterministic step that
  needs no AI judgement**, so **the software (Rust) runs the read primitive
  first — a DOM dump for a browser, the recent transcript for an AI tab —
  writes it to a file, and the prompt becomes "look at this file and act".**
  The first round trip disappears. Handing over via a file is a technique
  already proven by the discussion and browser features.
- Caveats: which read to call depends on the target's kind (it meshes with kind
  detection). State can go stale (SPAs) → pre-reading is fine for the first
  turn, but verifying and retrying after an action still needs a loop
  ("pre-read only the first time, rally afterwards only when needed"). A DOM
  dump can get huge → a thinned version (an accessibility tree, say) later.

### Generalizing "browser drive mode" to any tab (resolving a long-standing itch)

- The instinct that a browser-only "mode" violates the primitive rule is
  correct.
- **The right shape: a tab has a kind, and the drive feature derives the
  available toolbox from the target tab's kind.** A browser isn't special; it
  is one example of "a kind that has `browser_*`". AI and terminal tabs are
  "a kind that has send/read". The "browser drive" toggle in settings is
  re-read as **a kind selection**: "this tab's kind is browser (WebView)".
- **Caveat (the machinery is not monolithic)**: what unifies is the **concept,
  the UI, and the dispatch of the Lua vocabulary**. The internals genuinely
  differ per kind (browser = CDP/DOM, pty = key input and output scraping). "One
  feature whose look and Lua vocabulary switch by kind; separate wiring
  inside."
- **A brake is mandatory**: AI-generated Lua driving a logged-in browser risks
  sending or deleting the wrong thing. The same restraints as the rally — a
  chain limit, an emergency stop, and a confirmation before a browser submits.

### DRY: one `operate(A→T)` engine (shared with the existing "browser drive mode")

Reading the code: today's "browser drive" is already a primitive rather than a
mode. A browser tab is `browser <url>` turned into a WebView, and
`browser_open/find/click/fill/text` (caps.rs) are called by name from Lua — the
same ground the rally and automation stand on. The convenience bar's target
drive is **the same operation**, so make the inside one thing (DRY). But apply
that in three layers:

1. **The primitive layer** (`browser_*` / `send_to_tab` + scoped Lua) **is
   already shared.** Nothing new needed.
2. **The orchestration layer** (the whole sequence of A generating Lua to drive
   T) **is the part to unify** — one engine, `operate(source A, target T,
   request)`. Don't write a second implementation for the convenience bar.
3. **The entry points and the kind adapters**:
   - Two entry points, one engine: the "browser drive mode" setting =
     **the target is pinned persistently (sticky)**; the convenience bar =
     **specified ad hoc (transient)**. The difference is a thin parameter —
     whether the target is sticky or transient.
   - **The kind adapters stay separate** (a necessary difference, not
     duplication): browser = CDP/DOM, pty = key input and output reading. Layer
     ② picks the adapter from the target's kind.

To avoid the DRY misuse of crushing together things that merely look alike:
**unify ② only, reuse ①, keep ③ a thin branch.** As a side effect the existing
"browser drive mode" is absorbed as the sticky special case of the general
`operate(A→T)`, and the itch of a "browser-only mode" goes away.

### When to fire (readiness) — the engine's precondition

Driving an immature target causes accidents: an empty DOM, a half-painted page,
a pty mid-output, an AI mid-thought. **The problem is the same for transient and
sticky** (drive quickly right after launch and sticky breaks too), so the
readiness check belongs **in the shared operate engine** — fix it once and both
entry points are protected (an extra DRY win).

Reuse the signals that already exist (dispatch by kind):

- **pty / AI tabs**: the app's status detection (Working / BUSY / DONE / ASK).
  **Wait for DONE or idle** before pre-reading and driving — the same as the
  rally waiting for the other side's turn to finish.
- **Browser tabs**: CDP's load / `document.readyState === 'complete'` (plus
  network idle, optionally).

**The honest limits ("ready" can't be defined perfectly):**

- An SPA paints asynchronously after `load` → don't lean on the engine's gate
  alone; the Lua the AI emits should also use **defensive primitives such as
  wait-for-selector** (half engine, half Lua).
- "Has the output finished?" on a bare shell rests on prompt detection and is
  fuzzy (status detection is solid for AI tabs, heuristic for a raw shell).
- Right after launch the target's WebView may not exist yet.

**The on-timeout policy (don't make aborting the system default):**

- Defaulting to "abort, to be safe" produces **a system that stops constantly
  and is unusable** (rejected in favour of usability). But **a modal every time
  is interruption fatigue.**
- The landing spot: **make the on-timeout behaviour a user setting (`proceed /
  ask each time / abort`).** Decide once and it behaves that way from then on —
  the user holds the choice, with no interruption each time.
- **Make timeouts rare in the first place**: build the detection properly and
  give each kind a generous timeout. A timeout is not everyday logic, it is
  **a backstop** (the choice appears only in the exceptional case; normally it
  passes silently).
- **"Proceed" is kind-aware** (proceeding must not mean garbage):
  - Browser: proceeding pairs with **the AI's Lua wait-for-selector (its own
    defence)** — wait for the element, then touch it.
  - An AI/pty still thinking: "proceed right now" would act on half output and
    is wrong → here "proceed" means **queue it to run when the turn finishes**.
- Note: "treat it as done after N seconds" really only holds for **a browser
  load**. For **AI/pty, waiting for DONE is the normal behaviour** — never call
  it complete on time alone (too long → abort or queue).

→ The `operate(A→T)` sequence: **① wait for readiness (dispatch by kind) →
② pre-read the state into a file → ③ have the AI generate Lua → ④ run it →
⑤ loop back to re-reading if needed.** The engine's gate, plus the AI's Lua
waits, plus a user default for on-timeout (proceed / ask / abort), gets both
safety and usability.

---

## 4. The cross-cutting problem: how to know a tab's "kind"

Split the word along two axes:

1. **The structural kind (browser or terminal) is already known.** A browser is
   a WebView (CDP), a terminal is a ConPTY — different objects at creation. The
   UI state carries `kind:"browser"`. Zero guessing.
2. **What is inside a terminal tab (local / WSL / SSH / AI) is not recorded.**
   **Guessing from the command string is forbidden** — aliases, wrappers, shell
   functions, and "a bare bash you ssh out of later" all break it.

### The approach: look at the reality at run time, not the declaration

- **Walk the descendant processes under a terminal tab's PTY and read their
  image names at the moment it matters** (on attach, on drive). `wsl.exe`
  present → WSL; `ssh.exe` present → SSH; neither → local.
  - Robust against aliases (`alias k=ssh` still execs the real `ssh.exe`).
    "A bare terminal you ssh out of later" is picked up too, as long as
    `ssh.exe` is in the tree at that moment → the "the user never picked a kind,
    they just typed" problem solves itself.
- **Whether it is an AI** is known from another source: the tab's **profile**
  (claude / codex / gemini …).
- **By default, record nothing.** Derive it at the moment it matters from the
  structure (1) + the profile + the live process tree. Since no declaration is
  stored, the "what if they just typed something else?" problem disappears. If
  anything is stored, only an optional manual override hint.

### The honest limits

- Walking the process tree is quietly annoying on Windows (conhost /
  OpenConsole sit in between, permissions, races); multi-hop ssh and mosh slip
  through; the remote cwd of an SSH session isn't visible in the tree.
- → When it can't be determined, **treat it as local and offer a manual
  override**. Not perfect, but an order of magnitude sturdier than guessing from
  a string.

---

## The implementation order (agreed)

1. **File attachments** (the most concrete value, the lowest risk). "Inert save
   + reject executable magic + sniffed extension + never execute + clean up",
   local tabs first, WSL right away via `wslpath`, under the cwd with its own
   gitignore. SSH later (ControlMaster + sftp/scp → `~/.SHIKISHA/tmp`).
2. **The action list** (text/lua per item; foldering can wait).
3. **Making the bar summonable on the desktop** (the container for 1 and 2;
   never permanent).
4. **AI-generated Lua against a target tab** (a toolbox switched by kind, the
   pre-read state file, and the brakes). Hardest, last, opt-in, best-effort.

## Open questions

- Attachments: is "under the cwd" settled? (Reliable AI reads vs. polluting an
  arbitrary folder.)
- Attachments: the initial size limit and the initial set of extensions.
- Attachments: when tmp gets cleaned.
- SSH attachments: how to offer ControlMaster on every ssh tab (how to surface
  the launch-command extension).
- Driving: **the initial readiness timeouts (per kind)**. The on-timeout
  behaviour is settled as a user setting (`proceed / ask / abort`), but **which
  one ships as the default** is not (usability argues for "proceed", with the
  Lua's own defences).
- Driving: whether to add **network idle** for browsers (how hard to defend
  against SPAs), and how seriously to detect "output finished" on a raw shell
  (limiting it to AI tabs is a defensible cut).
- Driving: how to thin a DOM dump, and the UX of the brake (confirm before
  submit).
- The bar: the hotkey and the affordance that summons it on the desktop.
