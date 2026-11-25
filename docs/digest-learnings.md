# Digest learnings

What parts of shapr's output help a PR reviewer, found by blind reviewer
rounds, and how `--digest` is shaped by them. Newest round last.

## Method

Each round, every PR is reviewed three times by a fresh reviewer that sees
only its own input:

- A: the full JSON (`shapr -j owner/repo#N`)
- B: the digest (`shapr --digest owner/repo#N`, text)
- C: no shapr output

Every arm also gets the PR's title, description and diff (`gh pr diff`): a
reviewer sees those in practice, and shapr reads its claims from them. The
reviewer prompt is the same for every arm: review the PR for correctness,
structure-vs-behavior mixing and style consistency, at most five findings,
each with file, line, why it matters and confidence; then say which shapr
fields it used, ignored, or missed. Reviewers run on the same model.

Every distinct finding is labelled by hand from the code and the later
history: real (a defect, a mixing claim the code bears out, a style break),
plausible (defensible but unconfirmed, or a matter of taste), or noise
(wrong, or restating the description).

## PRs

| PR | why |
|---|---|
| dpep/polyid#109 | A reviewed rubocop pass: format and restyle hunks with a few logic and test edits mixed in. Does the digest let a reviewer skip the structure and find the logic? |
| dpep/polyid#107 | Spec support consolidated: a test-only refactor with helpers extracted and setup rewritten. Test code weighting. |
| dpep/polyid#104 | Registration split from the binary type, "no behaviour change": an extract that may not be one. |
| dpep/polyid#100 | A resolver named, "no behaviour change": shapr reports a near-miss extract with a reason. Does the near-miss note help? |
| dpep/polyid#96 | One owner for the uuid input rule, "no behaviour change", with a rename: a refactor shapr can't prove, the claimed-refactor conflict. |
| dpep/rwr#18 | A feature in Rust (naming the shadowing rule) with docs and tests: behavior, no structure. |
| dpep/rspec-networking#29 | Accept unpadded MACs: a small feature with a regex change, where a correctness bug would be in the logic. |
| flippercloud/flipper#1124 | 86 WebMock stubs dropping default arguments, test-only, mechanical: 73 logic hunks with one repeated near-miss reason. Grouping and size. |
| dpep/rq#36 | A real race fixed in Rust: a small behavior change with docs, where the review is all in a few lines. |

The polyid PRs merged as one stack, so later history mostly shows the next
step rather than a fix; ground truth is labelled from the code where history
says nothing.

## Round 0: first digest

Seeded from what earlier consumer and replay testers relied on: the summary
lines (verdict, what was compared, claims and unproven claims, the refactor
line), then the logic hunks ranked (code before tests, a near miss of a
structural kind first, then facets and review lines) with their facets, the
structure beside them, the residual's reason and relocated lines; hunks of
one file with the same reason as one line; at most 10 items and a count of
the rest; structural parts that aren't proven, with their assumptions and
move contexts; proven structure as counts to skip; diverging style
deviations; hunks outside the verdict as counts per category.

## Classifier notes

Issues seen in the rounds that belong to classification, not the digest.
