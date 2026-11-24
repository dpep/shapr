# Roadmap

Where shapr stands and what's next, as of 2026-11-23. [vision.md](vision.md) describes the system shapr belongs to; [decisions.md](decisions.md) records the reasons behind each step.

## Running

Work in flight, one branch per lane.

| Lane | Scope |
|---|---|
| `lane/align` | One shared per-file alignment, a deeper Tree, an exhaustive hazard table, syntax checks moved into the frontends. Structure only: no behavior changes |
| `lane/schema` | Drop verdict exit codes; a file `category`; docs don't flip the verdict; unknown kept outside the verdict; `review_lines` split into code and test; a meta record; claims judgement and GitHub input marked experimental |
| `lane/norms` | Style norms v1, quote style first ([vision](vision.md#norms)) |

## Next

In order:

1. **Proof status.** Each part carries its assumptions. The verdict and `review_lines` count only assumption-free parts. `git grep` settles uses outside the diff.
2. **Per-run classification.** Classify the whole hunk first, then its maximal changed runs.
3. **Operation ids** for operations that span hunks, so one rename across ten files reads as one operation.
4. **Fixtures as data**, shared with the TUI learning loop.
5. **A thin grokr slice** that consumes shapr's JSON on the maintainer's own PRs.
6. **New kinds in measured order:** `restyle` first, then namespace-crossing moves and renames. Others wait for evidence.
7. **Hunt and regression sweep**, then a 0.x release with the schema marked unstable.

## Held for the maintainer

- TUI review mode design.
- Release to crates.io and Homebrew.

## Why this order

Corpus measurements drive it. As of 2026-11, after the parts change ([decisions](decisions.md) #26):

- 0.8% of Ruby and Rust code lines are structural.
- Of 200 sampled logic hunks: 52.5% behavior, 35% test-only, 3.5% style, every other catalog kind 0 to 1.5%.

So the review-cost wins are in telling test code from behavior and in style, not in more refactoring detectors. Groundwork comes first because each new kind would otherwise multiply private aligners and duplicated tree helpers ([decisions](decisions.md) #29). Reproduce the numbers with `script/corpus.py` (see CLAUDE.md's Measuring section).

## Open questions

- Which reviewers and repos to harvest first.
- Whether person-level risk scores are used at work, and who can see them.
- The context pack format.
- How to measure author experience: commit history, review history, ownership files, or a mix.
- How author and file signals combine with shapr's change-type risk.
- What picks the response to a style deviation. Likely inputs: norm strength, the code context tool's role and ownership signals, PR stage.
- How norms score a hunk. Undecided on purpose: experiment and keep what produces signal.
- A name for the code context CLI.
