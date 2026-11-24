# shapr

Classify the shape of a code change. For each hunk in a diff, shapr says whether it is format-only, comment-only, a rename, a move, an extract or inline, or logic, and gives the change an overall verdict: structure, behavior, or mixed. A change that only restructures code is quick to review; one that also alters behavior hides the behavior among the moves, so keep the two apart.

shapr is a deterministic evidence tool: it parses both sides of every hunk, compares syntax trees, and reports what it found and how sure it is. "Structure" is a shape, not a safety claim.

Status: early development. Ruby (with [prism](https://github.com/ruby/prism)) and Rust (with [tree-sitter](https://tree-sitter.github.io/)) are parsed, and every kind below is detected in both; JavaScript, TypeScript and Python are to follow. Hunks in other files are `logic`.

Rust follows its own rules where the languages differ: each file is a module, so a definition moved to another file is a `move_with_changes` (context `module`), not a `move`; doc comments are comments, and attributes are code. [docs/schema.md](docs/schema.md#languages) has the full list.

## Kinds

| kind | verdict | example | review lines |
|---|---|---|---|
| `format_only` | structure | `def area(x)` becomes `def area( x )` | 0 |
| `comment_only` | structure | `/// Area in m2.` becomes `/// Area in square metres.` | 0 |
| `rename` | structure | parameter `x` becomes `side`, with every use | 0 |
| `move` | structure | two methods of a class swap places | 0 |
| `move_with_changes` | behavior | a method moves and one of its lines changes, or moves unchanged into another class | the changed lines, plus the definition's first line when its context changed |
| `extract` | structure | statements become a new method, replaced by a call to it | all code lines |
| `inline` | structure | the reverse of `extract` | all code lines |
| `logic` | behavior | anything else, such as `x * 2` becoming `x * 3` | all code lines |

Extracts and inlines are structure but still count toward review: refactoring bugs cluster there. Lockfiles and generated files count toward the verdict but not toward review lines.

## Usage

```sh
shapr main..HEAD             # a branch's commits
shapr main                   # main against the working tree
git diff --staged | shapr    # a diff on stdin (it needs git's `index` lines)
shapr -J main..HEAD          # NDJSON: a line per file and hunk, then a summary
shapr -j main..HEAD          # one pretty JSON object
shapr -q main..HEAD          # no output; the exit code is the verdict
shapr -C app main..HEAD      # in another repository, like git -C
shapr --message pr.md main   # also read claims from a PR description
shapr --completions zsh      # a completion script
```

With no range, shapr reads a diff from stdin and, like any filter, waits while stdin stays open. An empty stdin is a usage error (64), not an empty verdict, so a failed `git diff` upstream doesn't pass as clean.

Text output, for a commit that reformats one method's signature and changes another's body (exit 3, mixed):

```
lib/greeter.rb:2   format_only       1.00  same code and comments; whitespace differs
lib/greeter.rb:15  logic             1.00  code differs beyond whitespace and comments
mixed: 2 hunks (format_only 1, logic 1); review 2 lines; comments +0 -0; structure share 0.50
```

Hunks in files shapr can't parse are `logic` with confidence 0. To leave them out, filter the diff: `git diff main -- '*.rb' '*.rs' | shapr`.

## Output

[docs/schema.md](docs/schema.md) describes every field. Highlights:

- per hunk: `kind`, a graded `confidence` with the `evidence` behind it, `facets` saying what kind of logic change a `logic` hunk is (`condition`, `signature`, `error_handling`, `literal_only`, `test`, …), code and comment lines added and deleted, and `review_lines`, the code lines a reviewer still has to read (the last column of [Kinds](#kinds));
- per file: lines before the change, whether it is `generated` (a lockfile, a minified asset, or `linguist-generated`), and the same line counts;
- a summary: the verdict, counts per kind and facet, `structure_share`, and Kamei's diffusion measures (files, directories, subsystems, entropy);
- `summary.claims`: what the commit messages and a `--message` description claim (conventional types, purpose words, reverts, issues), with their source, and where the code doesn't bear a claim out. Claims never change a label: shapr reads the code first.

## Exit codes

| code | meaning |
|---|---|
| 0 | structure (or no hunks) |
| 1 | behavior |
| 3 | mixed |
| 2 | error (not a repository, a bad revision, stdin that isn't a diff) |
| 64 | usage (including no input) |

## Development

`make check` runs the gate (`cargo fmt --check`, clippy with warnings denied, the tests, docs). See [CLAUDE.md](CLAUDE.md) for the design and layout.

## References

- Kent Beck, [*Tidy First?: A Personal Exercise in Empirical Software Design*](https://www.oreilly.com/library/view/tidy-first/9781098151232/) (O'Reilly, 2023). Its "Separate Tidying" chapter puts tidyings (structure changes) in their own commits and pull requests, apart from behavior changes. That separation is what shapr checks.
- Kent Beck, [*Software Design: Tidy First?*](https://newsletter.kentbeck.com/), his newsletter, including [Management: Separate Tidying](https://newsletter.kentbeck.com/p/management-separate-tidying).
- Kent Beck, ["for each desired change, make the change easy (warning: this may be hard), then make the easy change"](https://x.com/KentBeck/status/250733358307500032) (2012): why tidying comes first.
- [docs/research.md](docs/research.md): the research on change shape, refactoring detection and risk behind shapr's design.
