# Manual

What is on the screen, what the keys do, and where things are. Everything here is
about the program as it ships; the settings screen shows what you have changed.

Translations sit beside this file as `docs/MANUAL.<code>.md`, and the `?` beside the
gear opens the one for the language you use.

---

## 1. What is on the screen

**INDEX** is the board: a table of every tab, what each is doing, and a menu of
letters. `Ctrl+B 0` brings it back from anywhere.

**The list on the left** is the tab bar. From the top:

- **The desk** — the name at the very top. Press it to switch to another.
- **Projects** — a heading per project (its colour and name), with its working
  folders under it. The heading's `+` makes a worktree of the project (parallel
  work); its ▾ puts the whole project away.
- **Working folders** — the cards under a heading: a state dot and the name, and
  on a second line the branch it is on. The original checkout wears a "primary"
  pill, and the worktrees stand under it. The tabs working in a folder are listed
  under its card. Right-click a folder card or a tab: **Rename** turns its name into a field
  right there (leaving the field saves it), and **Edit** opens its settings page.
  **Take this folder off the list** does just that, for any folder, the primary
  included: the folder and its files stay on disk, and the tabs standing in it
  close with it. A tab in the middle of something is left alone and named
  instead, so finish it or close it and the folder goes.
  A worktree's card has a red **Delete completely** at the bottom. It deletes the
  whole folder; links are unhooked first, so what they point to stays, and files
  copied in that git does not track go with it. It does not delete while there are
  uncommitted changes. Tick "Don't show this again" in the question and it deletes
  at once from then on (turn the question back on under Settings > Basic > Deleting a worktree).
- **A tab** — its number, its name, and a dot: **green** while it works, **blue**
  when it has answered, **amber** when it is waiting for you to answer. A green **ring** means
  it has answered but something it started is still at work -- a command left running, or a
  helper (subagent) the AI runs on the side -- and you can already type to it. Under the name, the
  branch it is on, its pull request, and any ports it is listening on; under that,
  what it last said about itself. The ports show on the folder's card too, after its branch.
  Press a port (`:3000` and so on) and the column on the
  right shows **Ports**: what listens in that folder, with the program and the tab it came
  from. Press a line to open it in a browser tab (on the PC, you can also open it in the PC's
  browser or copy the address). A folder's right-click menu (a long press on a phone) has
  "Show the ports" too. For a folder on a server the entry is "Open the server's ports", and
  on a MicroVM "Public URLs"; the machine is asked when you press Ask.
- Under INDEX and Issue, the **PROJECT** heading with a `+` at its end (add a project).
  At the very bottom, the gear (settings), `?` (this page), 💡 (ideas), the scissors
  (tools) and 🎛️ (quick commands).

**Quick commands** are a grid of buttons you make yourself. 🎛️ or `Ctrl+Shift+K` brings
them up over the whole window. Every press opens a new tab in the folder of the
tab you are looking at: a command runs in a new shell tab, and a prompt is given
to an AI started in a new tab. With no
folder in front, a command opens in your home folder (so a button can start a
program too) and a prompt is not sent. The line under the buttons says where
the one under the pointer will go before you press it. Make them, move them and
put them in folders under Quick commands in the settings. A secret named in a
button's text is replaced, when it is sent, with that secret's value from the
desk on screen.

**The middle** is the tab in view: a terminal, a browser page, or the git panel.
Once the window is divided, each pane's caption carries ▥ and ▤ to divide it again
and ✕ to close that view.
Open **CI** in the git panel to list the commit's checks. Press a check to open its
job's log in an editor tab that only reads (at the first error, when there is one).
A check still running, or one with no log to download, opens its page in a
browser tab instead.
An address or a file path in a terminal is underlined under the pointer. Press it
for what it can be opened with -- a browser tab, the editor at that line, the
default app, its folder -- or Ctrl+click to open it straight away (Settings >
Basic > Pressing an address or a path in a terminal). A program's link to a
file on another computer (`file://server/share/...`) opens as that computer's
file share from a tab on this PC.

**Along the top of the middle** are the tabs of the folder in view. Press one to
switch to it, and `+` to add one. A tab's ✕, or a middle click on it, closes that
tab. The ✕ shows on the tab in view and on the one under the pointer. Only when
its AI is working or waiting for an answer are you asked first. A closed tab can
be opened again from the ▾ at the end of the row; with a CLI that can resume a
conversation by name (Claude Code, Codex) the conversation comes back with it.
What was on its screen does not.

**The column on the right** (◨ in the title bar brings it out and puts it away) holds only what
the tab in view is used with: Files, Git and Chat beside an AI tab, Files and Git beside a
terminal or an editor, and Console beside a web page. A folder git does not hold has no Git.
A folder of a project that keeps decision records has ADR as well.
What you choose is remembered for each kind of tab, so going from a page back to an AI tab
brings back what you were reading beside the AI. Something a button calls up (the search of
every conversation, for one) stays in the column over other tabs until its ✕ is pressed.

**Files** is what is in the working folder. Press a file to open it in the editor. Right-click a row (hold it on a phone)
to edit it, put its path in the message box, copy its path, rename, duplicate or delete it. A folder's row, and the empty part of the list, also make a new folder, or a **new Markdown** file:
name it and choose what it starts from, a blank document or one of the folder's templates. The
templates are Markdown files in `.shikisha/templates/` of the working folder (folders inside it
too), so a team keeps them in its repository; one is listed by the `title:` of its front matter,
else by its file name. `{{title}}`, `{{file}}`, `{{date}}` and `{{time}}` in a template are filled
in for the new file; anything else in braces is left as written. What is deleted from a folder on this PC goes to the recycle bin, and can be
restored from there. What is deleted from a folder on another machine cannot be restored, and a folder with files in it cannot be deleted there.

**A Markdown file** opens in the editor three ways, chosen at the top: **Text** (the file as it
is written), **Visual** (edited as it reads, with a bar for headings, lists and links, and `/`
for tables, code, diagrams and formulas) and **Preview** (read as it is drawn: tables, task
lists, code in colour, Mermaid diagrams, KaTeX formulas, pictures from the folder, footnotes).
Visual editing changes only what you edit: the rest of the file keeps its spelling, blank
lines and line ends. A file that visual editing would rewrite (HTML in it, footnotes) opens as
Text and says why; a very large one too, with a button to open it visually anyway. **Contents**
lists the headings beside the document; Ctrl+F finds words in the drawn document, and in Visual
replaces them too. Links open where they belong: a heading in the document, another file of
the folder in this editor, a web page in a browser tab of the folder (with Ctrl, in this PC's
browser). **PDF** saves the drawn document as a PDF where you choose, light on white whatever
the screen's colours are, with its formulas, diagrams and pictures; on a phone it opens the
phone's own printing, where "Save as PDF" is one of the printers. Right-click a paragraph of the preview (hold it on a phone) to **write a note for the
AI** on it; the notes stay under their paragraphs, and **Hand to an AI** puts them all in an
AI tab's input as `path:line` lines, for you to read and send. Handed notes are taken off.

**ADR** lists the project's decision records: short Markdown files, kept in the repository in
the MADR format, that say what was decided and why. It is there for a project whose settings
turn it on (the project's ADR page, which also says the folder; `docs/decisions` unless you
write another). Search the list, or write a question and press **Ask AI**: the assistant AI
answers from the records and names the ones it used. **New ADR** opens a form in the sections
MADR gives a record, or in those of a template kept in the same folder (`adr-template.md`).
**Draft from the conversation** fills it from the latest conversation in the AI tab in front,
and adds that AI as somebody consulted. A new record is saved as a proposal, numbered after
the last one, and named after its title in any language. **Propose in a pull request** commits
that file alone, pushes it and opens a pull request for the team to discuss; on a protected
branch it makes a branch for it first. A proposal is decided with **Mark accepted** or
**Mark rejected**. An accepted record is not rewritten: **Replace with a new ADR** writes the
one that replaces it and marks the old one as replaced; only a typo is fixed in the editor.
AI tabs with the SHIKISHA-TERM skill follow accepted records and ask before going against one.

**Chat** reads three ways. **AIConfer** is what the AIs say to each other when one asks
another (after you name a tab with `@`, or on its own): one short line each, like a chat,
with a face for every tab and what each is doing along the top. It shows the conversation of
the AI tab in front -- its newest one; the bar over it offers the others it took part in, and
two conversations going on at once are never mixed. Under each line,
"What was asked" and "The whole answer" open the full text. Press a line to put a mark on it
(👍 ❤️ 🎉 👀 ✅ ❓); a card opens the commit, pull request, file or page an AI shared; a
decision made in a job shows as "Agreed". A line marked "first sentence" is an answer whose
AI wrote no line of its own. AIConfer opens by itself when one AI asks another (not while
you are using the window, and on a phone it is only chosen); turn that off, or change how
long a line may be, under Settings > AIConfer. **This conversation** is the tab in view, and
**Every conversation** searches them all.

**The bar above the input box** is the convenience bar: one press sends a stock
instruction to the AI in view (continue, explain, review, fix). The small picker at
its left chooses what the bar holds: the stock replies, your own quick actions, the
macro recorder, git, or an AI's command suggestion.

**The menu (⋯)** at the end of the bar over a web page holds **Find in page**,
**Downloads**, and the tools for somebody building the page (tick Develop under the
browser tab's Controls in the settings; the menu itself can be unticked there too): **Hard reload**; **Pick elements for the
AI**, which opens **Picked elements** in the column on the right -- press **Pick**, press
the parts of the page an AI should look at, add a note to any of them, and **Hand to an
AI** puts them in that AI's input as a draft (what each one is, where it sits, its markup
and its styles; values that look like keys, and the secrets the app holds, are replaced
with `[hidden]` first, and the panel says how many); **Developer tools**; and **Source
code** and **DOM**, which open the page's HTML as the server sent it, and as it stands
now, in an editor that reads and does not save. The same list, less the reload, is on the
page tab's right-click menu. What a page says on its console -- its
own logging, errors nobody caught, files that did not load -- is on the **Console**
tab of the column on the right, once it has been opened on that page (with what the page
said since it last loaded); it
can be filtered by kind, cleared, and handed to an AI the same way. For the whole of
the browser's DevTools, right-click the page's tab (hold it on a phone) and choose
**Open DevTools beside it**: they open as a page of their own in the other half of
the pane, and a phone is shown them and works them like any page. To hand work to another tab, name it
with @ in the input box.

**Find in page**: press **Ctrl+F** on a web page (or **Find in page** in the bar's menu,
which is how a phone opens it). It is one of the controls a browser tab chooses
(Find in page, under the tab's Controls in the settings); a page without it keeps the
browser's own small search box for Ctrl+F. Type, and every match is lit as you go, with the one
you are on in orange; **Enter** goes to the next match, **Shift+Enter** to the one before,
and **Esc** closes the bar. F3 and Shift+F3 move through the matches too. The window and a
phone watching the same page share one search, and the words stay when the page moves on.

**Downloads**: a file a web page saves goes to your Downloads folder, and the
**Downloads** list opens in the column on the right -- each file with how far it has got
and where it went. At the PC, open it or show it in its folder (a program is only ever
shown in its folder); on a phone, **Save to this device** brings it to the phone.
A page of a folder on an SSH server or a MicroVM saves to that machine instead,
in its `~/Downloads`, where the AI working in the folder can use it; the list
names the machine and the path, and **Save to this PC** brings a copy here. The
menu's **Downloads** brings the list back.

**Save as PDF**, in the bar's menu and on the page tab's right-click, saves the
page as it would print -- A4 unless the page names its own paper, with its
backgrounds -- named after the page's title, in this PC's Downloads folder. It
appears on the **Downloads** list like a file the page saved, so a phone takes it
with **Save to this device**. What the app lays over the page (its messages, the
pen) is left off the paper.

**The input box** at the bottom is where you type to the tab in view. On a phone
it is the only way in.

**The status line** at the very bottom says which desk this is, whether
automation is on, whether a phone is connected, the build, and a stop button that
halts every hand-over and every working AI at once. While you are looking at a tab whose AI has said
something about its usage limit, that line is shown here too; press it to put it
away.

## 2. Keys

The prefix is `Ctrl+B`, tmux-style. `Ctrl+B ?` shows the keys you actually have,
since every one of them can be changed in the settings.

Two keys need no prefix and work from any program: `Alt+Shift+K` opens the
quick commands and `Alt+Shift+M` the ideas, bringing SHIKISHA-TERM to the front
if another program is there. On a Mac they are `Ctrl+Alt+K` and `Ctrl+Alt+M`
(Control and Option), because Option and Shift with a letter is how a Mac types
letters such as ˛ and Œ. Change them under Settings > Shortcuts, in
"Keys that work from any program". For the other actions, choose the box beside
an action in "Keys inside SHIKISHA-TERM" on the same screen and press the
combination you want (`Ctrl+Shift+D`, `Alt+F4`, `F5`) to give it one of its own.

The digits are the tabs themselves: `Ctrl+B 0`–`9` goes to that tab (`0` is INDEX).

<!-- guide: keys -->

| Key | What it does |
|---|---|
| `Ctrl+B q` | Quit |
| `Ctrl+B n` | Next tab |
| `Ctrl+B p` | Previous tab |
| `Ctrl+B t` | Add a tab |
| `Ctrl+B &` | Close this tab (asks first while its AI is working) |
| `Ctrl+B T` | Reopen the tab closed last |
| `Ctrl+B r` | Restart this tab, carrying the conversation over |
| `Ctrl+B R` | Restart this tab from nothing |
| `Ctrl+B %` / `Ctrl+B |` | Split the screen beside this one |
| `Ctrl+B "` / `Ctrl+B -` | Split the screen below this one |
| `Ctrl+B o` | Move to the next pane |
| `Ctrl+B X` | Close this pane (the tab keeps running) |
| `Ctrl+B =` | Put the dividers back to even halves |
| `Ctrl+B s` | Put the tab bar away, or bring it back |
| `Ctrl+B g` | Show the changed files on the right, or put them away |
| `Ctrl+B <` | Move the divider left / up |
| `Ctrl+B >` | Move the divider right / down |
| `Ctrl+B w` | Desk list |
| `Ctrl+B W` | Next desk |
| `Ctrl+B [` | Copy mode (/ searches, n and N walk the matches) |
| `Ctrl+B c` | Copy the latest answer |
| `Ctrl+B l` | Lock input |
| `Ctrl+B a` | Automation on / off |
| `Ctrl+B x` | Emergency stop |
| `Ctrl+B b` | Send the prefix key itself to the program |
| `Ctrl+B ?` | This list |
| `Ctrl+B :` | Command palette |
| `Ctrl+B k` / `Alt+Shift+K (Mac: Ctrl+Alt+K)` | Quick commands |
| `Ctrl+B m` / `Alt+Shift+M (Mac: Ctrl+Alt+M)` | Ideas |

<!-- /guide -->

On INDEX the menu is single letters: `e` settings, `p` the palette, `f` find,
`i` the QR code for a phone, `r` restart stopped tabs, `w` switch desk,
`t` send a test notification, `k` the master password, `?` help.
With a master password set, the app starts **locked**: nothing is shown and
nothing can be done in it -- no tab, no settings, and no call through the
automation pipe -- until the password is given. There is no cancel; the one
other way out is Quit. Terminals that keep running on their own go on behind
the lock, and are shown again once it is open. It can be opened from a phone
too: the board's address shows the lock in the board's place, and the
password is taken there only over a line nobody between can read (this
machine itself, Tailscale, or HTTPS). Over a plain LAN address the page says
so instead of asking. Opened on either, it is open on both.
The master password is set or changed only in the program's own window. While
the screen runs as a program of its own (Settings > Basic > Screen and work),
and from a phone, `k` does not ask for it: turn that off and start the
program again to change it.

The mouse works too: wheel to scroll, Ctrl+wheel to change the terminal's text
size, drag to copy, right click to paste, click a tab name to switch. Drag the
divider between panes to rebalance them, double-click it for even halves. Drag the
tab bar's right edge to set its width, double-click for the width it ships with,
and drag it shut to give the whole window to the terminal.

## 3. Working folders and branches

Open **Manage work folders** in the left list to search by folder, project or machine and act on a selection.

- **Pin** brings a project and its pinned folders forward. The choice survives restarts.
- **Archive** asks before closing tabs and hiding folders from the usual list. Files and tab settings stay.
  Choose **Archived**, select the folders and press **Restore** to use them again.
- **Delete folders** always confirms the selected targets, then processes them in order. The question lists
  each folder's uncommitted and untracked files (open one to see its changes) and deletes them only on
  **Discard changes and delete**. Primary checkouts and pinned or busy folders stay. Open file editors and use
  by another workspace also prevent deletion; each folder keeps its result in the list.
- **Measure capacity** scans the visible folders in the background. **Largest first** orders the estimates.
  Linked contents are excluded. Each estimate has a measurement time or an error; it may differ from space freed by deletion.

Press the folder-plus at the end of the **PROJECT** heading to add a project: open a
folder on this PC, clone one from a URL, or make a new one. A project added opens with
whatever Settings > Basic > Default command says (PowerShell, Command Prompt or Git Bash;
PowerShell unless chosen). A folder left empty by closing its tabs opens it again when pressed.

**A project on a server you reach over SSH** is added from the same dialog. "A project on an SSH
host" takes a name, host, user, port and key file, or fills them in from an alias in
`~/.ssh/config`. Once there is a host, **Where** at the top of the dialog chooses this PC or
the host. On a host you walk its folders and add one, or clone from a URL on that machine; git
and the terminal run over there.

**Work on another branch at the same time**: press the `+` on the heading. The
**Create worktree** dialog opens:

- **Project** — which project it is made from;
- **Name or what to create it from** — four tabs. **Auto**, the one it opens on, has
  nothing to type: the worktree is named and described from what its AIs are asked
  (below). **GitHub** searches the project's issues and pull requests; picking one
  names the worktree and ties the work to it (a pull request's branch is fetched).
  **Branch** picks what it grows from. **Name** is typed; a name typed there is kept,
  and leaving it empty keeps Auto on;
- **AI** — what runs in the new folder. The default is Settings > AI agents > Assistant AI,
- **More** — one folder per AI, where it goes, things git does not carry (`.env`,
  `node_modules`) to bring along, and the exact `git worktree add` line.

The places AI tools keep for themselves inside a project (Claude Code's
`.claude/worktrees/`, `.claude/checkpoints/`, `.claude/mailbox/`, and the files where
it records its state on this PC, such as `agent-registry.json` and
`scheduled_tasks.json`) are not brought,
even when `.claude` is copied. These are the app's Default rules, listed under the
project's settings, Worktree Creation Rules › Rules for places inside folders. To bring one,
change its row to Copy. When a new version adds a Default rule after you have looked at a
project's rules, its row says "Added since you last looked", once. The dialog says, under a
row that is copied, what is not brought and how big it is.

Press **Create worktree** (`Ctrl+Enter`). A "Creating the worktree…" row appears under
the project's heading and turns into the folder's card once it is made. Its ✕ stops it
and takes back the half-made folder and the new branch. If it fails, the row says why
and offers **Try again** or **Dismiss**. Folders made this way live in
`~/SHIKISHA-TERM/branches/<project>/<name>`.

**Names written from what was asked.** A folder with Auto on (set on that folder's own
settings page; on by default for a worktree made from the dialog) is named and described by an
AI from the requests sent to the AIs in it: at once after the first one, then again when
an AI there finishes its work, at most once every 10 minutes. Only the requests are
sent, never the answers, and long code pasted into them is left out. Until it is
written the list shows the branch name; a name written this way is drawn a shade
quieter. Rest the pointer on a folder to read its name and summary; on a phone the
card carries a line of the summary, and holding a card down opens its menu with the
whole summary at the top. Changing the name or the summary by hand turns Auto off.
Which AI writes them is chosen under Settings > AI agents > Automatic names, and a
desk can choose another under Desk > Automatic names: the assistant AI, asked the
lightest way it can be, or one of the AI providers.

Worktrees made from a terminal or another tool show up under the heading as "Hiding N
found worktrees". Open it and **Show in the worktree list**, or press ✕ to keep them hidden.

A folder that is not on this PC (settings carried over from another machine)
says so in the list; press it and the program tells you what it would take to put
it back, and does it when you say so.

## 4. From your phone

Turn on phone access in the settings and scan the QR code. Every tab, its state,
and a box to reply are on the phone; what the PC can do, the phone can do too.
See [From your phone](https://shikisha-term.com/phone/) for the three levels of setup.

**Which machine draws a page.** When automation opens a browser page, it can be drawn on
this machine or on the device looking at it (Settings, "where pages are drawn"). This
machine is the default: watchable from a phone, still working with nobody connected, and
one signed-in session for every device. Either way **the page reaches the network from
this machine**, so a `localhost:3000` started here is the same thing seen from both sides.

## 5. Connecting to a server

Choose **Server (this program connects)** as a tab's kind, give it the address,
the port and the user, and save a password. The password is never shown again,
and no `user@host's password:` prompt appears. Opening the tab gives you that
server's terminal.

Automation can read and write files there too, by naming that tab (`sftp_ls`,
`sftp_get`, `sftp_put` and the rest -- see the automation reference).

**The same from a screen.** Add a tab and set its kind to "Files on a server
(SFTP)". The address, who signs in and the key are on that tab, beside the
kind -- the same fields a server terminal has. Then the tab is two lists of
files: this PC's folder on the left, that server's on the right. Tick what you
want and press Send or Bring here; the `...` at the end of a row makes a
folder, renames one, or deletes one. On a phone the switch at the top shows one
side at a time.

**Telling production from staging.** In a connection's settings, give the
server a name under "Name for this server" -- Production, Staging -- and pick a
colour. The name belongs to the server rather than the tab, so every tab that
reaches it, terminal or files, wears the name with a square of its colour, and
so does the question before a file there is deleted, renamed or replaced. A
server behind a bastion is a different server for each bastion. Tick "Ask for
this name to be typed before anything that cannot be undone" and those actions
wait until it is typed. Every named server is listed under "Server names" in
the settings.

**A server whose key is not the one from last time is refused.** If you
reinstalled the server, delete its line from `data/known-hosts.json`.

**An AI's settings file on another machine is written into only when you allow it.** The first time an
AI tab runs on a server or a MicroVM, the screen asks whether one entry may go into that AI's settings file
there, so it can report what it is doing (the file as it would be is shown before you choose). Without it
the tab still works; its state is read from the screen and the conversation record. To take it out later,
open that machine under Settings > Where it runs, untick the AI and save: only this app's entry is removed, the
next time the machine is reached. A machine an earlier version wrote into without asking is asked about
once: keep it, or take it out.

**Keeping a machine's AIs running while this app is away.** Open the machine under
Settings > Where it runs, put the SHIKISHA bridge on it, and choose under "While this app is away": stop them when SHIKISHA-TERM quits,
keep them running for a number of hours, or (a server only) keep them running for as long as they run.
Kept running, the AIs there go on working when SHIKISHA-TERM quits or this PC is off, and the next start
goes back to each of them -- the same AI, its screen as it was. Quitting asks whether to leave them running
or stop them all. While SHIKISHA-TERM is away, a call an AI makes that asks nothing back (a report, a note, a
notification) is kept there and handed over when SHIKISHA-TERM is back; any other call is told at once that the
PC is away, and is listed on that machine's page when it is back (which tab, which command, when -- never what
it said). The same page lists the AIs running there, with a button to stop each. A MicroVM kept
running is billed for the time chosen, and no longer than your E2B plan lets it run in one go. Tabs on one
machine, run as one user, are not kept apart by what each tab is allowed: to keep AIs apart, put them on
separate machines.

A tab that runs the `ssh` command itself still works as before (the kind is
"SSH (ssh.exe)").

## 6. Where passwords and tokens live

Register them under Settings > Desk > Secrets. Automation names them in one
short word and never receives the value. Registering one asks which pages it may
be filled in on, and whether automation may use it as a person only or as an AI
as well. What each box decides is in the [settings reference](SETTINGS.md).

Listing, changing and deleting happen on that same screen. Press a row to open it.

## 7. Automation

What to do when a tab finishes, asks a question, or goes quiet: hand the answer to
another tab, answer a confirmation, send a notification. Written in a few lines of
Lua, or described in plain words and written for you by an AI you already have.
See the [automation reference](https://shikisha-term.com/automation/).

## 8. Where the settings are

The gear at the foot of the list on the left opens them. These are all the
screens there are. **What is on each one** is in the
[settings reference](SETTINGS.md), which the program writes out of its own
settings screen, so it cannot fall behind.

<!-- guide: screens -->

**The program's settings**

- **Update** — Newer versions, and going back
- **Basic** — Tab width, chaining, language
- **AI agents** — Assistant AI, deciding AI, connections, agreements
- **GitHub / Git** — Personal access tokens, SSH keys and sign-ins
- **Worktrees** — Host-dependent project markers
- **Remote access** — Remote control & QR
- **Server version** — Open a SHIKISHA server's board in a window of its own
- **Notifications** — The phones that receive notifications
- **Site notifications** — Sites you allowed or blocked
- **Shortcuts** — What each key does
- **Quick commands** — Buttons that send a command or a prompt
- **Quick actions** — One-tap buttons in the input bar
- **Where it runs** — Servers over SSH, and MicroVMs, to cut worktrees on besides this PC
- **Server names** — Tell production from staging at a glance
- **Limits on handing work** — How far an AI may go when it hands work to other tabs or drives a page
- **AIConfer** — AIs asking each other things, shown as a chat
- **AI allowance** — Each AI subscription's allowances, and when they come back
- **External control** — Let programs drive this app
- **Carrying conversations** — What survives a restart
- **Files** — Automation & secrets paths
- **Run results** — Download past rally logs
- **Saved logins** — Browser logins kept for reuse
- **Snapshots** — Snapshot pictures taken by automation

**A desk's settings**

- **Basics** — Name, automation name, automation folder
- **Notifications** — Chats, this PC, phones
- **Automation permissions** — What a person and an AI may run
- **Secrets** — Passwords and tokens
- **Stop conditions** — When the joint work ends
- **Automatic names** — The AI that names and describes working folders
- **Automation doors** — Files and URLs a script can reach
- **Export** — This desk as one file

**A project's settings**

- **The project's page** — Worktree creation rules, name and checkout, Git authentication, protected branches, what the AI is told, setup

<!-- /guide -->

## 9. When something is wrong

- **The program does not start after a settings change** — double-click
  `Settings.cmd` beside the program. It opens only the settings screen, where the
  change can be undone.
- **A folder is marked as not on this PC** — press its heading; the program shows
  the steps it would take (clone, make the branch) and runs them when you say so.
- **A tab says it is working but nothing moves** — `Ctrl+B r` restarts it carrying
  the conversation over; `Ctrl+B R` starts a new one.
- **A hand-over runs away** — the stop button on the status line, or `Ctrl+B x`,
  halts every hand-over at once and interrupts every AI in the middle of a turn
  (the same Esc or Ctrl+C you would press in that tab; which key is written in
  its profile). Automation stays off until you turn it on again with `Ctrl+B a`.
- **What happened** — `logs/hooks.log` beside the program records every state
  change and every automation run, stamped with the seconds since the program
  started.
- **The zip shows a Windows warning on first start** — the zip is not code-signed;
  the Store copy is. [Why, and how to check the download](https://github.com/styleio/ShikishaTerm/blob/main/SIGNING.md).

## 10. Updating

The program looks, once at start and once a day while it runs, whether a newer
version is published. If one is, a small card in the sidebar says so, once. Its
two buttons install nothing: **Open Update** leads to Settings › Update, and
**Not now** puts the card away for that version.

Settings › Update is the one place a version is installed, whether you came from
the card or on your own. **Fetch and install** downloads the zip, checks it against
its published SHA256 and its signature, and then — after the same question
quitting asks while an AI is at work — swaps the files and starts the program
again. Your settings, data, logs, desks and scripts are not touched, and a
copy of the settings is made before the first start of the new version. The
version replaced is kept, and **Go back** on the same card puts it back. The
Store copy hands the same button to the Store, which installs and restarts.

Skipped a few versions? The newest one carries your settings forward one version
at a time, so nothing has to be installed in between.

**Keeping this PC's terminals running.** Settings › Basic › **Terminals on this
PC** is on from the start. The terminals of this PC (and the AIs in them) run in a
separate background process of their own rather than inside the app. Closing the
window, quitting the app, updating it, or the app closing unexpectedly does not stop
them: when the app starts again, each tab comes back to the same terminal, with its
screen, and what the AI there asks of the app through `shikisha` reaches the app
that is running now. Quitting says how many terminals go on running; to stop them,
answer **No** (stop every terminal and AI, then quit) there. The first time a
terminal is opened this way, a card in the left column says the same, once.

Turned off, terminals opened from then on run inside the app and stop when it
quits. Those opened while it was on and still running go on running (their tabs
come back to the same terminals, so the same AI never runs twice); under the
setting, how many there are is shown with a **Stop them** button. Restarting a tab
moves it into the app.

Keeping the PC awake (Settings › Basic › Keep the PC awake) goes on applying to the
terminals left running after the app quits. With "While an AI is working", that
lasts until the terminal of an AI that was working when the app quit has shown
nothing for 10 minutes.

The Store copy is the exception for updates: installing an update from the Store
closes the background process too, and the tabs then come back on their
conversations as they do without this setting. The setting says so too.
