# SHIKISHA-TERM UI style guide

Every change to what is on the screen follows this page: layout, colour, type,
spacing, which part to use, how it behaves, what it says. The values live in the
code; this page says what each one is for, and how a screen is judged before it
ships. The Japanese version is [STYLEGUIDE.ja.md](STYLEGUIDE.ja.md).

**Do not invent a value this page already has a role for.** No new hex colours, no
new font sizes, no new shadows, no new radii. When a role is missing, add it here
first, then use it.

---

## 1. Source of truth

| What | Where | Note |
|---|---|---|
| Colour tokens | `src/theme.rs` `css_vars()` | Written out from the chosen colour scheme at start; every page reads them as CSS variables |
| The board page (tab bar, panes, status line, dialogs) | `src/shell.rs` | One page serves the window and the phone |
| The settings page | `src/webui.rs` | Same tokens, three of them under older names (`--muted` `--accent` `--danger`) |
| Toasts | `src/toast.rs` | Shared by both pages |
| Every word on screen | `lang/en.json`, overlaid by `lang/ja.json` | Never a literal in the page; see §7 |
| How words are chosen | `.claude/RULES.md`, the sections on words shown to users and on writing Japanese | This guide does not repeat them |

The board page is rebuilt from a state object several times a second. Anything
drawn on it must survive being redrawn: build once, touch only what changed
(`dataset.said`, `dataset.key`), and never keep a press's target out of the DOM.

## 2. Colour roles

Colours are mixed from the scheme's own two ends, so a light scheme works without
a second palette. Name the role, never the colour.

### Surfaces

| Token | Role | Use it for |
|---|---|---|
| `--bg` | The page | The terminal's ground, dialog fields |
| `--panel` | A surface set on the page | Tab bar, status line, cards, menus, bubbles |
| `--raise` | Something chosen or grouped | The selected row, a household box, the thanks card, dialog buttons |
| `--hover` | Under the pointer | Rows and menu entries on hover only |
| `--sunk` | Set into the page | Read-only wells |
| `--tint` | Leaning toward the brand | A bar that is loading, a button that is armed |
| `--line` | Structure | Dividers, card and dialog borders, empty bar tracks |
| `--sel` / `--cursor` | The terminal's own selection and cursor | The terminal only; from the scheme when it names them |
| `--panel2` / `--sys` | The settings page's second and third surface | Settings page only, where they predate `--raise`; a new surface there uses `--raise` |

### Edges

Structure and a control are not the same job, and drawing both with `--line`
left forms looking like a flat sheet with words floating on it.

| Token | Role |
|---|---|
| `--line` | Structure: what divides or encloses. Quiet on purpose |
| `--edge` | The edge of something you can type in or press |
| `--edge-hi` | The same edge with the pointer on it |

### Text

| Token | Role |
|---|---|
| `--text` | What is read |
| `--dim` | What is glanced at: labels, meta, the folder name of a row |
| `--faint` | The line that explains a control, and placeholder text |

Three steps, not two: with the label and the sentence under it at the same
weight, a form has no order to read in.

### State (the only colours that carry meaning)

| Token | Means | Where it may appear |
|---|---|---|
| `--brand` | **Focus and "answered"**: the focused pane's underline, the DONE dot, the selected row's edge, the primary button's outline | Never as a border for structure; never for text that is not a state |
| `--live` | **Working**: the BUSY dot, a bar that is filling normally, AUTO ON | |
| `--warn` | **Needs a person, or nearly used up**: the QUESTION dot, a limit notice, a bar past 80%, a folder that drifted behind | |
| `--pick` | **Picked for an AI to fill in**: the border of the box somebody pressed to have the guide's ? write in it | Only what a person picks and a person can unpick. Never put on by the program |
| `--stop` | **Stopped or dangerous**: the EXIT dot, the stop button, a bar past 95%, delete | |

A colour used for a state must not be used for decoration nearby. A blue rule
reads as "a pane starts here" wherever it is drawn.

### The AI's own colour

Each AI has one colour (`--ai`: claude `#d97757`, codex `#19c37d`, gemini
`#4285f4`, deepseek `#5b7cff`, qwen `#a06bff`, aider `#e5644d`, kimi `#12b3a8`).
It is worn exactly twice on a tab row (left edge and name) and once in front of
anything that is that AI's, such as its allowance on the status line. Never on a
dot, never on a border of structure, never mixed with a state colour.

**On the tab being looked at, and on no other.** An unselected AI tab is the
quiet row a terminal gets: name in `--text`, mark in `--dim`, left edge bare.
Lit at once, the colours say what selection says, and the tab that really is
selected has nothing left to say it with. Weight is not colour: a name that is
600 stays 600 either way, so nothing moves as tabs are switched. A summary of
tabs that are put away is not a tab, and keeps its colours.

### A tab's face

Each tab has a face (`crates/core/src/face.js`, seeded by the desk's and the tab's id).
It is one of the robots of the orchestra the person conducts: the robots of the
site's pictures (a round head, a dark screen for a face with light eyes, an
antenna, ears), drawn flat with no outline, each holding one instrument. The
head, the ground, the antenna, the eyes and the instrument all come from the
seed. At 14px the instrument and the ears are left out and the head is drawn
larger. The values are the app's, not the scheme's: a face is who a tab is,
and it must not turn into someone else when the scheme changes.

Heads are `--player1` to `--player7` (the site's robots: coral, yellow, green,
pink, blue, lavender, teal), grounds the pale colours of the same seven families
`--player-back1` to `--player-back7` in the same order, and a head is never put
on the ground of its own family -- forty-two pairs, and no two tabs of a desk wear the same one; the head fills most of the circle, so up to seven tabs each get a head colour of their own before any is worn twice. The screen and the instruments' lines are
`--face-ink`, the eyes `--face-ink-light`.

| Family | Head | Ground |
|---|---|---|
| coral | `--player1` `#ff7a59` | `--player-back1` `#ffd9cc` |
| yellow | `--player2` `#ffcf3f` | `--player-back2` `#fff0b8` |
| green | `--player3` `#5fd39a` | `--player-back3` `#c9f1d8` |
| pink | `--player4` `#f590c8` | `--player-back4` `#ffd9ee` |
| blue | `--player5` `#5fb4ff` | `--player-back5` `#cfe9ff` |
| lavender | `--player6` `#8f7cf5` | `--player-back6` `#ddd5ff` |
| teal | `--player7` `#4fcfc4` | `--player-back7` `#c8f0ec` |

The faces before them (`vendor/boring-avatars/beam.js`) are kept: `FACE_LOOK`
in `shell.rs` set to `"beam"` puts them back. They use these five colours.

| Variable | Value |
|---|---|
| `--face1` | `#5fa0f5` blue |
| `--face2` | `#f096a6` pink |
| `--face3` | `#ffce2d` yellow |
| `--face4` | `#8390eb` lavender |
| `--face5` | `#f7cd9e` apricot |
| `--face-ink` | `#212021` eyes and mouth on a light body |
| `--face-ink-light` | `#ffffff` eyes and mouth on a dark body (none of the five today) |

**Inside the face's disc and nowhere else.** Not on a name, a dot, a border, a
bar or a background. The blue sits near `--brand` and the yellow near `--warn`;
out of the disc they would read as a state. In it, two of them together with
eyes and a mouth read as a character. No shadow and no outline: the faces were
checked as they are on both a light and a dark scheme.

Unlike the colours above, these mean nothing. Two faces in the same colour are
not saying the same thing.

### Contrast

Text on `--panel` and `--raise` must be `--text` or `--dim`, never a state colour
at 10px. Check every screen in one dark and one light scheme (the settings pick
them by name); `--dim` sits at 65% of the text on purpose because lighter was
unreadable on gentle schemes.

## 3. Typography

Two surfaces, two jobs. The board **is** a terminal, and is set in `--mono`
throughout -- that is the program's face. The settings page is a document you
read and a form you fill in, and is set in `--ui`; long Japanese prose in a
monospaced fallback is harder to read, not more characterful. On both pages
**anything a person could type back** -- a name automation uses, a command, a
path, a URL, an id -- is `--mono`, so that what is quoted looks quoted.

Hierarchy is made with size, weight and the three steps of text, never with a
third family.

| Size | Role | Examples |
|---|---|---|
| 14px | The terminal's own text (`--fs`), and a checkbox's label |
| 13px | Body of a form: what is typed into a field, a row that can be pressed | Fields, tab rows, menu entries |
| 12.5px | A button | Every button on the settings page |
| 12px | A field's name, a heading, a line of status | Form labels, the status line, the project's heading in a household |
| 11.5px | A quiet heading, and the line that explains something | A folder heading that is not a household's head, hints |
| 11px | Secondary | Chips, counts, hints under a control |
| 10px | Meta under a row | What a tab last said, where it is (branch, ports), the branch pill on a heading |
| 9px | A caret | ▾ ▸ only |

Weight 600 belongs to the one row that names the thing (the project heading, a card
title, the AI's name). Nothing else is bold. Letter-spacing `.02em` on 11-12px
headings only.

Numbers that are compared line up: `font-variant-numeric: tabular-nums`.

## 4. Spacing, radius, elevation

- A row is `padding: 7px 10px; gap: 8px`. A row's children step in: a tab under
  its folder by 26px, a branch under its project by 14px more.

**Space comes in steps, and every gap is one of them.** Without them a label sat
as far from its own field as the field sat from the next question, and a form
read as one long list of unrelated lines.

| Token | Value | What it separates |
|---|---|---|
| `--s1` | 4px | Parts of one word: a title and the "(optional)" after it |
| `--s2` | 8px | A label and its field; a box and its own text |
| `--s3` | 12px | Things in a row; the inside of a control |
| `--s4` | 16px | One card from the next |
| `--s5` | 20px | One question from the next; the inside of a card or dialog |
| `--s6` | 24px | The side margin of a dialog |
| `--s7` | 32px | The top of a page |

**Controls come in two heights.** A field is **36px** and a button **32px** (the one exception is §5's closing primary button), so a
button beside a field reads as the smaller thing and a row of buttons has a
rhythm of its own. Nothing is shorter: a 32px box with the text pressed against
its border is the shape of an admin panel from fifteen years ago.

| Token | Value | For |
|---|---|---|
| `--r-ctl` | 6px | What you press or type in: fields, buttons |
| `--r-card` | 10px | What holds them: cards, dialogs, floating menus |
| `--r-chip` | 4px | What labels them: chips, small pills |

`3px` for a bar and `999px` for the status line's pills and the quick command launcher's plates are the only exceptions.
- Elevation: one shadow, `0 8px 24px #0007`, and only on a layer that floats
  over the page (menus, bubbles, dialogs). Nothing set into the page has a shadow.
- Layers, from the bottom: panes (3-6) · composer (18-19) · bars over a pane
  (22-26) · reader (30) · the phone's drawer (30) · splash (40) · veil (50) ·
  the first-run bubble (55) · menus and the net veil (60). Take a slot that exists;
  do not add a number.

## 5. Parts

Use the part that exists. Adding a new kind of part is a change to this page.

| Part | What it is for | Anatomy |
|---|---|---|
| **Tab row** | One thing running | dot (state) · number · name · lock · activity bars; under it, where it is, then what it last said |
| **Folder tab** (`#strip`) | Switching inside the folder in view | mark (state colour) · name · ✕. The ✕ shows on the tab in view and the one under the pointer only, and keeps its room when hidden; a middle click closes too. After the tabs, `+`; at the far end, `▾` with the recently closed tabs, only while there are some. Closing asks first only when an AI's work would be cut off, and the app asks it, not the page |
| **State dot** | What the tab is doing now | 8px circle. **Colour says what kind of news, fill says whether it wants you.** Filled = somebody's turn right now (`BUSY` `QUESTION` `DONE` `FAILED`); ring only = the same news with nothing for you to do this minute (`BACKGROUND` `LIMIT` `EXIT`). Ring is `inset 0 0 0 2px`. `BUSY` is the only one that blinks |
| **PROJECT heading** (`.projhead`) | The divider over the list, and what acts on projects as a whole | quiet heading "PROJECT" · at its end 22px line-art buttons: sliders (how the list is drawn; only with two or more folders) · folder-plus (add a project; this one blinks on a first start) · + (new worktree) |
| **Project heading** (`.phead`) | Names one project, once | mark (git = the project's colour as a 10px square; not git = a line-art folder) · name 13px/600 · [while folded, the dot of whichever state is most urgent] · ▾ (fold the whole project) · `+` (git only: make a worktree). Every folder of the project stands under it as a card. The colour and the `+` belong to the heading; no card repeats them |
| **Folder card** (`.tab.folder.wcard`) | One working folder of a project (the checkout and its worktrees) | state dot · name 13px (`--dim` while it is written from what the folder's AIs are asked — Auto — and `--text` once a person names it) · ["primary" pill at 10px, the checkout only] · drift · a second line with the branch (10px mono `--dim`). Resting the pointer shows `[name]` and the summary (never the path). Where there is no pointer (`@media (hover: none)`) the summary is a line of its own between the name and the branch (10px mono `--dim`, cut with …). Held down (500ms, cancelled by moving 10px) a card opens the same list its right-click does, and when it has a summary the list opens with a heading that cannot be pressed (the name at 600, the whole summary under it at 11.5px `--dim`, a `--line` under both). The folder and what is inside it (the "N tabs" row, and the tab rows once brought out) stand in one box (`.fcard`): a 1px `--line` edge with 10px corners, `--s2` in from the column's sides, `--s4` to the next card. The rows inside draw no box of their own. **The box of the folder whose tab is in front wears `--brand` on its edge** (never told apart by a shade of its text alone). The checkout first, the worktrees under it as siblings. A card does not fold itself (its tabs fold from the "N tabs" row, the whole project from its heading). Only when the list is grouped by state do folders drop the card for the older single row that names its project and wears a branch pill |
| **Found worktrees row** (`.found`) | Says once, under its project, the worktrees git knows and the desk does not list | a quiet line right under the heading (11px `--dim`) · ▸ · "Hiding N found worktrees" · a 22px ✕ at the right (keep them hidden). Opened, a `--line` rule on the left and, per place (10px mono path and a count pill), up to three names, then "N more". **Each name carries a 22px line-drawn bin at its right** — `--dim`, `--stop` under the pointer — which throws that worktree away for good, through the same question a folder on the desk asks. It is a bin and not a second ✕: the ✕ above it only puts a line out of sight, and one mark for two acts, one of them undoable and the other not, is the mistake waiting to happen. Last, one line of explanation and two plain buttons (keep hidden · show on the desk). A kept project offers them back from the heading's right-click |
| **Folders put away row** (`.hidrow`) | Says that folders were put out of sight until the next launch, and brings them back | one quiet line at the top of the list (11px `--dim`), whatever the number: "N folders hidden" · a plain control at the right (Show them). **One line for all of them, never one each** — the list is only as wide as the column |
| **The folder that is not here** (`#held`) | A tab whose working folder is not on this machine: what is wrong, and every answer to it | a card in the focused pane's rectangle, where the terminal would be. Width `min(560px,100%)`, `--panel` with a 1px `--line` and `--r-card`, inside `--s5`, `--s7` from the top. Title 13.5/600 · one line (13px `--dim`) saying nothing is running here · the path in a `--sunk` well (11.5px mono, it is what the person goes and looks for) · [the reason a button is grey, 11.5px `--warn`] · a `--line` rule, and under it the answers: the breaking one at the left (`--stop` text, take the setting off the list) and, held to the right as one group, put it out of sight · choose another folder · the main one last (fetch it here). Under them one line 11.5px `--faint` about what the settings are shared with. The three on the right stay one group so a narrow width puts them on their own line with the main one still last (§5.3). Pressing the main one opens the dialog that shows what will run before it runs |
| **Worktree being made** (`.making`) | Says how far a worktree that was pressed has got, under its project, until it exists | a `--line` framed row under the heading (after the found worktrees row). The working dot (`BUSY`, the only blink) · name 13px · a 22px ✕ at the right (stop, and take back what was made) · on the second line what it is doing now (11px `--dim`, "Creating the worktree…"). While stopping the ✕ goes and the line says so. Failed: the dot becomes a `--stop` ⚠, the frame `--stop` at 35%, the second line the reason (`--stop`, wrapping), and under it two plain buttons (Try again · Dismiss). Made: it turns into a card in the same draw the desk reads the folder in. While files are being copied or deleted, a line under the second one says what, how many files of how many, and for how long (11px `--dim`), with a progress bar under it. No spinning mark (§6). Shown even with the project folded |
| **Pill** (status line) | One fact about now | text or a bar with words; pressable when pressing does something (limit notice), otherwise plain |
| **Server mark** (`.smark`) | The name a person gave a server ("Production", "Staging"), worn by whatever reaches that server | a radius-4 chip: an 8px square of the chosen colour and the name (11px `--text`), on a 14% tint of the colour with a 55% border. **Never on a server with no name** -- there is no colour-only mark. It has no state meaning and never paints a row or a button. Worn on the tab row (after the name), the tab over the panes, a pane's caption, a put-away set of tabs (its second line), the card of a folder on a server (leading its second line), the file panel's connection and its server-side crumbs, and leading the well of a question about something that cannot be undone. Up to 128px in a row, 96px in the tab over the panes and a caption; over the panes it keeps its width and the row scrolls, in a caption it shrinks after the name does. When the server asks for its name to be typed, that question adds a field for it under the well (5.1), and the primary button stays grey and answering (5.4) until it is typed, in any case |
| **Speed tag** (`.castspeed`) | How quickly a browser tab's page is driven in words, in its 📼 panel (whether a decision model decides) | One grey tag just left of the ⚙ (this page's models): 12px `--dim`, `--line` border, 4px corners. "Fast" when a decision model decides, and not pressable. Otherwise "Slow", pressable (`--text` under the pointer): the press opens the page's settings at In words, with the reason and the fix in a `--warn` box and the Decision model field lit (`.lookhere`, the glow that says where). The box goes once a decision model is chosen. Speed is said in words, not colour: both are grey |
| **Bar** | A number between 0 and 100 | `--line` track, `--live` fill, `--warn` past 80, `--stop` past 95; always with words beside it saying what it is of and when it resets. A bar of progress (copying, deleting, cloning, sending) keeps `--live` to the end: nearly done is good news, not a limit nearly reached. A wait that cannot be counted but has a known longest (a press that makes something on GitHub) gets a bar that reaches 100% at that many seconds, moved one step a second, with "N s (at most M s)" beside it; it fills at once when the answer is in |
| **Dialog** (`.vbox`) | One decision | title · one sentence · fields · the exact thing that will run (mono well) · one primary button at the right, ✕ at the top |
| **Every conversation** (the conversation panel's `#convoMode` and `#convoHead`) | Find a past conversation on any machine, read it, and pick it back up | One search box, the conversation panel's. Under it the file list's two-way switch ("This conversation", "Every conversation"). On every conversation the checkboxes are put away and the list is rows after §5.5: the CLI (`--brand` 11px) · the conversation's name (its folder) · its tab (`@name`, 11px `--dim`) · a pin (line drawing, `--brand`) · how long ago at the right; under it the words it was found in (who said them in 11.5px `--text`, then the words in `--dim`, the search words painted `--tint`). At the foot, the machines still being searched, those that could not be (`--warn`, by name), and the press that searches the paused MicroVMs. A row opens that conversation in the same panel as "This conversation", the place it was found marked, with a head (`#convoHead`): a quiet "← Every conversation" and an ordinary "Resume" at the right (with ▾ when the folder is gone, opening an `.fmenu` of the gone folder, the branch made into a worktree again, and the desk's folders by name and path). Tool runs, folding and marks are the panel's own |
| **Right-hand column** (`#side`, `SIDE_PANELS`) | Beside the tab in front, only what that tab is used with | The strip (`.sbar`) lists only the panels the kind of tab in front has: a page = Console; an AI = Files · Git · Chat; a terminal or an editor = Files · Git. A folder git does not hold has no Git, and a tab with no folder no Files. **What does not belong is taken off the strip**, not greyed (pressed, it could only say "not here"). The panel chosen is remembered per kind (`side_panels`); when the kind in front has no such panel, the first on the strip stands, for this drawing only (the choice is kept). **Calling a panel up** (a button that opens a feature) is one function, `sideReveal(name)`: open the column, add the panel to the strip, switch to it. A called panel's button carries a 22px ✕ inside it (11px `--dim`, `--hover` under the pointer), and stays on the strip over other tabs until its ✕ is pressed. A tab with nothing beside it says so in the column's empty line (`.sempty`) |
| **Chat** (the right-hand column, `#convopanel`) | Read what was said in the AI tab in front, newest first, and narrow it by who sent it | The column's panel while an AI tab is in front (and over any tab when called up; see the Right-hand column row). From the top: the file list's search box ("Search this conversation") · the checkboxes of 5.1 in a row that wraps (Sent by you · Sent by AI or automation · AI replies · Tool runs · Events · Pinned only) · the rows · a line at the foot (`.fsay`). **A row** is the reader's face and `.rturn`: over it, who said it (11px/600) and when (`--faint`, tabular-nums), and at the right a 22px line-drawn pin (`--brand` while pinned). A person's line carries the reader's rule on its left; what another tab, a job or automation sent carries that rule in `--edge-hi`, to tell the two apart. A note sits under the line, 11.5px `--dim`. Waits for an answer, stops and the like are quiet lines with their time (11.5px `--dim`); where a conversation began is centred 11px `--faint`. The work between two things said is the quiet "▸ Tool runs (N)" line of the past-conversation reader. Right-click (a long press on a phone) opens an `.fmenu`: Pin · Write a note · Copy. Typing narrows at once, and a moment later every conversation of the tab is searched. **Matches in kinds that are not shown** are said at the foot, "N more matches in kinds that are not shown", with an ordinary [Show them]. Newest first, so "Read earlier messages" is under the rows |
| **Dialog** (settings, `.framed`) | One record, edited | **header** (title · ✕) · **body** (fields) · **footer** (destructive at the left, then the reason it cannot be saved, then Cancel and the primary at the right), each divided by a `--line` |
| **Dialog** (choosing, `.picker`) | One place out of many | The same header, body and footer as `.framed`, with the body in **two columns** (left 200px = places to start from, right = breadcrumb · list). Width `min(760px,100%)`, body at a fixed height so the window does not grow and shrink with what is listed. A filter field under the header. One press on a row **selects** it; `›` or a double press **goes in** (on a phone `›` is the only way in). Footer: what is selected · an **add** button at the left · Cancel and the primary at the right. Line-drawn marks, no emoji. At 640px and below the left column becomes a row across the top |
| **A question stood over the board** (settings, `#floatbox`) | Ask one thing in the middle of work on the board (adding a tab, a new project's worktree rules) | The settings page itself becomes the dialog over the board; the settings' header and column stay out of sight. Header (title · the one fact · ✕) · body · footer ("More settings" at the left, Cancel and the primary at the right). Sized as §5.2's dialog or sheet. Esc and ✕ close it |
| **Field** | One thing to fill in | its name above it (12px `--text`), the control, the line that explains it under (11.5px `--faint`); `--s2` between them and `--s5` to the next field |
| **Boxed list** (`.rows`) | Several of the same thing | one border round the whole, `--line` between rows, `--panel2` on hover, a `›` at the right when the row opens something |
| **Sidebar** (settings) | Which setting is being edited | One column, the larger world first: the **General** heading and its entries · the **Desk** heading (initial plate · name · `▾` listing the desks; choosing one switches what is under it) and the desk's entries · the **Projects** heading with one row per project (colour square · name · path under it; a folder in no repository wears a line-art folder). Each row opens one page (a name and one line under it). No place to enter and leave. **No button adds anything** (projects, worktrees and tabs are added on the board). Worktrees and tabs are not listed: they open from the board's right-click (Project settings / Tab settings / Edit…) or from a row on the project's page. While such a page is open its project's row stays selected. A project, folder or tab page starts with breadcrumbs (desk › project › folder › tab; 12.5px `--dim`, the levels above pressable, the page itself `--text`/600 and not). A right-click menu opens where the pointer is |
| **List** (`.fmenu`) | Choose one of a few | floating, radius 10, one line per choice, closes on any press elsewhere |
| **Mention** (`@`, `.fmenu.mentions`, `#castmirror mark`) | Naming another tab of this desk (an AI, a terminal or a page) inside what the input bar sends | An ordinary `@` button right of the 📎 in the input bar, shown only while the tab in front is an AI (sent to a shell, the words would run as a command). The `@` button, or an `@` typed at the start of a word, opens a list (`.fmenu`) **over the bar**. A row is the mark the tab wears (`markFor`) · the name 13px · at the right the folder name 11px `--dim`. Nearest first (the same folder, then the same project, then the rest); the tab in front and shells are not offered. What is typed after the `@` narrows it. Arrows walk, Enter or Tab take, Esc puts it away. A pick leaves a **chip** in the text (`--raise`, corner 4px, a 1px ring of `--edge`), painted on a layer under the field so the letters never change width. Two tabs with one name read `@name (folder)`. Backspace takes a chip out whole. On sending, each chip becomes the tab id (`<@ID>`, the shape Slack sends). With nothing to offer, one line says why and what would make something appear. **When the AI in front has no skill yet**, the list is this card (`.mskill`) and nothing else (tabs listed under it read as noise and were passed over, and a pick reached an AI that could not act on it): title 13.5px/600 "Getting ready to ask other tabs (once)" · one sentence of what it does, 13px `--dim` · "Written to" 12px and the path in a well (`--sunk`, monospace 11.5px) · where to take it out, 11.5px `--faint` · at the right a quiet "Close" and the primary "Install". Closed, it comes back at every @ until the skill is in. Right after installing, one line above the tabs says it is in |
| **Way card** (`.apway`) | Choose the *way* to one result (adding a project: browse a folder, clone from a URL, create new) | One card is one pressable thing. A 28px tile with a line-art mark on the left, the title 13px/600, one line under it at 11.5px `--dim`, a ⏎ chip at the right end. **The way nearly everybody takes** stands alone on top as its own bordered card (at least 60px, mark in `--text`); the rest sit below in one enclosed list (at least 52px each, `--line` between) under an "Other ways" heading. Focus lands on the lone card the moment the dialog opens. The card the keyboard is on wears `--raise` and a `--brand` border and shows the ⏎ chip, so what Enter will do is always on screen. ↑↓ walk the cards. Pressing one commits nothing; the page it leads to does |
| **Where** (`#addproj .bpick`) | The machine a project is added on (this PC or an SSH host) | Only once there is a host, as a "Where" field at the top of the page. A 36px picker (line-art mark · name · the address at the right in 11px mono `--dim` · ▾). Its list is ✓ · name · address, then, under a rule, "+ Add an SSH host". With no host yet, an "A project on an SSH host" card sits in the list of other ways instead. On a host, a way that cannot be taken there (create new) is grey, and a press says why in `--warn` under the list (§5.4) |
| **A folder over there** (`.aprlist`) | Walk an SSH host's folders and pick one | the folder field (mono) with a refresh button; under it where it is (11.5px mono `--dim`) with a "git repository" pill when it is one, and a list fixed at 240px (30px rows · line-art mark · mono name, `↑ ..` first). One press goes in. While asking, the list says so; when the host cannot be reached, a `--warn` box in the list (what did not happen · the far end's words · Try again). The foot's primary button adds the folder being looked at, asking first when it is not a repository |
| **Closing primary button** (`button.go.wide`) | The one button that finishes a page (Clone, Create project) | The dialog's full width and **36px** tall -- the exception to 32px buttons: it is the only thing to press on its page, so it stands a step taller to say "here". Grey per §5.4 until it can go; a press then says why just above it and rings the field. A progress bar goes **under** it, so the button never moves |
| **Choice card** (`.scard`) | Inside a dialog, choose one of a few things told apart by their face (the AI in the first-start setup) | One card is one pressable thing (`role="radio"`). Radius 10, an `--edge` border, at least 56px tall, `--s3` inside. The name 13px/600, and under it the name a person would type (mono 11px `--dim`). An AI puts the mark its tab wears (`aiMark`) in front of its name, and wears its colour where a tab row does: the left edge and the name. Picked = the `--raise` surface with a `--brand` border. A press does not rebuild anything; it only moves the mark (`aria-checked`). Laid out in a grid that wraps when it does not fit. Under the cards, one line of field description saying what choosing does |
| **Bubble** (`#coach`) | Point at the next thing to press | one sentence · ✕ · a corner toward the anchor; at most one on screen |
| **Card** (sidebar) | Ask once | title · one line · two buttons (one primary, one quiet); goes away for good |
| **Toast** | Say what just happened | transient; never the only place an error lives |
| **Button grid** (quick commands, `#quick` / `.qegrid`) | Buttons a person made, found by where they are | Each button is square, corner 10, `--edge` rim. Its face is a line icon (Lucide, the shared part in `quick.rs`) and an 11px name on one line; only a prompt for an AI carries a small "AI" at the top left. **Launcher**: the buttons alone float on the dimming (`#00000099`) in `--panel` with the shadow `0 8px 24px #0007`, and empty places are not drawn. The dimming is dark in every scheme, so no words are written on it directly: they ride on `--panel` plates with 999px corners. Above: a plate with which folder, as a trail, and a plate with the key hint, ⚙ and ✕. Right under the buttons: a plate of page numbers and a one-line plate saying where the button under the pointer sends. Rows no page of the grid uses are not drawn. The buttons come in over 0.2s on opening and on turning a page only (not with reduced motion). A button with nowhere to go is the grey of §5.4 and says why on that line when pressed. Where the grid does not fit, the page's buttons stand in reading order. **Editor**: empty places are drawn dashed; press to pick, drag to move, press a folder twice to enter it. The first place inside a folder is always the way back |
| **Reorderable list** (quick actions in the settings, `.alist`) | Records whose order means something, set out in reading order to change | The boxed list of §5.5 with a 22px grip at the start of each row (the `grip-vertical` line drawing, `--faint`, `touch-action:none` so a finger can hold it). The row stands in columns: grip · name 13px · chip (Text or Lua; a Lua that does not parse wears a second, `--warn` chip) · what it sends (Lua in 12px mono, text in `--dim`, cut with …) · `›`. **The whole row is the way in**: it opens the dialog of §5.2 (`.framed`: name · text to insert or Lua to run · the Lua switch; delete at the left of the foot, cancel and save at the right). A folder's row opens its dialog too, and not by a second press: the first thing in its body is the way inside, a plain button the body's full width (the folder's line drawing · "Edit what is inside" · how many it holds in `--dim` · `›`), then the name. The order is the bar's order, left to right. Holding the grip lifts the row on `--raise` with the shadow; the other rows make way as it passes their middle, and the order is written once it is put down. From the keyboard, Alt+Up/Down moves it one row at a time. On a phone the row stacks as §5.5 says: name and chip on the first line, what it sends under them. The button that adds (plain) sits under the list |
| **Ideas** (`#ideas`) | Writing down what comes to mind, a card at a time (the 💡 in the bottom row, left of ✂️) | One box of the §5.2 measures on the quick commands' dimming (`#00000099`, the same layer): width `min(640px,100%)`, 56px from the top of the window, no motion. **Head**: the title "Ideas" 13.5/600 · a project choice (36px; first "No project", then the git repositories, a checkout and its worktrees being one; on opening, the project of the folder in front) · at the right an ordinary button "Show done ideas" (`--raise` with a ✓ while on) · ✕. At 640px and narrower the ✕ stays at the end of the first line and the button takes the second. **Body**: cards one under another, `--s2` apart, and last the writing line "New idea", where the caret is on opening. A card is typed into, so it is `--bg` with an `--edge` rim and corner 6, and wears the §5.1 ring while the caret is in it. Left to right: a 22px grip (six dots, `--faint`, held by a finger too) · a 15px check box · the words at 13px, as tall as they are · at the right end 22px line-icon buttons (copy, send to Issue). Done is `--dim` and struck through. A card sent to an Issue is done once that issue is made, and wears its number right of the words (`#12`, 11px monospace `--dim`, `--line` rim, corner 4), which opens the issue. Only the card being carried floats, on `--raise` with the shadow. A right-click on a card (holding its grip, on a phone) opens a list (`.fmenu`): copy · send to Issue · mark as done / not done · last, in `--stop`, "Delete". Done comes back with "Show done ideas"; deleted is gone from the file and does not. Delete is not asked again: the menu is already the second press. **Foot**: 11px `--faint`, "Enter: next card · Shift+Enter: new line · Saved as you type". There is no Save button. A file that cannot be read or written leaves its reason in a `--warn` box under the head. Esc, ✕ and a press on the dimming close it |
| **Ask bar** (`#ask`, `.pask`) | A script needs a person to do something on a page | one sentence · one primary button; `--warn` top edge (needs a person); drawn by the board under the page and never inside it, so only a person can press it; stays until the script takes it down |
| **Picking for an AI** (🎯, `#castpick`) | Point at parts of a page and hand them to an AI tab as a draft | a panel of the input bar on a page, one row that scrolls sideways: the switch (plain; `--raise` and a `--brand` edge while on) · one chip per element (`--raise`, `--edge`, radius 4px, 32px: its number 12px `--dim` tabular-nums · what it is · a note box · a 22px ✕) · "Hand to an AI ▾" in the `--brand` edge and letters (not filled: Send beside it is the main button) opening the AI tabs as the @ list draws them · a quiet "Clear". On the page itself, drawn by the page in a closed shadow root: a 2px outline over what the pointer is on, a small mono label above it, and one pill at the top saying what a press does and how many are picked. Nothing covers the page, so the wheel and a finger still scroll it |
| **Develop** (the bar over a page, `.navdev`, `devRows`) | A page builder's tools in one place | At the bar's right end, the line drawing `develop` and the word "Develop" (12px `--dim`, a `--line` edge as a thing to press; as wide as its word rather than the others' 28px square). Pressed, an `.fmenu` (`devlist`): Hard reload · Pick elements for the AI · Developer tools · Source code · DOM. A page tab's right-click menu has the same rows less the reload (the same `devRows`). A list opening over the page makes the page step aside while it is up (the page is a window of its own over this one, and would hide the list) |
| **Picked elements** (the column, `#pickpanel`) | Called up by "Pick elements for the AI"; picks get a note each and go to an AI | A called panel (its button carries a ✕). Built like the console: a head with Pick / Stop picking (on: `--brand` edge and a 14% wash) · at the right "Hand to an AI ▾" (`--brand` edge and letters) and a quiet Clear. A row: its number (11px `--dim`, tabular-nums) · what was picked (12px, … when long) · a 22px ✕ at the end, and under it the note as a field of 5.1 (36px). The foot says what to do next (11.5px `--faint`) and how many values were hidden (`--warn`). On a phone the column covers the page, so starting to pick puts it aside, and the edge brings it back |
| **Ports** (the column, `#portpanel`) | See what listens in the folder the column stands on, and open it | A called panel (its button carries a ✕). Called up from a port's chip on a folder card's second line (after the branch) and on a tab's row (10px mono, dotted underline), from a folder's menu ("Show the ports" here, "Open the server's ports" on a server, "Public URLs" on a MicroVM), and by `show_panel("ports")`. A head line saying what the list is (12px `--dim`); on a machine far away, a plain Ask / Ask again at its right (line-drawn refresh mark). A row is at least 36px and opens the port in a browser tab when pressed: the port (mono, 600, tabular-nums) · two lines under each other (the program, 12px `--text`; the owning tab's mark and name, 11px `--dim`) · 22px line icons at the end (open in this PC's browser, copy the address). For a folder on this PC those two are on the window only (on a phone, localhost is the phone). A MicroVM's public address offers the phone its own browser and a copy. A machine far away is asked only when somebody presses (asking wakes a paused MicroVM; a change of the folder in front asks nothing); not asked yet / asking / nothing there / failed (`--warn`) is said at 11.5px where the rows would be, not at the foot of an empty column; a folder on this PC with nothing listening says there what starts a port. Far away, the list ends with "Have the AI start the server" (line-drawn ✨)
| **Console** (the column, `#consolepanel`) | What the page in view said on its console | the column's panel while a page is in front (the column's rule is the Right-hand column row). A head of five switches, one per kind (the file list's two-way switch made five-way: `--brand` edge and a 14% wash while on), then "Hand to an AI ▾" in the `--brand` edge and letters and a quiet "Clear". Rows oldest first, as a console reads: time 11px `--faint` tabular-nums · the kind (`--stop` for an error, `--warn` for a warning, `--dim` otherwise) · file and line, the whole address under the pointer · the words 12px mono, wrapping. A foot with how many of how many are shown, and, while the list is short, where it starts and a quiet "Reload the page" |
| **Button** | Do the thing | primary = filled `--brand` (one per screen or dialog); plain = `--panel2` on an `--edge`; quiet = no border; destructive = `--stop` text, no border. Cancel and Close are quiet, never coloured |

**A button that cannot be pressed still answers.** Grey it -- grey, not a pale
version of the live colour, which still reads as the live colour -- but let the
press through and say, in a place that stays, why it is held and what would free
it. Then take the eye to the thing that has to change. A control that greys out
and then ignores the press is a dead end, and the person has nowhere to go.

Three rules from the rows above that are most often broken:

- **A number never stands alone.** `20%` is not a fact; `20% used · resets in 3h 45m`
  is. Every number has a name (what it is of) and, where it changes, when.
- **A bar never stands alone either.** The bar is the number; the words beside it
  are the sentence.
- **A rule the person has to learn is a rule that is wrong.** If two things are
  written differently (`example.com` meaning one thing and `http://example.com`
  another), write both out in full instead. What is in the box is what is
  compared.

### Where each part is in the code

**Look here before writing a part.** The table above is how a part looks; this
is the function that makes it. A part listed here is made by calling this
function. Do not write a lookalike with a function of its own -- the moment a
second one exists there are two places to fix. A part that is not here is
written as a shared function, and given a row here in the same commit.

The function names are read by a test in `crates/core/src/webui.rs`
(`every_code_entry_in_the_style_guide_exists`), which fails when one of them is
not on its page. A name changed is a name changed here too.

<!-- code-entries -->
| Part | Function to call | Page |
|---|---|---|
| Dialog (settings, `.framed`) | `openModal(...kids)` (give the returned frame `.framed`) | settings (`webui.rs`) |
| Confirming something that cannot be undone | `confirmAction(message, action, other)` (`other` names a second way through, only when there are two; pressed, it answers "other") | settings (`webui.rs`) |
| A question stood over the board (`#floatbox`) | `frameOpen(spec)`. Drawn again with `frameDraw()`, closed with `frameLeave()`, "More settings" is `frameMore()`, Esc and ✕ are `frameCancel()` | settings (`webui.rs`) |
| Field | `sfield(label, control, hint)`. A field tied to a setting: `field(obj, key, ph)`, `check(obj, key, label)`, `checkDefaultOn(obj, key, label)`, `choose(obj, key, opts)`, `pathField(obj, key, ph, kind, title)` | settings (`webui.rs`) |
| A name and what it sets, on one line | `row(label, ...kids)` | settings (`webui.rs`) |
| Card | `card(title, ...kids)` | settings (`webui.rs`) |
| List (`.fmenu`) | On the settings page `floatMenu(at, items, opts)`; on the board `openList(anchor, rows, tall, point)` (two pages' JavaScript, which cannot be shared) | settings (`webui.rs`), board (`shell.rs`) |
| Breadcrumb | `pageCrumbs(...parts)` | settings (`webui.rs`) |
| Fields few people need, folded | `foldMore(label, open, ...kids)` (the label says what is inside; open when a field inside already has a value) | settings (`webui.rs`) |
| Picking a folder or a file | `choosePath(kind, title, now)` (the system's own dialog on this PC, a walk through the folders on a phone). With a field beside it, `pathField(obj, key, ph, kind, title)` | settings (`webui.rs`) |
| A question about something that cannot be undone (board) | `askQuestion({title, say, what, label, go})` | board (`shell.rs`) |
| A job handed out between AI tabs (`.job`) | `jobRow(j)`, placed under the row of the tab that leads it (`jobsOf(t)`): the job, each task's state and the tab on it (a press goes there), the person's decisions as buttons, and Stop (asked first with `askQuestion`) | board (`shell.rs`) |
| Dialog (choosing, `.picker`) | `openBrowse(at, handBack)` | board (`shell.rs`) |
| Settings stood over the board | `openSettings(section, ret, folder)` (window and phone alike; on a phone `openCfgLayer(params, size)` makes the frame inside it) | board (`shell.rs`) |
| Line-drawn mark | `pickIcon(name)` | board (`shell.rs`) |
| "+ Add a MicroVM" at the end of a MicroVM list | `addMicrovm(chosen)` (the settings' own form over the board; once it is saved, the function handed in is given the new MicroVM's name and carries the list on with it) | board (`shell.rs`) |
| Choosing the AI a MicroVM is given | `machineAiPick(value, onChange)` (starting from `defaultMachineAi()`: the assistant AI, else one this PC has) | board (`shell.rs`) |
| What a MicroVM signs in to the git server as | `drawSignIn(box, note, shown, change)` (the account, the kind of token, and what to use instead of one that never ends, in one place). The AI's sign-in is `drawAiSignIn(box, note)`. A fact is a line under the control (11.5px `--dim`); something a person has to do is 5.1's `--warn` box with the plain button that fixes it inside, drawn by `warnBox(text, ...acts)` | board (`shell.rs`) |
| 5.1's `--warn` box in a board dialog | `warnBox(text, ...acts)` (what a person has to know or do, and the plain buttons that fix it, in one box; used by the sign-in notes and by a worktree's slow copy) | board (`shell.rs`) |
| Progress bar (`.pbar`) | `progressBar(pct)` (returns `{bar, set}`; give `set` 0-100 to fill it). The words go beside or above it; never the bar alone | board (`shell.rs`) |
| Toast | `toast(text, warn)` (`toast.rs` puts it into both pages) | both (`toast.rs`) |
| Chat (the right-hand column) | `drawConvo()`. Rows are `convoSay(r)`, `convoWork(r)` and `convoEvent(r)`; a tool run's contents are `workPieces(inside, work, query)` (shared with the past-conversation reader) | the board (`shell.rs`) |
| Calling a panel up in the right-hand column | `sideReveal(name)` (its ✕ is `sideDismiss(name)`, choosing on the strip `sideChoose(name)`; `show_panel` from Lua) | board (`shell.rs`) |
<!-- /code-entries -->

### 5.1 A field

One thing to fill in, and the same shape on every screen.

```
label            12px / 500 / --text
  ↕ 8px  (--s2)
control          36px high, 13px, --bg on 1px --edge, radius 6px, padding 0 12px
  ↕ 8px  (--s2)
hint             11.5px / --faint
  ↕ 20px (--s5)  to the next field
```

- The name goes **above** the control, at every width. A column of labels down
  the left leaves both halves short of room and is the shape of an admin panel.
- **A table is the exception.** A screen that asks the same question two dozen
  times over (the key bindings) is a table, not two dozen forms. There the name
  keeps its own column, because what the eye does there is run down a column
  looking for one row, not read a row (`.row.pair`).
- A checkbox is the exception: its own label sits beside it, 15px box, `--s2`
  between, label at 14px `--text`. Two related checkboxes sit `--s5` apart on
  one line, not in a column.
- Focus is a `--brand` border **and** a 3px ring at 22% of it. Never a heavier
  border alone: the box would change size as it is stepped through.
- Disabled is `--panel2` with `--dim` text and `cursor: not-allowed`.
- A field that is wrong takes a `--warn` border, and the reason goes directly
  under it in a `--warn` box (`--s2` `--s3` padding, radius 6px, background at
  9% and border at 35% of `--warn`).

### 5.2 A dialog

```
┌─────────────────────────────────────────┐
│ header   16px 20px   title 13.5/600 · ✕ │   ← 1px --line under
│ body     20px        fields             │
│ footer   12px 20px   [destructive]  …   │   ← 1px --line over
│                      [reason]           │
│                      [Cancel] [Primary] │
└─────────────────────────────────────────┘
  width min(560px, 100%) · radius 10px · --panel on 1px --line
  shadow 0 8px 24px #0007 · 56px from the top of the window
```

- **Escape closes it. Enter in any single-line field is the primary button.**
  A press on the backdrop closes it; a press that merely *ends* on the backdrop
  does not (selecting text and letting go past the edge is not a cancel).
- Focus lands on the first thing to fill in -- the first empty field, or the
  first one the person came to change.

**The sheet** is the same thing one size up: a whole page of settings about one
thing (a panel's ⚙, a folder's or a tab's "edit"), stood over the board it was
asked from. `min(1040px, 100%)` wide and `min(760px, 100%)` tall, in the same
place -- 56px down, 16px of edge, centred across. It is wide enough for the
settings' own two columns; a dialog's width would fold them into the narrow
arrangement meant for a phone. **The settings themselves are not a sheet**: the
gear that opens all of them opens a screen. A screen too small to leave any
board around either of them is given the whole of itself instead
(`runtime::SettingsPlace` holds both sizes and that rule, for the window and
for a browser alike).

**A floating panel** differs from a dialog in that **it does not stop the
board**. There is one so far -- the guide's ? -- and being able to talk to it
while touching the settings is the whole reason it exists, so it has no
backdrop and is picked up and moved by its head. 380px wide,
`min(560px, window height - 112px)` tall, `--panel` with 1px of `--line`,
10px corners, shadow `0 8px 24px #0007`. It first appears 16px in from the
bottom right. **It never leaves the screen**: 24px of the head always stays
inside the window. The head is 40px (title 13.5/600, ✕ at the right) and is
the only part that can be picked up (`cursor: grab`, `grabbing` while held).
**No backdrop, and Esc does not close it** -- closing is the ✕, or pressing
again the button that opened it (a panel that vanishes when Esc is pressed in
the screen behind it is not guiding anybody). **On a phone it does not
float**: a sheet up from the bottom, 70vh tall, corners on the top only. On a
narrow screen a floating panel makes the finger's drag and the page's scroll
fight over the same gesture.

### 5.3 Where the buttons live

One law, so that no screen has to be learned twice.

| Where | Save / confirm | Destructive | Cancel |
|---|---|---|---|
| A page (settings) | Top right of the sticky header, one for the whole page | Its own row **below the last card**, `--stop` text, no border | none -- the page is left by closing it |
| A dialog | Footer, far right, filled `--brand` | Footer, far **left** | Footer, beside the primary, quiet |
| A row in a list | none -- the row opens the dialog | inside that dialog | none |

A screen never has two primary buttons. When a dialog is open, the page's own
save is behind the scrim and is not the one being talked about.

The left end of a dialog's footer holds **one** of two things: a destructive
button, or a button that **adds to what the dialog is showing** (the picker's
"New folder"). Both are things to keep away from the primary. An add button is
a plain one (`--edge` outline), never in the destructive `--stop` text, and a
dialog never has both.

### 5.4 A button that cannot be pressed

**Grey, and still answers.** Not `disabled`: the press is taken and the reason
is given.

- It looks off: `--panel2`, `--line` border, `--faint` text, `cursor:
  not-allowed`, and no hover change. Never a faded version of the live colour --
  a pale blue still reads as blue.
- Pressing it writes the reason where it stays: `--warn`, 11.5px, on its own
  line above the buttons in the footer. It begins with what did not happen
  ("Cannot save:") and ends with what would free it.
- At the same moment the thing that has to change gets a 6px `--warn` ring for
  two 0.9s beats, and is scrolled into view.
- The reason follows what is wrong: when a different thing blocks the save, the
  sentence already on screen changes with it, and it goes when nothing blocks.

### 5.5 A list of records

```
.rows        1px --line all round, radius 6px, rows divided by 1px --line
  row        10px 12px, gap 12px, --panel2 on hover, whole row opens the dialog
             name (mono, 132px) · chip · what it is (--dim, ellipsis)
             · the count at the right (tabular-nums) · ›
```

The `›` is the affordance; there is no edit button. A row that is only readable
by hovering is not readable on a phone, so the row stacks there: the name takes
its own line and the rest follows under it.

## 6. Motion

Nothing moves forever. A continuous animation in the window costs a fifth of a
core for as long as it runs. The two allowed motions are `step-end` blinks: the
BUSY dot, and the pulse on the one thing to press next. Expanding content may
change size in one step. Respect the platform's reduced-motion setting where the
page can read it.

One motion that ends by itself sits outside this rule, and only one: on the quick
command launcher, the buttons come in over 0.2s when it opens and when a page is
turned (`#quick .qbtn`). It helps the eye find where to press, and nothing moves
once it is done. Not with reduced motion, and not on any other screen.

## 7. Words

The rules are in `.claude/RULES.md`; the checks that catch most slips on screen:

- A label is a noun ("Start here"); a button is a verb ("Make it"); a hint says
  what happens, not why the program does it.
- No developer words on screen: hook, primitive, protocol, token, worktree in
  running text (the product's name for it is "parallel work"; the git term may
  follow in parentheses once).
- No "please", "simply", "just", "you can", "optional", "you may ignore".
- Never claim what the code does not do. Result verbs ("made", "found", "sent")
  only after the result exists; while pending, say what is being done.
- Every string comes from `lang/en.json` through `T[...]` / `t()`; Japanese from
  `lang/ja.json`, written for a Japanese reader, not translated; common kanji only.

## 8. Both surfaces

The board page is the window and the phone. Every part must work with a finger:
nothing reachable only by hover, nothing smaller than 22px to press, menus that
open on tap. What the window can do, the phone can do, unless the page it opens
is this PC's (then the phone gets a plain link or nothing, and the reason is in a
comment).

## 9. Screen review

Before a session that changed the screen ends, take the screenshot and judge it
with this rubric. Name the three highest-impact fixes first, make them, and only
then hand the screen to `/announce`.

**Take it with `tools/debug/shoot.mjs`.** It photographs both languages, both
schemes and both widths at once. How to use it, and the other checking tools,
are in [`tools/debug/README.md`](../../tools/debug/README.md).

    node tools/debug/shoot.mjs tools/debug/scenes/files.mjs

**Answer each; a "no" is a fix, not a note.**

1. **Read cold.** Would someone who has never seen the program understand every
   word and every number on it? Each number has a name and, if it changes, a time.
2. **One thing to press.** Is the primary action obvious by placement and its
   filled `--brand`, and is everything else quieter? A button that cannot be
   pressed is grey, answers the press, and points at what has to change (§5.4).
3. **Two presses.** Does the common path finish in one or two actions when the
   program already knows enough to proceed?
4. **Focus and keys.** In a dialog, does focus land where Enter does the primary
   thing, and does Esc back out quietly?
5. **State colours mean their state.** Blue only for focus/answered, green for
   working, amber for a person or nearly used up, red for stopped. Nothing
   decorative wears them.
6. **Grid.** Rows and columns line up; dots line up in one column; numbers
   right-align when they are compared.
7. **Words.** Nouns for labels, verbs for buttons, no developer words, no filler,
   nothing claimed that has not happened. Japanese through the `natural-japanese`
   lint when a paragraph was written.
8. **Empty and wrong.** When there is nothing yet, does the screen say the next
   thing to press? When something failed, does the error stay where it can be
   read and acted on, with a link if the fix is elsewhere?
9. **Siblings.** Does it read as one design with the part next to it (same size,
   same radius, same words for the same act)? Are fields, dialogs and lists built
   to §5.1-5.5, are save and delete where §5.3 puts them, and is every gap one of
   `--s1`..`--s7` rather than a number invented on the spot?
10. **Both schemes, both surfaces.** Still right in a light scheme, and at phone
    width with a finger?
11. **Nothing moves forever.**
12. **Redraw-safe.** Survives being rebuilt several times a second: no flicker,
    no lost press, no open list shut by a state push.

Write the review as: **Top fixes** (3) · **Friction** (element by element) ·
**Changes made** · **Left as is, and why**.

## 10. When this page is silent

1. Match the sibling part: the one next to it in the same flow.
2. Match the surrounding chrome: the nearest surface's sizes and words.
3. Use the closest role on this page and say so in a comment.
4. Only then add a role here, in the same commit as its first use.
