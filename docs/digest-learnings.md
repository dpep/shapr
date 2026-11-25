# Digest learnings

What parts of shapr's output help a PR reviewer, found by blind reviewer
rounds, and how `--digest` is shaped by them. Newest round last.

## Method

Each PR is reviewed once per arm by a fresh reviewer that sees only its own
input:

- A: the full JSON (`shapr -j owner/repo#N`)
- B: the digest (`shapr --digest owner/repo#N`, text)
- C: no shapr output
- T: the default text output (`shapr owner/repo#N`), added in round 2

Round 1 ran A, B and C; round 2 ran B and T against round 1's A and C, whose
inputs don't change; round 3 ran A and B again, for a same-round comparison.

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

## Round 1: reviewing with the round-0 digest

27 reviewers (9 PRs × A, B, C). Every distinct finding was labelled by
hand against the code at the merge and the history after it: no PR in the
set has a follow-up fix or revert of its own change, so ground truth is the
code (the WebMock source for flipper's stubs, the polyid and rwr code at
the merge and today).

Real findings, and who caught them:

| PR | real finding | A | B | C |
|---|---|---|---|---|
| polyid#109 | `configuration_spec.rb:42`, `:75`: `stub_const` dropped in a lint pass, the test models now anonymous (a test behavior change in a structure PR) | yes | yes | yes |
| polyid#109 | `entry_points_spec.rb:34`: `where(...).first` became `find_by`, so the entry point the spec exists to cover changed | yes | yes | yes |
| polyid#107 | `cache_spec.rb:318`: the eviction test no longer proves the cache was warmed, so it passes without evicting anything | yes | yes | yes |
| rwr#18 | `src/cli/mod.rs:2744`: "not by the rule declaring this case" is false when the declaring rule also rewrote the snippet (unchanged on main) | yes | yes | no |
| rwr#18 | `--json` `case` and `rule` change meaning, and the CHANGELOG doesn't tell JSON readers | yes | yes | yes |
| rwr#18 | an unrelated CHANGELOG correction (D127/D129) mixed into the feature | yes | yes | yes |
| flipper#1124 | `cloud_spec.rb:51`: the rewrite leaves a bare `.to_return`, an empty response sequence, not WebMock's default 200 | yes | yes | yes (wrong mechanism) |
| rq#36 | `docs/DECISIONS.md:2751`: the new sentence isn't rewrapped with its paragraph | yes | yes | yes |
| **total** | | **8** | **8** | **7** |

polyid#104, #100, #96 and rspec-networking#29 have no real finding: their
findings are plausible (a guard now only the wrapper holds, a comment on the
wrong method, a seam with no second caller, a public method, EUI-64's
overlap with IPv6, limited by the matcher's `FFFE` check) or noise.

Noise, by arm (wrong on the code): A 5, B 6, C 6. Shared by all three: an
unseen reader of polyid#104's renamed ivar (there is none), polyid#96's
downcase-before-match (`UUID_PATTERN` is `/i`), rspec-networking#29's
unvalidated `normalize` input (every caller validates first), polyid#100's
public method (later PRs call it from outside the class). B and C alone:
polyid#107's `:widget` factory (it exists).

Sizes: the full JSON totals 339 KB over the 9 PRs, the round-0 digest
7.3 KB (2.2%), the default text output 22 KB (6.6%), the diffs 81 KB. The
digest is a fiftieth of the full JSON and a tenth of the diff.

Reading the result: B matched A on real findings at 2% of the size, and
C, with no shapr output, found 7 of the 8. On this set shapr's output
changes where a reviewer looks more than what it finds; the one finding C
missed (rwr#18's attribution) both shapr arms reached through the
ranked logic hunk (B) or its facets (A).

What reviewers used, ignored and missed:

- Used (A and B alike): the ranked logic hunks and their facets, which
  sent reviewers to the right hunk first; the near-miss reason
  (polyid#100's "not an extract of `polyid_key_target`"); the claim
  conflict ("claims refactor; 12 hunks not shown structural"), which made
  reviewers check a "no behaviour change" claim by hand; "outside the
  verdict: N docs hunks", which sent two B reviewers to the docs hunks
  where rq#36's and rwr#18's findings were. A reviewers also used the style
  norms' `aligned` directions to rule out style findings, which B had no
  way to do.
- Ignored: comment counts, `entropy`, `subsystems`, `move_contexts`,
  `config_trajectory`, per-norm evidence counts (A); the comment counts and
  the skip line (B).
- Missed or confusing in the digest: on flipper#1124 every item carried
  the same reason, so the ranking said nothing and the one risky site
  (`cloud_spec.rb:51`) sat in "and 18 more"; on polyid#109 code-before-tests
  ranked three idiom rewrites in `lib/` above the two test hunks holding the
  real findings; "beside comment_only" read as a mostly-comment hunk; the
  docs hunks were counted but not placed.

## Round 1 changes

- Residuals sharing a reason group across files, not only within one, so
  flipper#1124's 73 hunks read as 14 items and `cloud_spec.rb:51` shows.
- Ranking: a near miss first, then facets, then lines; tests no longer
  sort after code (polyid#109).
- Up to 15 items, and the rest are placed (`file:line`), not only counted:
  nothing is hidden behind a count.
- "beside X" became "the hunk also proves X".
- `style: no divergence in N hunks checked` when the norms judged the new
  code and found nothing, the confirmation A reviewers drew from `aligned`.
- Hunks outside the verdict are placed per category, not only counted.
- JSON: items and every other place are `"file:line"` strings (`at`).

Size after the change: 7.1 KB over the 9 PRs (2.1% of the full JSON):
grouping saved more on flipper#1124 than the placements and style line
added elsewhere.

## Round 2: the round-1 digest against the default text output

Built from main fa8549f plus the digest commits (main's 5a0bd94, merged
during the round, changes only help text). A and C inputs don't change
between rounds, so rerunning them would only measure reviewer variance; this
round spent those 18 reviewers on B (the round-1 digest) and a new arm, T,
the default text output (`shapr owner/repo#N`), to ask whether a new mode
earns its place over the text shapr already prints. Compared against round
1's A and C.

| PR | real finding | A (r1) | B | T | C (r1) |
|---|---|---|---|---|---|
| polyid#109 | `stub_const` dropped | yes | yes | yes | yes |
| polyid#109 | `where.first` became `find_by` | yes | yes | yes | yes |
| polyid#107 | eviction test proves nothing | yes | yes | yes | yes |
| rwr#18 | false "not by the rule declaring this case" | yes | partly: named only as an untested case | yes | no |
| rwr#18 | `--json` `case`/`rule` change unannounced | yes | yes | yes | yes |
| rwr#18 | unrelated CHANGELOG correction mixed in | yes | yes | no | yes |
| flipper#1124 | bare `.to_return` | yes | yes | yes | yes |
| rq#36 | DECISIONS.md line not rewrapped | yes | yes | yes | yes |
| **total** | | **8** | **7** (+1 partial) | **7** | **7** |

Sizes over the 9 PRs: B 7.1 KB, T 22 KB, A 339 KB.

Reading: every arm lands within one finding of every other, and the misses
differ by arm (B and C missed rwr#18's attribution, T its CHANGELOG mix),
which is what reviewer variance looks like on a set this small. The digest
holds its findings at a third of the text output's size and a fiftieth of
the full JSON's, but nothing here says it finds more.

What reviewers said, both B and T:

- Grouping worked: B's flipper#1124 reviewer reached `cloud_spec.rb:51`
  through its own line in the list; T's reviewer found it by scanning for
  the lines whose reason was empty among 60 identical ones.
- "unproven: claims docs; 8 hunks not shown structural" is hard to parse
  and doesn't say which hunks (B and T, as in round 1).
- polyid#109's `lib/polyid.rb:98` (`x.nil? ? true : x` to `x.nil? || x`)
  still ranked first on its `condition` facet, over the 7- and 8-line test
  hunks holding the PR's two real findings.
- Wanted from shapr itself, not the digest: an expression extract
  (polyid#100, again), a rename through an ivar symbol (polyid#104, again),
  "a rewrite leaves a call with no arguments" (flipper#1124, both arms), a
  regex quantifier widened (rspec-networking#29), and a test hunk that
  removes assertions (polyid#107).

## Round 2 changes

- The text output's `unproven:` and `conflict:` lines name the first three
  hunks behind the claim, as the `refactor:` line does. This is shapr's own
  output, not only the digest.
- Ranking: a near miss first, then lines, then facets. A facet marks an
  idiom rewrite as readily as a risk.

## Round 3: a fresh A against the round-2 digest

Built from main 5a0bd94 plus the digest commits. Both A (the full JSON,
which round 2 didn't change) and B (the round-2 digest) were rerun with
fresh reviewers, so B meets an A of its own round and the A arm gets a
second sample.

| PR | real finding | A | B |
|---|---|---|---|
| polyid#109 | `stub_const` dropped | yes | yes |
| polyid#109 | `where.first` became `find_by` | yes | yes |
| polyid#107 | eviction test proves nothing | yes | yes |
| rwr#18 | false "not by the rule declaring this case" | no | yes |
| rwr#18 | `--json` `case`/`rule` change unannounced | yes | yes |
| rwr#18 | unrelated CHANGELOG correction mixed in | yes | yes |
| flipper#1124 | bare `.to_return` | yes | partly: flagged as looking unfinished, called equivalent |
| rq#36 | DECISIONS.md line not rewrapped | yes | yes |
| **total** | | **7** | **7** (+1 partial) |

The A arm itself moved between its two samples (8, then 7: round 3's A
reviewer missed rwr#18's attribution, which round 1's caught), the same
size as every gap between arms.

What reviewers said: the digest's "the hunk also proves format_only" on
polyid#107's `cache_spec.rb:318`, the hunk holding the PR's one real
finding, read as "this hunk is safe" to two reviewers (round 2's T and
round 3's B), when its structure is one deleted blank line beside six
lines of removed assertions.

## Round 3 change

- Structure beside logic in a hunk is counted in lines, `beside it 1
  format_only line`, and `with` is an object of changed lines per kind.

## Stopping

Three rounds. B matched A in both rounds that ran them side by side (8 and
8, 7 and 7), and in round 2 sat within one finding of round 1's A, the same
distance A sits from itself across rounds. Another round would measure
reviewer variance, not the digest: every finding any arm missed was missed
by some other arm too, and the misses the reviewers attributed to shapr
are classification gaps (below), not digest gaps.

## Summary

The digest, as text (polyid#109, 797 bytes; its full JSON is 57.5 KB):

```
dpep/polyid#109: mixed: 19 hunks (6 format_only, 8 logic, 5 restyle); review 31 lines, 25 in tests; structure share 0.47
claims: style in 1 of 2 messages
unproven: claims style; 8 hunks are logic (lib/polyid.rb:98, lib/polyid/binary_uuid_type.rb:20, lib/polyid/version.rb:2, 5 more)
read first:
  spec/models/entry_points_spec.rb:32  8 lines (test)
  spec/models/configuration_spec.rb:42  7 lines (test)
  spec/models/configuration_spec.rb:75  5 lines (test)
  spec/polyid_spec.rb:43  3 lines (test)
  lib/polyid.rb:98  2 lines [condition]
  lib/polyid/binary_uuid_type.rb:20  2 lines
  lib/polyid/version.rb:2  2 lines
  spec/spec_helper.rb:45  2 lines (test)
skip, proven structure: 28 changed lines (6 format_only, 5 restyle); 11 hunks wholly structure
style: no divergence in 13 hunks checked
```

Sizes over the 9 PRs: the full JSON 339 KB, the default text output 22 KB,
the digest 7.5 KB as text (2.2% of the full JSON) and 15.9 KB as JSON
(4.7%), the diffs 81 KB.

Real findings per arm and round:

| round | A (full JSON) | B (digest) | T (text output) | C (no shapr) |
|---|---|---|---|---|
| 1 | 8 | 8 | | 7 |
| 2 | (round 1's 8) | 7 + 1 partial | 7 | (round 1's 7) |
| 3 | 7 | 7 + 1 partial | | |

What the evidence supports:

- The digest keeps what reviewers used from the full JSON (the logic hunks
  and their facets, near-miss reasons, claim conflicts, where the docs
  hunks are, a style all-clear) at a fiftieth of its size, with no loss of
  real findings. For an agent reading shapr beside the diff, it is the
  better input than the full JSON.
- It does not find more than the default text output, at a third of that
  output's size, and C, with no shapr output at all, found 7 of 8. On these
  9 PRs shapr changes where a reviewer looks first and how fast it can rule
  hunks out more than what it finds. A set of PRs with known, subtle bugs
  in large diffs would test that harder than this one could.
- Reviewers drew the same requests from every arm, which point at the
  classifier, not the presentation: the Classifier notes below.

Open questions:

- Does the digest earn a flag over the text output? It is a third of the
  size and groups repeated reasons (flipper#1124's 73 hunks become 14
  items), but reviewers found the same things in both. A cheaper move
  could be to group repeated reasons in the text output itself.
- Ranking moved twice and never changed a finding: on small PRs reviewers
  read every listed hunk. Whether ranking matters on 50-hunk PRs is untested.
- Should `-j --digest` exist? No reviewer read it; it was kept for agents
  that parse, and is 2x the text.
- The digest repeats a residual's confidence nowhere; reviewers of the full
  JSON read "logic 1.00" on a near miss as overconfident. Worth settling in
  the full output first.

## Classifier notes

Issues seen in the rounds that belong to classification, not the digest.

- Expression extract (polyid#100): `model = <expr>` becoming
  `model = polyid_key_target(key)`, where the new method's body is `<expr>`
  verbatim, is refused as "the statements assign model, which
  polyid_translate_condition also assigns". Both shapr reviewers called it a
  false negative, and the reason reads as a data-flow hazard where it is a
  granularity limit. Proposal: extract over an expression (the right-hand
  side of a binding, an argument) as well as statements.
- Instance-variable rename through a symbol (polyid#104): `@a_registered`
  renamed in its two uses and in `MEMOISED = %i[@a_registered]` is three
  logic hunks, the symbol a `literal_only` facet. Proposal: a field rename
  whose symbol spellings in the same file follow it.
- A Rust field's `///` doc comment counts as code (rwr#18, `src/cli/mod.rs:2541`):
  the hunk adds only `/// The rule declaring the case, ...` above a struct
  field, yet it is `code_added: 1` and `logic`. The README says doc comments
  are comments. Likely the field's doc comment is folded into the field node
  as its own text.
- Idiom rewrites that are the same program (polyid#109): `x.nil? ? true : x`
  to `x.nil? || x`, `super(value)` to a bare `super` where `value` isn't
  reassigned. Three of eight logic hunks there are these; a reviewer ranked
  them as the noise that pushed the real test changes down. Proposal:
  candidates for `restyle` variant families, each proven on the trees.
- Cross-file helper extraction in specs (polyid#107): a spec's private
  helper replaced by a shared support helper or a FactoryBot trait is logic
  with `code_similarity` 0. Not a cheap proposal; recorded for the corpus.
- A near-miss residual reports confidence 1.0 (polyid#100, flipper#1124): two
  A reviewers read "logic, 1.00" on a hunk whose reason says it nearly
  proved a structural kind as overstated. Worth a decision on what a
  residual's confidence means beside `near`.
- Claims from commit subjects: "README" or "changelog" in a commit subject
  makes the change claim `docs`, then conflicts with its behavior hunks
  (rspec-networking#29, rwr#18), noise both A reviewers flagged; "Fixes the
  flake" in rq#36's description claims nothing (`fix: 0`).
- polyid#107's rename near miss, "renames strip_uuid to create, but
  strip_uuid's definition isn't renamed", where the helper was deleted and
  its calls replaced: a reviewer read it as a dangling reference.
- A rewrite that leaves a call with no arguments (flipper#1124's bare
  `.to_return`, an empty WebMock response sequence) reads like the 72
  other dropped defaults. Every reviewer who found it wanted it flagged.
  Proposal: a `default_argument` near miss that empties the argument list
  says so in its reason.
- A test hunk that removes assertions (polyid#107's eviction test, six
  expectations to one) is the riskiest hunk in a spec refactor and reads
  like any other logic. Reviewers in every shapr arm asked for it. Proposal:
  a facet on a test residual whose assertions (`expect`, `assert*`) drop
  in count.
- A regex quantifier widened (rspec-networking#29, `\h{2}` to `\h{1,2}`)
  has no facet; reviewers wanted "accepts more input" spelled out.

