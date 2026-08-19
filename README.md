<p align="center">
  <img src="https://raw.githubusercontent.com/styleio/ShikishaTerm/main/assets/banner.png" alt="SHIKISHA-TERM — Run Claude Code, Codex and Gemini side by side" width="820">
</p>

<p align="center">
  <b>Run Claude Code, Codex and Gemini side by side — and let them hand work to each other.</b><br>
  A single portable <code>.exe</code> for Windows. No install, no admin rights, no API keys.
</p>

<p align="center">
  <sub><b>Works with</b> Claude Code · Codex · Gemini · Aider · Ollama · DeepSeek · Qwen — or any shell over SSH</sub>
</p>

<p align="center">
  <a href="https://github.com/styleio/ShikishaTerm/stargazers"><img alt="Stars" src="https://img.shields.io/github/stars/styleio/ShikishaTerm?style=social"></a>
  <a href="LICENSE"><img alt="MIT" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <a href="https://github.com/styleio/ShikishaTerm/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/styleio/ShikishaTerm?include_prereleases"></a>
  <a href="https://github.com/styleio/ShikishaTerm/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/styleio/ShikishaTerm/actions/workflows/ci.yml/badge.svg"></a>
  <a href="README.ja.md"><img alt="日本語" src="https://img.shields.io/badge/README-日本語-red.svg"></a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/styleio/ShikishaTerm/main/assets/demo.gif" alt="SHIKISHA-TERM in action — several AIs side by side, an AI-vs-AI debate, and a downloadable result" width="820">
</p>

<p align="center">
  <sub>⭐ If this is useful, a <b>star</b> helps other people find it.</sub>
</p>

---

> **Check on your AI from your phone — and tell it to carry on.**
> Scan a QR code and you can see what every tab is doing and send instructions,
> from the train, from a café, from bed.

## Why

Running one terminal AI is easy. Running **four** is not.

You end up with a window per agent, no idea which one is waiting for you, and a lot of
copy-pasting between them. SHIKISHA-TERM puts them in one window and knows the difference
between *working*, *finished* and *waiting for a human* — and can move work between agents
on its own.

It talks to whatever runs in a terminal, so it is not tied to one vendor: Claude Code,
Codex CLI, Gemini CLI, DeepSeek, Ollama, Aider, or a plain shell over SSH.

**Most AI terminals run a single agent. This one runs several — and moves work between them.**

For example:

- A **code → review loop**: one agent writes, another reviews, back and forth until it passes — then a Slack ping.
- An **AI-vs-AI debate** you can watch play out, with the verdict saved to a file.
- Kick off a long job and **check on it from your phone** on the way home — and tell it to carry on.

## Install

1. Download `SHIKISHA-TERM.zip` from [Releases](https://github.com/styleio/ShikishaTerm/releases/latest)
2. Unzip it anywhere — a USB stick and a synced folder both work
3. Run `SHIKISHA-TERM.exe`

It opens a window of its own. Nothing needs to be running it — no terminal to set up, no
font or colour scheme to get right first.

Nothing is written outside that folder. There is no installer and no runtime to install.

> It uses **your existing AI subscriptions** through their CLIs. No API keys are stored,
> and none are needed.

### About the Windows warning

Release binaries are being set up for **code signing with a free certificate from
[SignPath Foundation](https://signpath.org/)** — see the [code signing policy](SIGNING.md).
Until signed builds ship, the first run shows **"Windows protected your PC"** because the
executable is not yet code-signed.

Click **More info → Run anyway** to continue. If you would rather check first — and being
wary of an unsigned executable from the internet is the right instinct:

- Every release is built by [GitHub Actions](.github/workflows/release.yml) from this
  source, not on anyone's laptop. The build log for your download is public
- A `SHIKISHA-TERM.zip.sha256` is published next to the zip:

  ```powershell
  Get-FileHash SHIKISHA-TERM.zip -Algorithm SHA256
  ```

## Quick start

On the first run the screen tells you to press `[e]`, which opens the settings screen
inside the same window. There you pick which AI runs in which folder — no JSON editing
required.

<p align="center">
  <img src="https://raw.githubusercontent.com/styleio/ShikishaTerm/main/assets/settings.png" alt="SHIKISHA-TERM settings — configure each tab (name, command or SSH host, working folder, automation) in a form, with no JSON editing" width="880">
</p>

To open just the settings (also the way back in if a broken config stops the app from
starting), double-click **`Settings.cmd`**.

## What it does

<p align="center">
  <img src="https://raw.githubusercontent.com/styleio/ShikishaTerm/main/assets/desktop.png" alt="SHIKISHA-TERM desktop — a mission-control dashboard of several AI agents, each showing whether it is working, waiting or done" width="880">
</p>

- **Tabs with real status** — every tab shows whether it is working, done, or waiting for
  you, detected from the screen itself rather than from any vendor API
- **Workspaces** — swap the whole tab layout per project, like virtual desktops. Export one
  to a single file, automation scripts included, and hand it to another machine or person
- **Automation** — "when this finishes, hand the result to the review tab", "answer this
  confirmation automatically". Written in Lua, or described in plain language and written
  for you by an AI you already have installed
- **Runaway protection** — a limit on how many times agents may hand work to each other,
  an emergency stop, and per-tab input locks
- **Notifications** — Slack / Telegram when a job finishes
- **Phone access** — check status and send instructions from outside (see below)
- **A real terminal** — SSH, Docker, WSL, jump hosts, key files, port forwarding, session
  logs, legacy encodings, IME input and mouse support

## Use it from your phone

<p align="center">
  <img src="https://raw.githubusercontent.com/styleio/ShikishaTerm/main/assets/phone.png" alt="SHIKISHA-TERM on a phone — a live dashboard of several AI agents, each showing whether it is working, waiting or done" width="340">
</p>

Turn on "Use from your phone" in the settings and a QR code appears. Scan it and you get a
small web page listing every tab, its status, and a box to send instructions.

**Know where it is reachable from** — this feature lets a phone run commands on your machine:

- Only people **on the same network** can connect. It is never published to the internet
- With **[Tailscale](https://tailscale.com/)** (free) only your own devices can reach it,
  encrypted, from anywhere. This is the safest way and the one to prefer
- Without Tailscale it works on your **home LAN only**. Anyone on the same Wi-Fi who knows
  the URL and the token could use it — do not turn it on for shared or public Wi-Fi
- Binding to a public address (`remote.allow_public`) never happens unless you write it in
  the config file yourself

See [SECURITY.md](SECURITY.md) for the full threat model.

## Keys

The prefix is `Ctrl+B`, tmux-style. `Ctrl+B ?` opens the help.

| Key | Action |
|---|---|
| `Ctrl+B q` | Quit |
| `Ctrl+B 0`–`9` | Switch tab (0 = INDEX) |
| `Ctrl+B w` / `W` | Workspace list / next |
| `Ctrl+B l` | Toggle input lock |
| `Ctrl+B r` | Restart the tab |
| `Ctrl+B [` | Copy mode (`c` copies the latest response) |
| `Ctrl+B a` / `x` | Automation on/off / emergency stop |
| `Ctrl+B 0` → `i` | Show the QR code for phone access |
| `Ctrl+B 0` → `k` | Set or change the master password (encrypts `secrets.json`) |

The mouse works too: wheel to scroll, drag to copy, right click to paste, click a tab name
to switch.

## Automation

An automation is a few lines of Lua dropped into a folder, named after the moment it should
run:

```lua
-- on_done.lua — hand the finished work to the review tab, and stop after 5 rounds
if tab.chain_depth == 0 then return end          -- a human started this; do nothing

local rounds = shikisha.get_var("rounds") or 0
if tab.output:match("LGTM") or rounds >= 5 then
  shikisha.notify("slack", "Review finished (" .. rounds .. " rounds)")
  return
end
shikisha.set_var("rounds", rounds + 1)
shikisha.send_to_tab("reviewer", "Please fix these points:\n" .. tab.output)
```

Automations run in a sandbox: **no file access and no network by default**, and
notifications can only go to targets you registered.

Full reference: **[docs/AUTOMATION.md](docs/AUTOMATION.md)** — also reachable from the
settings screen.

## Folder layout

```
SHIKISHA-TERM.exe   the app
Settings.cmd           opens only the settings screen
config.json            general settings + the list of workspaces
secrets.json           notification targets and tokens (encryptable, never share)
workspaces/            workspace definitions (shareable per project)
profiles/              per-AI status detection rules
scripts/               automation scripts
logs/                  session and automation logs
lang/                  interface languages (en.json is the base)
docs/AUTOMATION.md     how to write automations
```

Copy `config.example.json` and the other `*.example.json` files to get started.

## Language

The interface follows your Windows language and falls back to English. You can also set it
in the settings screen or with `"language"` in `config.json`.

| Language | Status |
|---|---|
| English | ✅ base |
| 日本語 | ✅ complete |
| *your language* | [contributions welcome](docs/TRANSLATING.md) |

Copy `lang/en.json` to `lang/<code>.json` and translate the values — that is the whole
process. Keys you leave out fall back to English, so a partial translation still works.
See [docs/TRANSLATING.md](docs/TRANSLATING.md).

## Where it fits

It is deliberately narrow, and honest about it:

- **Windows only** today. It is built on ConPTY; other platforms are not supported yet —
  [★ star the repo](https://github.com/styleio/ShikishaTerm) to hear if that changes
- **It drives CLIs, it does not replace them.** Your AI subscriptions, logins and settings
  stay exactly as they are
- **It is a terminal, not an IDE.** It manages sessions and moves text between them

## Contributing

Translations, profiles for AI CLIs it does not know yet, bug reports — all welcome.
Start with [CONTRIBUTING.md](CONTRIBUTING.md), and please keep to the
[Code of Conduct](CODE_OF_CONDUCT.md). Questions or ideas? Open a
[Discussion](https://github.com/styleio/ShikishaTerm/discussions).

## Building

```
cargo build --release
```

Rust (MSVC toolchain) is required. The result is a single executable with no runtime
dependencies.

## Documentation

- [Automation reference](docs/AUTOMATION.md) — events, variables, commands, examples
- [Translating](docs/TRANSLATING.md) — how to add a language
- [Design notes](DESIGN.md) — terminology, architecture, and the safety model ([日本語](DESIGN.ja.md))
- [Security](SECURITY.md) — threat model and how to report an issue
- [Code signing policy](SIGNING.md) — how release binaries are built and signed

## Support

If SHIKISHA-TERM saves you time, you can support its development on
[Ko-fi](https://ko-fi.com/styleio). Thank you.

## License

[MIT](LICENSE)
