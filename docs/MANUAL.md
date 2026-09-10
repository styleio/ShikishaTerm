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

- **The workspace** — the name at the very top. Press it to switch to another.
- **Working folders** — a heading per folder, with the tabs working in it under
  it. A folder is where an AI runs; the colour on its heading is which project it
  belongs to. Its `+` opens the folder's menu (add a tab, work on another branch,
  edit).
- **Branches of a project** stand a step in under the project's own folder, in
  one box. The project's heading says which branch it is on and how many branches
  hang under it; press that count to put them all away or bring them all back.
- **A tab** — its number, its name, and a dot: **green** while it works, **blue**
  when it has answered, **amber** when it is waiting for you to answer. Under the name, the
  branch it is on, its pull request, and any ports it is listening on; under that,
  what it last said about itself.
- **Add a working folder** at the bottom of the list, then the gear (settings)
  and `?` (this page).

**The middle** is the tab in view: a terminal, a browser page, or the git panel.
Once the window is divided, each pane's caption carries ▥ and ▤ to divide it again
and ✕ to close that view.

**The bar above the input box** is the convenience bar: one press sends a stock
instruction to the AI in view (continue, explain, review, fix). The small picker at
its left chooses what the bar holds: the stock replies, your own quick actions, the
macro recorder, git, an AI's command suggestion, or the target another tab drives.

**The input box** at the bottom is where you type to the tab in view. On a phone
it is the only way in.

**The status line** at the very bottom says which workspace this is, whether
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
| `Ctrl+B r` / `Ctrl+B R` | Restart this tab carrying the conversation over / from nothing |
| `Ctrl+B %` / `Ctrl+B "` | Split the screen beside / below this one |
| `Ctrl+B o` | Move to the next pane |
| `Ctrl+B X` | Close this pane (the tab keeps running) |
| `Ctrl+B =` | Put the dividers back to even halves |
| `Ctrl+B <` / `Ctrl+B >` | Move the divider left or up / right or down |
| `Ctrl+B s` | Put the tab bar away, or bring it back |
| `Ctrl+B w` / `Ctrl+B W` | Workspace list / next workspace |
| `Ctrl+B [` | Copy mode (`/` searches, `n` and `N` walk the matches) |
| `Ctrl+B c` | Copy the latest answer |
| `Ctrl+B l` | Lock input on this tab |
| `Ctrl+B a` | Automation on / off |
| `Ctrl+B x` | Emergency stop |
| `Ctrl+B b` | Send the prefix key itself to the program |
| `Ctrl+B :` | Command palette |
| `Ctrl+B ?` | The key list |

On INDEX the menu is single letters: `e` settings, `p` the palette, `f` find,
`i` the QR code for a phone, `r` restart stopped tabs, `w` switch workspace,
`t` send a test notification, `k` the master password, `?` help.

The mouse works too: wheel to scroll, Ctrl+wheel to change the terminal's text
size, drag to copy, right click to paste, click a tab name to switch. Drag the
divider between panes to rebalance them, double-click it for even halves. Drag the
tab bar's right edge to set its width, double-click for the width it ships with,
and drag it shut to give the whole window to the terminal.

## 3. Working folders and branches

**Add a working folder** at the bottom of the list. Pick the folder and it appears
with a `+` to press. Every tab you add from that `+` starts in that folder.

**Work on another branch at the same time**: press the `+` on a folder that is in
a git repository and choose **Parallel work (git worktree)**. The dialog shows
what will happen before it happens:

- the name of the new branch (leave it and a name is chosen), and what it grows
  from;
- **Start here** — what runs in the new folder: the same tabs as this project,
  one of the AIs installed on this machine, or nothing;
- **One folder per AI** — tick it and pick the AIs, and one folder is made per AI,
  each branch named for its AI, so the same task can be given to several at once;
- the folder that will be made and the exact `git worktree add` line;
- **Bring along** — things git does not carry (ignored files such as `.env` or
  `node_modules`) that can come with it.

Press **Make it**. The new folder appears under the project in the list. A folder
made this way lives in `<project>.worktrees/<branch>` beside the project.

A folder that is not on this PC (settings carried over from another machine)
says so in the list; press it and the program tells you what it would take to put
it back, and does it when you say so.

## 4. From your phone

Turn on phone access in the settings and scan the QR code. Every tab, its state,
and a box to reply are on the phone; what the PC can do, the phone can do too.
See [From your phone](https://shikisha-term.com/phone/) for the three levels of setup.

## 5. Connecting to a server

Choose **Server (this program connects)** as a tab's kind, give it the address,
the port and the user, and save a password. The password is never shown again,
and no `user@host's password:` prompt appears. Opening the tab gives you that
server's terminal.

Automation can read and write files there too, by naming that tab (`sftp_ls`,
`sftp_get`, `sftp_put` and the rest -- see the automation reference).

**A server whose key is not the one from last time is refused.** If you
reinstalled the server, delete its line from `data/known-hosts.json`.

A tab that runs the `ssh` command itself still works as before (the kind is
"SSH (ssh.exe)").

## 6. Where passwords and tokens live

Register them under **Secrets** on the workspace's settings page. Automation
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
workspace's settings page. Press a row to open it.

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
again. Your settings, data, logs, workspaces and scripts are not touched, and a
copy of the settings is made before the first start of the new version. The
version replaced is kept, and **Go back** on the same card puts it back. The
Store copy hands the same button to the Store, which installs and restarts.

Skipped a few versions? The newest one carries your settings forward one version
at a time, so nothing has to be installed in between.
