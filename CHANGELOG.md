# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once it reaches its first tagged release.

## [Unreleased]

## [0.1.2] - 2026-08-21

### Added
- **The composer** — the sub-input bar is no longer mobile-only. Summon it on
  the desktop window, attach a file by paste or drag (saved beside the tab, and
  handed over as a path the AI can read), and fire **quick actions** that insert
  canned text or run Lua. A starter set ships localized, and the actions are
  edited in Settings rather than by hand.
- **📼 Record a browser page as Lua** and play it back: clicks are anchored to
  their text, throwaway ids are refused, and the recording runs in the
  background so the input box is never taken hostage.
- **Drive another tab (`operate`)** — aim the active AI at a target tab ad hoc,
  wait for the target to settle, stage its state to a file so the first round
  trip isn't wasted, and hold a brake on runaway chains. What happens when the
  limit is hit is now the user's choice.
- **Browser tabs** get the composer instead of a bespoke bar; Send injects into
  the shown browser, shared with the phone. The address bar searches Google when
  the text isn't a URL.
- **Phone**: open Settings natively over a reverse proxy, fill a portrait
  screen, and pinch to zoom the relay.
- **Settings**: flat card-unit navigation with a shortcut straight into the
  quick actions, and a collapsible General group.
- Docs and site: a demo video and product screenshots on the landing page, and
  `DESIGN.md` is now English-first (Japanese as `DESIGN.ja.md`).

### Changed
- The remote access token is kept out of the phone's URL, off the QR screen, and
  out of the phone card in Settings.

### Fixed
- Remote access now actually releases its port on shutdown, so turning it off
  and back on works.
- `operate` addresses a target browser by its key rather than its display name.
- The composer keeps its panel dropdown open across state pushes, and rebuilds
  the panel on a tab switch so the switcher comes back.
- Settings opens onto the current workspace instead of auto-expanding
  workspace 1.
- A quick action whose Lua won't parse is refused at save time.

## [0.1.1] - 2026-08-19

### Added
- Phone access got much better: remote state is pushed over a WebSocket instead
  of polled, history can be paged back through, the phone sizes the terminal to
  its own screen, and typing goes through the sub-input bar.
- The window can honestly disconnect a remote session (the token is rotated on
  the spot), and the phone shows an overlay when the feed stops.
- Notifications: a Slack/Telegram editor in General settings, plus a per-tab
  "notify on answer" toggle that needs no Lua.
- A "Show flags" button that runs the selected AI CLI's `--help`.

### Fixed
- The terminal reflows on any resize, not just window resizes, and its width is
  no longer shrunk twice by the tab bar.
- Model tabs no longer send an empty key (401) when secrets are encrypted.
- The ~8s black screen after the master password is replaced by a splash.
- File capabilities are confined by the real (symlink-resolved) path, and the
  local HTTP servers are hardened (body limits, privacy headers).

## [0.1.0] - 2026-08-18

The first public release. It is pre-1.0 and evolving quickly. Highlights:

### Added
- Run multiple AI CLIs side by side in one window (Claude Code, Codex CLI,
  Gemini CLI, Aider), plus model-API bridges (DeepSeek, Ollama/Qwen, …).
- Per-tab status read from the screen — working / done / waiting — so it works
  with any CLI rather than one vendor's API.
- Workspaces: swap the whole tab layout per project; export one to a single file.
- Automation in Lua (or described in plain language and written by an AI), with a
  sandbox that has no file or network access by default, plus runaway limits, an
  emergency stop, and per-tab input locks.
- AI-vs-AI discussion and code review: several agents debate or review, a judge
  rules, and the whole exchange is rendered as a chat-style **Result** tab with a
  one-click Markdown download.
- Phone access over the local network (QR pairing), safest over Tailscale.
- Notifications to Slack / Telegram when a job finishes.
- A real terminal underneath: SSH, Docker, WSL, jump hosts, key files, port
  forwarding, session logs, legacy encodings, IME input, and the mouse.
- Interface localization (English base, Japanese complete; more welcome).

[Unreleased]: https://github.com/styleio/ShikishaTerm/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/styleio/ShikishaTerm/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/styleio/ShikishaTerm/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/styleio/ShikishaTerm/releases/tag/v0.1.0
