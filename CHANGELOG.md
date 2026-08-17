# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once it reaches its first tagged release.

## [Unreleased]

The project is pre-1.0 and evolving quickly. Highlights of the current build:

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

[Unreleased]: https://github.com/styleio/ShikishaTerm/commits/main
