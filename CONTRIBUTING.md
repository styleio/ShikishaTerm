# Contributing

Thanks for looking. Issues and pull requests are both welcome, and you do not need to write
Rust to help.

## Good places to start

**Translate the interface.** Copy `lang/en.json` to `lang/<code>.json`, translate the
values, open a PR. Nothing has to be registered in code, and keys you leave out fall back
to English so a partial translation is genuinely useful. See
[docs/TRANSLATING.md](docs/TRANSLATING.md).

**Add a profile for an AI CLI.** `profiles/*.json` is how the app recognises whether a tool
is working, finished, or waiting for an answer. If your favourite CLI is not detected
correctly, a profile is a small JSON file — see `profiles/claude.json` for the shape.

**Report what broke.** Terminal emulation has endless edge cases. A bug report with the CLI
you ran, the terminal output, and what you expected is valuable even without a fix.

## Pull requests

- Keep one PR to one subject. Small is easy to review and gets merged fast
- `cargo test` must pass. `cargo build` must be warning-free
- Add a test when you fix a bug — a test that fails before your change and passes after is
  the most useful thing in the PR
- Match the surrounding style. Comments in this codebase explain **why**, not what

## Building

```
Dev.cmd                # build, stage into run\, and launch from there
Dev.cmd release        # same, from the release build
cargo test             # all offline, no PTY-less environment needed
```

Rust with the MSVC toolchain on Windows. There is nothing else to install — Lua is vendored
and built from source.

**Launch through `Dev.cmd` rather than running `target\debug\...` directly.** Config and
scripts live beside the executable, so the two build outputs each end up with their own
settings — an automation deleted in one is still live in the other, which is confusing to
diagnose. `Dev.cmd` keeps a single `run\` folder (gitignored) and refreshes only the
application files into it, so your config, scripts and secrets are never overwritten.

## Layout

| File | What lives there |
|---|---|
| `src/main.rs` | TUI, main loop, key handling, config hot reload |
| `src/tab.rs` | One terminal session (PTY, screen, restart) |
| `src/detect.rs` | Deciding whether a tab is working / done / asking |
| `src/hooks.rs` | The Lua automation engine |
| `src/caps.rs` | The sandbox: what an automation may touch |
| `src/webui.rs` | Settings screen (served locally) |
| `src/remote.rs` | Phone screen |
| `src/i18n.rs` | Language files |
| `src/crypto.rs` | Master password and secret encryption |

[DESIGN.md](DESIGN.md) has the architecture and the reasoning behind it (in Japanese).

## Security

Please do not file security problems as public issues — see [SECURITY.md](SECURITY.md).

## License

By contributing you agree that your contribution is licensed under the [MIT License](LICENSE).
