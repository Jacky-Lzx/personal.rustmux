# Human Review Baseline and Implementation Progress

## Baseline and review status

Prepared on 2026-09-12. The implementation reference is `main` commit
`e7fe6a1219754c383eb4fc7e40d30f2232def43f` (`feat: add explicit session creation shortcuts`).
This is a fixed reference, not a claim that its implementation has been reviewed.
The checklist below is a **proposal awaiting the owner's agreement**. There are
no features recorded as accepted and no agreed coverage denominator yet. Later additions to
`main` belong in a separate backlog until the owner chooses a new baseline.

## Implementation snapshot — 2026-09-13

The local `main-human` branch is at `c5bd994` (`feat: support soft terminal
reset`). It now contains a working single-shell CLI, terminal model, parser,
renderer, tests, CI and branch-specific documentation. The two tracks retain
independent histories; do not merge all of `main` into `main-human`.

This snapshot describes committed implementation and available test coverage.
It does not establish owner feature acceptance or certify a full-screen editor
walkthrough. The acceptance checklist and denominator remain proposals.

| Area | Implementation commits on `main-human` | Available verification sources |
| --- | --- | --- |
| Bootstrap and documentation | `5d3102e`, `fbaf76d` | Cargo setup, CI workflows and the branch's `docs/` |
| PTY shell lifecycle (H01) | `ff58361` | `tests/pty_lifecycle.rs`: controlling terminal, startup failure, child exit and cleanup |
| Input loop, resize and cleanup (H02, H04) | `66ded2e`, `acc3c53` | `tests/terminal_loop.py`, `tests/terminal_loop.rs` and `src/terminal.rs` unit tests |
| Screen model, cursor/erase, styles and Unicode (H03) | `b63dd6c`, `8932c11`, `0a1aef9`, `2440f6c` | Screen/parser unit tests, `tests/text_style.rs`, `tests/unicode_screen.rs` |
| Alternate screen, grid resize, rendering and CLI integration (H03, H04) | `da9c631`, `fdfa974`, `283fd74`, `26acf59` | `tests/alternate_screen.rs`, `tests/screen_resize.rs`, `tests/render.rs`, nested-PTY harness |
| Saved cursor and visibility (H03, part of H28) | `e2ba001` | `tests/cursor.rs` |
| Scrolling regions, line and character editing (H03) | `710ef10`, `65f6be4`, `5ac7534` | `tests/scroll_region.rs`, `tests/line_edit.rs`, `tests/character_edit.rs` |
| Origin, insert and automatic wrap modes (H03) | `589bef8`, `ffb0893`, `3efa096` | `tests/origin_mode.rs`, `tests/insert_mode.rs`, `tests/auto_wrap.rs` |
| Configurable tab stops and terminal status replies (H03) | `f3ada98`, `fef097c` | `tests/tab_stops.rs`, `tests/status_replies.rs` |
| DEC special graphics, full reset and soft reset (H03) | `6cdbda5`, `6d2e0ac`, `c5bd994` | `tests/character_sets.rs`, `tests/terminal_reset.rs`, `tests/soft_reset.rs` |

Verification sources above identify existing checks, **not a fresh test-run
result**. This documentation update inspected the committed implementation and
test sources; it did not rerun the terminal suite or a live editor walkthrough.
Review the exact snapshot locally with `git show c5bd994` and read its
usage documentation with `git show c5bd994:docs/index.md`.
The local combined documentation build uses `main-human` by default.

The implementation still has one shell and no windows, splits, persistent
sessions, scrollback or Kitty extensions. Input bytes are forwarded, but
bracketed-paste mode negotiation is not part of this snapshot:
`3769e30` on `codex/human-bracketed-paste` is a separate candidate, not yet in
`main-human`. Full-screen editor compatibility remains to be verified.

At inspection time, local `origin/main-human` pointed to `e2ba001`, 11 commits
behind local `main-human`. This is a cached remote-tracking ref, not a live
remote check. The hosted documentation may therefore differ from this local
snapshot; Pages builds the published branches. No push or deployment is implied
by this update.

## Proposed feature ledger

Each row is a candidate acceptance unit, not an implementation prescription.
Before agreeing on the denominator, split any row that cannot be independently
reviewed and verified in a reasonably small change. The reference column points
to documentation at the fixed baseline; consult that revision when behavior on
`main` changes. The verification column specifies planned checks, not results.

Implementation commits and test sources for active rows are listed in the
snapshot above. Issue/PR links, executed verification results and owner acceptance
are **not recorded** unless explicitly supplied. Local preparation
is not an issue or PR. Update those fields as work proceeds, and preserve the
final reviewed SHA when linking to a discussion. Accepted counts follow the
[track policy](development-tracks.md#human-reviewed-feature-coverage).

| ID | Behavior / acceptance boundary | Planned verification | Baseline reference | Status |
| --- | --- | --- | --- | --- |
| H01 | Start an interactive shell on a PTY; report startup failure and reclaim resources | PTY integration: valid/invalid shell, child exit and descriptor cleanup | `src/shell.rs`, `src/terminal.rs` | implemented; acceptance not recorded |
| H02 | Forward ordinary input, UTF-8, control keys and bracketed paste without corruption | Byte fixtures plus interactive shell and Ctrl-C | `src/input.rs` | in progress; paste mode pending |
| H03 | Render text, cursor, styles, wide characters and alternate screen | Terminal fixtures plus a full-screen editor | `src/terminal.rs`, `src/render.rs` | in progress; editor verification pending |
| H04 | Propagate resize and restore the outer terminal on exit or recoverable failure | Repeated resize, shell exit, injected I/O failure; compare terminal settings | `src/app.rs` | implemented; acceptance not recorded |
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
| H28 | Preserve hyperlinks, colors, cursor shape and per-pane protocol state | Focus/screen switches and terminal query fixtures | `docs/reference/terminal-compatibility.md` | in progress; cursor/style subset only |
| H29 | Deliver configured notifications and maintain bell/unread state | Filter/threshold tests and live focus/notification checks | `docs/configuration/notifications.md` | not started |

These 29 proposed rows are a planning inventory, **not an agreed total**. In
particular, configuration, session management and protocol rows may need further
splitting before the denominator is frozen. Performance checks accompany the
relevant feature; passing a benchmark alone does not establish acceptance.

## Milestone 1: a usable single-pane terminal

The original review breakdown below remains a planning reference. Bootstrap,
PTY lifecycle, input/rendering integration and resize/cleanup code are now on
`main-human`; the snapshot above records their commits. H02/H03 still have
compatibility and verification gaps, and milestone acceptance is not recorded.

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

## Historical bootstrap verification record

Local PR 0 candidate prepared on 2026-09-12. This historical candidate was
superseded by bootstrap commit `5d3102e` on `main-human`; the results below
apply only to the earlier candidate, not the current implementation:

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

To read the historical candidate locally from either worktree:

```sh
git diff b59e6c1 9eec31a
git show --stat 9eec31aa22b0995a8a4fdac745482fa3d2a2fab0
```

Build, formatting, Clippy and runtime results above were obtained on `73bf1a5`.
Subsequent changes through candidate `9eec31a` update the owner direct-commit
policy, PR template and README. The candidate whitespace check passed;
executable/build inputs are unchanged. Owner review remains pending on this updated final revision.

PR 0 intentionally has no PTY or behavior test suite. Its smoke check verifies
only the placeholder contract and is not evidence of terminal functionality.

## Continuation and publication

Keep this ledger and the shared progress page on `main`. Update implementation
commits as work reaches `main-human`; add verification results and explicit
owner acceptance separately. Features on candidate branches do not count as
implemented on the target branch.

The combined Pages workflow builds the published `main` and `main-human`
branches. Publish the relevant branch changes before expecting the hosted
books to match a local build; see [Development and Testing](development.md)
for build and deployment details. Local branch state does not confirm remote
rules, PR reviews, hosted CI results or owner acceptance.

Repository rules should require PRs for AI and other contributor submissions
while allowing the owner's personal direct updates. AI must not use an owner
bypass merely because it shares the owner's credentials. Required checks must
match the human branch's workflows, not unrelated `main` fuzz targets.
