# Roadmap

One ordered plan for shapr and the tools around it, as of 2026-11-23. [vision.md](vision.md) describes the system; [decisions.md](decisions.md) records the reasons. Each step names the gate that lets it start, so the order changes only when evidence does.

## What the corpus says

Measured with `script/corpus.py` (CLAUDE.md, Measuring) after the parts change ([decisions](decisions.md) #26, #31):

- 0.8% of Ruby and Rust code lines are structural.
- Of 200 sampled logic hunks: 52.5% behavior, 35% test-only, 3.5% style, every other catalog kind 0 to 1.5%.

What follows from it:

- More refactoring detectors pay little. The review-cost wins are telling test code from behavior, then style.
- shapr's main product is an honest size of the behavior change, not a count of refactorings.
- The next evidence has to come from a consumer using the JSON, not from another detector.

## Running

One branch per lane. They land in this order, so the schema changes once and later lanes rebase onto the shared alignment.

| Order | Lane | Scope |
|---|---|---|
| 1 | `lane/schema` | Exit 0 whatever the verdict; a file `category`; docs don't flip the verdict; unknown kept outside the verdict; `review_lines` split into code and test; a leading meta record; claims judgement and GitHub input marked experimental ([decisions](decisions.md) #28, #30) |
| 2 | `lane/align` | One shared per-file alignment, a deeper Tree, an exhaustive hazard table, syntax checks moved into the frontends. Structure only: `script/corpus.py diff` against main shows no label change ([decisions](decisions.md) #29) |
| 3 | `lane/norms` | Style norms v1: quote style only ([vision](vision.md#norms), [decisions](decisions.md) #32, #39). Rebases onto `lane/align`; its output is experimental |

## Plan

In order. A step starts when its gate is met.

| # | Step | Why | Gate to start | Unblocks |
|---|---|---|---|---|
| 1 | **Proof status** (`lane/proof`). Each part lists its unchecked assumptions (`unseen_uses`, `search_capped`, `public_api`, `ambiguous_match`); the verdict and `review_lines` count only assumption-free parts as structure; uses outside the diff are searched with `git grep` (#47, #51) | A rename with callers outside the diff counts as proven structure today; confidence is computed and then ignored by every aggregate | `lane/schema` and `lane/align` landed | An honest verdict; routing between shapr and the classifier (#37) |
| 2 | **Per-run classification.** Classify the whole hunk first, then its maximal runs of changed lines | Labels stop depending on diff context; measured on the corpus: more lines proven, no false structure (#36) | Step 1, so hunk output changes once | Branch-wide labels worth reading |
| 3 | **Schema 2 candidate.** One bump covering the schema lane and steps 1 and 2; `script/corpus.py diff` explained row by row | A consumer should meet one schema, not three | Steps 1 and 2 | Steps 4 and 5 |
| 4 | **Fixtures as data.** `before`, `after` and expected labels as files, with a shared test harness; seeded with the 200 hand-labeled hunks | Every correction becomes a regression test; lanes stop colliding in the same test files; the learning loop needs this format | Step 3 | The classifier's eval set; the TUI; a coding agent working the failing set |
| 5 | **A thin grokr slice**, in its own repo: run shapr on the maintainer's own PRs, apply one pack rule (never mix; a size gate on behavior lines, tests apart), log each finding with accept, edit or dismiss, and record the head commit when a PR is first seen | The schema has no consumer; this tests whether shapr's facts change a review | Step 3 | Schema freeze; every tool under Later |
| 6 | **`restyle` kind** (`lane/restyle`, #54): the same tree apart from lexical tokens | The only catalog kind above noise (3.5% of logic hunks); it shares its syntax-variant table with norms | `lane/align` landed; a corpus count of candidate hunks | Verifiable format-only PRs; norms beyond quotes |
| 7 | **Hunt, regression sweep, 0.x release** with the schema marked unstable | Publishing pins expectations; do it after a consumer has read the JSON | Step 5 in use for two weeks with no schema change wanted | Outside users |

## Later, each behind a gate

Not ordered among themselves; the gate decides.

| Tool or change | Gate to start | First cheap test |
|---|---|---|
| Classifier specialist | The grokr slice runs; fixtures exist | Score a prompt against the 200 hand labels. The ceiling is known: about 11% of logic hunks could be downgraded (#31) |
| Decomposer wired in | The slice's size gate fires on real PRs | Hand a mixed PR's parts and residual to the existing persona; check each proposed commit replays |
| Blind reader | The slice runs | Compare its summary with the description on ten merged PRs |
| TUI review mode | Hand labeling from `script/corpus.py sample` files is the bottleneck | None needed until then; the fixture format (step 4) is the contract |
| First push vs merge | The slice has recorded first-seen head commits for about 20 merged PRs | `shapr FIRST..MERGED` on those; read what the labels say about what review changed |
| Code context CLI | A repo with more than one author is under review (the work codebase) | On the maintainer's own repos every author is the owner, so there is no signal to test yet |
| Harvesting past reviews | The slice produces findings to compare | Replay five old PRs; count the human's landed comments it finds |
| More norms | Quote style validated against `rubocop --auto-gen-config` on two real repos; `restyle` landed | One norm at a time, each with the same validation (#39) |
| Namespace-crossing moves and renames | A corpus count shows them above noise after step 2 | `script/corpus.py sample` |
| Operation ids | A consumer counts distinct operations (the size gate) and hunk ids prove too weak (#35) | |
| Retrospective signals, risk model | A few hundred labeled outcomes exist (#20) | Lines added alone as the baseline |
| A third language | A consumer reviews code in it | Step 4's fixtures, translated into the new language, as its conformance suite |
| Cloud trigger, shadow mode | The local slice's kept-comment rate is measured | |

## Waiting for evidence

- Guard clause, extract variable and constant, delete_dead, consolidate conditional, redundant return: each 0 to 1.5% of sampled logic hunks (#31).
- Comment bloat and spell-check: no measurement yet.
- Scores about people (#20, research).

## Held for the maintainer

- Where the TUI lives, and its design.
- Release to crates.io and Homebrew (step 7).

## Open questions

- Which reviewers and repos to harvest first.
- Whether person-level risk scores are used at work, and who can see them.
- The context pack format.
- How to measure author experience: commit history, review history, ownership files, or a mix.
- How author and file signals combine with shapr's change-type risk.
- What picks the response to a style deviation. Likely inputs: norm strength, the code context tool's role and ownership signals, PR stage.
- How norms score a hunk. Undecided on purpose: experiment and keep what produces signal.
- A name for the code context CLI.
