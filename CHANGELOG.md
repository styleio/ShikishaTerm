# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once it reaches its first tagged release.

## [Unreleased]

### Fixed
- **Reopening the app comes back to the conversation, not to an empty prompt.**
  Closing SHIKISHA-TERM in the middle of a job and starting it again put the
  workspace back, put the panes back, and quietly started every AI on a brand
  new conversation. The id of the one it had been running was written down and
  read back at startup, but it was only ever handed over by Ctrl+B r, on a tab,
  one at a time — and nothing on screen said so, which is why the way back was
  not findable by anyone who did not already know it was there. The settings
  screen even promised the opposite, in as many words. Each tab is now launched
  back into the conversation it was having, so its history is on screen again;
  `Pick up where this tab left off`, in that tab's settings, turns it off for a
  tab that is better off clean. A conversation that has been deleted since, a
  tab that is new, and a CLI that cannot be told which conversation to resume
  all still start fresh, silently, because starting fresh is what a tab
  normally does.

### Changed
- **Restarting belongs to the pane it restarts.** The ↻ next to the emergency
  stop could only ever reach whichever pane had focus, so with the screen
  divided the other half could not be restarted without first going and
  standing in it. Each pane's caption now carries the pair itself: ⟳ carries
  the conversation on, ⟲ starts a new one — the two keys Ctrl+B r and Ctrl+B R
  already stood for, on the pane they are drawn on. A pane whose thing has
  already exited restarts on the first press; a live one asks twice. A phone
  keeps the button in its status bar, where it is not ambiguous: there are no
  panes to point at, and a phone watching an SSH tab that dropped is exactly
  who needs it.

## [0.3.6] - 2026-08-27

Looking at INDEX no longer wrecks what the AI had drawn, and the executable
finally says who wrote it.

### Fixed
- **A screen covering the panes no longer resizes the terminals underneath.**
  Opening INDEX, or the settings form, told every running AI that its window
  had shrunk to 20 columns by 5 rows — then told it to grow back the moment the
  cover lifted. Anything that redraws on a resize, which is every AI CLI built
  on Ink (Qwen Code, Gemini CLI, Claude Code), reflowed its whole interface into
  that and could not put it back: what returned was blank rows and a frame cut
  off mid-line, repairing only where the next keystroke made the program draw
  again. That is why typing appeared to fix it a piece at a time. The cause was
  the same one behind the misplaced toast in 0.3.5 — a hidden element reports a
  rectangle of zeros rather than no rectangle at all — caught that time where
  things are *placed* and missed where they are *measured*. Measured on a
  running window, opening INDEX went 118x47 → 20x5 → 118x47; it now stays
  118x47 throughout. A covered pane has not changed size; it is only not being
  shown.

### Changed
- **The executable says who wrote it.** `LegalCopyright` read "MIT License",
  which is the name of a license and not a copyright notice — no holder, no
  year, which is the one thing that field exists to state. It now reads
  `Copyright (c) 2026 styleio. MIT License.`, `CompanyName` names the author
  rather than repeating the product, and `OriginalFilename` and `InternalName`,
  both of which were empty, are filled in.

### Added
- **The download says what it is made of.** MIT, BSD, ISC and Apache all grant
  what they grant on one condition — that their copyright notice travels with
  the code — and the zip carried none of them, which is a download handed out
  outside the grant and the first thing an OSS compliance review looks at.
  `THIRD-PARTY-NOTICES.txt` now ships beside `LICENSE` with the full terms of
  every component the Windows binary is built from, 209 of them: not a
  general list of what the project has used, but what this build links.
  `tools/notices.ps1` writes it, `dist.list` carries it, and CI fails a pull
  request that adds a dependency without rerunning it. The file is written in
  a fixed order, with the crate itself left out, and reading only each crate's
  own root for its terms — a crate that repeats its licence header atop every
  source file was otherwise credited with the text of one of them, picked in
  whatever order the filesystem happened to hand them over. All three are what
  let that check compare the same text on two machines.
- **A copyleft dependency can no longer arrive in silence.** `about.toml` names
  the licenses that may travel inside the download; one that is not on that
  list stops generation rather than reaching a user by way of some transitive
  version bump. Nothing in the tree is copyleft today.

## [0.3.5] - 2026-08-27

The dashboard answers again. Every menu item on it that replies with a
message had its reply drawn off the screen, which on a new install is the
first thing anyone meets.

### Fixed
- **The dashboard's menu answers again.** On a brand-new install, five of its
  items did nothing at all: *Connect from your phone*, *Restart stopped tabs*,
  *Switch workspace*, *Send a test notification*, *Master password* — no action,
  no explanation, nothing to tell a first-time user whether the app was broken
  or they were. Each of those answers with a message, and the message was being
  drawn off the screen: the dashboard and the settings form cover the panes, a
  hidden pane reports a rectangle of zeros rather than no rectangle at all, and
  the toast — which is seated over the pane it is about — was placed by those
  zeros at `left:-290px, bottom:998px` in a 900px-tall window. With the panes
  covered, the thing being talked about is the whole content area, so that is
  what it is seated on now.
- **Nothing is left sitting on a pane that isn't there.** The same zeros placed
  the composer and its ✏️ pen, which is why they were invisible on the dashboard
  rather than wrong. They now come and go with there being a pane to type into,
  on both the window and a phone — a Send on the dashboard was addressed to a
  pane that is not in front, and reached nobody.
- **"Switch workspace" says when there is nowhere to switch to.** With a single
  workspace it opened nothing and said nothing, which is indistinguishable from
  a broken button.

## [0.3.4] - 2026-08-27

A discussion that starts when you ask it to and says so when it cannot, one
place to type whatever pane you are in front of, and a line that goes to the tab
it was addressed to.

### Added
- **`shikisha.note(tab, "text")`** writes a line **on** a tab's screen for the
  person watching. Nothing reaches what runs there and no answer is expected —
  the counterpart of `send_to_tab` for a pane that has to be told something
  without being asked anything.

### Changed
- **One sub-input bar, for every pane.** A model pane used to carry a composer
  of its own — its own field, its own Send, its own Enter — pinned to its floor,
  while every other pane used the shared bar. Two composers meant two answers to
  every question asked of one, and they had already drifted: the quick actions
  and the phone's key row existed on only one of them. There is one field now.
  On a model pane it offers the quick actions and nothing more (there is no
  command line there to aim a target at or to suggest a command into), plus the
  key row on a phone, and the empty field names the pane it will speak to.

### Fixed
- **A discussion actually starts, finishes, or says why it did not.** Opening a
  workspace left the opening speaker's briefing sitting in its input box as an
  unsent draft, showed nothing at all in the panes driven by a model, and — once
  someone was finally nudged — went quiet for good. Four separate holes behind
  the one symptom:
  - Automation spoke before the AI could listen. Every tab spent its first
    800ms looking freshly resized, so its startup output was discarded as
    redraw noise and the readiness gate mistook a launching CLI for a settled
    one. The gate now asks the screen itself to hold still, and waits out a CLI
    standing at its own trust prompt.
  - A model bridge cannot be briefed — it is stateless and would speak before a
    topic exists — so its pane was left blank and a participant looked like a
    tab that had failed to start. Its stance and role are written on its screen
    instead (see `shikisha.note`).
  - A model pane reported "done" while its reply was still in flight, because
    only its screen was being watched and nothing moves during an HTTP round
    trip. An opening speaker that starts itself was therefore re-asked forever.
  - A participant that ends its turn without saying anything is asked once
    more; if it still says nothing the run ends with the reason on its screen
    and in the transcript, instead of every seat waiting on a hand-off that
    will never come.
- **A finished line goes to the tab it was meant for.** The topic box floats
  over whichever pane you are looking at, and it sends two things: "look at the
  opening speaker" and "here is a line". The line carried no address, so it was
  handed to whatever pane was in front when it arrived — and nothing promises
  those two land in that order. Typed while watching another AI, the topic
  reached that AI, or vanished without a trace. Lines name their recipient now,
  and how one is delivered (told to a model, typed at a prompt) is decided in
  one place rather than at each edge — which also mends sending to a model pane
  from a phone, which had been typing at a keyboard that does not exist.
- **A dialog no longer vanishes when a selection is dragged out of it.** Select
  text in the new-workspace wizard, overshoot the field, and let go anywhere
  outside — the form closed, taking everything typed into it. A click belongs to
  the nearest ancestor of where the button went down and where it came up, and
  the backdrop covers the whole screen, so a hurried drag to the end of a line
  arrived as "a click on the backdrop". Dialogs close on the press that starts
  outside them now, which is what the vault and the command palette already did.

## [0.3.3] - 2026-08-26

One message bar for the whole app, the target you pick is the one that is
remembered, and a handful of things that were true in one place and quietly not
true in another.

### Upgrading
- **A tab that had "browser-control mode" set keeps its target** — the same
  setting is now the 🎯 aim, and the picker on screen shows it. What changed is
  when it starts: the tab is briefed once you give it a goal, rather than at
  launch. Nothing needs editing.

### Changed
- **Messages are one toast, and it goes away.** The window, the settings screen
  and the transcript view each carried their own message bar with their own
  timing, and the window's — the one the app itself writes to — faded never: it
  waited for a keystroke, so a notice shown at startup could still be sitting
  there an hour later. There is now one toast (`src/toast.rs`) for all of them.
  It fades on its own after a few seconds (longer for a warning, longer again
  for a long message), and a click or tap takes it away at once.

- **A message can be taken with you.** Every toast carries a 📋 button that
  copies its text and then leaves. Only that button touches the clipboard —
  clicking the message itself just dismisses it, so getting a notice out of the
  way never overwrites what you were about to paste.

- **What you aim at is remembered, and there is no second place to set it.** The
  settings screen had a "browser-control mode" chosen per tab, from before the
  🎯 picker existed. Two places said the same thing and the settings one could
  not see the other: it read "off" while a browser was being driven, and it
  offered only browsers although 🎯 can aim at an AI tab too. That card is gone.
  Picking a target on screen IS the setting now — it is written down against
  that tab and it comes back on the next start.

- **An aim no longer takes a tab's automation away.** A tab given a target used
  to be handed the built-in agent at launch *instead of* the Lua written for it,
  silently. The aim now borrows the pane's script while it is attached and hands
  it back when it is let go, so a tab keeps its own automation either way.

- **A tab is briefed when there is work, not at launch.** Being aimed at
  something is not yet doing it: the operator hears about its target when a goal
  is given, so opening a workspace no longer fires a turn at an AI nobody has
  asked for anything.

### Fixed
- **A division stays where you put it.** Showing a tab that is already on screen
  now moves the aim to it, and not one pane changes what it holds. The two panes
  used to trade contents, so an automation handing work back and forth — the AI
  on the left, the browser it drives on the right — made the halves swap sides on
  every turn. Nothing was ever hidden by that, but the arrangement was yours, and
  a view that jumps is harder to read than one that sits still. A tab that is
  nowhere on screen still lands in the pane you are looking at.

- **The pen comes back when you walk to another tab.** Closing the composer over
  a browser tab left no way to open it again: the pen was hidden there on
  purpose — the placed page draws its own, ours would be underneath it — but
  that answer was frozen at the moment of closing, so the next tab had no pen
  either, and no way back into the composer. Where you are is half of what
  decides the pen, and it changes without the bar being touched, so it is
  settled from the state on every update now. One place decides it, for both the
  window and the phone.

- **A message is no longer cut in half by a page beside it.** The toast sat at
  the bottom of the window; with the window divided and a browser in one half,
  half the message was behind it — a page is a window of its own, and no
  z-index puts anything above it. The window now seats its toast over the pane
  in front, which is what the message is about, and when that pane holds a page,
  the page draws the message itself (as it already draws the pen).

- **An operation started from the screen obeys the workspace's referee.** The
  🎯 path handed the built-in commander an empty set of stop conditions, so the
  "Stop conditions" written for that workspace applied when the same browser was
  driven from the settings file and quietly did not when it was driven from the
  picker.

- **A model brain knows it is driving.** Whether a model tab steers a browser
  (its system prompt, and whether its turn reaches the orchestrator) was read
  from the settings file once, at launch. Aimed from the screen it went on
  answering as a plain chat, its fenced Lua arriving mangled by the terminal's
  line wrapping. It now follows what the tab is actually aimed at.

## [0.3.2] - 2026-08-26

A patch for the phone: the control that says it has disconnected a phone now
disconnects it, and the tab a phone is watching is drawn at the size of the
phone.

### Upgrading
- **A phone that was already paired has to open its link once more.** The new
  admission is a session handed out when the pairing link is opened, and a phone
  running from before the update holds none — so it shows its "disconnected"
  screen the first time. Tap **Reconnect** (a fixed token), or scan the QR code
  again (the default), and it is back. Nothing needs to be changed in the
  settings.

### Fixed
- **"Disconnect" now actually disconnects the phone.** Ending a remote session
  from the window used to drop the sockets and nothing else. A second and a half
  later the phone reconnected on its own — so a person who had just cut a phone
  watched it go on watching, and go on driving a browser, from a screen that
  said it had been disconnected. Admission is now a session handed out when the
  pairing link is opened, and the disconnect revokes it: the phone's screen goes
  dark at once, its next request is refused, and its touches reach nothing, even
  on the input socket it still holds open. With a fixed token (which cannot be
  rotated) that phone can pair again by opening the link — the veil offers it as
  a button, and the window's control says so instead of promising otherwise.

- **The tab a phone is watching is sized for the phone, panes or no panes.** The
  pane in front now keeps the size last reported by whoever is looking at it. At
  the window that is the pane's own rectangle, so nothing there changes; a phone
  reports the one screen it has — it is never sent the division — and the tab in
  front is finally drawn to it. Before, the tab being watched was handed the
  window's shape: too wide, so its right-hand side hung off the screen with no
  way to reach it, and too short, leaving a dead band along the bottom.

- **The relayed picture is the size of the pane it sits in, not of the frame
  that arrives.** A canvas left to its own devices keeps the incoming frame's
  pixel size — on a phone, the PC's page at twice the width of the screen. And
  since the phone reports its screen shape from that same box, it reported the
  frame's shape straight back, so the PC never re-shaped the page to suit the
  phone and the black band under the picture could never close.

## [0.3.1] - 2026-08-25

A follow-up to the split panes of 0.3.0: the window is now divided with the
mouse as readily as with the keyboard, everything drawn over a pane knows which
pane it belongs to, and a tab whose command already says how to resume is left
alone.

### Added
- **The tab bar's edge can be taken hold of.** Drag it and the bar follows; drag
  it shut and the window is all terminal. `Ctrl+B s` does the same from the
  keyboard and brings the bar back the width it was, double-click for the width
  it ships with. Every one of those writes the one width in the settings file,
  so the next start opens the way you left it — and the config field that
  claimed to set that width, unread for as long as the window has drawn the bar,
  finally means something.

- **Every pane is captioned, the only one included.** The two divide controls
  (▥ and ▤) live in the caption, so the first division no longer needs the
  keyboard — you could not use the pane captions to divide until you had
  divided. The last pane cannot be closed, so its caption offers no ✕: a control
  that refuses is worse than one that was never there.

- **An empty half invites the tab that will fill it.** A division no longer
  refuses for want of a free tab. "+ Add a tab" opens the settings form on its
  add-a-tab page, and the new tab lands in *that* pane — the one that was
  pressed — with the form closing behind it. Cancel and the pane waits, as it
  was.

- **The settings screen shows the line a tab will really be launched with.**
  Under the command field is the command itself, with the arguments the app adds
  picked out in the accent colour and a sentence saying why they are there. The
  app answers the question rather than imitating it, so the promise and the
  launch cannot drift apart.

### Changed
- **INDEX and the settings form are screens, not panes.** Both are about the app
  rather than about one of the things running in it. They cover the window now,
  and the layout waits underneath, whole, returning the moment a running tab is
  picked. Surface 0 no longer means the board; it means nothing is in this pane
  yet.
- **A blue rule means the focus is here, and nothing else.** Full-strength brand
  colour is for state — the focused pane's underline, a bar that is loading, a
  mode that is armed; structure inside a panel is drawn with the plain line
  colour. The composer's inner seam was reading as the start of another pane.
- **The divider between panes is visible.** A hairline down the middle of the
  9px grab handle: 9px takes the pointer, 1px is what the eye sees, and under
  the hand the hairline gives way to the handle lighting up. Two dark terminals
  side by side used to read as one region.

### Fixed
- **A command that already says how to resume keeps its own word.** A tab
  written as `claude --dangerously-skip-permissions --resume` had our
  `--session-id <uuid>` pushed in beside it, which Claude Code refuses outright
  — the tab died on launch and every restart rebuilt the same dead line. The
  words we look for come from the profile's own resume templates, so a CLI that
  spells it as a subcommand is recognised by the same rule as one that spells it
  as a flag.
- **A page placed in a pane stays inside that pane.** The room held back for the
  composer was written over the focused pane's offset, so a placed browser
  painted down the whole column — over the pane beneath it and over the very pen
  it was making room for.
- **Clicking into a placed page is clicking on its pane.** A browser pane could
  only be entered by its caption, and the pen that summons the composer never
  appeared over one. What the page reports is taking the keyboard, not being
  clicked, so a rally clicking through a form cannot pull the keyboard out from
  under someone typing in another pane.
- **The pen is drawn by the page it has to float over.** No element can be
  stacked above a placed page — it is a window of its own — so the page draws the
  pen itself and nothing is reserved any more. There is no longer a band of
  nothing under every browser.
- **The composer opens at the foot of the pane you summoned it from**, rather
  than at the window's floor — or, with a browser placed down there, nowhere at
  all.
- **The pane you just left keeps showing what was on it**, instead of sitting
  empty until its terminal happened to print something.
- **The focused pane is measured whenever the geometry moves.** The rectangle
  every layer above a pane draws with is in pixels while panes are laid out in
  percentages, so resizing the window left a browser landing in the old
  rectangle until some other pane was focused.

## [0.3.0] - 2026-08-25

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

  **The mouse can do all of it too.** A pane's caption carries ▥ and ▤ beside
  its ✕, so a pane can be divided where it stands without knowing a key. The
  divider between panes is draggable — the thing anyone tries first, and until
  now the one thing that answered nothing — and double-clicking it puts the
  halves back to even. A divider is addressed by its place in the tree rather
  than by the panes either side of it, so dragging the boundary of a nested
  layout moves that boundary and not another one elsewhere on the screen. A
  browser placed in a pane is held back from the divider by a few pixels: it is
  a native layer over the page, and flush against the line it would swallow the
  half of the grab handle that overhangs it.

  **Automation can divide the screen too**, in the same words the keys use:
  `split_pane("right")`, `close_pane()`, `focus_pane("left")`,
  `equalize_panes()` — and, since the external API is the same table of
  commands, from outside the app as well. Putting a browser beside the agent
  driving it is `split_pane("right")` then `show("br")`: two commands that each
  do one thing, rather than one that only ever makes the arrangement whoever
  wrote it had in mind. `Ctrl+B =` does the equalizing from the keyboard.

- **A restart no longer throws the conversation away.** `Ctrl+B r` was the only
  way out of a CLI that had died, hung, or updated itself — and it started the
  agent again with nothing, losing everything that had been said. It now carries
  the conversation over, and `Ctrl+B R` is there for when a clean slate is what
  you wanted. Automation and auto-restart follow the same rule, so a tab that
  comes back on its own comes back as itself.

  Which conversation belongs to which tab is not guessed at. Where a CLI accepts
  a session id at launch, this app hands it one, so two agents of the same kind
  in the same folder are told apart with certainty rather than by whichever
  record happens to be newest. Where it does not, the conversation it started is
  read back from the CLI's own records — but only for a tab that could not have
  been any other, since those records name a folder and never a tab. A CLI that
  can report its own conversation as it starts is asked to; the settings screen
  shows what would be written into its config file before writing anything, and
  keeps the previous file beside it.

  When none of that can settle it, the restart says why it started a new
  conversation instead. Coming back holding somebody else's is worse than coming
  back empty. The settings screen's **Carrying conversations** card says, per
  CLI, which of these applies.

- **What was on screen last time comes back.** The division into panes is put
  back as it was, and each tab remembers the conversation it was having — so
  `Ctrl+B r`, on a tab nobody has spoken to yet, picks up where the last run of
  the app left off. No new key and no new gesture: that is what the key already
  meant, now reaching across a restart of the whole app.

  Conversations are **offered, never restored on their own**. Someone who quit
  to be rid of one should not find it waiting for them, and a tab that has been
  used carries its own conversation rather than yesterday's. Before resuming,
  the conversation is checked to be still on this machine — a deleted one gets
  a plain sentence instead of whatever the CLI says when handed an id it has
  never heard of.

  What is **not** restored: anything that was not this app's to keep. A
  terminal's contents, a shell's history, a half-typed command. Restoring the
  photograph of a live thing invites people to trust it.

- **An agent can say what it is doing.** The state dot beside a tab is read off
  its screen, so it can only ever say "busy" or "waiting". Now the thing in the
  tab can say the rest — `shikisha.set_status("build", "running tests 3/5")` and
  `shikisha.set_progress(0.6)` put that under its name in the tab bar, and on
  the phone, which draws the same list. Detection stays for the CLIs that will
  not tell us; this is for the ones that will.

  Entries are keyed, so a build script and the agent it is watching can both
  speak without overwriting each other, and an empty value takes one away —
  finishing needs no second verb. A caller that is not a tab (a script you ran
  yourself) may name the tab it means; anything running in a tab means itself.

- **A CLI that has never heard of this app can speak up too.** The notification
  escapes every terminal understands — `\e]777;notify;Title;Body\a`, `\e]9;…`,
  and the richer `\e]99;…` — are picked up out of the terminal stream and shown
  under that tab's name, with a toast if you are looking somewhere else. Nothing
  to install and no profile to write, which is the point: it works over ssh and
  inside containers, where nothing of ours is present.

- **A browser page can be photographed.** `browser_snapshot` captures the page
  as a PNG through the browser's own devtools — what a person would see, not a
  re-render — so a rally can keep a visual record of what it did, and you can
  glance at where an agent got to without switching to the tab. The pictures
  are saved beside the browser data and shown under Settings → Snapshots, each
  removable. This is the `--snapshot-after` a rally needs.

- **The board says what the app itself costs.** Beside the build stamp, the
  window now shows its own footprint — processor and memory across everything
  it is running: the terminal, the agents it launched, the browser it embeds.
  The weight is stated plainly rather than left for a task manager to reveal,
  the same honest figure the per-agent COST column is built from.

- **A notification can now be caught by automation.** When a program rings the
  terminal — a bell, an OSC notification, even one from a CLI running over ssh
  where nothing of ours is installed — a new `on_notify(tab, text)` hook fires.
  Until now those only became an in-app toast; the hook lets you do what the
  toast cannot — forward it to your phone with `shikisha.notify(...)`, route it
  by which tab it came from, log it — while the toast still shows. Without a
  hook nothing changes.

- **"Find" now searches the present as well as the past.** The same box that
  searches recorded conversations also searches the live output of every open
  tab, so "which of my agents mentioned that error" is one search across all of
  them at once. Matches in an open tab come first — a live one is likelier the
  thing being looked for — and selecting one switches to that tab; matches in a
  past conversation follow, and selecting one reopens it. Only the recent
  stretch of each tab's history is scanned, so it stays quick with several
  agents running.

- **A command palette.** `Ctrl+B :` (or "Command palette" on the board) opens one
  box that finds and runs anything: go to a tab, open settings or the Vault, run
  any of the rebindable actions, fire one of your own quick actions — filtered
  as you type, run on Enter. The action list is the one keys table the rest of
  the app already uses, so the palette shows exactly what the app can do and
  nothing it cannot, and running one is the very keystroke pressing it would
  send — a rebound key and a moved prefix are already accounted for. Works from
  the phone too.

- **Past conversations can be searched and reopened.** The board's menu has
  "Find past work": a search box over every conversation the AI CLIs have
  recorded — claude under `~/.claude/projects`, codex under `~/.codex/sessions`
  — matched by what was actually said in them. Reopening one adds a tab that
  resumes it, in the folder it happened in, the same way a restart carries a
  conversation over. Nothing is indexed ahead of time: the records are read
  when asked, newest first, and the search says so when it stops before the end
  rather than implying the list is the whole of the past. What makes a record
  findable is format-blind — the text is searched as text and the id and folder
  taken from where every one of these tools keeps them — so it survives the
  formats changing between releases. Works from the phone too.

- **The dashboard says what each agent is costing.** The SESSIONS table on the
  board now has a COST column: processor use and memory for each tab, summed
  over the agent and everything it started — because a tab's cost is the dev
  server and the language server three processes down, not the shell we
  launched. With several agents running, "which one is pinning a core or eating
  a gigabyte" was a question that needed a task manager and a hunt through
  processes that all show the same program's name; now it is a column. Read
  from the same process-tree walk the ports come from, on a two-second beat.
  Processor use is a real rate (two readings and the gap between them), so the
  first look shows memory but never a made-up percentage. A tab costing nothing
  — a browser, an idle shell — shows nothing.

- **A saved browser login now includes localStorage, not only cookies.** Many
  modern web apps keep the token that says you are signed in in localStorage,
  which a cookie-only save would miss; `browser_state_save` now captures both
  and `browser_state_load` restores both, read and written in the page's own
  origin through the devtools protocol.

- **A browser login can be saved once and reused.** Sign in on a browser tab,
  and `browser_state_save` keeps that login under a name; a later rally loads it
  with `browser_state_load` instead of signing in from scratch — the real cost
  of a rally is arriving logged out. It saves the login of *our own* browser
  profile, read through the same devtools the browser uses, so httpOnly cookies
  (which is where a login actually lives) come too. It does not reach into
  Chrome or Firefox: current Chrome encrypts its cookies so only Chrome can
  read them, on purpose, and impersonating it to lift them would be fragile and
  the wrong side of a line the browser's authors drew deliberately. Saved
  logins live beside the browser data, never leave the machine, and are listed
  and removable under Settings → Saved logins.

- **Each tab says where it is.** Under its name: the git branch it sits on, the
  pull request that branch is on, and any ports it is listening on —
  `main #12 :3000`. Both are cheap to know and
  expensive to ask for; with six agents running, "which one is serving on 3000"
  otherwise costs six tab switches. The branch is read from the file git keeps
  it in rather than by running git, so a huge repository costs the same as a
  small one and one mid-rebase answers anyway; a worktree reports its own
  branch, which is the case that matters when several agents share a
  repository. The ports are matched against everything the tab started, since
  what listens is almost never the shell — it is the dev server three
  processes down. All of it goes away when it stops being true.

  Pull request numbers use the GitHub sign-in already on the PC — what `gh`
  stored when you logged in, or `GITHUB_TOKEN` if you keep one. Nothing to set
  up and nothing stored by this app; asking someone to paste a token into a
  second place so a terminal can show them a number they can already see on a
  website is not a trade worth offering. Where there is no sign-in there is no
  number, and the settings say so rather than leaving it a silence to guess at.
  An open one shows only `#12` — the word would be on every row and tell nobody
  anything; `merged`, `closed` and `draft` each get one, because each means
  "stop waiting for this".

- **Which key does what can be changed**, and it is changed by naming the
  action rather than the key: you say what you want *split the screen* to be,
  not what `Ctrl+B %` should become. The settings screen lists every action
  with the key it answers to right now. The prefix key moves too, which is the
  first thing anyone arriving from tmux wants. A key can also be given back, if
  you would rather your shell had it. The help screen is built from the same
  table the window dispatches on, so it shows the keys you actually have —
  including the ones you moved — and an action you switched off is not listed
  as though it still worked.

- **The colours can be changed, by the name you already call them.** The list in
  the settings is every scheme this PC already has — the ones Windows Terminal
  is carrying, plus any scheme file dropped into `config/themes`, in the format
  colour schemes are published in — so a theme you already use is picked by its
  own name rather than rebuilt by hand. What ships with the app is the app's
  own colours and nothing else: other people's schemes are other people's, and
  they arrive by being pointed at rather than copied in. The scheme reaches the
  terminal's sixteen colours *and* the window around them, so choosing a light
  one turns the whole app light, settings screen included, and the change lands
  on the window that is open rather than the next one.

- **An overlay is no longer hidden behind a browser tab.** The help, the
  workspace list and the pairing QR are drawn by the page; a browser tab is a
  window of its own living inside ours, and no amount of stacking puts a drawn
  thing over it. Browsers now step aside while something is being shown over
  the screen, and keep their pages while they do.

- **The terminal's font can be changed.** Ctrl+wheel over the terminal makes it
  bigger or smaller — the gesture people already try in a terminal, a browser
  and an editor, so it costs no key and needs no telling. The size is kept in
  the settings, where the font itself can also be named. Changing the size
  re-measures the cell grid, so the program in the tab is told the rows and
  columns it really has rather than being left drawing at the old shape.

- **The history can be searched.** In copy mode (`Ctrl+B [`), `/` opens a line
  to type into and jumps to the match, `n` and `N` walk through the rest. The
  search wraps rather than stopping at the end — copy mode opens at the newest
  line, where a search that only looked one way would answer "not found" about
  something sitting plainly above. The match is put near the middle of the
  screen, so what came before and after it are both visible.

### Fixed
- **Messages no longer hide behind the input bar.** The message line had no
  place in the stacking order, so anything it said while the sub-input bar was
  open was drawn underneath it and seen by nobody.
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

[Unreleased]: https://github.com/styleio/ShikishaTerm/compare/v0.3.5...HEAD
[0.3.5]: https://github.com/styleio/ShikishaTerm/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/styleio/ShikishaTerm/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/styleio/ShikishaTerm/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/styleio/ShikishaTerm/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/styleio/ShikishaTerm/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/styleio/ShikishaTerm/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/styleio/ShikishaTerm/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/styleio/ShikishaTerm/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/styleio/ShikishaTerm/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/styleio/ShikishaTerm/releases/tag/v0.1.0
