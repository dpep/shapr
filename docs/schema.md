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
  stdout; the same one-line message is on stderr too. Under `-q` stdout stays
  empty, error records included: the exit code and stderr carry the error.

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
  comment (`# frozen_string_literal: true`) and a first-line shebang
  (`#!/usr/bin/env ruby`) are code. In a file shapr didn't
  parse, every changed line counts as code.

## Hunk kinds

A hunk is one `@@` section of the diff, narrowed to its changed region:
first changed line to last, with the context between them.

| kind | structure? | meaning |
|---|---|---|
| `format_only` | yes | Same syntax tree before and after, ignoring whitespace and comments, and the same comment text in the same places. |
| `comment_only` | yes | Same syntax tree ignoring whitespace and comments; comment text differs, or a comment moved to another node (a doc comment to another item). |
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
  `send(:old)` is `logic`: it would now raise. A method reached by a computed
  name counts when the name's fixed text could spell it:
  `define_method("get_#{x}")` could be `get_old`, and `public_send(name)` or
  `send("get_" + n)` could be anything, so in a change holding one no method
  is renamed.
- A local isn't renamed in a scope that reads locals by name at runtime
  (`binding`, `eval`, `local_variables`, a string `instance_eval`; an ERB
  template's `result(binding)`), and a method isn't renamed when it reads its
  own name (`__method__`, `__callee__`).
- A keyword parameter keeps its name (callers pass it), a renamed method
  doesn't call `super` (it would reach the parent's method of the new name),
  and global variables aren't renamed.
- Neither name of a method rename is a [reserved name](#reserved-names): the
  language calls those without a call site shapr can see.
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

## Reserved names

Some method names are reached without a call shapr can see, so defining,
removing or renaming a method of that name can change code anywhere: it is
never an `extract`, `inline` or `rename`.

- Ruby: every method of `Object`, `Kernel` and `BasicObject`, public and
  private (`inspect`, `hash`, `to_s`, `initialize`, `method_missing`,
  `respond_to?`, `send`, `puts`, `p`, `format`, `require`, `freeze`, `tap`,
  …), since every object already has them; and the protocol and hook methods
  the runtime calls by convention (`each`, `call`, `to_ary`, `to_str`,
  `to_proc`, `coerce`, `marshal_dump`, `included`, `inherited`,
  `method_added`, `const_missing`, …).
- Rust: `main`, and the methods of std traits that syntax or the prelude
  reaches (`drop`, `clone`, `fmt`, `to_string`, `eq`, `cmp`, `hash`, `next`,
  `deref`, `index`, `add` and the other operators, `from`, `into`, `default`,
  …): an inherent method of one of these names wins over the trait's.

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
  comments, so editing one, or turning a `//` comment into a `///` doc, is
  `comment_only`. A doc comment's fenced code is a doctest, which `cargo test`
  compiles and runs: its fence lines (info string included) and the lines
  between them are code, as is a `/** */` block holding a fence. Attributes
  (`#[...]`, `#![...]`, `#[doc = "..."]`) are code, and belong to the item
  after them.
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
  renamed. Likewise an item, or a method of an `impl`, under an attribute
  that may read its name (`#[no_mangle]`, `#[export_name]`, `#[pyfunction]`,
  `#[wasm_bindgen]`, a route, any attribute macro, a derive beyond that list)
  isn't renamed; `allow`, `cfg`, `cold`, `deny`, `deprecated`, `doc`,
  `expect`, `forbid`, `ignore`, `inline`, `must_use`, `non_exhaustive`,
  `repr`, `rustfmt::skip`, `should_panic`, `test`, `track_caller` and `warn`
  don't read names.
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
| `detail` | object | Kind-specific facts about the change, such as rename pairs. Empty (`{}`) unless the kind's section documents it. A `logic` hunk that resembles a structural kind it couldn't be proven to be carries `near` (see [Extract and inline](#extract-and-inline) and [Confidence](#confidence)). |
| `facets` | string array | What kind of logic change a `logic` hunk is, in the table's order; empty for every other kind. See [Facets](#facets). |

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
| `risk` | string | `"jump"`: the chunk holds code bound to its method or its place: `return`, `break`, `next`, `redo`, `retry`, `yield`, `super`, `block_given?` or a blockless `proc`/`lambda`/`Proc.new` (control flow and the method's block); `binding`, `eval`, `local_variables` or a string `instance_eval`/`class_eval` (the method's locals by name); `__method__`, `caller`, `caller_locations`, `$~`, `$_` or a match reference (`$1`, `$&`) (the method's frame); `__LINE__` or Rust's `line!()`/`column!()` (the code's line); and in Rust `?`, `.await`, `break`, `continue` or a macro outside std. Also when the caller reads its frame anywhere (`binding`, `$1`): the chunk's locals and matches leave it, or arrive. `"block_parameter"`: it uses the caller's `&block`. `"wiring"`: the arguments aren't exactly the chunk's free variables in parameter order, the chunk assigns a local of the caller, or a value used after the chunk isn't returned. `"collision"`: the method's name is a [reserved name](#reserved-names), or already appears, in any file of the change, on the side where the method doesn't exist (a def, a call, a `send`, a plain literal such as `:name`, as a `method_missing` compares): defining it would capture that use, removing it would break it; or another def of the name is on the method's own side, so the method takes part in dispatch beyond its one call. `"drop"` (Rust): code follows the chunk in its block and the chunk binds a local other than the returned one, or takes arguments, so a value would be dropped at another point. `"tiny"`: under 8 nodes. |

shapr sees only the files in the diff: a subclass override or a caller elsewhere of the new or removed method is invisible to it.

## Facets

A `logic` hunk's `facets` say what kind of logic change it is: facts the code
shows, never what a commit message claims. They don't change `kind` or the
verdict, and a hunk can carry several or none. Path facets apply to any file;
the others need both sides parsed cleanly (`evidence.method` `"ast"`).

| facet | meaning | how it is found |
|---|---|---|
| `test` | Test code. | The path is under a `spec`, `test`, `tests` or `__tests__` directory, or named `*_spec.rb`, `*_test.rb`, `test_*.rb`, `*_test.rs`, `tests.rs`; or every changed code line lies inside a Rust `#[test]` (`#[tokio::test]`, `#[rstest]`) fn or `#[cfg(test)]` mod. |
| `manifest` | A dependency manifest. | By name: `Gemfile`, `gems.rb`, `*.gemspec`, `Cargo.toml`, and the manifests beside the other [lockfiles](#generated-files) (`package.json`, `go.mod`, `pyproject.toml`, `Pipfile`, `Podfile`, `composer.json`, `mix.exs`, `pubspec.yaml`, `Package.swift`, `flake.nix`). |
| `adds_definition` | A method, class or module that didn't exist. | A definition the hunk touches whose name, qualified by its enclosing definitions, the other side of the file doesn't define. A Rust `impl` is named by its header. |
| `removes_definition` | A method, class or module that no longer exists. | The same, before side. A method renamed and edited both adds and removes. |
| `signature` | An existing method's interface. | A method on both sides, touched by the hunk, whose parameter list or annotations (Rust: return type, generics, visibility) differ, whitespace aside. |
| `condition` | What the code branches on. | A conditional (`if`, `unless`, `while`, `until`, `case`, `when`, `in`; Rust `if`, `while`, `match`, a match arm) whose test sits on a changed line, or a comparison or boolean operator (`==`, `<`, `>=`, `&&`, `and`, `!`, …) whose operator does, is on one side and not the other: added, removed, or testing something else. A line added inside a branch's body is not a condition change. |
| `error_handling` | How errors are raised, propagated or recovered. | An error construct with its own syntax on a changed line is on one side and not the other, compared by kind, name, operator text and the classes it names. Ruby: `raise`/`fail`, `rescue` (clause and modifier), `ensure`. Rust: `?`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, `bail!`, `ensure!`, `anyhow!`, `Err(…)` built or matched, and `unwrap`, `expect`, `unwrap_err`, `expect_err`, `map_err`, `ok_or`, `ok_or_else`, `unwrap_or`, `unwrap_or_else`, `unwrap_or_default`, `context`, `with_context`. Changing an argument under a `?` is not. |
| `literal_only` | Only literal values changed: config-like. | The changed regions are equal once every literal (number, string, symbol, boolean, nil, regexp; a string's text inside an interpolation) and comment is masked, and hold the same number of literals. A symbol naming code (`send(:x)`, see `DynamicName` in the rename rules) is not a literal here. |

Code inside a definition the hunk adds or removes doesn't add `condition` or
`error_handling`: `adds_definition` already says all of it is new.

Facets describe; they don't judge intent. A fix and a feature both change
conditions. Claims about intent come from messages, kept apart from the code's
facts.

## Claims

`summary.claims` holds what the messages around the change *claim*: the
commit messages of the range (`git log` over the commits `git diff RANGE`
spans: `A..B` and `A...B` read the commits in B and not A, a single revision
`R` reads `R..HEAD`) and the description given with `--message FILE` (a PR
body; `-` reads stdin when a RANGE is given). A piped diff has no commit
messages. For a GitHub pull request (see [GitHub input](#github-input)) the
commit messages are the pull request's commits as the GitHub API lists them
(up to 250) and the description is its title (the subject) and body, unless
`--message` gives one, which replaces it; for a GitHub commit, its message.
Claims are read by fixed rules, never change a hunk's `kind`,
`facets` or the verdict, and carry their source, so a consumer can weigh them
against the code rather than instead of it.

| field | type | meaning |
|---|---|---|
| `messages` | int | Messages read: one per commit, plus one for the description. |
| `hints` | array | Each hint, in message order, once per message. Below. |
| `categories` | object | Messages carrying at least one hint of each category, every category present: `fix`, `feature`, `refactor`, `perf`, `test`, `docs`, `chore`, `revert`. |
| `conflicts` | array | Claims the code doesn't bear out. Below. |

Each hint:

| key | type | meaning |
|---|---|---|
| `hint` | string | `type`: a conventional-commit type (`fix:`, `feat(api):`, known types only). `keyword`: a purpose word in the subject (`fix`, `bug`, `regression`, `crash`, `hotfix`; `add`, `introduce`, `support`, `implement`, `allow`; `refactor`, `rename`, `move`, `extract`, `tidy`, `cleanup`, `simplify`; `perf`, `faster`, `optimize`; `test`, `spec`, `coverage`; `docs`, `readme`, `typo`, `changelog`; `chore`, `bump`, `deps`, `upgrade`, `release`, `lint`; `revert`). `revert`: a git revert's `Revert "subject"` (the quoted subject, whose own words are not read as claims) or its `This reverts commit <id>`. `issue`: `#12`, `KEY-12` (standards such as `UTF-8` aside), or an issue or pull URL (as `#12`). `breaking`: `type!:` or a `BREAKING CHANGE:` footer. `no_behavior_change`: "no behavior change", "no functional change", "no logic change", "behavior preserving", "pure refactor" (any case, hyphens as spaces) or the word `NFC`. |
| `value` | string | What matched, lowercased for types and keywords. |
| `category` | string or null | The category the hint claims: `feat` is `feature`, `style` is `refactor`, `build` and `ci` are `chore`, `no_behavior_change` is `refactor`, an issue after `fix`/`fixes`/`fixed` is `fix`; `null` for `breaking` and other issues. |
| `source` | string | `commit`, `message` (`--message`), or `pull_request` (a GitHub pull request's title and body). |
| `commit` | string or null | The commit's id, 12 hex digits; `null` for a description. |
| `field` | string | `subject` (the first non-empty line) or `body`. Purpose words are read only in the subject, where they summarize; a body mentions too much in passing. |

Each conflict is a count of what the code shows, not a verdict on the author:

| key | type | meaning |
|---|---|---|
| `claim` | string | The claimed category. |
| `finding` | string | `behavior_hunks`: the claim is `refactor` or `docs`, no message claims `fix`, `feature` or `perf`, and shapr couldn't prove `count` compared hunks structural. `non_test_behavior_hunks`: the same for a `test` claim, counting behavior hunks without the `test` facet. `no_behavior_hunks`: the claim is `fix`, `feature` or `perf` and every hunk is structure. |
| `count` | int | Behavior hunks behind the finding (0 for `no_behavior_hunks`). Hunks in files shapr didn't compare (`evidence.method` `"none"`) aren't counted: their `logic` is unknown, not evidence. |

"Couldn't prove structural" is not "changes behavior": a `refactor` claim with
behavior hunks may be a refactoring shapr can't recognize. The conflict says
where to look.

## Confidence

Confidence is computed from the evidence, not assigned per kind.

- `method` `"ast"`: 1 for `format_only`, `comment_only` and `logic`. Tree
  comparison is exact, so it settles those three. `rename`, `move`,
  `move_with_changes`, `extract` and `inline` compute their own score: see
  [Renames](#renames), [Moves](#moves) and
  [Extract and inline](#extract-and-inline).
- `method` `"line"`: text equality can't see a change in how whitespace
  parses (`puts -x` and `puts - x`), so it never proves structure. A hunk
  whose code text matches is `logic` with confidence 0 and `detail.near`
  naming the kind it resembles, `"format_only"` or `"comment_only"`. One
  whose code text differs is `logic` at most **0.5** (the line cap), falling in
  proportion to the region's lines that hold a syntax error: `0.5 × (1 − lines
  with an error / lines in both regions)`.
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
| `visibility` | A method in a different visibility section: it crossed a bare `private`, `protected`, `public` or `module_function`. A constant or class isn't affected by one. |
| `load_order` | A different order relative to a sibling that names it or that it names at load time (`alias_method`, `private :x`, a redefinition, a constant read, a Rust `macro_rules!` of a macro it invokes) or, for anything but a method, any statement that runs code at load time (a `require`, a DSL call, a constant whose value calls something: `X = Counter.next`). |
| `directives` | Across files, different file-level directives (`# frozen_string_literal`). |
| `position` | The unit reads its own line (`__LINE__`, Rust's `line!()`), or its own file (`__FILE__`, `__dir__`, `require_relative`, Rust's `file!()`) and moved to another file. |

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
| `facets` | object | `logic` hunk count per facet (see [Facets](#facets)), every facet present; a hunk with two facets counts under both. |
| `files` | int | Files changed (Kamei's NF). |
| `directories` | int | Distinct directories holding them (ND); the repo root counts as one. |
| `subsystems` | int | Distinct top-level directories (NS); files at the root count as one. |
| `entropy` | number | Kamei's change entropy, −Σ pₖ log₂ pₖ, where pₖ is file k's share of the changed lines (`added + deleted`), over files that aren't generated. 0 for one file. |
| `added`, `deleted`, `code_added`, `code_deleted`, `comment_added`, `comment_deleted`, `review_lines` | int | Sums over the hunks that aren't generated. |
| `behavior_lines` | int | `review_lines` summed over hunks of behavior kinds (`logic`, `move_with_changes`): the behavior change's size, comments and unchanged moved lines excluded. |
| `structure_share` | number 0–1 | Changed lines (`added + deleted`) in structural hunks over all changed lines, generated hunks left out; 0 when nothing changed. |
| `claims` | object | What the commit messages and `--message` description claim, kept apart from the code's facts. See [Claims](#claims). |
| `source` | object or null | The GitHub pull request or commit analyzed (see [GitHub input](#github-input)); `null` for a local range or stdin. |

## GitHub input

RANGE may name a GitHub pull request or commit instead of local revisions:

| form | names |
|---|---|
| `https://github.com/OWNER/REPO/pull/N`, with anything after it (`/files`, `/commits`, a query, a fragment) | pull request N |
| `github.com/OWNER/REPO#N`, `OWNER/REPO#N` | pull request N |
| `#N` | pull request N of the current repository's GitHub remote: `origin` when it is on github.com, else the only remote that is; several and none of them `origin` is a usage error |
| `https://github.com/OWNER/REPO/commit/SHA`, `OWNER/REPO@SHA` (7 to 40 hex digits) | that commit |
| `https://github.com/OWNER/REPO/pull/N/commits/SHA` | that commit alone |

A form without `://` that also resolves as a local revision (a branch named
`acme/widgets#12`) is the local revision. Something shaped like a GitHub
reference that isn't one of these forms is a usage error (64) listing them.

A pull request is diffed from the merge base of its base and head commits
to its head, `git diff BASE...HEAD`, the diff GitHub's Files tab shows. A
commit is diffed against its first parent; a root commit against the empty
tree.

`source`:

| field | type | meaning |
|---|---|---|
| `kind` | `"pr"` or `"commit"` | What was named. |
| `url` | string | Its page on github.com. |
| `repo` | string | `OWNER/REPO`. |
| `number` | int or null | The pull request's number; for a commit named inside a pull request, that pull request's; else null. |
| `title` | string | The pull request's title, or the commit message's first line. |
| `base` | string | The full commit id the diff starts from: the merge base for a pull request, the first parent for a commit (the empty tree's id for a root commit). |
| `head` | string | The full commit id the diff ends at. |
| `merge` | bool | The commit is a merge, diffed against its first parent only. |

Metadata comes from the GitHub REST API: a pull request and the comparison
of its base and head (two requests), or the commit (one). The token is
`GH_TOKEN`, else `GITHUB_TOKEN`, else `gh auth token` when gh is installed and
logged in, else none (public repositories only, at GitHub's unauthenticated
rate limit). The token is never written to output or errors.

Objects are fetched with git's own credentials into a cache of bare partial
clones, `<cache>/github.com/<owner>/<repo>.git`: only the two commits, their
trees, and the blobs the diff reads. The current repository is never
written to. The cache is `--cache-dir`, else `SHAPR_CACHE_DIR`, else
`$XDG_CACHE_HOME/shapr`, else `~/.cache/shapr`; a relative path is a usage
error. A second run on the same commits fetches nothing. `SHAPR_GITHUB_API_URL`
and `SHAPR_GITHUB_GIT_URL` replace `https://api.github.com` and
`https://github.com`.

## `cache` record

`--status` and `--clear-cache` print the cache as it stands afterwards; one
object under `-j` or `-J`.

| field | type | meaning |
|---|---|---|
| `record` | `"cache"` | |
| `path` | string | The cache directory, with the home directory as `~`. |
| `repositories` | array of strings | `github.com/OWNER/REPO` per cached clone, sorted. |
| `bytes` | int | Their size on disk. |

`--clear-cache` removes `<cache>/github.com` and nothing else in the
directory.

## Exit codes

| code | meaning |
|---|---|
| 0 | `structure` or `empty` |
| 1 | `behavior` |
| 3 | `mixed` |
| 2 | operational error: git failed (not a repository, a bad revision), stdin unreadable, stdin holds no diff (no file header), or GitHub failed (unreachable, not found, rate limited, a fetch refused) |
| 64 | usage: a bad command line, no input (no range and an empty or blank stdin), a malformed GitHub form, an ambiguous `#N`, or a relative cache directory |

An empty diff from a range (`shapr main..main`) is the verdict `empty`, exit 0.
An empty stdin is not: it is what a failed `git diff` upstream of the pipe
leaves, so it exits 64. A diff whose only change is a rename or mode change
has file headers and no hunks, and is `empty` too.
