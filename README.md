# shapr

Classify the shape of a code change. For each hunk in a diff, shapr lists the structural changes it can prove (format-only, comment-only, a rename, a move, an extract or inline), each with the lines it explains, and the logic lines left over, and gives each hunk and the change an overall verdict: structure, behavior, or mixed (or none and unknown, for changes outside the code that counts). A change that only restructures code is quick to review; one that also alters behavior hides the behavior among the moves, so keep the two apart.

shapr is a deterministic evidence tool: it parses both sides of every hunk, compares syntax trees, and reports what it found and how sure it is. "Structure" is a shape, not a safety claim.

Status: early development. Ruby (with [prism](https://github.com/ruby/prism)) and Rust (with [tree-sitter](https://tree-sitter.github.io/)) are parsed, and every kind below is detected in both; JavaScript, TypeScript and Python are to follow. Hunks in other files are `logic`, and judged by their [category](#verdicts).

Rust follows its own rules where the languages differ: each file is a module, so a definition moved to another file is a `move` that isn't structure (context `module`); doc comments are comments (their doctests are code), and attributes are code. [docs/schema.md](docs/schema.md#languages) has the full list.

## Kinds

Each hunk is a list of parts. A part is one proven change of these kinds, with the lines it covers; the leftover lines no part explains are one `logic` part, so a rename and an edit in one hunk are a `rename` part and the edited line as `logic`, and the hunk is `mixed`.

| kind | verdict | example | review lines |
|---|---|---|---|
| `format_only` | structure | `def area(x)` becomes `def area( x )` | 0 |
| `comment_only` | structure | `/// Area in m2.` becomes `/// Area in square metres.` | 0 |
| `restyle` | structure | `{:a => 'x'}` becomes `{a: "x"}`, or `S { a: a }` becomes `S { a }` | 0 |
| `default_argument` | structure; behavior when the method is a gem's (a subclass outside may override it) | `fetch(x, retries: 3)` becomes `fetch(x)` where `fetch` declares `retries: 3` | 0; all code lines when not proven |
| `rename` | structure; behavior when it renames a library's public API | parameter `x` becomes `side`, with every use | 0; all code lines when not proven |
| `move` | structure; behavior when it lands among different code (another class, past a `private`) | two methods of a class swap places | 0; the definition's first line when it lands among different code |
| `extract` | structure | statements become a new method, replaced by a call to it | all code lines |
| `inline` | structure | the reverse of `extract` | all code lines |
| `logic` | behavior | anything else, such as `x * 2` becoming `x * 3`, or the edited lines of a moved method | all code lines |

A part is structure only when it is proven: a rename's old name is searched for across the repository (`git grep`), and one still used outside the change is `logic`; a part that relies on something shapr can't check (a library's public API, an ambiguous move) lists it in `assumptions` and isn't structure.

A hunk's `kind` is a one-word rollup: `logic` unless every part is structure, else its most substantial part kind in this table's order. Extracts and inlines are structure but still count toward review: refactoring bugs cluster there.

## Verdicts

Each file has a category (`code`, `test`, `docs`, `fixture`, `config`, `manifest`, `generated`, `vendored`, `binary`, `unknown`), and only code, test, config and manifest files count toward the verdict, so a docs edit never makes a change `behavior`. Files under a `fixtures/`, `__fixtures__/` or `testdata/` directory are `fixture`: test data, bucketed and not classified, since the tests that read them judge them:

| change | verdict |
|---|---|
| only structure parts, in files that count | `structure` |
| only logic | `behavior` |
| both | `mixed` |
| a README or fixture edit, whitespace in an unparsed file, or nothing | `none` |
| only files shapr doesn't judge (a lockfile, a binary, an unparsed language) | `unknown` |

A config or manifest edit is `behavior`. A README edit or a lockfile beside a reformat leaves it `structure`; `summary.verdicts` counts the hunks outside the verdict. The full rules are in [docs/schema.md](docs/schema.md#verdicts).

## Usage

Every form is a git command: `shapr X` classifies what `git show X` or `git diff X` shows.

```sh
shapr HEAD                   # the last commit (git show HEAD)
shapr main..HEAD             # a range (git diff main..HEAD); `shapr main HEAD` is the same
shapr main...                # a branch's commits, from where it left main (git diff main...)
shapr --merge-base main      # everything on the branch, committed or not (git diff --merge-base main)
shapr .                      # uncommitted changes, staged and not (git diff HEAD -- .)
shapr --staged               # staged changes only (git diff --cached); --cached is the same
shapr main..HEAD -- lib      # any form, limited to paths
git diff --staged | shapr    # a diff on stdin (it needs git's `index` lines)
shapr -J main..HEAD          # NDJSON: a meta line, a line per file and hunk, then a summary
shapr -j main..HEAD          # one pretty JSON object
shapr -C app main..HEAD      # in another repository, like git -C
shapr --message pr.md main...   # also read claims from a PR description
shapr https://github.com/acme/widgets/pull/12   # a GitHub pull request, as its Files tab shows it
shapr acme/widgets#12        # the same; `#12` uses this repository's GitHub remote
shapr acme/widgets@1a2b3c4   # a GitHub commit, against its first parent
shapr --status               # the cache of GitHub repositories; --clear-cache empties it
shapr norms lib/             # a codebase's style norms, its hotspots and a cleanup plan, no diff needed
shapr --completions zsh      # a completion script
```

| shapr | same as | `meta.input` |
|---|---|---|
| `shapr REV` | `git show REV`: the commit against its first parent | `commit` |
| `shapr A..B`, `shapr A B` | `git diff A B`: compare two commits, from A's tree to B's, so A's own changes aren't in it (`shapr A^ B` includes them) | `range` |
| `shapr A...B`, `shapr main...` | `git diff A...B`: from the merge base | `range` |
| `shapr --merge-base REF [REV]` | `git diff --merge-base REF [REV]`: without REV, the merge base of REF and HEAD against the working tree | `merge_base_worktree`, `merge_base_range` |
| `shapr PATH...`, `shapr .` | `git diff HEAD -- PATH...`: uncommitted changes | `worktree` |
| `shapr --staged [REV]`, `--cached` | `git diff --cached [REV]`: the index against HEAD, or REV | `staged` |
| any form `[--] PATH...` | the same, limited to paths | `meta.paths: true` |
| no arguments | the diff on stdin | `stdin` |

Like `git diff`, a form that ends in the working tree leaves untracked files out: shapr says so on stderr (`1 untracked file not shown: lib/new.rb (git add -N lib/new.rb to include it)`) and counts them in `meta.untracked`.

Arguments are read as git reads them: revisions first, then paths. A name that is both a revision and a path is a usage error asking for `--`: `shapr lib --` is the revision, `shapr -- lib` the path. A merge commit alone is refused with the command for each parent (`REV^1..REV`, `REV^2..REV`).

GitHub input is experimental: its forms, `meta.source` and the cache may change before 1.0. A GitHub pull request or commit needs no checkout: shapr reads its metadata from the GitHub API (with a token from `GH_TOKEN`, `GITHUB_TOKEN` or `gh auth token`, if any) and fetches just the two commits, with your git credentials, into a cache of partial clones under `~/.cache/shapr` (`--cache-dir` or `SHAPR_CACHE_DIR` to move it). Your repository is never written to. The forms and the cache are in [docs/schema.md](docs/schema.md#github-input).

With no arguments, shapr reads a diff from stdin and, like any filter, waits while stdin stays open. An empty stdin is a usage error (64), not an empty verdict, so a failed `git diff` upstream doesn't pass as clean.

Text output, for a commit that reformats one method's signature and changes another's body (mixed). The summary comes first, starting with what was compared (`commit 1a2b3c4`, `main...HEAD`, `uncommitted changes in lib/cart.rb`, `staged changes`, `stdin`), so an empty result names where shapr looked; then each hunk. A line with nothing to say (no claims, no comment changes) is left out:

```
commit 1a2b3c4: mixed: 2 hunks (1 format_only, 1 logic); review 2 lines; structure share 0.50
claims: refactor in 1 of 1 message
unproven: claims refactor; 1 hunk not shown structural

lib/greeter.rb:2   format_only       1.00  same code and comments; whitespace differs
lib/greeter.rb:11  logic             1.00  [literal_only]
                   style Style/StringLiterals: diverging at line 11; class expects single_quotes (0.95: class 2 of 2, directory 12 of 12)
```

Each line is a part: its place, kind, confidence and reason (none for a plain `logic` part), with its facets in brackets. A `style` line says how the hunk's new code sits against the style around it (here a double-quoted string where the class uses single quotes). `claims` reads the commit messages; `unproven` is a claim the code doesn't bear out. Under `-j` the verdict is `.summary.verdict`; under `-J` it is the last line's `.verdict`.

`shapr norms` text output, for a directory where one method breaks the quote style:

```
norms over 5 files in lib
  Style/StringLiterals                 single_quotes 93% (14/15)
  Layout/TrailingWhitespace            no_trailing_whitespace 100% (31/31)
  Layout/IndentationWidth              2 100% (8/8)
hotspots (deviations per 100 lines; norms broken)
  files
    lib/greeter.rb  7.7 (1 in 13 lines; Style/StringLiterals)
  methods
    lib/greeter.rb:10 Greeter#bye  33.3 (1 in 3 lines; Style/StringLiterals)
plan (apply in order; diffs under -j)
  1. tier 2 lexical  Style/StringLiterals -> single_quotes: 1 fix on 1 line in 1 file (93% -> 100%), a restyle change
```

The plan's diffs are at `.steps[].edits[].diff` under `-j`, ready for `git apply`:

```sh
shapr norms -j lib | jq -r '.steps[].edits[].diff' | git apply
```

Hunks in files shapr can't parse are `logic` with confidence 0 in JSON; in a file of unknown category they are outside the verdict (`unknown`). Text output names them by category instead: `README.md:1  docs change (outside the verdict)`, `config/app.yml:3  config change`, `spec/fixtures/cart.rb:1  fixture change (outside the verdict)`, `Makefile:1  unknown file (outside the verdict)`.

## Usage log

Each `classify` or `norms` run appends one JSON line to `$XDG_STATE_HOME/shapr/usage.jsonl` (else `~/.local/state/shapr/usage.jsonl`), so the commands and forms people use can steer what gets built: the time, shapr's version, the command, the input form, the names of the flags given, the output format, the exit code, the duration, and counts (files, hunks, parts per kind and the verdict; for `norms`, files and plan steps). It never records paths, repository names, revisions, code, commit messages or URLs, and nothing leaves your machine.

`SHAPR_USAGE=0` (or `false`, `off`, `no`) turns it off, `1` (`true`, `on`, `yes`) turns it on, and `SHAPR_USAGE=/absolute/path` moves it. `CI=true` turns it off unless `SHAPR_USAGE` is set. `shapr --usage` summarizes it, `shapr --usage --reset` empties it, and `shapr --status` shows where it is and its size. The log keeps its newest half once it passes 5 MB, and a log that can't be written never fails a run.

## Output

[docs/schema.md](docs/schema.md) describes every field. Highlights:

- per hunk: its `parts`, each with a `kind`, the lines it covers, a graded `confidence` with the `evidence` behind it, and for the `logic` part `facets` saying what kind of logic change it is (`condition`, `signature`, `error_handling`, `literal_only`, …); the hunk's `verdict` and `kind`; code and comment lines added and deleted, and `review_lines`, the code lines a reviewer still has to read (the last column of [Kinds](#kinds));
- per file: its `category` (`code`, `test`, `docs`, `fixture`, `config`, `manifest`, `generated`, `vendored`, `binary`, `unknown`), lines before the change, and the same line counts;
- a summary: the verdict, hunk counts per verdict and category, `review_lines` with `test_review_lines` split out, counts of parts per kind and facet, `structure_share`, and Kamei's diffusion measures (files, directories, subsystems, entropy);
- per hunk, experimental: `style`, how its new code sits against the style norms inferred from the code around it (Ruby's, listed in [docs/schema.md](docs/schema.md#style)), with the deviations and a fix for each; deleting code chooses no style;
- experimental, `summary.config_trajectory`: per norm `.rubocop.yml` sets, whether the change adds or removes offenses against it;
- experimental, `shapr norms PATH...`: per norm the style the code follows and its adherence (`single_quotes 87% (412/473)`), how far the code is from its config, the files, classes and methods least like the rest, and a cleanup plan in risk tiers (layout, then restyles, then renames) whose steps are `git apply`-ready diffs, written so each tier's pull request classifies as `format_only` or `restyle` ([docs/schema.md](docs/schema.md#shapr-norms));
- `summary.claims`: what the commit messages and a `--message` description claim (conventional types, purpose words, reverts, issues), with their source; and, experimental, the categories they claim and where the code doesn't bear a claim out. Claims never change a label: shapr reads the code first.

## Exit codes

The verdict is in the output; the exit code says only whether shapr ran.

| code | meaning |
|---|---|
| 0 | success, whatever the verdict |
| 2 | error (not a repository, a bad revision, stdin that isn't a diff) |
| 64 | usage (including no input) |

## CI recipe

A pull request gate that fails a change mixing structure with behavior:

```yaml
- uses: actions/checkout@v4
  with:
    fetch-depth: 0   # shapr needs the base commit, not a shallow clone
- run: script/shape-gate.sh origin/${{ github.base_ref }}
```

```sh
#!/usr/bin/env bash
# script/shape-gate.sh BASE: fail when the change is `mixed`.
set -uo pipefail
out=$(shapr -J "$1...HEAD")   # three dots: the PR's own commits, as GitHub shows them
case $? in
  0) ;;
  64) echo "shape-gate: usage error" >&2; exit 2 ;;
  *) echo "shape-gate: shapr failed" >&2; exit 2 ;;
esac
head -n1 <<<"$out" | jq -e '.record == "meta" and .schema == 1 and .command == "classify"' >/dev/null \
  || { echo "shape-gate: unexpected schema or command" >&2; exit 2; }
verdict=$(tail -n1 <<<"$out" | jq -r .verdict)
echo "shape: $verdict"
[ "$verdict" != mixed ]
```

- Read the schema version and the command from the first record, `meta.schema` and `meta.command`, and stop on one you don't know: a `shapr norms` output has no `verdict`, which `jq` reads as `null`.
- Gate on verdicts, not on lists of kinds: the summary's `verdict`, a hunk's `verdict`, or a part's `structure`. A kind with `structure: false` (a public API rename) is behavior, and new kinds may appear.
- `none` (docs and fixtures only) and `unknown` (lockfiles, unparsed files) are outside the verdict; `summary.verdicts` counts them.
- The exit code is 0 whatever the verdict, 2 when shapr couldn't run, 64 for a usage error. The policy (failing on `mixed`) is the script's.

## Development

`make check` runs the gate (`cargo fmt --check`, clippy with warnings denied, the tests, docs). See [CLAUDE.md](CLAUDE.md) for the design and layout, and [docs/vision.md](docs/vision.md), [docs/roadmap.md](docs/roadmap.md) and [docs/decisions.md](docs/decisions.md) for where shapr is headed and why.

## References

- Kent Beck, [*Tidy First?: A Personal Exercise in Empirical Software Design*](https://www.oreilly.com/library/view/tidy-first/9781098151232/) (O'Reilly, 2023). Its "Separate Tidying" chapter puts tidyings (structure changes) in their own commits and pull requests, apart from behavior changes. That separation is what shapr checks.
- Kent Beck, [*Software Design: Tidy First?*](https://newsletter.kentbeck.com/), his newsletter, including [Management: Separate Tidying](https://newsletter.kentbeck.com/p/management-separate-tidying).
- Kent Beck, ["for each desired change, make the change easy (warning: this may be hard), then make the easy change"](https://x.com/KentBeck/status/250733358307500032) (2012): why tidying comes first.
- [docs/research.md](docs/research.md): the research on change shape, refactoring detection and risk behind shapr's design.
