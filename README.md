# shapr

Classify the shape of a code change. For each hunk in a diff, shapr says whether it is format-only, comment-only, a rename, a move, an extract or inline, or logic, and gives the change an overall verdict: structure, behavior, or mixed (Kent Beck's *Tidy First*: a change should alter structure or behavior, not both).

shapr is a deterministic evidence tool: it parses both sides of every hunk, compares syntax trees, and reports what it found and how sure it is. "Structure" is a shape, not a safety claim.

Status: early development. Ruby is parsed (with [prism](https://github.com/ruby/prism)) and every kind below is detected in it; Rust, JavaScript, TypeScript and Python are to follow. Hunks in other files are `logic`.

## Kinds

| kind | verdict | example | review lines |
|---|---|---|---|
| `format_only` | structure | `def area(x)` becomes `def area( x )` | 0 |
| `comment_only` | structure | `# Area in m2.` becomes `# Area in square metres.` | 0 |
| `rename` | structure | parameter `x` becomes `side`, with every use | 0 |
| `move` | structure | two methods of a class swap places | 0 |
| `move_with_changes` | behavior | a method moves and one of its lines changes | the changed lines |
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
shapr --completions zsh      # a completion script
```

Text output, for a commit that reformats one method's signature and changes another's body (exit 3, mixed):

```
lib/greeter.rb:2   format_only       1.00  same code and comments; whitespace differs
lib/greeter.rb:15  logic             1.00  code differs beyond whitespace and comments
mixed: 2 hunks (format_only 1, logic 1); review 2 lines; comments +0 -0; structure share 0.50
```

Hunks in files shapr can't parse are `logic` with confidence 0. To leave them out, filter the diff: `git diff main -- '*.rb' | shapr`.

## Output

[docs/schema.md](docs/schema.md) describes every field. Highlights:

- per hunk: `kind`, a graded `confidence` with the `evidence` behind it, code and comment lines added and deleted, and `review_lines`, the code lines left to read once formatting, comments, renames, exact moves and the unchanged body of a changed move are discounted (extracts and inlines still count);
- per file: lines before the change, whether it is `generated` (a lockfile, a minified asset, or `linguist-generated`), and the same line counts;
- a summary: the verdict, counts per kind, `structure_share`, and Kamei's diffusion measures (files, directories, subsystems, entropy).

## Exit codes

| code | meaning |
|---|---|
| 0 | structure (or no hunks) |
| 1 | behavior |
| 3 | mixed |
| 2 | error |
| 64 | usage |

## Development

`make check` runs the gate (`cargo fmt --check`, clippy with warnings denied, the tests, docs). See [CLAUDE.md](CLAUDE.md) for the design and layout.
