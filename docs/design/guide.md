# The guide (press ? and it answers)

Turn the **?** beside the gear from a link to the manual into **something that
answers**. It answers how this program is used and how it is set up, walks the
person to the settings screen that holds the answer, and writes into the field
they picked. Whoever is chosen under Settings > Basic > Assistant AI answers.

This is a memo written before the work, so that what was decided, and why, is
on the record.

---

## 1. The frame that keeps it from going stale

Anything an AI reads that a person wrote goes out of date from the day it was
written, and it **answers from it anyway**, so nobody finds out. So everything
it reads is **built from a machine-readable source, every time**.

| Layer | What | From | How exact |
|---|---|---|---|
| **The map of screens** | the 30 settings screens (id, name, subtitle) | `globalSections()` / `deskSections()` in `webui::PAGE` | whole (the list is read as it stands) |
| **The settings words** | the 1,100-odd `settings.*` texts (labels, hints) | `lang/<code>.json` | whole (the screen is drawn from it) |
| **Which screen a word is on** | word → screen | the two above, joined (§2) | just under six in ten. The rest name no screen |
| **Keys and permissions** | the `Ctrl+B` list, the automation permission table | `keys::listing()` / the `grants` catalog | whole |
| **The manual** | what is on the screen, starting out, when something is wrong | `docs/MANUAL.*.md` (prose, written by hand) | watched by §4 |

**One place is written by hand**, and even there a test checks that everything
it names by name still exists.

## 2. How a word's screen is found

`globalSections()` reads
`{id:"remote", label:T["settings.sec.remote"], build:remoteCard}`, and
`deskSections()` has the same shape. So **the `T["..."]` reachable from the
function named in `build` are the words that screen shows**.

It follows two levels deep, and **past the first step does not follow into a
function three or more screens call**, which is a shared part rather than one
screen's own. Without that, everything travels through `el` and `row` and every
word ends up on every screen (measured: the 43 words of `settings.bring.*` land
on 14 screens).

The first step is not held to that rule, because **what a screen calls itself is
that screen's however many others call it too**. While it was, the git screen
came out with five words on it: `gitFields` had been cut away as shared.

As it measures:

| | |
|---|---|
| Screens | 30 |
| Words placed on a screen | 663 of 1,117 |
| On two screens | 65 |
| On more than two | 9 |
| Screens with no word | 0 |

The 454 words left over go into the index **naming no screen**.
Saying nothing about where a thing is beats saying something false.

**Some of it cannot be read from the source at all.** The 143 words looked up by
a name the page builds as it runs (`T["settings.dsec." + id]`) are written down
nowhere. That is a limit, and it is accepted: demanding that every word name a
screen would mean a person writing annotations forever, which is the very thing
that goes stale.

## 3. Three outputs, one source

Three things come off the same index. **Not one copy is made.**

- **The index an AI reads** … `/api/guide` (JSON). The ? panel hands it over with the question
- **The reference a person reads** … `docs/SETTINGS.md` / `SETTINGS.ja.md` (generated,
  kept in the repository). `site/scripts/sync-docs.mjs` carries it to the site
- **What the manual pulls in** … a `<!-- guide: <screen id> -->` line in
  `docs/MANUAL.*.md` becomes that screen's list of entries when the file is generated

The reference is committed because the site's build reads nothing but `docs/`.
Being committed, it can fall behind, which is what §4 is for.

## 4. The gates (tests)

| It fails when | Because |
|---|---|
| the generated reference differs from the `docs/SETTINGS*.md` in the tree | it was committed without regenerating |
| a screen on the map has no words | a card was renamed, or emptied |
| the manual names a screen id the map does not have | the screen went, or was renamed |
| a manual's pull-in mark points at a screen the map does not have | the same |
| a `settings.*` word is not in `lang/en.json` | the existing `every_word_a_page_asks_for_exists` sees it |

The reference is rewritten with `SHIKISHA_WRITE_DOCS=1 cargo test guide` — an
environment variable rather than a feature, so the feature list stays as it is.

## 5. The ? panel

**A page the app places** (the machinery `runtime::SettingsPlace` already is).
Floated inside the board with HTML, it would be hidden the moment the settings
open full — a panel for guiding somebody through the settings that disappears
when the settings open is no panel at all.

- The app holds the rectangle, and it can be picked up and moved. While it is
  held only an outline moves, and the page is put down where it is let go
  (a native child view dragged every frame smears on Windows)
- Where it was left lives in `data/` (losing it costs nothing)
- On a phone it does not float. It comes up from the bottom as a sheet
  (floating on a narrow screen makes the finger's drag and the page's scroll
  fight over the same gesture)

## 6. Walking somebody there (an action is always asked first)

An answer may name a screen. **Only an id on the map of §1** can be named.

Nothing happens until it is pressed: under the answer stands one button,
"Open Settings > Phone connection", and **the person presses it**. That is the
same manner as the existing ask strip a script raises when it needs a person.

## 7. Writing on somebody's behalf (the yellow frame)

While the ? is open, pressing a field on the settings screen makes it **the
field that was picked**.

**What the field is, is read from the screen being drawn**, not from the index:
the DOM in front of the person is the thing they are looking at, so it cannot
disagree. The `<label>` of the `.row` it sits in, the `.hint` under it, and what
kind of input it is (text, number, tick, choice) are handed over as they stand.

- **The value is not handed over.** Only the label and the kind. Paths, host
  names and account names do not go to an AI company by default
- **A secret's field cannot be picked.** A `type=password` field, the secrets
  fields and the token fields are out of the picking. No road is built for a
  string an AI wrote to land in one
- **A person presses save.** Writing only makes the page unsaved, so the gate
  that is already there does the asking

## 8. When the deeper answer is wanted

For the kind of question the index does not hold (why does it behave like this,
what is this error), a road is kept to an AI with its tools, **taken only when a
person presses for it**. It is not the default road. Speed and cost differ by an
order of magnitude (about 1,400 tokens asked the light way for claude, about
9,500 with tools).

## 9. What is not done

- **Writing a manual for the AI by hand.** Stale from the day it is written
- **Answering by reading GitHub as it runs.** As fresh as the index, but slower,
  dearer and less accurate (a small model guessing from 700KB of screen code)
- **Copying the screen's words into the index.** The index is built from the
  dictionary and the screen, every time
