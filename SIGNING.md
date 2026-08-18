# Code Signing Policy

SHIKISHA-TERM's Windows release binaries are code-signed. Free code signing is
generously provided by [SignPath Foundation](https://signpath.org/), with a
certificate from [SignPath.io](https://signpath.io/).

## Roles

- **Authors** — write and change the source code.
  Currently: [@styleio](https://github.com/styleio).
- **Reviewers** — review and approve pull requests before they are merged.
  Currently: [@styleio](https://github.com/styleio).
- **Approvers** — authorize each release for signing.
  Currently: [@styleio](https://github.com/styleio).

All maintainers use multi-factor authentication on their accounts.

## How releases are built and signed

- Every release is built from this repository's source by
  [GitHub Actions](.github/workflows/release.yml) — never on a developer's
  machine.
- Each signing request is reviewed and approved manually before the artifact is
  signed.
- The product name and version are set in the executable's metadata and enforced.

## Privacy

This program will not transfer any information to other networked systems unless
specifically requested by the user or the person installing or operating it.
Network access happens only for things you set up yourself — the AI CLIs you run,
the optional phone remote, and the notifications you configure.
