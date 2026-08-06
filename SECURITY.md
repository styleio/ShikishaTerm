# Security

SHIKISHA-TERM starts programs on your machine, and — if you turn it on — lets a phone
send instructions to them. That is the whole point of the tool, so it is worth being
precise about what is and is not exposed.

## Reporting a vulnerability

Please **do not open a public issue** for a security problem. Use GitHub's
[private vulnerability reporting](https://github.com/styleio/ShikishaTerm/security/advisories/new)
instead. A first reply should come within about a week.

Include what you did, what happened, and what you expected. A proof of concept helps but
is not required.

## Threat model

### What the tool assumes

- The machine it runs on is trusted, and so is the person at the keyboard
- The AI CLIs it launches are the ones you already trust enough to install
- **The output of an AI is not trusted input.** Automations decide what to send; the AI
  cannot make the app do something an automation did not ask for

### What is protected, and how

**Automation sandbox.** Lua automations run with no file access and no network access by
default. `io` and `os` are removed from the interpreter. They can only reach the outside
world through capabilities you register in `config.json` by name — a script cannot build a
path or a URL of its own. Auth tokens are never visible to a script; the app attaches them.
`config.json`, `secrets.json`, `.env` and `.lua` files are never readable or writable from
an automation, even inside an allowed folder. Every file and network operation is logged to
`logs/hooks.log`.

**Runaway protection.** Automatic hand-offs between agents carry a depth counter and stop
at a configurable limit (10 by default). Anything you type resets it to zero. Nothing is
sent automatically within 5 seconds of you touching a tab. `Ctrl+B x` stops all automation
immediately.

**Secrets at rest.** `secrets.json` can be encrypted with a master password
(Argon2id → AES-256-GCM), written atomically. Running without a password is allowed and
is your call; the file is then plain text on disk.

**Settings screen.** Bound to `127.0.0.1` on a random port, with a random one-time token
required on every request. It is not reachable from other machines.

**Phone access (off by default).** When enabled:

- The bind address is resolved to a **private network only** — Tailscale (100.64.0.0/10)
  or a LAN address. A public address is refused unless `remote.allow_public` is set by
  hand in the config file
- Every request needs a token, compared in constant time
- The QR code and URL contain that token, so treat them like a password

**What phone access does *not* protect against.** On a plain LAN, anyone who obtains the
URL and token can send instructions to your AI sessions, and those sessions can run
commands. This is why Tailscale is the recommended setup: with it, only devices on your own
tailnet can reach the port at all. Do not enable phone access on shared or public Wi-Fi.

### Out of scope

- A compromised machine, or a malicious user at the keyboard
- Malicious behaviour by the AI CLIs themselves
- Anything reachable because you explicitly set `remote.allow_public`

## Things to keep in mind

- **Never commit `secrets.json`, `config.json` or `.env`** — they are gitignored for a
  reason. Workspace files under `workspaces/` are meant to be shared; keep credentials out
  of them
- **Session logs record what the AI printed**, including anything it read. Treat
  `logs/` as sensitive
- **Automation scripts are code.** Review one before running it, exactly as you would a
  script someone sent you
