# Human Review Baseline and First Milestone

## Baseline and review status

Prepared on 2026-09-12. The implementation reference is `main` commit
`e7fe6a1219754c383eb4fc7e40d30f2232def43f` (`feat: add explicit session creation shortcuts`).
This is a fixed reference, not a claim that its implementation has been reviewed.
The checklist below is a **proposal awaiting the owner's agreement**. There are
no accepted features and no agreed coverage denominator yet. Later additions to
`main` belong in a separate backlog until the owner chooses a new baseline.

The existing checkout remains on `main`. A sibling worktree, `Rustmux-human`,
holds `codex/human-bootstrap`; its PR target will be `main-human`.
`main-human` starts at an empty root commit used only as a branch anchor. It
contains no application, CI, or documentation and represents no accepted work.
AI and other contributors submit content, including this AI-prepared skeleton,
through an owner-reviewed PR. The owner may personally commit directly without
a PR; Git author metadata alone does not qualify an AI submission for that exception.
The two tracks intentionally have independent histories; do not merge all of
`main` into `main-human`. Feature branches share ancestry with `main-human`.

## Proposed feature ledger

Each row is a candidate acceptance unit, not an implementation prescription.
Before agreeing on the denominator, split any row that cannot be independently
reviewed and verified in a reasonably small change. The reference column points
to documentation at the fixed baseline; consult that revision when behavior on
`main` changes. The verification column specifies planned checks, not results.

For every row, the issue, PR/final SHA, verification evidence, and owner acceptance
record are **not recorded** unless explicitly filled in below. Local preparation
is not an issue or PR. Update those fields as work proceeds, and preserve the
final reviewed SHA when linking to a discussion. Accepted counts follow the
[track policy](development-tracks.md#human-reviewed-feature-coverage).

| ID | Behavior / acceptance boundary | Planned verification | Baseline reference | Status |
| --- | --- | --- | --- | --- |
| H01 | Start an interactive shell on a PTY; report startup failure and reclaim resources | PTY integration: valid/invalid shell, child exit and descriptor cleanup | `src/shell.rs`, `src/terminal.rs` | not started |
| H02 | Forward ordinary input, UTF-8, control keys and bracketed paste without corruption | Byte fixtures plus interactive shell and Ctrl-C | `src/input.rs` | not started |
| H03 | Render text, cursor, styles, wide characters and alternate screen | Terminal fixtures plus a full-screen editor | `src/terminal.rs`, `src/render.rs` | not started |
| H04 | Propagate resize and restore the outer terminal on exit or recoverable failure | Repeated resize, shell exit, injected I/O failure; compare terminal settings | `src/app.rs` | not started |
| H05 | Create, switch, rename and close windows with the documented focus behavior | Interactive workflow and model assertions | `docs/guide/windows-panes.md` | not started |
| H06 | Split, close and zoom panes; reject impossible layouts without mutation | Layout tests and interactive splits | `docs/guide/windows-panes.md` | not started |
| H07 | Move focus directionally, resize and swap panes | Uneven-layout tests and keyboard/mouse walkthrough | `docs/guide/windows-panes.md` | not started |
| H08 | Move panes between windows without restarting their processes | PID/content preservation and invalid-destination tests | `docs/guide/automation.md` | not started |
| H09 | Open and hide a floating terminal while preserving its process and directory | Interactive lifecycle checks | `docs/getting-started/concepts.md` | not started |
| H10 | Inherit working directories using OSC 7 and documented process fallbacks | Shell, Yazi and missing-directory cases | `docs/guide/windows-panes.md` | not started |
| H11 | Apply modes, configurable keybindings, help and clickable hints | Input dispatch tests and actual hint interaction | `docs/guide/keybindings.md` | not started |
| H12 | Validate configuration and hot reload supported options and themes | Valid/invalid TOML and live reload checks | `docs/configuration/index.md`, `docs/configuration/themes.md` | not started |
| H13 | Create, attach and detach named sessions; keep detached processes alive | Session lifecycle tests | `docs/guide/sessions.md` | not started |
| H14 | Reject nested sessions and a second interactive client without disturbing the first | Concurrent clients and nested invocation | `docs/guide/sessions.md` | not started |
| H15 | List and stop sessions with documented aliases, confirmation and snapshot behavior | CLI status/output fixtures and lifecycle tests | `docs/guide/sessions.md` | not started |
| H16 | Browse, search and explicitly create sessions using configurable manager keys | Browse/search/name-entry walkthrough, empty results and conflicts | `docs/guide/sessions.md` | not started |
| H17 | Rename, disconnect and delete sessions; preserve ordering and connection metadata | Manager actions plus restart and deletion checks | `docs/guide/sessions.md` | not started |
| H18 | Save and restore layouts manually and automatically with orderly writes | Round trips, default-off autosave, rename/delete during writes | `docs/guide/sessions.md` | not started |
| H19 | Optionally save plain or styled scrollback without executing saved output | Legacy/new snapshot fixtures, width changes and control-sequence filtering | `docs/guide/sessions.md` | not started |
| H20 | Browse and search bounded history | Limit, wrap and search navigation tests | `docs/guide/history.md` | not started |
| H21 | Select/copy history and edit history or previous command output | OSC 52 capture, editor fallback, OSC 133 and heuristic cases | `docs/guide/history.md` | not started |
| H22 | Control a session through bounded script requests | CLI integration, invalid IDs, input/capture limits | `docs/guide/automation.md` | not started |
| H23 | Validate and launch project layouts, including detached startup commands | Invalid layouts launch nothing; valid relative paths and restoration | `docs/guide/automation.md` | not started |
| H24 | Route extended keyboard, mouse and focus events with pane/screen isolation | Protocol fixtures and two-pane interactive applications | `docs/reference/terminal-compatibility.md` | not started |
| H25 | Preserve Kitty graphics and placeholders across redraws and session switches | Chunk fixtures, fuzzing and actual Yazi preview walkthrough | `docs/guide/kitty-yazi.md` | not started |
| H26 | Route drag-and-drop requests and translate coordinates | Protocol fixtures and actual GUI/pane transfers | `docs/guide/kitty-yazi.md` | not started |
| H27 | Route rich clipboard and file transfer responses to their originating pane | Concurrent request IDs and multi-pane integration | `docs/guide/kitty-yazi.md` | not started |
| H28 | Preserve hyperlinks, colors, cursor shape and per-pane protocol state | Focus/screen switches and terminal query fixtures | `docs/reference/terminal-compatibility.md` | not started |
| H29 | Deliver configured notifications and maintain bell/unread state | Filter/threshold tests and live focus/notification checks | `docs/configuration/notifications.md` | not started |

These 29 proposed rows are a planning inventory, **not an agreed total**. In
particular, configuration, session management and protocol rows may need further
splitting before the denominator is frozen. Performance checks accompany the
relevant feature; passing a benchmark alone does not establish acceptance.

## Milestone 1: a usable single-pane terminal

Work sequentially, with one coherent review at a time. The PR numbers below
are local sequence names, not GitHub PR numbers. No session daemon, persistence,
splits or Kitty extensions are needed for this milestone. The binary should stay
in its worktree's `target/` directory during review rather than replacing the
installed everyday Rustmux.

| Change | Scope and reading order | Acceptance / evidence required |
| --- | --- | --- |
| PR 0: bootstrap | README and review rules → Cargo manifest/lockfile → entry point → CI | Build, fmt and Clippy pass; binary honestly reports skeleton status; no terminal changes; owner explicitly reviews final SHA |
| PR 1: PTY lifecycle (H01) | Ownership and error-path design → shell spawn and PTY wrapper → integration tests | Shell runs with a controlling PTY; invalid shell reports failure; child and descriptors are reclaimed on exit/error |
| PR 2: input and display (H02–H03) | Data flow and screen model → input/output boundaries → rendering → tests | Shell input, UTF-8, Ctrl-C, paste, wide text and a full-screen editor work; split this PR if review scope is too large |
| PR 3: resize and cleanup (H04) | Terminal-state guard → resize propagation → event-loop shutdown → failure tests | Child observes new size; repeated resize works; normal/error exits restore echo, canonical mode, cursor and alternate screen |

PR 1 must handle its own resource cleanup from the start; PR 3 completes the
outer-terminal lifecycle rather than postponing cleanup of earlier code.
Dependencies are introduced only in the PR that needs them, with a short reason
and the boundary whose behavior we rely on. No application dependencies are
needed for PR 0. Unit/integration checks do not replace the real terminal
walkthrough required for the usable-terminal milestone.

## Per-change review record

For AI and other contributor submissions, copy this into the linked issue and
PR discussion as appropriate. Use `Refs` rather than automatic closing keywords
until the owner confirms issue acceptance. For the owner's direct commits, no PR
or separate PR review record is required: record the commit SHA, verification
and feature acceptance in the ledger or linked issue.

```text
Track: main-human
Feature IDs and linked issue:
Behavior and explicit exclusions:
Acceptance scenarios:
Implementation approach and differences from the reference main:
Reading order and ownership/error-path notes:
Verification commands, environment, results and limitations:
Final commit to review:
Owner review record: pending (owner supplies this personally)
Owner feature acceptance record: pending (separate from merge if necessary)
```

After a revision, list the changed files and rerun affected checks; obtain review
of the new final SHA. Authorship, an instruction to start implementation and
passing CI are not acceptance. Merge only after the owner's explicit review,
and close an issue only after the owner confirms its acceptance or disposition.

## Bootstrap verification record

Local PR 0 candidate prepared on 2026-09-12:

| Field | Record |
| --- | --- |
| Empty target anchor | `b59e6c1e73f6fd307bfe5cd92ca6ea8a6e5faf61` |
| Candidate branch | `codex/human-bootstrap` |
| Final candidate commit | `9eec31aa22b0995a8a4fdac745482fa3d2a2fab0` |
| Diff | 8 new files, 137 lines; no inherited application code |
| Local environment | macOS; rustc 1.97.1, cargo 1.97.1 |
| Formatting | `cargo fmt --all --check` passed |
| Build | `cargo build --locked --offline` passed |
| Clippy | `cargo clippy --all-targets --all-features --locked --offline -- -D warnings` passed |
| Runtime smoke check | Exact diagnostic on stderr, empty stdout, exit status 1 verified |
| Git whitespace check | Staged diff passed `git diff --cached --check` |
| Linux / hosted CI | Not run locally; workflow prepared for publication |
| GitHub issue / PR | Not created |
| Owner review / acceptance | Pending; no merge or accepted feature recorded |

To read the entire first review locally from either worktree:

```sh
git diff main-human..codex/human-bootstrap
git show --stat 9eec31aa22b0995a8a4fdac745482fa3d2a2fab0
```

Build, formatting, Clippy and runtime results above were obtained on `73bf1a5`.
Subsequent changes through candidate `9eec31a` update the owner direct-commit
policy, PR template and README. The candidate whitespace check passed;
executable/build inputs are unchanged. Owner review remains pending on this updated final revision.

PR 0 intentionally has no PTY or behavior test suite. Its smoke check verifies
only the placeholder contract and is not evidence of terminal functionality.

## Remote setup and continuation

This preparation creates local branches and commits only. No GitHub issue, PR,
label, repository rule or remote branch is implied by a local record. When
publishing is requested, publish the empty `main-human` anchor and bootstrap
branch, create the `track:main-human` issue and PR, and link their actual URLs.
The empty anchor gives GitHub a common ancestor for the bootstrap PR.

Configure repository rules separately to require PRs for AI and other contributor
submissions, while allowing the owner's personal direct updates to `main-human`.
Do not let AI use an owner bypass merely because it shares the owner's credentials.
The owner's direct commits need no separate PR review record; feature acceptance
still requires evidence. Required checks must match the
workflows present on the human branch, not unrelated `main` fuzz targets.
The bootstrap CI checks macOS and Linux on PRs and pushes to `main-human`;
these checks support review but do not enforce or certify the owner's identity.

Keep this ledger and the shared progress page on `main`. After each accepted
feature, update its evidence here and the progress record together. The first
review task is PR 0's complete diff against the empty anchor; its acceptance
adds infrastructure, not an accepted terminal feature.
