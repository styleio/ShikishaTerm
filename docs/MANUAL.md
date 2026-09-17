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
  A worktree's card has a red **Delete completely** at the bottom. It deletes the
  whole folder; links are unhooked first, so what they point to stays, and files
  copied in that git does not track go with it. It does not delete while there are
  uncommitted changes. Tick "Don't show this again" in the question and it deletes
  at once from then on (turn the question back on under Settings > Basic > Deleting a worktree).
- **A tab** — its number, its name, and a dot: **green** while it works, **blue**
  when it has answered, **amber** when it is waiting for you to answer. Under the name, the
  branch it is on, its pull request, and any ports it is listening on; under that,
  what it last said about itself.
- Under INDEX and Issue, the **PROJECT** heading with a `+` at its end (add a project).
  At the very bottom, the gear (settings), `?` (this page), the scissors (tools) and
  🎛️ (quick commands).

**Quick commands** are a grid of buttons you make yourself. 🎛️ or `Ctrl+B k` brings
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

**Along the top of the middle** are the tabs of the folder in view. Press one to
switch to it, and `+` to add one. A tab's ✕, or a middle click on it, closes that
tab. The ✕ shows on the tab in view and on the one under the pointer. Only when
its AI is working or waiting for an answer are you asked first. A closed tab can
be opened again from the ▾ at the end of the row; with a CLI that can resume a
conversation by name (Claude Code, Codex) the conversation comes back with it.
What was on its screen does not.

**The bar above the input box** is the convenience bar: one press sends a stock
instruction to the AI in view (continue, explain, review, fix). The small picker at
its left chooses what the bar holds: the stock replies, your own quick actions, the
macro recorder, git, an AI's command suggestion, or the target another tab drives.

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

| Key | What it does |
|---|---|
| `Ctrl+B q` | Quit |
| `Ctrl+B 0`–`9` | Switch tab (0 = INDEX) |
| `Ctrl+B n` / `Ctrl+B p` | Next tab / previous tab |
| `Ctrl+B t` | Add a tab |
| `Ctrl+B &` | Close this tab (asks first while its AI is working) |
| `Ctrl+B T` | Reopen the tab closed last |
| `Ctrl+B r` / `Ctrl+B R` | Restart this tab carrying the conversation over / from nothing |
| `Ctrl+B %` / `Ctrl+B "` | Split the screen beside / below this one |
| `Ctrl+B o` | Move to the next pane |
| `Ctrl+B X` | Close this pane (the tab keeps running) |
| `Ctrl+B =` | Put the dividers back to even halves |
| `Ctrl+B <` / `Ctrl+B >` | Move the divider left or up / right or down |
| `Ctrl+B s` | Put the tab bar away, or bring it back |
| `Ctrl+B g` | Show the changed files on the right, or put them away |
| `Ctrl+B w` / `Ctrl+B W` | Desk list / next desk |
| `Ctrl+B [` | Copy mode (`/` searches, `n` and `N` walk the matches) |
| `Ctrl+B c` | Copy the latest answer |
| `Ctrl+B l` | Lock input on this tab |
| `Ctrl+B a` | Automation on / off |
| `Ctrl+B x` | Emergency stop |
| `Ctrl+B b` | Send the prefix key itself to the program |
| `Ctrl+B :` | Command palette |
| `Ctrl+B k` | Quick commands |
| `Ctrl+B ?` | The key list |

On INDEX the menu is single letters: `e` settings, `p` the palette, `f` find,
`i` the QR code for a phone, `r` restart stopped tabs, `w` switch desk,
`t` send a test notification, `k` the master password, `?` help.

The mouse works too: wheel to scroll, Ctrl+wheel to change the terminal's text
size, drag to copy, right click to paste, click a tab name to switch. Drag the
divider between panes to rebalance them, double-click it for even halves. Drag the
tab bar's right edge to set its width, double-click for the width it ships with,
and drag it shut to give the whole window to the terminal.

## 3. Working folders and branches

Press the folder-plus at the end of the **PROJECT** heading to add a project: open a
folder on this PC, clone one from a URL, or make a new one. A project added starts with
Settings → Basic → **Default command** running (PowerShell, Command Prompt or Git Bash; PowerShell
unless chosen). A folder left empty by closing its tabs opens it again when pressed.

**A project on a server you reach over SSH** is added from the same dialog. "A project on an SSH
host" takes a name, host, user, port and key file, or fills them in from an alias in
`~/.ssh/config`. Once there is a host, **Where** at the top of the dialog chooses this PC or
the host. On a host you walk its folders and add one, or clone from a URL on that machine; git
and the terminal run over there.

**Work on another branch at the same time**: press the `+` on the heading. The
**Create worktree** dialog opens:

- **Project** — which project it is made from;
- **Name or what to create it from** — three tabs. **GitHub** searches the project's
  issues and pull requests; picking one names the worktree and ties the work to it
  (a pull request's branch is fetched). **Branch** picks what it grows from.
  **Name** is typed; leave it empty and a name is chosen;
- **AI** — what runs in the new folder. The default is Settings → Basic → Assistant AI;
- **More** — one folder per AI, where it goes, things git does not carry (`.env`,
  `node_modules`) to bring along, and the exact `git worktree add` line.

Press **Create worktree** (`Ctrl+Enter`). A "Creating the worktree…" row appears under
the project's heading and turns into the folder's card once it is made. Its ✕ stops it
and takes back the half-made folder and the new branch. If it fails, the row says why
and offers **Try again** or **Dismiss**. Folders made this way live in
`~/SHIKISHA-TERM/branches/<project>/<name>`.

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

A tab that runs the `ssh` command itself still works as before (the kind is
"SSH (ssh.exe)").

## 6. Where passwords and tokens live

Register them under **Secrets** on the desk's settings page. Automation
names them in one short word and never receives the value. Two things are asked
when you register one:

- **Where this secret may be used** -- it is filled in on those pages and
  nowhere else. `https://example.com` is that whole site,
  `https://example.com/api` only the pages under `/api`, and
  `https://*.example.com` the site and every subdomain
- **Who may use it** -- two answers, **a person** and **an AI**, and to begin
  with only the first. Tick the AI as well for the ones a script an AI set
  going should be able to use; tick only the AI and it becomes a key kept for
  an AI's errands, which nobody spends by hand

Listing, changing and deleting all happen in that same **Secrets** card on the
desk's settings page. Press a row to open it.

## 7. Automation

What to do when a tab finishes, asks a question, or goes quiet: hand the answer to
another tab, answer a confirmation, send a notification. Written in a few lines of
Lua, or described in plain words and written for you by an AI you already have.
See the [automation reference](https://shikisha-term.com/automation/).

## 8. When something is wrong

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

## 9. Updating

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
