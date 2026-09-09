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
| How words are chosen | `.claude/RULES.md` "利用者に見せる言葉", "日本語の文章" | This guide does not repeat them |

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
| `--line` | Structure | Every border, rule, divider and empty bar track |
| `--sel` / `--cursor` | The terminal's own selection and cursor | The terminal only; from the scheme when it names them |
| `--panel2` / `--sys` | The settings page's second and third surface | Settings page only, where they predate `--raise`; a new surface there uses `--raise` |

### Text

| Token | Role |
|---|---|
| `--text` | What is read |
| `--dim` | What is glanced at: labels, meta, hints, the folder name of a row |

### State (the only colours that carry meaning)

| Token | Means | Where it may appear |
|---|---|---|
| `--brand` | **Focus and "answered"**: the focused pane's underline, the DONE dot, the selected row's edge, the primary button's outline | Never as a border for structure; never for text that is not a state |
| `--live` | **Working**: the BUSY dot, a bar that is filling normally, AUTO ON | |
| `--warn` | **Needs a person, or nearly used up**: the QUESTION dot, a limit notice, a bar past 80%, a folder that drifted behind | |
| `--stop` | **Stopped or dangerous**: the EXIT dot, the stop button, a bar past 95%, delete | |

A colour used for a state must not be used for decoration nearby. A blue rule
reads as "a pane starts here" wherever it is drawn.

### The AI's own colour

Each AI has one colour (`--ai`: claude `#d97757`, codex `#19c37d`, gemini
`#4285f4`, deepseek `#5b7cff`, qwen `#a06bff`, aider `#e5644d`, kimi `#12b3a8`).
It is worn exactly twice on a tab row (left edge and name) and once in front of
anything that is that AI's, such as its allowance on the status line. Never on a
dot, never on a border of structure, never mixed with a state colour.

### Contrast

Text on `--panel` and `--raise` must be `--text` or `--dim`, never a state colour
at 10px. Check every screen in one dark and one light scheme (the settings pick
them by name); `--dim` sits at 65% of the text on purpose because lighter was
unreadable on gentle schemes.

## 3. Typography

One family: `--mono`, the terminal's font, everywhere, on both pages. It is the
program's face; a proportional font anywhere would be a second product. Hierarchy
is made with size, weight and `--dim`, never with a second family.

| Size | Role | Examples |
|---|---|---|
| 14px | Body of the page | Dialog input, the terminal's own text (`--fs`) |
| 13px | A row that can be pressed | Tab rows, dialog buttons, menu entries |
| 12px | A heading or a line of status | The status line, the project's heading in a household, card titles in settings |
| 11.5px | A quiet heading | A folder heading that is not a household's head, dialog hints |
| 11px | Secondary | Chips, counts, hints under a control |
| 10px | Meta under a row | What a tab last said, where it is (branch, ports), the branch pill on a heading |
| 9px | A caret | ▾ ▸ only |

Weight 600 belongs to the one row that names the thing (the project heading, a card
title, the AI's name). Nothing else is bold. Letter-spacing `.02em` on 11-12px
headings only.

Numbers that are compared line up: `font-variant-numeric: tabular-nums`.

## 4. Spacing, radius, elevation

- A row is `padding: 7px 10px; gap: 8px`. Children of a row sit one step in:
  26px for a tab under a folder, +14px for a branch under its project.
- Radius: **8px** for anything set on the page (dialogs, cards, fields, buttons),
  **10px** for a floating menu or bubble, **4px** for a chip or a small pill,
  **3px** for a bar, **999px** only for the status line's pills.
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
| **Folder heading** | Where tabs run | mark (square = project, branch line = branch) · caret · name · [branch pill] · [branch count] · drift · `+` |
| **Household** | A project and its branches | `.family` box on `--raise`; head at 12px/600; branches a step in |
| **Pill** (status line) | One fact about now | text or a bar with words; pressable when pressing does something (limit notice), otherwise plain |
| **Bar** | A number between 0 and 100 | `--line` track, `--live` fill, `--warn` past 80, `--stop` past 95; always with words beside it saying what it is of and when it resets |
| **Dialog** (`.vbox`) | One decision | title · one sentence · fields · the exact thing that will run (mono well) · one primary button at the right, ✕ at the top |
| **List** (`.fmenu`) | Choose one of a few | floating, radius 10, one line per choice, closes on any press elsewhere |
| **Bubble** (`#coach`) | Point at the next thing to press | one sentence · ✕ · a corner toward the anchor; at most one on screen |
| **Card** (sidebar) | Ask once | title · one line · two buttons (one primary, one quiet); goes away for good |
| **Toast** | Say what just happened | transient; never the only place an error lives |
| **Ask bar** (`#ask`, `.pask`) | A script needs a person to do something on a page | one sentence · one primary button; `--warn` top edge (needs a person); drawn by the board under the page and never inside it, so only a person can press it; stays until the script takes it down |
| **Button** | Do the thing | primary = `--brand` outline; quiet = `--line` outline; destructive = `--stop` outline. Cancel/Close are quiet, never coloured |

Two rules from the rows above that are most often broken:

- **A number never stands alone.** `20%` is not a fact; `20% used · resets in 3h 45m`
  is. Every number has a name (what it is of) and, where it changes, when.
- **A bar never stands alone either.** The bar is the number; the words beside it
  are the sentence.

## 6. Motion

Nothing moves forever. A continuous animation in the window costs a fifth of a
core for as long as it runs. The two allowed motions are `step-end` blinks: the
BUSY dot, and the pulse on the one thing to press next. Expanding content may
change size in one step. Respect the platform's reduced-motion setting where the
page can read it.

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

**Answer each; a "no" is a fix, not a note.**

1. **Read cold.** Would someone who has never seen the program understand every
   word and every number on it? Each number has a name and, if it changes, a time.
2. **One thing to press.** Is the primary action obvious by placement and the
   `--brand` outline, and is everything else quieter?
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
   same radius, same words for the same act)?
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
