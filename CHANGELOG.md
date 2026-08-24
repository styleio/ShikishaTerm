# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once it reaches its first tagged release.

## [Unreleased]

### Added
- **An external control API.** A program outside the app can call the same
  commands automations are written with, over a named pipe
  (`\\.\pipe\shikisha-<pid>`, one JSON object per line). The method name is the
  Lua command with the `shikisha.` taken off — there is no second vocabulary,
  and `list` answers with the app's own table of commands, so it cannot fall
  behind what the app can really do. `lua` hands over a whole chunk, for a loop
  or a branch in one round trip.

  Every tab's process is launched knowing `SHIKISHA_PIPE`, its own
  `SHIKISHA_TOKEN`, and `SHIKISHA_TAB` — so an agent in a tab can drive the app
  with no setup, and because the key is the tab's own, what it sends counts
  against the same chain limit as work handed over on screen. The pipe carries
  an access list naming your account alone. The **External control** card in
  the settings screen chooses between `children` (the default), `user`, and
  `off`, and takes effect the moment it is saved — the pipe opens or closes
  without a restart, while the keys already handed to running tabs stay valid
  across the switch.

- **Split panes.** The content area divides into a tree of panes, and each pane
  shows one tab: `Ctrl+B %` beside, `Ctrl+B "` below, arrows or `Ctrl+B o` to
  move between them, `<` `>` to move the divider, `Ctrl+B X` to close the view
  (the tab behind it keeps running). Clicking a pane focuses it; its caption
  carries the tab's name and state dot.

  A pane is sized for what is in it — a terminal is told the rows and columns
  its own pane actually has, not the whole window's, and a browser placed in a
  pane is placed at that pane's rectangle. So an agent can sit beside the
  browser it is driving, or two agents beside each other, and both stay
  correctly laid out.

  A surface is only ever in one pane: picking a tab that is already showing
  elsewhere swaps the two panes rather than running the same terminal at two
  different widths. Undivided, everything looks and behaves exactly as before.

### Fixed
- **Lua that never returns no longer takes the window with it.** The engine runs
  on the main loop, so a `while true do end` typed into the composer — or sent
  from a phone, which could already be done — froze the app outright, with no
  keystroke and no redraw until it was killed from the task manager. An entry
  into Lua now has an instruction budget and comes back with an explanation
  instead. It counts instructions, not time, so a command that legitimately
  waits half a minute on a page is never mistaken for a runaway.
- **A `config.json` saved by a Windows editor is read again.** Notepad's "UTF-8"
  and PowerShell's `Set-Content -Encoding utf8` both write a byte-order mark;
  the parser rejected the file, loading moved on to the next candidate, and the
  app came up on someone else's settings while the edit appeared to do nothing.

## [0.2.0] - 2026-08-24

### Breaking
- **Handing work to a tab no longer moves the screen.** `send_to_tab` did two
  jobs — pass the work, and pull your eyes along with it — and the second one
  obeyed different rules from `shikisha.show`, which ignored both your setting
  and the quiet moment after you move the view yourself. During a rally, where
  the screen switches several times a round, "don't switch on me" was a promise
  the app did not keep. `show` is now the only thing that moves the view, and it
  asks first. **Automation that relied on the old behaviour needs a
  `shikisha.show(tab)` line before its `send_to_tab`** — the built-in
  orchestrators already have one.
- **The `follow_ball` setting is now `auto_switch`** ("Auto-switch"), because it
  no longer follows a ball: it decides whether automation may switch tabs at all.
  A `follow_ball` left in `config.json` is ignored; the default is on either way.
- **Browser data moved into the folder the config names.** Every page shares one
  cookie jar until you say otherwise, and each profile now gets its own — see
  below. Existing browser logins do not carry over; sign in again once.

### Added
- **A restart beside the emergency stop.** It relaunches the one tab you are
  looking at, rather than every exited tab at once. A tab that has already
  exited (the SSH that dropped) goes on the first press; a live one asks twice,
  because a stray tap next to the stop button must not take a running
  conversation with it. Browser panes are included: a page has no reload that
  can take it back to where it started, and some have no reload at all.
- **A rally digests the page for you.** Every round stages a numbered list of the
  operable elements, so an operator never spends a move asking what is on screen.
  Clicks are anchored to their text rather than to ids that change, waiting for
  the page to settle is automatic, and the whole run can be downloaded as a
  replay macro.
- **The rally can hand a login back to a person.** When a site needs a human —
  sign-in, CAPTCHA — the run notifies you (carrying the phone's URL when remote
  is on), waits up to half an hour, and picks up where it left off.
- **✨ Ask for a shell command in plain words**, and 🔍 an optional one-shot
  survey of the machine so those suggestions know what is installed. The command
  arrives as a draft in the composer; you still press Send.
- **A bookmarkable phone link.** Set a fixed access token and the phone keeps
  working across restarts without re-reading the QR. Cutting the phone off still
  cuts it off.
- **The complete command reference, in both manuals.** Half of what automation
  can call — 55 commands, `shikisha.show` among them — had never been written
  down, and those manuals are what the app hands an AI as the specification when
  it writes automation: a command that is not in them does not exist. Both
  languages now carry the full list, and a test fails if any command, or anything
  on the `tab` table, goes missing from either.
- **`tab.id`** — a hook can finally tell which tab it is on by the automation
  name that survives a rename, rather than by the display name a person edits or
  the number that shifts when tabs are reordered.

### Changed
- **Separate browser profiles are now actually separate.** Each page carried its
  own data folder and none of them were ever used, so "separate profile" and
  "private" were settings that did nothing — a private page came back to a site
  still logged in, and a throwaway one left nothing to throw away. Each gets its
  own store now, all under the one folder the config names, and a closed private
  page is erased the moment the browser lets go of it.
- **The phone shows the sub-input bar on a terminal tab by default**, and the ✏️
  pen is the way back once you close it.
- An attachment is saved beside its own tab even when a browser pane sits between
  them.

### Fixed
- **A browser no longer machine-translates the terminal.** Chrome on a phone read
  the terminal's English output, decided the page was English, and translated it
  — and since every run of cells is a box sized in columns, text of another
  length simply piled up on its neighbours. The page now says which language it
  is and refuses translation outright.
- **Phone: choosing a folder no longer freezes the app.** The button opened a
  file dialog on the PC and held the request until someone dismissed it there.
  Those buttons are left off the phone, and the endpoints behind them answer with
  a refusal rather than a window nobody can see.
- **Phone: the composer stays shut once you shut it.** Tapping the terminal
  summoned it back and erased your ✕ at the same time, so the ✕ meant nothing.
  The composer also stops the phone keyboard correcting what it is handed: a
  corrected password is a wrong password.
- **Phone: the tab bar's + works again.** It sent an intent that becomes a
  keystroke only the window can carry out, so the button did nothing at all. It
  now walks to the settings page and asks for the same thing. The hop that trades
  the URL token for a cookie was also throwing away which screen to open.
- **A broken `config.json` says so, instead of showing an empty screen.** The
  settings page now names the file, quotes the line, marks the character the
  parser stopped at, and holds Save until it's fixed and reloaded — previously
  the parse failure was swallowed, the form came up blank, and pressing Save
  would have written that blankness over the real configuration. A broken
  workspace file is handled the same way, for the same reason.
- **Opening the Quick actions card no longer counts as an edit.** Drawing it
  filled in `lua: false`, which lit up "unsaved" on arrival and wrote that
  default into `config.json` on the next save.
- **Phone: "Edit settings" on INDEX actually opens the settings.** The board was
  forwarding it as a keystroke, which only ever lands in the window, so from a
  phone the entry looked alive and did nothing. It now walks to the phone's own
  reverse-proxied settings page. An entry the window alone can carry out (the
  master password) is shown dimmed instead of silently doing nothing.
- **Phone: the settings screen fits the screen.** The sidebar became a drawer
  behind a ☰, the header keeps only what a thumb needs (where you are, Close,
  Save) with the rest moved into the drawer, and long labels, hints and paths
  wrap instead of running off the edge — the page no longer scrolls sideways at
  any width down to 320px. The desktop layout is unchanged.
- **The terminal survives being resized mid-draw.** Narrowing the window while a
  full-width character sat on the fold could take the whole app down (vt100
  0.16.2).
- **The build stamp follows the commit.** It watched only `.git/HEAD`, whose
  contents never change when you commit, so the label kept naming the previous
  commit — defeating the one thing it exists for.
- A phone access token too short to be a secret now refuses to start rather than
  quietly falling back to a generated one.
- The composer's hint fits on one line on a narrow phone, where it used to wrap
  and have its second line sliced in half.

## [0.1.2] - 2026-08-21

### Added
- **The composer** — the sub-input bar is no longer mobile-only. Summon it on
  the desktop window, attach a file by paste or drag (saved beside the tab, and
  handed over as a path the AI can read), and fire **quick actions** that insert
  canned text or run Lua. A starter set ships localized, and the actions are
  edited in Settings rather than by hand.
- **📼 Record a browser page as Lua** and play it back: clicks are anchored to
  their text, throwaway ids are refused, and the recording runs in the
  background so the input box is never taken hostage.
- **Drive another tab (`operate`)** — aim the active AI at a target tab ad hoc,
  wait for the target to settle, stage its state to a file so the first round
  trip isn't wasted, and hold a brake on runaway chains. What happens when the
  limit is hit is now the user's choice.
- **Browser tabs** get the composer instead of a bespoke bar; Send injects into
  the shown browser, shared with the phone. The address bar searches Google when
  the text isn't a URL.
- **Phone**: open Settings natively over a reverse proxy, fill a portrait
  screen, and pinch to zoom the relay.
- **Settings**: flat card-unit navigation with a shortcut straight into the
  quick actions, and a collapsible General group.
- Docs and site: a demo video and product screenshots on the landing page, and
  `DESIGN.md` is now English-first (Japanese as `DESIGN.ja.md`).

### Changed
- The remote access token is kept out of the phone's URL, off the QR screen, and
  out of the phone card in Settings.

### Fixed
- Remote access now actually releases its port on shutdown, so turning it off
  and back on works.
- `operate` addresses a target browser by its key rather than its display name.
- The composer keeps its panel dropdown open across state pushes, and rebuilds
  the panel on a tab switch so the switcher comes back.
- Settings opens onto the current workspace instead of auto-expanding
  workspace 1.
- A quick action whose Lua won't parse is refused at save time.

## [0.1.1] - 2026-08-19

### Added
- Phone access got much better: remote state is pushed over a WebSocket instead
  of polled, history can be paged back through, the phone sizes the terminal to
  its own screen, and typing goes through the sub-input bar.
- The window can honestly disconnect a remote session (the token is rotated on
  the spot), and the phone shows an overlay when the feed stops.
- Notifications: a Slack/Telegram editor in General settings, plus a per-tab
  "notify on answer" toggle that needs no Lua.
- A "Show flags" button that runs the selected AI CLI's `--help`.

### Fixed
- The terminal reflows on any resize, not just window resizes, and its width is
  no longer shrunk twice by the tab bar.
- Model tabs no longer send an empty key (401) when secrets are encrypted.
- The ~8s black screen after the master password is replaced by a splash.
- File capabilities are confined by the real (symlink-resolved) path, and the
  local HTTP servers are hardened (body limits, privacy headers).

## [0.1.0] - 2026-08-18

The first public release. It is pre-1.0 and evolving quickly. Highlights:

### Added
- Run multiple AI CLIs side by side in one window (Claude Code, Codex CLI,
  Gemini CLI, Aider), plus model-API bridges (DeepSeek, Ollama/Qwen, …).
- Per-tab status read from the screen — working / done / waiting — so it works
  with any CLI rather than one vendor's API.
- Workspaces: swap the whole tab layout per project; export one to a single file.
- Automation in Lua (or described in plain language and written by an AI), with a
  sandbox that has no file or network access by default, plus runaway limits, an
  emergency stop, and per-tab input locks.
- AI-vs-AI discussion and code review: several agents debate or review, a judge
  rules, and the whole exchange is rendered as a chat-style **Result** tab with a
  one-click Markdown download.
- Phone access over the local network (QR pairing), safest over Tailscale.
- Notifications to Slack / Telegram when a job finishes.
- A real terminal underneath: SSH, Docker, WSL, jump hosts, key files, port
  forwarding, session logs, legacy encodings, IME input, and the mouse.
- Interface localization (English base, Japanese complete; more welcome).

[Unreleased]: https://github.com/styleio/ShikishaTerm/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/styleio/ShikishaTerm/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/styleio/ShikishaTerm/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/styleio/ShikishaTerm/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/styleio/ShikishaTerm/releases/tag/v0.1.0
