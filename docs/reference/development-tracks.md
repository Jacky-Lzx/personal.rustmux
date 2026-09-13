# Development Tracks and Human Review

Rustmux has two development tracks with different review requirements. The goal
is to keep exploring quickly while building a version whose implementation the
project owner has personally reviewed and accepted.

## Two branches, two review requirements

| | `main` | `main-human` |
| --- | --- | --- |
| Purpose | Rapid exploration and continued feature development | Incremental implementation under the owner's personal review |
| Code sources | Human-written or AI-generated; much of the existing implementation is AI-generated | Human-written or AI-generated |
| PR review | May be performed by AI | Must be performed by the project owner personally |
| Issue acceptance and closure | May be performed by AI | Must be confirmed by the project owner personally |
| Implementation | Serves as the feature reference | May use different modules, designs, and commits |

The word **human** describes the review requirement. It does not claim that all
code was written without AI. AI may generate code, propose designs, run tests,
and provide review suggestions on either track. On `main-human`, none of these
activities substitutes for the owner's review.

`main-human` does not need to copy `main` commit for commit. Features may be
reimplemented in smaller units, with different internal architecture and their
own commit history. Feature behavior and acceptance criteria provide the common
reference. Copying or cherry-picking an implementation does not make it reviewed.

## Issues

Use one track label per issue:

- `track:main` for work intended for `main`;
- `track:main-human` for work intended for `main-human`.

An issue should identify the behavior to implement, acceptance criteria, and
how those criteria will be checked. If a feature needs work on both tracks,
create two linked issues so implementation and acceptance can proceed
independently. Completion on `main` does not close its `main-human` counterpart.

AI may investigate and implement either issue. For a `track:main-human` issue,
AI must obtain the owner's personal confirmation before marking it accepted or
closing it. This also applies when closing an issue as
obsolete or not planned; such closures do not count as completed features.

## Pull requests

The PR's target branch determines its review requirements. Add the matching
track label for filtering; a label never overrides the target branch's policy.

The project owner may commit directly to `main-human` without a PR or a separate
PR review record. The owner is responsible for reviewing those changes before
committing. This exception applies to commits personally made by the owner;
AI submitting changes under the owner's Git identity does not qualify.

Changes submitted by AI or other contributors must go through a PR and the
owner's personal review. For these PRs, include:

1. The linked issue and the feature or behavior being changed.
2. The implementation approach, including intentional differences from `main`.
3. Verification results and any remaining limitations.
4. The commit being reviewed and a link to the owner's review record.

AI review and passing CI are supporting evidence, not approval for `main-human`.
The owner must review changes added after an earlier approval. Do not merge an
updated PR using an approval that covered an older revision only.

If the owner chooses to use a PR, their personal decision to merge the reviewed
final revision is sufficient; no separate self-review record is required. For
PRs submitted by AI or others, record the owner's review of the final revision.
An AI must never create this confirmation on the owner's behalf or infer it
from the Git author field.

Direct owner commits still need verification and explicit feature acceptance
before they increase coverage or close an issue. Record their commit SHA in the
acceptance ledger instead of a PR link.

Avoid automatic closing references for `track:main-human` issues before the owner
has confirmed their acceptance. A merged PR may implement only part of an issue.

These are project policies. This document does not configure GitHub labels,
branch protections, reviewer requirements, or CI triggers. Repository settings
and workflows must be configured separately to support the two tracks; a green
check alone does not establish compliance with this policy.

## Human-reviewed feature coverage

<label for="human-review-progress"><strong>Progress: not yet measured</strong></label>
<progress id="human-review-progress" aria-describedby="human-review-progress-note" style="display: block; width: 100%; max-width: 32rem; margin: 0.75rem 0;"></progress>
<p id="human-review-progress-note">Implementation snapshot updated on 2026-09-13: main-human provides a single-shell terminal with parsed rendering, resize and an expanding control-sequence subset. Owner agreement on the acceptance checklist is pending; implementation progress does not establish accepted coverage.</p>

| Tracking field | Current record |
| --- | --- |
| Snapshot date | 2026-09-13 |
| Local `main-human` implementation | `c5bd994` — soft terminal reset |
| Implementation record | [Committed capabilities, test sources and remaining gaps](human-review-plan.md#implementation-snapshot--2026-09-13) |
| Reference `main` commit | `e7fe6a1219754c383eb4fc7e40d30f2232def43f` |
| Agreed feature checklist | [Draft prepared; owner agreement pending](human-review-plan.md) |
| Features recorded as accepted | None recorded yet |
| Total features in this baseline | Not yet established |
| Coverage percentage | Not yet available |

Coverage measures **accepted functionality relative to a fixed `main` baseline**.
It does not measure lines of code, commit counts, time spent, or the proportion
of code written by a human. An implementation in progress or awaiting review
does not count as complete.

To establish and maintain the progress record:

1. Select a reference commit on `main` and agree on a checklist of reasonably
   small, independently verifiable features. Give each feature equal weight.
2. Track each item as **not started**, **in progress**, **awaiting owner review**,
   or **accepted**. Record its issue, PR or implementation commit, verification
   evidence, and the owner's acceptance record.
3. Count an item as accepted only after its implementation is present on
   `main-human`, its acceptance criteria pass, and the owner has reviewed and
   accepted that revision. If acceptance is withdrawn, remove it from the count.
4. Calculate coverage as `accepted items / total baseline items × 100`. Update
   the table and progress bar together, with a date and links to the evidence.
   Set the bar's `max` to the total and `value` to the accepted count once the
   denominator is known; until then it deliberately has no numeric value.
5. Record features added to `main` after the baseline separately for a later
   phase. Do not silently expand the denominator or imply that the percentage
   covers all of the latest `main` functionality.

The owner confirms acceptance; automation may summarize those recorded decisions
but must not infer approval from authorship, test results, or an AI review.
Keep this shared progress page on `main` alongside the published documentation,
linking to implementation and review evidence from `main-human`.

The first milestone has progressed from a bootstrap skeleton to a single-shell
CLI with PTY lifecycle, input forwarding, parsed rendering, resize and terminal
restoration. Terminal control support now includes scrolling and editing, cursor
and mode state, tab stops, status replies, DEC graphics, and full/soft reset.
Bracketed-paste negotiation is still on a candidate branch; editor compatibility
and milestone acceptance are not recorded. Windows, splits, persistent sessions
and Kitty extensions remain unimplemented on this snapshot of `main-human`.

See the [baseline ledger and first milestone plan](human-review-plan.md) for
the proposed checklist, current implementation snapshot, historical bootstrap
record, and evidence template.
