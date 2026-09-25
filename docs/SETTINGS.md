# Settings

Every setting this program has, screen by screen, in the order the settings themselves list them. What is on the screen and what the keys do is in the manual; this is what each entry means.

<!-- Written by the program from its own settings screen. Edit the settings, not this file. -->

## The program's settings

The same on every desk. Open them from the gear at the foot of the list on the left.

### Basic

Tab width, chaining, language

- **Tab bar width** — Pixels. Leave empty for the default width, or drag the bar's edge in the window — 0 puts it away
- **Automatic chain limit** — How many times AIs may hand work to each other in a row
- **Answer settle time** — Seconds of quiet before an answer counts as finished. Short values hand work over mid-thought
- **Checking on a tab that keeps working** — Seconds before automation is told again that a tab is still working. Empty or 0 means it is never told again. This is how a script notices an AI that has stopped answering, so set it generously -- cutting a long think short costs more than noticing a hang a minute late
- **Auto-switch** — Only automation that asks to be watched switches the screen; handing work to a tab does not. Stays put for a few seconds after you move around yourself, and never while the settings are open
- **Where to start** — Remembered by name, so reordering or adding desks does not change where you land. Turn this off to always start at the first one.
- **Deleting a worktree** — Turned off, "Delete completely" in the list's right-click menu deletes the worktree at once, without asking.
- **The ✕ button** — The AIs keep working and the phone stays connected. Click the icon in the notification area to bring the window back. To quit, right-click that icon and choose Quit, or press Ctrl+B q. Turn this off to quit when the window is closed. Either way, you are asked first while a tab is still working.
- **Screen and work** — Nearly all of this program's memory is the screen. Run it separately and the screen can go -- for want of memory, or in a fault -- while the tabs, the terminals and every AI at work in them carry on; a new screen is put back over them. Closing the screen yourself still does whatever “The ✕ button” above says. The board is served to this machine and nothing else; sharing it with a phone is a separate setting. Takes effect the next time this program starts.
- **Copying from full-screen tools** — tmux, Neovim, fzf and most full-screen tools copy this way, including over ssh -- with this off, copying inside them does nothing. Reading the clipboard is never allowed, so nothing running in a tab can see what you copied elsewhere
- **Terminal engine** — Windows has one of these and this program carries its own. Which one is in use decides how a program's output reaches the screen. The one shipped here is a fixed version we have checked, so the answer does not depend on how up to date this machine happens to be — on an older Windows, the one in the box can be some way behind.
- **Default command** — What a project opens with when it is added, and what an empty folder opens when pressed. A PC without Git Bash opens PowerShell.
- **Browser data** — Where the browser cache and login state live. Sharing carries logins across PCs but adds Drive sync load
- **Where pages are drawn** — Only matters when a device is connected to this one. Drawn here, a page can be watched from a phone, keeps working with nobody connected, and is one signed-in session for every device. Drawn on the connected device it is faster and sharper, but it needs that device to be there and only it can see the page. Either way the page reaches the network through this machine, so a port opened here means the same thing on both sides. New pages only — a page already open stays where it is.
- **UA (browser name)** — The name sites see this browser by. Leave it empty and it looks like Edge. Fill it in when a site says something like "this browser may not be secure" and will not let you sign in — putting a name like Chrome here often gets you through. Pages opened after you change it use the new name. A tab can be given its own name, which wins over this one.
- **Terminal font** — The font and size the terminal is drawn in. Ctrl+wheel over the terminal changes the size at any time, and it is remembered here. Leave the name empty for the built-in stack, chosen so box-drawing characters and Japanese each take one cell.
- **Colours** — The colour scheme, for the terminal and for the window around it. The list is every scheme this PC already has — the ones Windows Terminal is carrying, plus any scheme file dropped into the config/themes folder — so a theme you already use can be picked by the name you already know it by. Choosing a light scheme turns the whole window light.
- **Language** — Screen language. Automatic follows the OS. Choosing here overrides the OS and takes effect after a restart (add lang/<code>.json to contribute a translation)

### Worktrees

Host-dependent project markers

**Host-dependent project markers**

- **File names** — When a file of one of these names is in the original folder or in a folder directly inside it, the project is treated as host-dependent. Separate names with commas.
- **Folder names** — When the original folder's path contains a folder of one of these names, the project is treated as host-dependent. Separate names with commas.

### AI agents

Assistant AI, deciding AI, connections, agreements

**Assistant AI and deciding AI**

- **Assistant AI** — Answers every question this app asks, and is what a new AI tab runs.
- **Yolo mode** — New AI tabs skip the AI's permission checks
- **Deciding AI** — Picks the next move on a page driven in words, and answers a script's ai_choose. A decision model is far quicker; a conversation model or a subscription AI works too.

**Automatic names**

- **AI for automatic names** — Writes the name and summary of a working folder with Auto on, and the branch name that follows them. A desk can choose another one.
- **Model for those names** — A name costs a fraction of an answer this way. Turn it off if the names it writes are not recognisable.
- **Model**

### Git accounts

Tokens, and the sign-ins of this PC's git and gh

- **Token**
- **Display name**

### Update

Newer versions, and going back

- **Looking** — One request to api.github.com for the newest version number; nothing is sent. The Store copy asks the Store instead. Installing is always the button above.

### Remote access

Remote control & QR

**Use from your phone**

- **Turn on** — Let me check status and send instructions from my phone
- **Port** — Usually no need to change
- **Password** — Optional second factor: the URL token in a notification alone no longer opens the board. Empty = off (your own risk). Applies on save (existing connections are cut)
- **Fixed token** — On, the string below is the token and the phone keeps it in its URL and storage (bookmarkable; a discarded tab needs no new QR). Disconnect still stops that phone's screen and touches at once, but the token is unchanged, so it can come back by opening the link again. To shut a phone out for good, change this string. Stored in plain text in config.json — pair it with a password
- **Devices that have this board's key**
- **Tailscale** — Only your own devices can reach this address, from anywhere. The safe way to use it
- **Home network** — Anyone on the same Wi-Fi could use this link. Fine at home, not on a café or office network
- **This PC only** — This address exists only inside this PC, so no phone can reach it
- **Open to the internet** — This address is reachable from outside. Anyone who gets the link can operate this PC

### Shortcuts

What each key does

**Keys inside SHIKISHA-TERM**

- **Prefix key** — Pressed before the keys below. Ctrl+B unless you change it — people coming from tmux often want Ctrl+A.

### Quick commands

Buttons that send a command or a prompt

- **Icon**
- **Inside**
- **Which AI** — This AI is started in a new tab in the folder in front, and given the prompt.
- **Sends to**
- **Press Enter at the end** — Untick to leave the text typed in, not sent.

### Quick actions

One-tap buttons in the input bar

- **Name** — What the button in the bar says.
- **Text to insert** — Dropped into the composer when the button is pressed, ready to send.
- **Lua to run** — Run when the button is pressed. It is checked here before it can be saved.
- **Run as Lua instead of inserting text** — Advanced. On tap the body runs as Lua instead of being typed into the composer, with the same commands automation has.
- **Name** — What the folder's button in the bar says.

### Where it runs

Servers over SSH, and MicroVMs, to cut worktrees on besides this PC

- **Name** — What the picker calls it. Two accounts on one server are two entries
- **Image** — The service's image a project's MicroVM is made from
- **Minutes before it pauses** — Running untouched this long, a MicroVM pauses. Paused, it keeps everything; a request to one of its addresses, or opening it again, starts it where it stopped.
- **Address** — ssh://you@machine:22
- **Colour** — A small square in front of the name. The name is always shown with it.
- **Ask for this name to be typed before anything that cannot be undone** — Deleting, renaming or replacing files on this server waits until the name is typed.
- **Name for this server** — Shown beside every tab that reaches this server, and in every question about something that cannot be undone there:

### Server names

Tell production from staging at a glance

- **Colour** — A small square in front of the name. The name is always shown with it.
- **Ask for this name to be typed before anything that cannot be undone** — Deleting, renaming or replacing files on this server waits until the name is typed.
- **Name for this server** — Shown beside every tab that reaches this server, and in every question about something that cannot be undone there:

### Operate a tab

Limits for 🎯 driving another tab

**Operate a tab (🎯)**

- **Max turns**
- **Max seconds**
- **Max output size**
- **When a limit is reached** — "Keep going" resets the budget and trusts the operator to finish on its own — pick it if the operate stops on you too often.
- **Settle wait (ms)** — After each action, wait until the page stops changing (up to this long) before reading it. 0 = don't wait.
- **Ask before acting** — A brake: pause for you to approve a step on the page before it runs. Declining holds the run.

### AI allowance

The 5-hour and 7-day windows of Claude and Codex

This screen has nothing to fill in. What it shows depends on what is set elsewhere.

### External control

Let programs drive this app

**External control (named pipe)**

- **Who can call** — "What this app started" means a tab's CLI and whatever it starts in turn. Each tab is given its own key as it launches.
- **Connect to**

### Carrying conversations

What survives a restart

This screen has nothing to fill in. What it shows depends on what is set elsewhere.

### Files

Automation & secrets paths

- **Automation (shared)** — Used when a tab has none of its own
- **Secrets file** — The file that holds passwords and tokens. It can be encrypted -- set a master password with [k] on the app's INDEX screen

### Run results

Download past rally logs

This screen has nothing to fill in. What it shows depends on what is set elsewhere.

### Notifications

The phones that receive notifications

This screen has nothing to fill in. What it shows depends on what is set elsewhere.

### Saved logins

Browser logins kept for reuse

This screen has nothing to fill in. What it shows depends on what is set elsewhere.

### Snapshots

Snapshot pictures taken by automation

This screen has nothing to fill in. What it shows depends on what is set elsewhere.

## A desk's settings

Each desk answers these for itself, so two desks can work in different ways. Open them from Desk in the settings.

### Basics

Name, automation name, automation folder

**Desk**

- **Name**
- **Name used by automation** — Secrets are filed under this, and automation uses it. Renaming the desk on screen changes nothing
- **Definition file**
- **Automation**

### Notifications

Chats, this PC, phones

- **Default destination**
- **Chat id**
- **The main destination**

### Automation permissions

What a person and an AI may run

This screen has nothing to fill in. What it shows depends on what is set elsewhere.

### Secrets

Passwords and tokens

- **Where this secret may be used**

### AI × AI discussion

Several AI tabs discussing or working together

- **Participants**
- **Turn order**
- **Round limit**
- **Judge**
- **Moderator**

### Stop conditions

When the joint work ends

This screen has nothing to fill in. What it shows depends on what is set elsewhere.

### Automatic names

The AI that names and describes working folders

- **Model**

**Automatic names and summaries**

- **AI** — An assistant AI is asked the lightest way it can be: no tools, and the smallest model it has when AI agents says to use one. An AI provider runs on that provider's account.
- **Branch name** — The first name written replaces the one this app drew (mighty-gannet), once. A branch you named yourself, and one that has already been pushed, keep their names.

### Automation doors

Files and URLs a script can reach

This screen has nothing to fill in. What it shows depends on what is set elsewhere.

### Export

This desk as one file

This screen has nothing to fill in. What it shows depends on what is set elsewhere.

## A project's settings

Each project answers these for itself: a team's rules are its repository's, and a client's project and your own can sit on one desk. Open them from Projects in the settings, or from Project settings in a folder's right-click menu on the board.

### The project's page

Worktree creation rules, name and checkout, git account, protected branches, what the AI is told, setup

**Project**

- **Name**
- **Checkout** — Worktrees of this repository are cut from here
- **On other machines**
- **Repository**

**Git account**

- **Account to use**
- **Delete this project** — The working folders and tabs stay. Only their tie to this project goes

**Worktree Creation Rules**

- **Branch prefix** — Put in front of every branch made for this project, whether the name is typed, drawn or written by an AI. Leave it empty for no prefix.
- **Worktree Placement** — The folder this project's worktrees are made in. Begin it with {worktrees} (the app's place) or {origin_folder} (the original folder), or write an absolute path. {project} is the project's name and {origin} the original folder's name. A worktree made beside the original ({origin_folder}\..) is named "original-name" and stands at the same depth as the original.
- **Files to Inherit from Original**

## Shortcuts

As the program ships. Whatever has been changed is under Settings > Shortcuts.

- `Ctrl+B q` — Quit
- `Ctrl+B n` — Next tab
- `Ctrl+B p` — Previous tab
- `Ctrl+B t` — Add a tab
- `Ctrl+B &` — Close this tab (asks first while its AI is working)
- `Ctrl+B T` — Reopen the tab closed last
- `Ctrl+B r` — Restart this tab, carrying the conversation over
- `Ctrl+B R` — Restart this tab from nothing
- `Ctrl+B % / Ctrl+B |` — Split the screen beside this one
- `Ctrl+B " / Ctrl+B -` — Split the screen below this one
- `Ctrl+B o` — Move to the next pane
- `Ctrl+B X` — Close this pane (the tab keeps running)
- `Ctrl+B =` — Put the dividers back to even halves
- `Ctrl+B s` — Put the tab bar away, or bring it back
- `Ctrl+B g` — Show the changed files on the right, or put them away
- `Ctrl+B <` — Move the divider left / up
- `Ctrl+B >` — Move the divider right / down
- `Ctrl+B w` — Desk list
- `Ctrl+B W` — Next desk
- `Ctrl+B [` — Copy mode (/ searches, n and N walk the matches)
- `Ctrl+B c` — Copy the latest answer
- `Ctrl+B l` — Lock input
- `Ctrl+B a` — Automation on / off
- `Ctrl+B x` — Emergency stop
- `Ctrl+B b` — Send the prefix key itself to the program
- `Ctrl+B ?` — This list
- `Ctrl+B :` — Command palette
- `Ctrl+B k / Alt+Shift+K` — Quick commands
- `Ctrl+B m / Alt+Shift+M` — Ideas
