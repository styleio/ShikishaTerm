# Changing a file — design notes

> Status: settled 2026-09-12, before any of it was built.
> This file exists to record why each decision was made, and what was turned down.
> Japanese: [file-edit.ja.md](file-edit.ja.md).

## Background

The column on the right grew a list of the files in the working folder. Pressing
one put its path in the box below — enough to hand to an AI, not enough to fix
one line yourself. Sitting next to the terminal, **reading it, changing a line
and saving** should finish here.

## The line this holds — a place to read, not an IDE

**People write real code in their own editor.** What this is for is reading what
an AI just wrote, fixing a line, saving, and handing the place back. So:

- no type checking, no completion, no go-to-definition, no refactoring. Colour,
  brackets and indentation, and that is the list
- no language server. What runs here is a terminal, and the real tools are in it

Without that line, this becomes a second-rate IDE by a hundred small steps.

## 1. What draws the text — one library, carried

A bare `<textarea>` is not an option: no tab key, no indentation, a broken undo
history. Nor is a multi-megabyte editor — the board is one string inside
`shell.rs`, there is no bundler, and **the same page is read by a phone over the
network**.

**Ace** (`ace-builds`, BSD-3). The core is one 475KB file with no build step;
brackets, indentation, undo and search come with it, and a language is one small
file each.

- **Carried inside the executable** (`include_bytes!`, see `crates/core/src/ace.rs`).
  Laid beside it, a missing file kills the feature without a word; carried, the
  window and the phone are answered from the same bytes
- **No diagnostics worker** (`useWorker: false`), and no worker files shipped.
  The line above, in a setting
- **Its colours are this page's**, written as a theme in our own variables. No
  theme file is shipped
- One entry in `THIRD-PARTY-NOTICES.txt`, the way the console binaries have one

## 2. Where it opens — the list on the right, the file in a divided pane

The list stays in the column. A file pressed there **divides the screen and
opens beside what is running**: it needs width, and reading it next to the
terminal is the point.

**How long a tab lives** — this took several passes to settle:

| State | Where the file goes |
|---|---|
| No editor at all | **A throwaway one appears, and every file after that replaces its contents.** Not written to the settings; closing it is the end of it |
| An editor in front | **That one takes it** |
| An editor open but not in front | The first of that folder is brought into view |

- **Nothing has to be set up first.** Press a file, read a file
- **Pressing ten files leaves one editor**, not ten
- **Somebody who wants one pinned can have one**: "What to run: kind → Editor"
  puts an editor in a folder, the way the git panel sits in one. Two files side
  by side is the same answer — a second editor, and press while looking at it
- Where it lands is decided by **what is in front**, which is the rule the rest
  of the screen already uses

One surface underneath (`Surface::Editor`). One made from the settings and one
made by pressing a file are **the same screen**; only how long they live differs.

## 3. Being written from outside (the real work)

This program's whole premise is that **an AI is editing the same file**. The
rules:

1. **A file changed under an unsaved draft is not reloaded.** That would throw
   the typing away. Say so, and let the person choose: reload, or save what is
   here over it. Both are buttons on the notice -- a notice that names a way out
   it does not offer sends the person to Save, which rule 3 refuses
2. **A clean editor follows the file.** When the AI next door is the one
   writing, that is exactly what you want to see
3. **A save that would land on newer bytes is refused, not won.** The read hands
   out a mark of the contents; the write checks it first. The one write without
   a mark is the notice's "save mine over it", pressed after being told
4. **Nothing saves itself.** The person presses save, or Ctrl+S. A thing that
   writes on its own, beside an agent that also writes, fails in a shape nobody
   can unpick

What makes 1 and 2 possible is one metadata call per open editor per frame
(when it was last written, and how long it is). Cheap enough to ask every time,
and asking every time is what makes this something the editor notices rather
than something it has to be told.

## 4. Handing a place to the AI

Finding the spot and saying "here" is why an editor belongs inside a terminal.

- The form is **`path:line`** — what compilers and every AI CLI read — and
  `path:from-to` when something is selected
- Two places to say it from: the list (a whole file) and the editor (the line
  under the cursor)
- **Relative or absolute is worked out here**: the same folder as the tab the
  box is aimed at means relative, because the AI in it runs there; a different
  folder or another machine means absolute
- The way in is a button (and a key). **Dragging is added on top later** — it is
  awkward with a finger, so the way that works everywhere comes first. It is not
  a reason to leave something out

## 4.1 Safety

- Reading and writing stay **inside that tab's working folder**; every path goes
  through `local_under`, the same fence the list keeps
- **Too big is not opened.** Past the limit it says so
- **Binary is not opened.** There are no lines in it to show
- **No autosave**, by default or otherwise (see rule 4)

## 5. Turned down

### Monaco
**No.** Five web workers, several megabytes and a bundler. The board is one
string and the phone reads the same page. The editor it is usually paired with
turns every diagnostic off anyway and treats it as a reading surface — so the
core of a smaller one is enough.

### A visual Markdown editor
**No.** Round-tripping always breaks some spelling of the syntax, and then the
screen has to say "this file can only be edited as text" — a caveat built by
hand. Markdown is edited as text.

### More kinds of viewer on day one (PDF, notebooks, CSV, images)
**No.** What opens from the list is **text**. Everything else says it cannot be
opened here. What to add is decided after using it.
