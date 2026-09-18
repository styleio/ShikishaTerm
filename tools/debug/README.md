# Debugging and checking tools

The tools you run by hand: to find out what the program is actually doing, and
to confirm that a change does what it says. None of them ships and none of them
runs in CI. Start here before writing one — the tool you need may already exist.

## When you need a tool that is not here

**Put it in this folder and give it a row below.** A script written into a
session's temporary folder is gone when the session is, and the next person
rebuilds it from nothing — having first lost an afternoon finding out it was
needed. A tool that is here and listed is found; a tool that is only mentioned
somewhere is not.

Say at the top of the file what it is for, how to run it, and what it needs.

## Where things live

| Kind | Where | Why there |
|---|---|---|
| Scripts for debugging and checking | `tools/debug/` | This folder |
| Rust programs for debugging and checking | `src/bin/`, `crates/core/src/bin/` | cargo builds a program from there, and moving one means telling `Cargo.toml` where it went. They are listed below all the same |
| Tools a build, CI or a release runs | `tools/` | Not debugging tools: something automatic depends on their names and places. `notices.ps1`, `stage.ps1`, `msix.ps1`, `conpty.ps1` and the rest |

## Names

A script that only runs in one environment says so in its name, between the name
and the extension:

| Mark | Runs on | Example |
|---|---|---|
| `.win` | Windows only | `thing.win.ps1` |
| `.wsl` | WSL, or any Debian-family Linux with `apt` | `sftp-server.wsl.sh` |
| none | Anywhere Node and cargo run | `shoot.mjs` |

A Rust program cannot carry the mark, because its file name is the name of the
command. **The Runs on column below is what to trust**, for every tool; the mark
in a name is there so the folder can be read at a glance.

## Finding out what is going on

| Tool | Runs on | What it is for | How to run it |
|---|---|---|---|
| `src/bin/pty_probe.rs` | anywhere | A command in a pseudo terminal, with everything it says captured for about ten seconds. `--watch` never types into it — some of what is worth watching is an AI, and typing into one is a turn on somebody's account | `cargo run --bin pty_probe -- [--watch] <command> [args...]` |
| `src/bin/vt_writer.rs` | Windows | A stand-in for a program that draws on a terminal, writing the same bytes every run, so that two runs differ only in the pseudo console that carried them | `cargo run --bin vt_writer -- <poured \| redrawn \| sequences>` — the `frame_bench` measurement runs it |
| `src/bin/sshd_probe.rs` | anywhere | A small SSH server on the loopback: one user and password, a terminal that echoes, and files over one folder. For pointing the running app at. It opens files the way OpenSSH does, so what passes here passes there | `cargo run --bin sshd_probe -- 2222 tester hunter2 [folder]` |
| `src/bin/sftp_probe_check.rs` | anywhere | Connects the way the app does and prints what crosses the wire — to tell "the library said no" apart from "nothing arrived" | `cargo run --bin sftp_probe_check -- 2222 tester hunter2` |
| `crates/core/src/bin/e2b_probe.rs` | anywhere | Whether a terminal and files in a cloud sandbox work, against the real service. Needs an E2B key, costs money while the sandbox lives, and kills it on the way out | `cargo run -p shikisha-core --bin e2b_probe` |
| `crates/core/src/bin/asks_probe.rs` | anywhere | What this app would hear from a CLI's own record of a conversation, against a real record, with nothing running. How a person's words are told apart from what tools returned and what the CLI told itself is described per CLI in `profiles/<name>.json` under `resume.asks`; when a CLI changes how it writes those files, this is what says so. Every line it prints should be something a person typed | `cargo run -p shikisha-core --bin asks_probe -- "Claude Code" <record.jsonl>` |
| `crates/core/src/bin/resume_probe.rs` | anywhere | Why each tab of an install would, or would not, come back to the conversation it was having. Asks the same question a launch asks, against a real install's settings and its real `data\last-session`, with nothing running -- and prints every part of the answer: what is remembered, what the tab is, whether the CLI's record of that conversation is still on this computer | `cargo run -p shikisha-core --bin resume_probe -- <install folder>` |

## Confirming a change

| Tool | Runs on | What it is for | How to run it |
|---|---|---|---|
| `tools/debug/instance.win.ps1` | Windows | A copy of the app that is nobody's: its own folder, settings, state, door and ports, started from this checkout's build. What makes two agents debugging at once possible -- one running copy per folder is the rule, so a copy of its own is the only way to have a second. **Ports come from 9400-9499, two at a time** (even: the board, odd: the window's DevTools port), the first free pair, so two of these cannot land on each other. It writes the client's MCP settings and prints where, along with the process id and the key's file; nothing of a copy somebody is using is read, written or stopped | `cargo build`, then `tools/debug/instance.win.ps1 [-At <folder>] [-Mcp <path to .mcp.json>] [-Port <n>] [-Cdp <n>]`, and `-Stop` when done |
| `SHIKISHA-TERM.exe --mcp` | anywhere | Not a debugging tool but the way to drive one: the app's own automation primitives, spoken as MCP tools, against a copy that is already running. `tools/list` is the app's own `list`, so it answers for what can really be called. The instance above prints the command | in a client's MCP settings: `<exe> --mcp --pid <its process id> --token-file <its root>\data\api-token` |
| `tools/debug/shoot.mjs` | anywhere, with Chrome | Photographs the board page in both languages, both colour schemes and at window and phone width, into `target/shots`. This is how section 9 of [the screen rules](../../docs/design/STYLEGUIDE.md) is marked: nothing has to be running, so the screen being judged is never the one on the way to it | `node tools/debug/shoot.mjs tools/debug/scenes/files.mjs [--only <scene>]` |
| `tools/debug/scenes/` | — | Which screens `shoot.mjs` puts up, a line each. How to write one is at the top of `files.mjs` | — |
| `tools/debug/settings-shoot.mjs` | anywhere, with Chrome | Photographs the settings page the same way, served by `settings_serve` over the scene's own settings, so a scene can type into it and press what asks the app something | `node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-servers.mjs [--only <scene>]` |
| `src/bin/settings_serve.rs` | anywhere | Serves the settings page from the app's own code over a copy of a settings file in a temporary folder, and prints its address. What `settings-shoot.mjs` opens | `cargo run --bin settings_serve -- <lang> [settings.json]` |
| `src/bin/page_dump.rs` | anywhere | Writes the board page out as it is served. What `shoot.mjs` opens | `cargo run --bin page_dump -- <lang> [light]` |
| `tools/debug/sftp-server.wsl.sh` | WSL | A real OpenSSH server on `127.0.0.1:2222`, with nothing installed. The file commands have been wrong in a way only a real server shows; `sshd_probe` is this project's own code and can agree with a mistake | `bash tools/debug/sftp-server.wsl.sh`, then the `live_sftp` tests below |
| `tools/debug/sftp-panel.win.mjs` | Windows, with WSL | The file panel through the running app's own window: this checkout's build started in a folder of its own, one panel pointed at the server above, and its buttons pressed — compare, send, replace, the delete question, a folder out and back, and a folder with automation stopped. Every step is checked against the files on both machines. The tests below this row could not see three faults this found | `cargo build`, then `node tools/debug/sftp-panel.win.mjs` |
| `tools/debug/ideas.win.mjs` | Windows | The ideas through the running app's own window: this checkout's build started in a folder of its own with two git repositories (one with a worktree somewhere else entirely) and a folder that is no repository, and the ideas used as a person does — Ctrl+Shift+M from the terminal and from a text box, and a key moved in the settings, the bulb, typing, Enter and Shift+Enter, an edit, ticking one off, carrying one by its grip, its right-click menu (done keeps a card, delete takes it out of the file), sending one to an Issue (and done with its number once the issue is made), a card with no project, opening from the distant worktree, and a repository taken out of the settings. Every step is checked against `config/ideas.json` itself | `cargo build`, then `node tools/debug/ideas.win.mjs` |
| `tools/debug/carry-lines.win.mjs` | Windows | Choosing what a worktree brings along by .gitignore line, through the running app's own dialog: this checkout's build started in a folder of its own with a repository whose one line matches many folders, a row changed by hand, the line chosen on the second tab, applied on the list. Checks that every row of the line changed and the hand-changed row did not, that the choice is in `config/config.json`, and that the dialog opened again starts from it | `cargo build`, then `node tools/debug/carry-lines.win.mjs` |
| `tools/debug/git-in-terminal.win.ps1` | Windows | Whether a git typed in a terminal tab signs in as the account its project chose: this checkout's build started in a folder of its own, with one git account, a project that chose it, and a tab whose command asks git itself who it would sign in as. Checks the account's token and name reach that terminal, that git would use them for that server, and that another server is left to this machine's own sign-in. `-NoAccount` is the other half: with nobody's account chosen, the terminal is untouched. The token is a made-up string that reaches no server | `cargo build`, then `tools\debug\git-in-terminal.win.ps1 [-NoAccount] [-Keep]` |
| `tools/debug/worktree-naming.win.mjs` | Windows | What a typed name becomes, through the running app's own dialog: this checkout's build started in a folder of its own with a repository to cut branches from, and a name typed into the box three ways -- already in the letters a branch and a folder can both hold, partly in them, and not at all. Checks what the line under the box says each time, that a drawn name does not change with the next keystroke, and -- after the button -- the branch git really made, the folder really on the disk, and the name really in `config/config.json` | `cargo build`, then `node tools/debug/worktree-naming.win.mjs` |
| `tools/debug/folder-naming.win.mjs` | Windows | Whether a folder names itself from what was typed straight into its AI, with nothing installed anywhere: this checkout's build started in a folder of its own, its AI a stand-in that writes down the command line it was given and answers the one-shot call this app makes. Reads the conversation id the app minted out of that line, writes a record under it the way Claude Code writes one (a person's words between two tool answers), and checks that the name and summary land in `config/config.json` while the CLI's own settings file is never touched | `cargo build`, then `node tools/debug/folder-naming.win.mjs` |
| `tools/debug/folder-view.win.mjs` | Windows | Pressing a folder's name shows that folder on its own: this checkout's build started in a folder of its own with two folders, the screen split on the first, the second pressed. Checks that the second is not put into one side of the first's split, and that pressing the first again brings its split back | `cargo build`, then `node tools/debug/folder-view.win.mjs` |
| `tools/debug/keep-running.win.mjs` | Windows, with `claude` signed in | An AI at work is left alone by what is done around it: this checkout's build started in a folder of its own, a real Claude Code given a job of over a minute in one worktree, and meanwhile another worktree of the project deleted, the desk switched, and the desk on screen deleted while the AI's desk is renamed. Checked against Claude Code's own conversation files: the job finishes in the same conversation and no second one starts. `--only` does one of the three alone, to find which one does it. Spends one short turn of the account | `cargo build`, then `node tools/debug/keep-running.win.mjs [--only=worktree\|switch\|desks]` |
| `tools/debug/past-back.win.mjs` | Windows | A tab that came up on a conversation of nobody's offers the way back: this checkout's build started with a folder that has two conversations behind it and nothing remembered about the tab, then the offer in the pane's caption pressed, the list read, and one picked. Checks that the tab is relaunched resuming that conversation and that the offer goes. A stub CLI stands in for the real one, so no account is used and nothing leaves the machine | `cargo build`, then `node tools/debug/past-back.win.mjs` |
| `tools/debug/shot-window.win.ps1` | Windows | Photographs one copy of the app's window, picked by the folder it runs from — so a copy somebody is using, sitting on top, is not what gets taken | `tools/debug/shot-window.win.ps1 -Under <folder> -Out <file.png>` |
| `live_sftp` tests in `crates/core/src/hooks.rs` | anywhere | The file commands, Lua, the panel's folder template and Compare, all through the code the app runs, against the server above. Ignored unless `SHIKISHA_LIVE_SFTP` names a server | See the top of `sftp-server.wsl.sh` |
| `tools/sandbox.ps1` | Windows | The package on a Windows that has never seen this project (Windows Sandbox): a new install, the Store package, or an upgrade over the version before. It stays in `tools/`, where it was written, so that what already points at it still finds it | `tools/sandbox.ps1 [-App <folder> \| -Msix <path> [-From <old>]]` |

## Pointing a client at a copy

`instance.win.ps1` writes the settings itself, because the door's name carries
the copy's process id and that is new at every start. By default they land at
`<the copy's folder>\mcp.json`; `-Mcp` puts them where the client will look --
an agent's own worktree, as `.mcp.json`, is the usual answer. The file is
merged, so other servers already in it stay, and a file that is not JSON is
refused rather than replaced.

    {"mcpServers": {"shikisha": {"command": "<exe>", "args": ["--mcp", "--pid", "1234",
                                                             "--token-file", "<root>\\data\\api-token"]}}}

Claude Code reads `.mcp.json` from the folder it is started in, or takes the
file outright with `--mcp-config <path>`. Gemini's CLI keeps the same shape
under `mcpServers` in its own settings. Codex wants TOML, so the `mcp=` line
this prints is the one to translate by hand.

`-Name` changes what the server is called, for registering two copies with one
client.
