# Privacy Policy

SHIKISHA-TERM is a terminal that runs on your own computer. It has no account to
sign in to, no server of ours behind it, and no analytics or telemetry of any
kind. **We — the developers — never receive your data.** There is nothing for us
to receive: nothing in this program reports to us.

Last updated: 2 September 2026.

## What stays on your machine

Everything the program keeps is an ordinary file on your own disk, next to the
program (the portable download) or under
`%LOCALAPPDATA%\SHIKISHA-TERM` (the Microsoft Store copy):

- your settings, workspaces and automation scripts,
- what the terminals showed, and the transcripts of AI discussions you saved,
- logs of what the program itself did.

You can read, back up or delete any of it with Explorer. Uninstalling the Store
copy removes the program; the folder above is yours to delete when you want to.

API keys and webhook URLs you enter are stored **encrypted** (AES-GCM, with a key
derived by Argon2) in that same folder, and are only ever sent to the service each
one belongs to.

## When this program uses the network

Only where you set it up yourself. Nothing below happens until you configure it,
and each one goes straight to the party you chose — never through us.

| What you set up | Where it connects |
| --- | --- |
| The AI command-line tools you run in a tab (Claude Code, Codex, Gemini, Aider, …) | Those programs are separate products with their own accounts and their own privacy policies. SHIKISHA-TERM starts them and reads their screen; it does not see or store their credentials. |
| An **assistant model** (the AI that writes automation for you, or judges a discussion) | The provider you picked, with the API key you entered — for example Anthropic, OpenAI, Google, or a model running locally on your own machine. |
| The **phone remote** | Your own phone, over your own network. It is a small web server on your machine that you reach directly. Over [Tailscale](https://tailscale.com/) it is your own private network, end-to-end encrypted; no traffic passes through us. |
| **Notifications** | The Slack or Telegram webhook you supplied. |
| The **GitHub** pull-request panel | `api.github.com`, with the token you supplied. |
| **Automation you wrote** that calls `http.*` | Wherever your own script points it. |
| The **update check** (portable download only) | `api.github.com`, once at start-up, to read the latest published version number. It sends nothing but a request for that public page. The Microsoft Store copy does not do this — the Store updates it. |

## What we collect

Nothing. No usage statistics, no crash reports, no identifiers, no email address.
The program never contacts a server operated by us, because there is none.

## Children

SHIKISHA-TERM is a software-development tool. It is not directed at children and
collects no information from anyone, including children.

## Changes

If this policy ever changes, the new version is published at
<https://shikisha-term.com/privacy/>, and the date above changes with it.

## Contact

Questions about this policy: open an issue at
<https://github.com/styleio/ShikishaTerm/issues>. The Microsoft Store listing
also carries a support contact for the copy installed from there.
