# Contributing to Rustmux

Choose the development track before opening an issue or pull request:

**Current contribution scope:** `main-human` only accepts PRs that fix bugs in
its existing code and issues reporting those bugs. Feature implementation PRs
and feature-related issues are not currently accepted on `main-human`; target
`main` and use `track:main` instead, including for features already available on
`main` but not yet implemented on `main-human`.

Use `track:main-human` for eligible bug reports and bug-fix PRs targeting
`main-human`. This scope restriction does not change the owner's ability to
implement features through personal direct commits.

- **`main` / `track:main`**: human-written and AI-generated contributions are
  welcome; review and issue acceptance may be performed by AI.
- **`main-human` / `track:main-human`**: code may also be AI-generated, but every
  change must be personally reviewed by the project owner before it enters the
  branch. Issue acceptance and closure require the owner's confirmation.

The PR target branch determines the policy. AI suggestions and passing tests do
not replace the owner's review for `main-human`. AI must not approve, merge, close,
or mark work accepted on that track on the owner's behalf without the required
personal review or acceptance. The owner may commit directly to `main-human`
without a PR or a separate PR review record. This exception applies to commits
personally made by the owner, not AI actions using the owner's Git identity.
Other contributors and AI must use a PR and obtain the owner's final-revision
review. Direct commits do not automatically establish feature acceptance.

Implementations on `main-human` may be reorganized or rewritten independently;
they do not have to reproduce the commit history of `main`. When a bug affects
both tracks, use linked, separate bug issues so fixes and acceptance can proceed
independently.

Read [Development Tracks and Human Review](docs/reference/development-tracks.md)
for the full policy, issue and PR workflow, and feature coverage record. See
[Development and Testing](docs/reference/development.md) for local checks.
