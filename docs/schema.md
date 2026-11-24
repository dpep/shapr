# Output schema

shapr's JSON is a public contract: field names and the values of closed
vocabularies (`record`, `kind`, `verdict`, `evidence.method`) change only with
a bump of `schema` in the summary record. New fields may be added without a
bump; consumers must ignore fields they don't know.

Schema version: **1**.

## Formats

- `-J` / `--ndjson` (the primary machine format): one compact object per
  line. For each file, its `file` record, then its `hunk` records; last, one
  `summary` record.
- `-j` / `--json`: one pretty object, `{"files": [...], "hunks": [...],
  "summary": {...}}`. Each element is the same object as its NDJSON line. A
  single document can't stream, which is why both formats exist.
- An operational or usage error under either format is one
  `{"record": "error", "error": "<message>", "code": <exit code>}` object on
  stdout; the message is on stderr too.

Every field below is always present. Fields that can be `null` say so.

## Conventions

- Lines are 1-based. A line run is an object with `start` and `count`,
  covering lines `start` through `start + count - 1`. A `count` of 0 is an
  empty run sitting immediately before line `start` (where an insertion
  lands, or where a deletion was).
- Paths are repo-relative, as the diff names them.
- Scores are rounded to 2 decimals where they are computed.
- Line counts: a line *carries code* when it holds a non-whitespace byte
  outside any comment, and *carries a comment* when it holds comment text. A
  line can do both (`x = 1 # note`), and a blank line does neither. A magic
  comment (`# frozen_string_literal: true`) is code. In a file shapr didn't
  parse, every changed line counts as code.

## Hunk kinds

A hunk is one `@@` section of the diff, narrowed to its changed region:
first changed line to last, with the context between them.

| kind | structure? | meaning |
|---|---|---|
| `format_only` | yes | Same syntax tree before and after, ignoring whitespace and comments, and the same comment text. |
| `comment_only` | yes | Same syntax tree ignoring whitespace and comments; comment text differs. |
| `rename` | yes | Only identifiers change, and every use of each old name in its scope became the new name. Any parameter added, removed, reordered or given a default is `logic`. See [Renames](#renames). |
| `move` | yes | Code removed in one place appears unchanged elsewhere, in the same file or another. Reordering methods within a class or module is a `move`; reordering anything else (callbacks, validations, `include`s, other DSL calls) is `logic`. See [Moves](#moves). |
| `move_with_changes` | no | A move whose body also changed, or whose body is unchanged but now runs among different code (another owner, visibility section, load order, file-level directives, or Rust module): read the changed lines and where it landed. See [Moves](#moves). |
| `extract` | yes | Statements leave one method and become the body of a new method in the same class, replaced by a call to it. See [Extract and inline](#extract-and-inline). |
| `inline` | yes | The inverse of `extract`: a method and its only call disappear, and its body takes the call's place. |
| `logic` | no | Anything else. The default: a hunk shapr can't prove structural is `logic`. |

"Structure" describes the shape of a change, not its risk: extracts and
inlines in particular are linked to later bug fixes and still deserve review.

Tree equality is exact: node kinds, child order, and every non-whitespace
byte of each node's source must match, and leaves (string content, heredoc
bodies, symbols, numbers) match byte for byte, whitespace included. So
`do…end` versus `{…}`, a change of quote style, a trailing comma in Ruby, or
any reordering is `logic`.

## Renames

A hunk is a `rename` when its two sides align node for node (same kinds,
roles and child order, same source text apart from names and whitespace)
and every name that differs passes these checks:

- Each old name becomes one new name, and each new name comes from one old
  name, within the scope.
- The old name's definition is renamed (a call or reference renamed alone
  points at something else).
- Nothing in the scope still uses the old name after the change, and nothing
  used the new name before it. For a local, a bare method call of that name
  counts; for a method, constant or field, so does a dynamic reference
  (`send(:old)`, `const_get`, and for a field an `attr_*` of the same name)
  and any plain literal spelling the name (`:old`, `"old"`, `old:`), since
  DSLs such as callbacks and `autoload` name code that way. A leftover
  `send(:old)` is `logic`: it would now raise.
- A keyword parameter keeps its name (callers pass it), a renamed method
  doesn't call `super` (it would reach the parent's method of the new name),
  and global variables aren't renamed.
- A renamed dynamic reference (`send(:old)` → `send(:new)`) follows a method,
  constant or field rename that passes.

Scopes: a `local` (variable or parameter) lives in its enclosing method,
class or module; a `field` in its file when it is a Ruby instance or class
variable, and in every parsed file of the change when it is a Rust struct
field; a `method` or `constant` (class and module names included) in every
parsed file of the change.

`detail`:

| key | type | meaning |
|---|---|---|
| `renames` | array | One object per rename the hunk makes. |
| `from`, `to` | string | The old and new name. |
| `kind` | string | `local`, `field`, `method` or `constant`. |
| `occurrences` | int | Uses renamed across the change, the definition included; a class or module name counts once. |

`evidence`, beside `method`:

| key | type | meaning |
|---|---|---|
| `renamed` | int | Sum of the hunk's renames' `occurrences`. |
| `uses` | int | Uses of the old names shapr saw in their scopes before the change. More than `renamed` when another hunk deleted some. |
| `unseen_uses` | bool | Some name can be used in code shapr doesn't see (fields, methods, constants: other files, subclasses, templates). |

`confidence` is the weakest rename's score. A local's is `renamed / uses`.
Where uses can be unseen, the unseen ones are smoothed in as one unknown
(Laplace's rule of succession): `(renamed + 1) / (uses + 2)`, so a method
renamed with two call sites scores 0.8, and one with none 0.67.

## Generated files

A file is `generated` when a tool writes it and the change that matters is
reviewed elsewhere:

- a lockfile, by name in any directory: `Cargo.lock`, `Gemfile.lock`,
  `Package.resolved`, `Pipfile.lock`, `Podfile.lock`, `bun.lock`,
  `composer.lock`, `flake.lock`, `go.sum`, `mix.lock`,
  `npm-shrinkwrap.json`, `package-lock.json`, `pnpm-lock.yaml`,
  `poetry.lock`, `pubspec.lock`, `uv.lock`, `yarn.lock`;
- a minified asset: `*.min.js`, `*.min.css`;
- a path `.gitattributes` marks `linguist-generated`. The attribute wins
  either way: `-linguist-generated` (or `=false`) unmarks a lockfile. It is
  read as `git check-attr` sees it, from the working tree's
  `.gitattributes`, not the diffed revisions'.

A generated file's hunks are classified like any other, so a lockfile
(unparsed) is `logic` and a dependency change sways the verdict to
`behavior` or `mixed`. They don't count toward size: their `review_lines` is
0, and the summary's line sums, `behavior_lines`, `entropy` and
`structure_share` leave them out. Their own `added`, `deleted` and other
line counts stay as the diff has them. Generated-but-reviewed files such as
a Rails `db/schema.rb` are not on the list; mark them in `.gitattributes`
if they should be.

## Languages

`language` is `"ruby"` for `.rb`, `.rake`, `.gemspec`, `.ru`, `Gemfile` and
`Rakefile` (parsed with prism) and `"rust"` for `.rs` (parsed with
tree-sitter). Any other file is `null` and its hunks `logic`. The rules above
are stated in Ruby's terms; for Rust:

- Comments: `//`, `/* */` and doc comments (`///`, `//!`, `/** */`) are
  comments, so editing one is `comment_only`. Attributes (`#[...]`, `#![...]`,
  `#[doc = "..."]`) are code, and belong to the item after them.
- A trailing comma (before a closing bracket, or ending a match arm) never
  changes the parse, so adding or dropping one is formatting, as rustfmt
  does when it reflows a list. A one-element tuple's comma is code.
- A macro's arguments can't be parsed: each token is compared on its own
  (`a << b` and `a < < b` differ), and an identifier in them, or a `{name}`
  in one of their string literals, may mean anything. A rename leaving the
  old name there, or one whose new name was already there, is `logic`; one
  renaming it along with a local or a function passes. A macro outside std
  (`bail!`, `ensure!`) may hide a `return`, so a chunk holding one is risk
  `jump`, as is `?` or `.await`.
- Renames: a `field` is a struct field, used from any file. Fields and
  enum variants of a type deriving anything but `Clone`, `Copy`, `Default`,
  `Eq`, `Hash`, `Ord`, `PartialEq` or `PartialOrd` (`Debug` prints the names,
  serde reads them), or carrying another attribute that may read them, aren't
  renamed.
- Moves: a unit is a function, struct, enum, union, trait, `impl`, module,
  constant, static or type alias. Each file is its own module, so a
  definition moved to another file is a `move_with_changes` with context
  `module`. Struct fields, enum
  variants and `macro_rules!` don't move. An `impl` is its methods' owner,
  told apart from another by its header.
- Extract and inline: a `self` parameter rides on the call's receiver, and
  parameter types are allowed. Values are dropped when their block ends, so
  moving statements into a function can move a drop: risk `drop` (see
  [Extract and inline](#extract-and-inline)).

## `file` record

| field | type | meaning |
|---|---|---|
| `record` | `"file"` | |
| `file` | string | Path after the change; the old path for a deleted file. |
| `old_file` | string or null | Path before the change; `null` for an added file. Differs from `file` on a rename. |
| `language` | string or null | The frontend that parsed it (`"ruby"` or `"rust"`, see [Languages](#languages)); `null` when none did. |
| `parsed` | bool | Every hunk in the file was compared as trees (`evidence.method` `"ast"`). |
| `generated` | bool | A lockfile or generated file. See [Generated files](#generated-files). |
| `lines_before` | int or null | Lines in the file before the change (Kamei's LT); 0 for an added file; `null` when its content couldn't be read. |
| `hunks` | int | Hunk records that follow for this file. |
| `added`, `deleted`, `code_added`, `code_deleted`, `comment_added`, `comment_deleted`, `review_lines` | int | Sums over the file's hunks, generated or not. |

## `hunk` record

| field | type | meaning |
|---|---|---|
| `record` | `"hunk"` | |
| `id` | int | Position in the output, from 0. Kind-specific `detail` refers to other hunks by `id`. |
| `file`, `old_file`, `language` | | As in the `file` record. |
| `parsed` | bool | Compared as trees. `false` when either side has a syntax error (compared as text instead) or the file wasn't parsed at all. |
| `generated` | bool | The hunk's file is generated: its `review_lines` is 0 and the summary's size sums leave it out. See [Generated files](#generated-files). |
| `before`, `after` | line run | The changed region on each side. |
| `kind` | string | See [Hunk kinds](#hunk-kinds). |
| `confidence` | number 0–1 | How strongly the evidence supports `kind`. See [Confidence](#confidence). |
| `reason` | string | One short human-readable sentence. Not a contract: don't match on it. |
| `evidence` | object | What the label rests on. Always has `method`; other keys depend on the kind. See [Evidence](#evidence). |
| `added`, `deleted` | int | `+` and `-` lines (Kamei's LA and LD for this hunk). |
| `code_added`, `code_deleted` | int | Of those, lines carrying code. |
| `comment_added`, `comment_deleted` | int | Of those, lines carrying a comment, so comment growth is visible. |
| `review_lines` | int | Changed code lines a reviewer still has to read. `format_only`, `comment_only`, `rename` and `move`: 0. `move_with_changes`: the hunk's code lines listed in its `detail.moves` `changed_before` (for a unit it removed) or `changed_after` (for a unit it added), plus, for an entry with a `context`, its `from_line` or `to_line` (the unit's first code line, where the new context shows), so the unchanged body of the move is discounted. `extract`, `inline` and `logic`: `code_added + code_deleted`; extracts and inlines are structure but still need reading. 0 in a `generated` file. |
| `detail` | object | Kind-specific facts about the change, such as rename pairs. Empty (`{}`) unless the kind's section documents it. |

## Evidence

| key | type | when | meaning |
|---|---|---|---|
| `method` | string | always | `"ast"`: both sides parsed cleanly and their trees were compared. `"line"`: a side has a syntax error, so the region's text was compared, whitespace and comments aside. `"none"`: nothing was compared (no frontend for the file, a binary file, or content that couldn't be read). |
| `similarity` | number 0–1 | `method` `"ast"` and the code differs | Dice coefficient over the subtrees of the two sides' changed code: 1 when they share every subtree, 0 when none. |

## Extract and inline

An `extract` or `inline` is proven, not scored: the caller is identical on both sides once the call is swapped for the statements (the *chunk*), the new method's body equals the chunk once its parameters are renamed to the call's arguments, and the methods' container still holds the same other code, in order, apart from siblings that changed on their own. Those siblings' hunks keep their own label; a hunk mixing the refactoring with any other change stays `logic`. The new method's parameters must be plain required positionals, the call's arguments plain local variables, and a value used after the chunk comes back as the call's result (`x = b(...)`). Every hunk the refactoring touches gets the label, or none does.

`detail` on each labeled hunk:

| key | type | meaning |
|---|---|---|
| `method` | string | The method created (`extract`) or removed (`inline`). |
| `caller` | string | The method the chunk left or joined. |
| `hunks` | int array | `id`s of every hunk the refactoring touches, this one included. |

`evidence` keys beside `method`:

| key | type | meaning |
|---|---|---|
| `statements` | int | Statements in the chunk. |
| `nodes` | int | Syntax nodes in the chunk: its size. |
| `params` | int | The method's parameters. |
| `verbatim` | number 0–1 | Share of the chunk's nodes that match the method's body without renaming a parameter. |
| `returns` | bool | The call's result is assigned: a value flows back to the caller. |

Confidence is `size × (1 + verbatim) / 2`, where `size` is `nodes / 16`, capped at 1. A chunk under 8 nodes stays `logic`: equality of a body that small says little, and a one-call method is more often an override hook than a tidying. A renamed node is an inferred correspondence, so it counts half.

A hunk that matches this shape but would change behavior stays `logic` and says why in `detail`, beside `method`, `caller` and `hunks`:

| key | type | meaning |
|---|---|---|
| `near` | string | `"extract"` or `"inline"`. |
| `risk` | string | `"jump"`: the chunk holds `return`, `break`, `next`, `redo`, `retry`, `yield`, `super` or `block_given?`. `"block_parameter"`: it uses the caller's `&block`. `"wiring"`: the arguments aren't exactly the chunk's free variables in parameter order, the chunk assigns a local of the caller, or a value used after the chunk isn't returned. `"collision"`: the method's name already appears, in any file of the change, on the side where the method doesn't exist (a def, a call, a `send`): defining it would capture that use, removing it would break it; or another def of the name is on the method's own side, so the method takes part in dispatch beyond its one call. `"drop"` (Rust): code follows the chunk in its block and the chunk binds a local other than the returned one, or takes arguments, so a value would be dropped at another point. `"tiny"`: under 8 nodes. |

shapr sees only the files in the diff: a subclass override or a caller elsewhere of the new or removed method is invisible to it.

## Confidence

Confidence is computed from the evidence, not assigned per kind.

- `method` `"ast"`: 1 for `format_only`, `comment_only` and `logic`. Tree
  comparison is exact, so it settles those three. `rename`, `move`,
  `move_with_changes`, `extract` and `inline` compute their own score: see
  [Renames](#renames), [Moves](#moves) and
  [Extract and inline](#extract-and-inline).
- `method` `"line"`: at most **0.5** (the line cap): text equality can't see a
  change in how whitespace parses. It falls in proportion to the region's lines
  that hold a syntax error: `0.5 × (1 − lines with an error / lines in both
  regions)`.
- `method` `"none"`: 0. The `logic` label is the safe default, not a finding.

## Moves

A *unit* is a method, class, module or constant definition of at least 3
syntax nodes, sitting directly in a file or a class or module body (not in
a block or conditional). Each file's trees are aligned body by body, larger
nodes first; a unit the alignment leaves out was removed or added. A removed unit pairs with an
added one, in the same file or another, by equal tree (`move`) or, failing
that, by equal name and kind with `similarity` at least 0.5
(`move_with_changes`). Definitions align by name, so one that keeps its
place among its siblings was edited, not moved.

A pair is a `move_with_changes`, even with an unchanged body, when the code
around its new place differs. Its `context` names each difference:

| context | meaning |
|---|---|
| `module` | Rust, across files: each file is a module, so the paths it resolves and its own path change. |
| `owner` | A different enclosing class, module or `impl` (a Rust namespace's attributes count: `#[cfg(unix)] mod imp` is not `#[cfg(windows)] mod imp`). |
| `visibility` | A different visibility section: it crossed a bare `private`, `protected`, `public` or `module_function`. |
| `load_order` | A different order relative to a sibling that names it or that it names at load time (`alias_method`, `private :x`, a redefinition, a constant read) or, for anything but a method, any statement that runs code at load time (a `require`, a DSL call). |
| `directives` | Across files, different file-level directives (`# frozen_string_literal`). |

The body is relocated, not rewritten, so a reviewer reads where it landed
rather than every line of it; but its meaning can change, so it is never a
`move`. Code that isn't a unit (a DSL call, a definition under the size floor
or inside a block) never pairs: reordering it is `logic`.

A hunk is a move when every changed code line lies in a paired unit or in
code the alignment kept unchanged. Both halves are labeled.

`evidence` keys for `move` and `move_with_changes`:

| key | type | meaning |
|---|---|---|
| `similarity` | number 0–1 | The lowest similarity among the hunk's pairs: Dice coefficient over the two units' subtrees. 1 for `move`. |
| `nodes` | int | Syntax nodes in the smallest unit moved. |
| `candidates` | int | The most units, among the hunk's pairs, that matched equally well; 1 when every match is unique. |

`detail` for `move` and `move_with_changes`:

| key | type | meaning |
|---|---|---|
| `context` | string array | Every `context` among the hunk's entries, in the table's order; empty for a `move`. |
| `moves` | array | One entry per pair touching the hunk, below. |

Each `moves` entry:

| key | type | meaning |
|---|---|---|
| `name` | string or null | The declared name. |
| `from_hunk`, `to_hunk` | int or null | `id` of the hunk removing the unit and of the hunk adding it; equal for a reorder inside one hunk. |
| `from_line`, `to_line` | int or null | The unit's first code line in its old file and in its new file. |
| `context` | string array | What differs around this unit in its new place (see the table above); empty when nothing does. |
| `similarity` | number 0–1 | This pair's similarity. |
| `changed_before`, `changed_after` | int array | Lines, in the removed unit's old file and the added unit's new file, that the other doesn't share, whitespace aside. Empty for an exact move. |

Confidence is `similarity × (1 − 1/nodes) ÷ candidates`, the weakest pair
in the hunk: a match is weaker when it isn't exact, when the unit is small
enough to match by chance, and when several units could be its other half.

## `summary` record

| field | type | meaning |
|---|---|---|
| `record` | `"summary"` | |
| `schema` | int | This schema's version. |
| `verdict` | string | `"structure"`: every hunk is a structural kind. `"behavior"`: none is. `"mixed"`: both. `"empty"`: no hunks. The exit code carries it too. |
| `hunks` | int | Hunk records. |
| `unparsed_hunks` | int | Hunks with `parsed: false`. |
| `generated_hunks` | int | Hunks with `generated: true`. They count in `hunks`, `kinds` and `verdict`, and nowhere below. |
| `kinds` | object | Hunk count per kind, every kind present. |
| `move_contexts` | object | `move_with_changes` hunk count per `context` (see [Moves](#moves)), every context present; a hunk with two contexts counts under both. |
| `files` | int | Files changed (Kamei's NF). |
| `directories` | int | Distinct directories holding them (ND); the repo root counts as one. |
| `subsystems` | int | Distinct top-level directories (NS); files at the root count as one. |
| `entropy` | number | Kamei's change entropy, −Σ pₖ log₂ pₖ, where pₖ is file k's share of the changed lines (`added + deleted`), over files that aren't generated. 0 for one file. |
| `added`, `deleted`, `code_added`, `code_deleted`, `comment_added`, `comment_deleted`, `review_lines` | int | Sums over the hunks that aren't generated. |
| `behavior_lines` | int | `review_lines` summed over hunks of behavior kinds (`logic`, `move_with_changes`): the behavior change's size, comments and unchanged moved lines excluded. |
| `structure_share` | number 0–1 | Changed lines (`added + deleted`) in structural hunks over all changed lines, generated hunks left out; 0 when nothing changed. |

## Exit codes

| code | meaning |
|---|---|
| 0 | `structure` or `empty` |
| 1 | `behavior` |
| 3 | `mixed` |
| 2 | operational error: git failed, input unreadable |
| 64 | usage: a bad command line, or no input |
