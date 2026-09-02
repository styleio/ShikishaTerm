# Code Signing Policy

There are two ways to get SHIKISHA-TERM, and they are signed differently.

| | Signed by | First run |
| --- | --- | --- |
| **Microsoft Store** | Microsoft, with their own certificate | No warning |
| **The zip on [Releases](https://github.com/styleio/ShikishaTerm/releases)** | Nobody — it is unsigned | SmartScreen warns; see [the README](README.md#about-the-windows-warning) |

A package submitted to the Microsoft Store is signed by Microsoft as part of
publishing it, so the Store copy carries a real, verifiable signature that this
project does not have to buy or hold. Free certificates were applied for
elsewhere first — [SignPath Foundation](https://signpath.org/) — and that
application was not accepted.

The portable zip stays unsigned. It is the same program, built by the same
workflow from the same tagged commit; what it lacks is a certificate, not
provenance. Until that changes, check the download against the SHA256 published
next to it rather than trusting the absence of a warning.

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
