# Code Signing Policy

SHIKISHA-TERM has applied for free code signing from
[SignPath Foundation](https://signpath.org/), with a certificate from
[SignPath.io](https://signpath.io/). The application is pending.

Until it is granted, Windows release binaries are **not** signed, and the first
run shows a SmartScreen warning — see [the README](README.md#about-the-windows-warning)
for what to check instead. This document is the policy those signed releases will
follow once signing is in place.

## Roles

- **Authors** — write and change the source code.
  Currently: [@styleio](https://github.com/styleio).
- **Reviewers** — review and approve pull requests before they are merged.
  Currently: [@styleio](https://github.com/styleio).
- **Approvers** — authorize each release for signing.
  Currently: [@styleio](https://github.com/styleio).

All maintainers use multi-factor authentication on their accounts.

## How releases are built and signed

Every release is already built from this repository's source by
[GitHub Actions](.github/workflows/release.yml) — never on a developer's machine —
and a SHA256 is published next to the zip.

Once signing is in place:

- Only artifacts produced by that workflow, from a tagged commit, are submitted
  for signing.
- Each signing request is reviewed and approved manually by an Approver before
  the artifact is signed.
- The product name and version are set in the executable's metadata and enforced.

## Privacy

This program will not transfer any information to other networked systems unless
specifically requested by the user or the person installing or operating it.
Network access happens only for things you set up yourself — the AI CLIs you run,
the optional phone remote, and the notifications you configure.
