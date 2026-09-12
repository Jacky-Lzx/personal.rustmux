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

Every change entering `main-human` must go through a PR and the owner's personal
review. Include:

1. The linked issue and the feature or behavior being changed.
2. The implementation approach, including intentional differences from `main`.
3. Verification results and any remaining limitations.
4. The commit being reviewed and a link to the owner's review record.

AI review and passing CI are supporting evidence, not approval for `main-human`.
The owner must review changes added after an earlier approval. Do not merge an
updated PR using an approval that covered an older revision only.

For an owner-authored PR, record the owner's explicit review of the final commit
in the PR discussion or review checklist before merging. Do not treat authorship
alone as proof of review. An AI must never create this confirmation on the owner's
behalf. For PRs authored by others, use the owner's review on the final revision.

Avoid automatic closing references for `track:main-human` issues before the owner
has confirmed their acceptance. A merged PR may implement only part of an issue.

These are project policies. This document does not configure GitHub labels,
branch protections, reviewer requirements, or CI triggers. Repository settings
and workflows must be configured separately to support the two tracks; a green
check alone does not establish compliance with this policy.

## Human-reviewed feature coverage

<label for="human-review-progress"><strong>Progress: not yet measured</strong></label>
<progress id="human-review-progress" aria-describedby="human-review-progress-note" style="display: block; width: 100%; max-width: 32rem; margin: 0.75rem 0;"></progress>
<p id="human-review-progress-note">The feature baseline and acceptance ledger have not been established. No percentage is reported, and no existing implementation is assumed to have passed the owner's review.</p>

| Tracking field | Current record |
| --- | --- |
| Reference `main` commit | To be selected by the owner |
| Agreed feature checklist | Not yet established |
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
3. Count an item as accepted only after its implementation is merged into
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

A suggested first milestone is a usable single-pane terminal: start a shell,
forward input, render output, resize, and exit correctly. It is a starting point
for the owner's checklist, not an already approved baseline.
