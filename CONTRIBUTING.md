# Contributing to Rustmux

Choose the development track before opening an issue or pull request:

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
they do not have to reproduce the commit history of `main`. Use linked, separate
issues when the same feature needs work on both tracks.

Read [Development Tracks and Human Review](docs/reference/development-tracks.md)
for the full policy, issue and PR workflow, and feature coverage record. See
[Development and Testing](docs/reference/development.md) for local checks.
