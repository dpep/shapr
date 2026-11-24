# Vision

shapr is one evidence tool in a larger system: an automated PR reviewer that leaves few comments, each concrete and backed by evidence. This doc describes that system, where shapr fits, and what exists today. [roadmap.md](roadmap.md) says what's next; [decisions.md](decisions.md) records why things are the way they are.

## Goals

- **High signal.** Few comments, each concrete and evidence-backed. No style nits.
- **Learns from the maintainer.** Record what's accepted, edited, dismissed and missed, and turn it into rules.
- **Runs semi-autonomously in the cloud**, eventually.
- **Starts small.** The maintainer's own PRs and open source tools first, then a work codebase.

## What exists

| Piece | What it does | Status |
|---|---|---|
| shapr | Classifies the shape of a change: proven structural parts plus a logic residual, per hunk | Exists (Ruby, Rust) |
| Norms (in shapr) | Judges each changed hunk's style against its surrounding code; `shapr norms <file>` reports a file's norms | In progress |
| grokr (working name) | The PR reviewer: orchestration, context packs, specialists, learning loop | Planned |
| Classifier | Structure-vs-behavior judgement on the hunks shapr leaves as logic | Planned |
| Decomposer | Suggests how to split a mixed or oversized PR | Exists as a standalone agent persona; not yet wired in |
| Blind reader | Summarizes a change from code alone, then checks the description against it | Planned |
| Code context CLI | Who is changing what, and how risky the area is | Planned; name undecided |
| TUI review mode | Steps through hunks to confirm or relabel; verdicts become fixtures | Planned |
| Risk model | Predicts risk from diff features and retrospective outcomes | Planned |

## Architecture

Two layers ([decisions](decisions.md) #5, #15):

- **Evidence tools** are narrow, deterministic and composable: shapr, the code context CLI, and existing code tools (trekr for Ruby call resolution, contour for semantic search). They never call an LLM.
- **grokr** orchestrates. It runs the tools, hands their JSON to specialists, and keeps the feedback log and learning loop. Agents judge only what tools can't settle.

grokr itself has two parts:

- **Core library (reusable engine):** orchestration, feedback log, learning loop. Mechanism lives here: detection, classification, feedback.
- **Context packs (swappable):** review guide, choice of specialists, rules. One per project, one for a work codebase. Policy lives here, such as Tidy First's never-mix rule. The maintainer's pack ships as the opinionated default; longer term, packs are shareable.

## Review philosophy

- Small, focused PRs. One change at a time.
- Kent Beck's Tidy First: a PR changes structure or behavior, never both.
- Push authors to break big problems into smaller ones.
- Read the code first, then check the description matches. Don't trust the description.

## Specialists

| Specialist | Sees | Returns |
|---|---|---|
| Structure-vs-behavior classifier | The diff, shapr's JSON and repo read access; never the PR description | Per hunk: label, confidence, one-sentence reason; plus distinct-operation count and verdict |
| Decomposer | A mixed or oversized PR | Smaller PRs to ship in parallel or in sequence |
| Blind reader | Code only | A summary of the change; a separate step compares it with the author's description and flags gaps |

The classifier accepts shapr's structural parts and judges only the logic residual: could any caller, test or user observe a difference? Its authority is earned:

- It may escalate structure to behavior on its own. Downgrading behavior to structure goes to the maintainer in the TUI.
- Authority per change type grows once its record of disputes proves right.
- Every label, from shapr or the classifier, carries a confidence. Low confidence goes to the TUI first, and scores are checked against the maintainer's verdicts for calibration.

| shapr | Classifier | Action |
|---|---|---|
| High, agrees | High | Accept |
| Low | High | Lean on the classifier; downgrades still need sign-off |
| Any gap between them | | Flag for closer review; the gap feeds the corpus |

## shapr's place

shapr answers *what kind of change is this*. [README.md](../README.md) covers usage, [schema.md](schema.md) the output contract, [refactorings.md](refactorings.md) which refactorings it can prove, and [research.md](research.md) the prior art.

Ideas for shapr beyond what's built:

- **Comments.** Flag comment bloat (comments growing well beyond the code they describe) and spell-check new comment text.
- **Test evidence.** Run the old test suite against the new code; if a PR had to edit tests, behavior likely changed.

## Norms

Consistency wins: new code should match the code around it, unless the PR is deliberately changing the format. Norms live in shapr ([decisions](decisions.md) #32).

- **Where a formatter or linter exists** (RuboCop, Standard), it is the authority and the reviewer never comments on formatting.
- **Otherwise, infer local norms** from the file and its neighbors and flag only code that departs from them.
- **Deliberate format changes** land as their own format-only PR, which shapr can verify.

v1 (in progress):

| Aspect | v1 |
|---|---|
| Per hunk | A `style` field: each deviation with norm, expected style, found style, confidence, location and a suggested edit |
| Per file | `shapr norms <file>`: one record per norm (norm, scope, inferred style, confidence, deviating lines); no diff needed |
| Language | Ruby, through shapr's language frontends. Rust already has strong formatters |
| Norms | Choice-of-style RuboCop cops: indentation (`Layout/IndentationWidth`, `Layout/IndentationConsistency`), spacing (`Layout/SpaceAroundOperators`, `Layout/SpaceInsideHashLiteralBraces`, `Layout/EmptyLineBetweenDefs`, `Layout/TrailingWhitespace`), quotes (`Style/StringLiterals`, first), trailing commas (`Style/TrailingCommaInArrayLiteral`, `...InHashLiteral`, `...InArguments`), hash syntax (`Style/HashSyntax`), naming (`Naming/MethodName`, `Naming/VariableName`) |
| Inference | Count each style per scope; a style is a norm at 80% or more (configurable). Below that, stay silent |
| Scope | The enclosing method first, widening to class or file when needed. A new file compares against neighboring files. Uncertain means abstain |
| Linter config | Lean on configured norms. A mismatch between configuration and the code's internal consistency is itself signal |
| Mixed hunks | New code still scores as aligned or divergent, at lower weight |
| Skipped | Generated, vendored and documentation files |
| Modes | Report, with suggested edits as data. Rewrite comes next |
| Validation | Small Ruby fixtures with known norms; compare inferred styles with `rubocop --auto-gen-config` on two real repos |

shapr learns from RuboCop's style cops; it never wraps or runs RuboCop. Later: the rest of RuboCop's configurable cops, ranked by how often they vary between repos, and repo-wide inference.

Format-only hunks are judged by whether their scope ends up more consistent:

| Direction | Meaning |
|---|---|
| Converging: normalizing | A divergent section brought to the existing norm |
| Converging: renorming | The scope moved wholesale to a new style; ideally its own PR |
| Diverging | Consistency went down; flag it |

The goal is a codebase that converges over time. Scoring is undecided on purpose: experiment and keep what produces signal.

A deviation is signal, not a verdict. Choosing a response is a separate step, out of scope for v1:

1. Ask the author to fix it in review.
2. Suggest the change for the author to accept.
3. Push a commit to the PR.
4. Leave it, and normalize later in an automatic maintenance PR after merge.

## Code context

Who is changing what should shape how hard the review pushes. Authors get far more discretion in their own and their team's code than elsewhere. These signals belong in a separate CLI ([decisions](decisions.md) #33): shapr answers *what kind of change*, the code context tool answers *who and where*.

| Signal | Values |
|---|---|
| Repo relationship | The maintainer's own repo, their team's, or someone else's |
| Author relationship | The maintainer, a teammate, a code owner (per CODEOWNERS), another team, an outside contributor |
| Experience | How much the author has worked in these files or this system (past commits, reviews, time since last touch) |
| File and system risk | Churn, fixes and reverts, judged against each file's own baseline |

It reads git history, CODEOWNERS and team membership, and emits JSON per file. Owners and experienced authors get lighter, more trusting reviews; newcomers and outsiders get deeper reviews and more explanation. Context sets depth and tone, never pass/fail.

## Dual use: authoring and review

Every tool serves both ends of a PR. Authors (human or agent) run them before opening; reviewers run the same checks after.

| Tool | For the author |
|---|---|
| shapr | Warns that a change mixes structure and behavior or is too big, and suggests the split |
| Norms | Fixes formatting to match local norms before the PR goes up |
| Code context | Says how risky the area is and who owns it: whom to ask, how careful to be |

Problems get caught by the author, so review stays on what tools can't judge. And the maintainer dogfoods the tools on every PR they write, not only the ones they review.

## Learning loop

- Log every suggested comment and the maintainer's action: accepted, edited, dismissed. Log comments they wrote that the bot missed.
- Periodically turn the logs into rules and good/bad examples in the review guide.
- Track precision: the share of bot comments kept.

**Classifier loop.** Per hunk, record shapr's label, the classifier's and the maintainer's. Agreement is a confirmed example. Disagreement becomes a test fixture with the maintainer's label as truth, so every correction is a regression test and the failing set is shapr's to-do list. The classifier surfaces likely tool weaknesses; the maintainer settles disputes.

- **Feedback surface.** First a local TUI, a mode of shapr (likely ratatui), that recreates GitHub-style before/after diffs and steps through hunks with single keys to confirm or relabel. Later, verdicts as PR replies once precision allows.
- **Fixtures.** Verdicts write straight into a fixtures folder as a growing corpus: one JSON file per hunk (before code, after code, shapr label and confidence, classifier label and confidence, the maintainer's verdict), named by content hash to dedupe. A coding agent works through failing fixtures to improve shapr. A learnings log sits alongside the corpus.

## First push vs merge

Snapshot a PR's hunks at first push and again at merge; the difference is what iteration and review changed.

- **Signal:** each change between snapshots is something the first draft got wrong or missed, whether fixed by an agent iteration, a manual edit, or a reviewer comment.
- **Learning:** classify those changes with shapr (tidying, behavior fix, formatting) and feed them back, so authoring tools catch them before the first push next time.
- **Attribution:** link each change to its trigger where possible (review comment, CI failure, self-edit).

## Harvesting past reviews (proposed)

Mine past PR reviews from the maintainer and chosen reviewers, as ground truth and as style to model.

- **Pull:** GitHub API review comments (each carries its diff hunk, file and line), review summaries, and PR conversation comments, filtered by author.
- **Outcome:** did the code at that line change in a later commit, and was the thread resolved? Changed means the comment landed.
- **Filter:** drop LGTMs, approvals and pure nits.
- **Uses:**
  1. Eval set: replay old PRs through the reviewer and measure how many of the human's landed comments it finds.
  2. Review guide: cluster recurring themes into rules, and pick strong comments as examples per reviewer.
  3. Persona packs: one context pack per reviewer, modeled on their habits.
- **Care:** for colleagues, ask before modeling them. Keep work data in the work context pack.

## Retrospective signals (proposed)

Revisit merged PRs weeks and months later to learn what review missed and where risk concentrates.

- **Signals after merge:** reverts (strongest), bug-fix follow-ups touching the same lines, hotfixes, refactors, feature extensions. Trace fixes back to the PR that introduced the lines (the SZZ approach).
- **Feed back into review:**
  1. Missed findings: for each later fix, did the reviewer flag it up front? Misses become eval cases.
  2. Risk by change type: are mixed structure-and-behavior PRs reverted more often? This tests Tidy First on real data.
  3. Reputation scores for files, systems, change types and people, from churn, fixes and reverts (hotspot analysis).
- **Attribution against a baseline:** compare a person's fix and revert rate on a file with everyone's rate on that file. If anyone who touches it breaks it, the risk belongs to the file (a high-risk context), not the person.
- **CI history (deferred):** red-then-green pushes are weak evidence, since many people use CI as their test runner on purpose. If used, judge CI when the PR is marked ready for review and treat flaky tests separately. Context only, never a score.
- **Use:** scores set review depth and which specialists run, never pass/fail on their own.
- **Care:** churn has confounders (core files and hard tasks attract fixes). Scores about people are sensitive: keep them private to the reviewer, explainable, and decayed over time.

## Risk model (proposed)

Part of the feedback loop: learn from every PR as it goes.

- **Features per diff:** shapr's parts, metaprogramming present (`send`, `define_method`, `method_missing`), specs added or modified, files touched, size, and file and system risk scores.
- **Labels:** retrospective outcomes (reverts, follow-up fixes) plus the maintainer's verdicts.
- **Model, staged:** a simple, explainable model first; the baseline and the bar a feature must clear are in [decisions](decisions.md) #20. Consider learned embeddings only once the corpus is large.
- **Use:** predicted risk sets review depth, alongside the reputation scores.
- **Size gate:** bigger means riskier and less understood. A PR over a few files or a few hundred lines goes to the decomposer. Measure behavior-change size, not raw lines: exclude generated files and lockfiles, and score refactors by how mechanical they are. One rename applied across many files is one operation (high surface area, little change); several distinct renames or refactors count separately, and each could land as its own PR.

## Rollout

1. Local, run by hand on the maintainer's own PRs and past PRs; compare with real reviews.
2. Cloud trigger (GitHub Action or webhook), shadow mode: findings shown only to the maintainer.
3. Drafted comments the maintainer approves.
4. Autonomous posting only for categories with proven precision.
