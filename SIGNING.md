# Code Signing Policy

There are two ways to get SHIKISHA-TERM, and they are signed differently.

| | Signed by | First run |
| --- | --- | --- |
| **Microsoft Store** | Microsoft, with their own certificate | No warning |
| **The zip on [Releases](https://github.com/styleio/ShikishaTerm/releases)** | Nobody — it is unsigned | SmartScreen warns; see [the README](README.md#about-the-windows-warning) |
| **The Linux build** (tarball, `.deb`, `.rpm`) | This project's own Ed25519 key | Checked by `install.sh` before anything is placed |

A package submitted to the Microsoft Store is signed by Microsoft as part of
publishing it, so the Store copy carries a real, verifiable signature that this
project does not have to buy or hold. Free certificates were applied for
elsewhere first — [SignPath Foundation](https://signpath.org/) — and that
application was not accepted.

Paid certificates were not the answer either, and not because of the price
itself. Nothing this project runs on is paid for, apart from its domain name:
the Store developer account is free, the builds run on a public repository's
GitHub Actions, the site is hosted for nothing, and there is no server of ours
to keep alive because there is no server of ours at all. A code-signing
subscription — roughly ten dollars a month — would be the first recurring bill
of any size, and a bill has to be met. Meeting it eventually means taking it
from the people using the program, which is the one thing this is built not to
do. So the Store route signs the build for nothing, which covers most people who
install it, and the zip is what is left over: the free certificates for open
source are given on reputation rather than on payment.

The portable zip stays unsigned in Windows' sense: it carries no code-signing
certificate, so SmartScreen has nothing to check. It is the same program, built
by the same workflow from the same tagged commit; what it lacks is a certificate,
not provenance. Check the download against the SHA256 published next to it rather
than trusting the absence of a warning.

It does carry one signature of its own. `SHIKISHA-TERM.zip.sig` is an Ed25519
signature over the zip, made in the release workflow with a key that exists only
in the repository's secrets; the public half is compiled into the program
(`crates/core/src/update.rs`). It is what the program's own updater checks before
it puts a downloaded version in place — a zip whose signature the key does not
accept is refused, whatever its SHA256 says. Windows does not read it, and it is
not a substitute for a certificate.

On Linux that same signature is the whole of it. There is no SmartScreen to
warn anybody and no certificate to buy, so every Linux artifact — the tarball,
the `.deb` and the `.rpm` — carries a `.sig` made with the same key, and
`packaging/linux/install.sh` checks both the SHA256 and that signature before it
puts anything in place. Either check failing means nothing is installed. The key
it checks against is written into the installer as a PEM, and a test in the
repository compares it with the one compiled into the program, so the two cannot
drift apart.

## Roles

- **Authors** — write and change the source code.
  Currently: [@styleio](https://github.com/styleio).
- **Reviewers** — review and approve pull requests before they are merged.
  Currently: [@styleio](https://github.com/styleio).
- **Approvers** — authorize each release. Currently: [@styleio](https://github.com/styleio).

All maintainers use multi-factor authentication on their accounts.

## How releases are built

Every release — both the zip and the Store package — is built from this
repository's source by [GitHub Actions](.github/workflows/release.yml), from a
tagged commit, never on a developer's machine. A SHA256 is published next to the
zip. The product name and version are set in the executable's metadata from
`Cargo.toml`, so a build cannot claim to be a version it is not.

The Store package contains the same binary as the zip. What differs is where the
installed copy keeps your things: an installed package runs from a read-only
folder, so its settings, data and logs live under `%LOCALAPPDATA%\SHIKISHA-TERM`
instead of beside the program.

## Privacy

This program will not transfer any information to other networked systems unless
specifically requested by the user or the person installing or operating it.
Network access happens only for things you set up yourself — the AI CLIs you run,
the optional phone remote, and the notifications you configure. The full statement
is the [privacy policy](docs/PRIVACY.md).
