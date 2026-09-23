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
| **Worktree being made** (`.making`) | Says how far a worktree that was pressed has got, under its project, until it exists | a `--line` framed row under the heading (after the found worktrees row). The working dot (`BUSY`, the only blink) · name 13px · a 22px ✕ at the right (stop, and take back what was made) · on the second line what it is doing now (11px `--dim`, "Creating the worktree…"). While stopping the ✕ goes and the line says so. Failed: the dot becomes a `--stop` ⚠, the frame `--stop` at 35%, the second line the reason (`--stop`, wrapping), and under it two plain buttons (Try again · Dismiss). Made: it turns into a card in the same draw the desk reads the folder in. No spinning mark (§6). Shown even with the project folded |
| **Pill** (status line) | One fact about now | text or a bar with words; pressable when pressing does something (limit notice), otherwise plain |
| **Server mark** (`.smark`) | The name a person gave a server ("Production", "Staging"), worn by whatever reaches that server | a radius-4 chip: an 8px square of the chosen colour and the name (11px `--text`), on a 14% tint of the colour with a 55% border. **Never on a server with no name** -- there is no colour-only mark. It has no state meaning and never paints a row or a button. Worn on the tab row (after the name), the tab over the panes, a pane's caption, a put-away set of tabs (its second line), the card of a folder on a server (leading its second line), the file panel's connection and its server-side crumbs, and leading the well of a question about something that cannot be undone. Up to 128px in a row, 96px in the tab over the panes and a caption; over the panes it keeps its width and the row scrolls, in a caption it shrinks after the name does. When the server asks for its name to be typed, that question adds a field for it under the well (5.1), and the primary button stays grey and answering (5.4) until it is typed, in any case |
| **Bar** | A number between 0 and 100 | `--line` track, `--live` fill, `--warn` past 80, `--stop` past 95; always with words beside it saying what it is of and when it resets |
| **Dialog** (`.vbox`) | One decision | title · one sentence · fields · the exact thing that will run (mono well) · one primary button at the right, ✕ at the top |
| **Dialog** (settings, `.framed`) | One record, edited | **header** (title · ✕) · **body** (fields) · **footer** (destructive at the left, then the reason it cannot be saved, then Cancel and the primary at the right), each divided by a `--line` |
| **Dialog** (choosing, `.picker`) | One place out of many | The same header, body and footer as `.framed`, with the body in **two columns** (left 200px = places to start from, right = breadcrumb · list). Width `min(760px,100%)`, body at a fixed height so the window does not grow and shrink with what is listed. A filter field under the header. One press on a row **selects** it; `›` or a double press **goes in** (on a phone `›` is the only way in). Footer: what is selected · an **add** button at the left · Cancel and the primary at the right. Line-drawn marks, no emoji. At 640px and below the left column becomes a row across the top |
| **Field** | One thing to fill in | its name above it (12px `--text`), the control, the line that explains it under (11.5px `--faint`); `--s2` between them and `--s5` to the next field |
| **Boxed list** (`.rows`) | Several of the same thing | one border round the whole, `--line` between rows, `--panel2` on hover, a `›` at the right when the row opens something |
| **Sidebar** (settings) | Which setting is being edited | One column, the larger world first: the **General** heading and its entries · the **Desk** heading (initial plate · name · `▾` listing the desks; choosing one switches what is under it) and the desk's entries · the **Projects** heading with one row per project (colour square · name · path under it; a folder in no repository wears a line-art folder). Each row opens one page (a name and one line under it). No place to enter and leave. **No button adds anything** (projects, worktrees and tabs are added on the board). Worktrees and tabs are not listed: they open from the board's right-click (Project settings / Tab settings / Edit…) or from a row on the project's page. While such a page is open its project's row stays selected. A project, folder or tab page starts with breadcrumbs (desk › project › folder › tab; 12.5px `--dim`, the levels above pressable, the page itself `--text`/600 and not). A right-click menu opens where the pointer is |
| **List** (`.fmenu`) | Choose one of a few | floating, radius 10, one line per choice, closes on any press elsewhere |
| **Way card** (`.apway`) | Choose the *way* to one result (adding a project: browse a folder, clone from a URL, create new) | One card is one pressable thing. A 28px tile with a line-art mark on the left, the title 13px/600, one line under it at 11.5px `--dim`, a ⏎ chip at the right end. **The way nearly everybody takes** stands alone on top as its own bordered card (at least 60px, mark in `--text`); the rest sit below in one enclosed list (at least 52px each, `--line` between) under an "Other ways" heading. Focus lands on the lone card the moment the dialog opens. The card the keyboard is on wears `--raise` and a `--brand` border and shows the ⏎ chip, so what Enter will do is always on screen. ↑↓ walk the cards. Pressing one commits nothing; the page it leads to does |
| **Where** (`#addproj .bpick`) | The machine a project is added on (this PC or an SSH host) | Only once there is a host, as a "Where" field at the top of the page. A 36px picker (line-art mark · name · the address at the right in 11px mono `--dim` · ▾). Its list is ✓ · name · address, then, under a rule, "+ Add an SSH host". With no host yet, an "A project on an SSH host" card sits in the list of other ways instead. On a host, a way that cannot be taken there (create new) is grey, and a press says why in `--warn` under the list (§5.4) |
| **A folder over there** (`.aprlist`) | Walk an SSH host's folders and pick one | the folder field (mono) with a refresh button; under it where it is (11.5px mono `--dim`) with a "git repository" pill when it is one, and a list fixed at 240px (30px rows · line-art mark · mono name, `↑ ..` first). One press goes in. While asking, the list says so; when the host cannot be reached, a `--warn` box in the list (what did not happen · the far end's words · Try again). The foot's primary button adds the folder being looked at, asking first when it is not a repository |
| **Closing primary button** (`button.go.wide`) | The one button that finishes a page (Clone, Create project) | The dialog's full width and **36px** tall -- the exception to 32px buttons: it is the only thing to press on its page, so it stands a step taller to say "here". Grey per §5.4 until it can go; a press then says why just above it and rings the field. A progress bar goes **under** it, so the button never moves |
| **Choice card** (`.scard`) | Inside a dialog, choose one of a few things told apart by their face (the AI in the first-start setup) | One card is one pressable thing (`role="radio"`). Radius 10, an `--edge` border, at least 56px tall, `--s3` inside. The name 13px/600, and under it the name a person would type (mono 11px `--dim`). An AI puts the mark its tab wears (`aiMark`) in front of its name, and wears its colour where a tab row does: the left edge and the name. Picked = the `--raise` surface with a `--brand` border. A press does not rebuild anything; it only moves the mark (`aria-checked`). Laid out in a grid that wraps when it does not fit. Under the cards, one line of field description saying what choosing does |
| **Bubble** (`#coach`) | Point at the next thing to press | one sentence · ✕ · a corner toward the anchor; at most one on screen |
| **Card** (sidebar) | Ask once | title · one line · two buttons (one primary, one quiet); goes away for good |
| **Toast** | Say what just happened | transient; never the only place an error lives |
| **Button grid** (the quick command launcher, `#quick`) | Buttons a person made, found by where they are | Each button is square, corner 10, `--edge` rim. Its face is a line icon (Lucide, the shared part in `quick.rs`) and an 11px name on one line; only a prompt for an AI carries a small "AI" at the top left. **Launcher**: the buttons alone float on the dimming (`#00000099`) in `--panel` with the shadow `0 8px 24px #0007`, and empty places are not drawn. The dimming is dark in every scheme, so no words are written on it directly: they ride on `--panel` plates with 999px corners. Above: a plate with which folder, as a trail, and a plate with the key hint, ⚙ and ✕. Right under the buttons: a plate of page numbers and a one-line plate saying where the button under the pointer sends. Rows no page of the grid uses are not drawn. The buttons come in over 0.2s on opening and on turning a page only (not with reduced motion). A button with nowhere to go is the grey of §5.4 and says why on that line when pressed. Where the grid does not fit, the page's buttons stand in reading order. The first place inside a folder is always the way back. They are made and changed in the settings, in the **reorderable list**, which draws no grid |
| **Reorderable list** (quick commands in the settings, `.qlist`) | Records whose order means something, set out in reading order to change | The boxed list of §5.5 with a 22px grip at the start of each row (the `grip-vertical` line drawing, `--faint`, `touch-action:none` so a finger can hold it). The row stands in columns: grip · 28px picture (`--line` rim, corner 6) · name 13px · chip (where it sends; an AI reads "AI · its name") · what it sends (a command in 12px mono, a prompt in `--dim`, cut with …) · `›`. **The whole row is the way in**: a button opens the dialog of §5.2 (`.framed`: the icon beside how the button will look on the launcher · name · where it sends · command or prompt · Enter; delete at the left of the foot, cancel and save at the right); a folder is walked into (a trail above the list, with a plain "Edit this folder" button at its right). The order is the launcher's reading order. Only once the launcher has more than one page is the list split into a box per page, under a "Page N" line (11.5px `--dim`). Holding the grip lifts the row on `--raise` with the shadow; the other rows make way as it passes their middle. The middle of a folder's row (all but its top and bottom quarter) and the names in the trail above are **places to put it in**, marked with a `--brand` rim and a 12% ground. Let go among the rows, it only **hands the places already used out again in the new order**: an empty place left on purpose, and the pages, stay where they were. Let go over a place to put it in, it goes last in that grid. From the keyboard, Alt+Up/Down moves it one row at a time. The buttons that add (plain) sit under the list |
| **Ideas** (`#ideas`) | Writing down what comes to mind, a card at a time (the 💡 in the bottom row, left of ✂️) | One box of the §5.2 measures on the quick commands' dimming (`#00000099`, the same layer): width `min(640px,100%)`, 56px from the top of the window, no motion. **Head**: the title "Ideas" 13.5/600 · a project choice (36px; first "No project", then the git repositories, a checkout and its worktrees being one; on opening, the project of the folder in front) · at the right an ordinary button "Show done ideas" (`--raise` with a ✓ while on) · ✕. At 640px and narrower the ✕ stays at the end of the first line and the button takes the second. **Body**: cards one under another, `--s2` apart, and last the writing line "New idea", where the caret is on opening. A card is typed into, so it is `--bg` with an `--edge` rim and corner 6, and wears the §5.1 ring while the caret is in it. Left to right: a 22px grip (six dots, `--faint`, held by a finger too) · a 15px check box · the words at 13px, as tall as they are · at the right end 22px line-icon buttons (copy, send to Issue). Done is `--dim` and struck through. A card sent to an Issue is done once that issue is made, and wears its number right of the words (`#12`, 11px monospace `--dim`, `--line` rim, corner 4), which opens the issue. Only the card being carried floats, on `--raise` with the shadow. A right-click on a card (holding its grip, on a phone) opens a list (`.fmenu`): copy · send to Issue · mark as done / not done · last, in `--stop`, "Delete". Done comes back with "Show done ideas"; deleted is gone from the file and does not. Delete is not asked again: the menu is already the second press. **Foot**: 11px `--faint`, "Enter: next card · Shift+Enter: new line · Saved as you type". There is no Save button. A file that cannot be read or written leaves its reason in a `--warn` box under the head. Esc, ✕ and a press on the dimming close it |
| **Ask bar** (`#ask`, `.pask`) | A script needs a person to do something on a page | one sentence · one primary button; `--warn` top edge (needs a person); drawn by the board under the page and never inside it, so only a person can press it; stays until the script takes it down |
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
