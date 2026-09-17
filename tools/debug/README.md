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

## Confirming a change

| Tool | Runs on | What it is for | How to run it |
|---|---|---|---|
| `tools/debug/shoot.mjs` | anywhere, with Chrome | Photographs the board page in both languages, both colour schemes and at window and phone width, into `target/shots`. This is how section 9 of [the screen rules](../../docs/design/STYLEGUIDE.md) is marked: nothing has to be running, so the screen being judged is never the one on the way to it | `node tools/debug/shoot.mjs tools/debug/scenes/files.mjs [--only <scene>]` |
| `tools/debug/scenes/` | — | Which screens `shoot.mjs` puts up, a line each. How to write one is at the top of `files.mjs` | — |
| `src/bin/page_dump.rs` | anywhere | Writes the board page out as it is served. What `shoot.mjs` opens | `cargo run --bin page_dump -- <lang> [light]` |
| `tools/debug/sftp-server.wsl.sh` | WSL | A real OpenSSH server on `127.0.0.1:2222`, with nothing installed. The file commands have been wrong in a way only a real server shows; `sshd_probe` is this project's own code and can agree with a mistake | `bash tools/debug/sftp-server.wsl.sh`, then the `live_sftp` tests below |
| `tools/debug/sftp-panel.win.mjs` | Windows, with WSL | The file panel through the running app's own window: this checkout's build started in a folder of its own, one panel pointed at the server above, and its buttons pressed — compare, send, replace, the delete question, a folder out and back, and a folder with automation stopped. Every step is checked against the files on both machines. The tests below this row could not see three faults this found | `cargo build`, then `node tools/debug/sftp-panel.win.mjs` |
| `tools/debug/ideas.win.mjs` | Windows | The ideas through the running app's own window: this checkout's build started in a folder of its own with two git repositories (one with a worktree somewhere else entirely) and a folder that is no repository, and the ideas used as a person does — Ctrl+B m, the bulb, typing, Enter and Shift+Enter, an edit, ticking one off, carrying one by its grip, its right-click menu (done keeps a card, delete takes it out of the file), sending one to an Issue (and done with its number once the issue is made), a card with no project, opening from the distant worktree, and a repository taken out of the settings. Every step is checked against `config/ideas.json` itself | `cargo build`, then `node tools/debug/ideas.win.mjs` |
| `tools/debug/shot-window.win.ps1` | Windows | Photographs one copy of the app's window, picked by the folder it runs from — so a copy somebody is using, sitting on top, is not what gets taken | `tools/debug/shot-window.win.ps1 -Under <folder> -Out <file.png>` |
| `live_sftp` tests in `crates/core/src/hooks.rs` | anywhere | The file commands, Lua, the panel's folder template and Compare, all through the code the app runs, against the server above. Ignored unless `SHIKISHA_LIVE_SFTP` names a server | See the top of `sftp-server.wsl.sh` |
| `tools/sandbox.ps1` | Windows | The package on a Windows that has never seen this project (Windows Sandbox): a new install, the Store package, or an upgrade over the version before. It stays in `tools/`, where it was written, so that what already points at it still finds it | `tools/sandbox.ps1 [-App <folder> \| -Msix <path> [-From <old>]]` |
