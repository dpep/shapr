# Output schema

shapr's JSON is a public contract: field names and the values of closed
vocabularies (`record`, `kind`, `verdict`, `category`, `assumptions`,
`evidence.comparison`)
change only with a bump of `schema` in the `meta` record. New fields may be
added without a bump; consumers must ignore fields they don't know. Fields
marked *experimental* may change in any release before 1.0.

Schema version: **1**.

## Formats

- `-J` / `--ndjson`: one compact object per line, for line-oriented
  consumers (`jq -c`, `grep`, a log pipeline). First one `meta` record; then
  for each file, its `file` record and its `hunk` records; last, one
  `summary` record. Classification looks at the whole change at once (a
  move pairs hunks across files), so the lines arrive together at the end of
  the run, not as each file is read.
- `-j` / `--json`: one pretty object, `{"meta": {...}, "files": [...],
  "hunks": [...], "summary": {...}}`. Each element is the same object as its
  NDJSON line.
- An operational or usage error under either format is one
  `{"record": "error", "schema": 1, "error": "<message>", "code": <exit code>}`
  object on stdout; the same one-line message is on stderr too. Under `-q`
  stdout stays empty, error records included: the exit code and stderr carry
  the error.

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

## Parts

A hunk is one `@@` section of the diff, narrowed to its changed region:
first changed line to last, with the context between them.

A hunk is made of *parts*. Each part is a structural change shapr proved,
with the changed lines it explains; the *residual* is a last part, of kind
`logic`, holding exactly the changed lines no proven part explains. Parts
never share a line, and together they hold every changed line. A hunk with
nothing proven is one `logic` part.

| kind | structure? | meaning |
|---|---|---|
| `format_only` | yes | Same syntax tree before and after, ignoring whitespace and comments, and the same comment text in the same places. Beside logic, only changed code lines whose code is unchanged form this part (a reformat); changed comments and blank lines are read with the logic unless they are a run of their own (see [Runs](#runs)). |
| `comment_only` | yes | Same syntax tree ignoring whitespace and comments; comment text differs, or a comment moved to another node (a doc comment to another item). |
| `restyle` | yes | The same program spelled another way: the code differs only in spellings the language's frontend proves equivalent (quote style, hash syntax, block delimiters, redundant parentheses, ...). See [Restyle](#restyle). |
| `rename` | yes, unless it has `assumptions` | Only identifiers change, and every use of each old name in its scope became the new name. Any parameter added, removed, reordered or given a default is `logic`. See [Renames](#renames). |
| `move` | yes, unless its `detail.context` or `assumptions` is non-empty | Code removed in one place appears elsewhere, in the same file or another. Reordering methods within a class or module is a `move`; reordering anything else (callbacks, validations, `include`s, other DSL calls) is `logic`. A moved definition whose body also changed is a `move` part plus the changed lines in the residual. One moved unchanged into different surroundings (another owner, visibility section, load order, file-level directives, or Rust module) is a `move` part that isn't structure. See [Moves](#moves). |
| `extract` | yes, unless it has `assumptions` | Statements leave one method and become the body of a new method in the same class, replaced by a call to it. See [Extract and inline](#extract-and-inline). |
| `inline` | yes, unless it has `assumptions` | The inverse of `extract`: a method and its only call disappear, and its body takes the call's place. |
| `logic` | no | The residual: anything else. A line shapr can't prove structural is `logic`. |

Each part is proven against the whole of both trees, residual included, so
new code in the residual can break a part's proof (a rename whose new name
new code also defines): the part is then not claimed and its lines stay
`logic`.

A part is structure only when it is proven: see [Proof status](#proof-status).

### Runs

Diff context merges edits a few lines apart into one hunk, so a hunk can
hold several *runs*: maximal stretches of changed lines with no context line
between them. A hunk is classified whole first; when it isn't all structure
and holds more than one run, each run is classified again as if it were a
hunk of its own, and the parts proven on its runs join the hunk's parts
where they claim no line those hold. So a reformat or a comment fix a few lines from a
logic edit is its own part, and a reflow git splits into runs, which only
the whole hunk proves, stays one part. Hunks, their ids and their `before`
and `after` runs don't change. A run's `format_only`, `comment_only` or
`rename` part also needs the run's code under the same enclosing syntax on
both sides: code re-indented under a wrapper another run of the hunk adds
(an `if` around it) stays `logic`.

A hunk's `kind` is a one-word rollup of its parts: `logic` unless every
part is structure, else its most substantial part kind in the table's order
(a rename beside a reformat is a `rename`). Its `verdict` also weighs the
file's category: see [Verdicts](#verdicts).

## Verdicts

A verdict answers one question: what does the change do to the code that
counts? Only files of category `code`, `test`, `config` and `manifest`
count. `logic` means "no part proven", which is evidence of a behavior
change only where shapr compared the code; elsewhere the verdict says it
didn't judge.

A hunk's `verdict` is the first that applies:

| verdict | when |
|---|---|
| `unknown` | The file is `binary`, `generated` or `vendored`. |
| `none` | The file is `docs`. |
| `none` | shapr has no parser for the file, and its changed lines differ only in trailing whitespace and blank lines (indentation counts: YAML, Python and Makefiles read it). |
| `structure`, `behavior`, `mixed` | A `code`, `test`, `config` or `manifest` file shapr compared (`evidence.comparison` `"ast"` or `"line"`): `structure` when every part is structure, `behavior` when none is, `mixed` otherwise. |
| `behavior` | A `config` or `manifest` file shapr has no parser for: its text changed beyond whitespace, and that text is the setting. |
| `unknown` | Anything else: an `unknown` file, or `code` or `test` whose content couldn't be read (`evidence.comparison` `"none"`). |

The change's `verdict` rolls up the hunks whose verdict is `structure`,
`behavior` or `mixed`: `structure` when every part among them is structure,
`behavior` when none is, `mixed` otherwise. Hunks whose verdict is `none` or
`unknown` don't sway it. With no such hunks, it is `unknown` when some hunk
is `unknown`, else `none`. So:

- a README-only change is `none`, and so is an empty diff;
- a README edit beside a reformat stays `structure`;
- a config change is `behavior`;
- a lockfile-only change is `unknown`, and a lockfile beside a reformat is
  `structure`: check `summary.verdicts.unknown` for what wasn't judged.

"Structure" describes the shape of a change, not its risk: extracts and
inlines in particular are linked to later bug fixes and still deserve review.

Tree equality is exact: node kinds, child order, and every non-whitespace
byte of each node's source must match, and leaves (string content, heredoc
bodies, symbols, numbers) match byte for byte, whitespace included. So
`do…end` versus `{…}`, a change of quote style or a trailing comma in Ruby
is not `format_only`; it is a `restyle` where the trees prove it the same
program, and any reordering is `logic`.

## Proof status

A part counts as structure only when nothing it relies on is left
unchecked: its `assumptions` are empty. An operation that reaches code
outside the change (a renamed method's callers, the name an extract adds or
an inline removes) is checked against the repository at the after revision
(the range's end, the fetched head of a GitHub change, or the working tree
when the change ends there; for a diff on stdin, see below): shapr searches its files for the old name as a
whole word (`git grep -w`), then parses each file found and applies the same
rule as to the files in the change (a use, a dynamic reference, a plain
literal such as `:old`), so the word in a comment or a sentence doesn't
count; a file it can't parse cleanly (a template) counts. A file outside the
change that still uses the name makes the operation `logic`, and the
residual's `reason` names the file.
What the search can't settle stays on the part:

| assumption | meaning |
|---|---|
| `unseen_uses` | The repository couldn't be searched (a diff on stdin outside a repository or not ending in the index or the working tree, a failed search), or holds the name in a file shapr can't parse for a use (YAML, JavaScript; see below), so code outside the change may still use the name. |
| `search_capped` | The repository holds more than 20,000 files of the language: they aren't searched. |
| `public_api` | The name is part of a library's public surface, so callers outside this repository can break: a renamed or inlined public definition in a gem's `lib/` (beside a `*.gemspec`), or a `pub` item (or a `pub` trait's item or `pub` enum's variant) in a Rust library crate (a `[lib]` target or `src/lib.rs`, not `publish = false`, outside its binaries, tests, examples, benches and build script). A Ruby method is public unless `private` (a `private` section, `private def`, `private :name`); protected counts. A private module's `pub` items count, as they may be re-exported. Instance variables never do. |
| `ambiguous_match` | A `move` whose unit matched more than one other unit equally well (`evidence.candidates` above 1): which one it came from is a guess. |

A part with assumptions is reported with `structure: false` and its lines
to read: every code line for a `rename`, `extract` or `inline`, each moved
definition's first code line for a `move`. Its `reason` says why (`public
API rename: ...`). There is no separate verdict for assumed structure: such
a hunk is `behavior` or `mixed`.

The search reads the files a language's code can reach a name from (Ruby:
`.rb`, `.rake`, `.gemspec`, `.ru`, ERB, Haml, Slim and builder templates,
`Gemfile`, `Rakefile`; Rust: `.rs`): tracked files, and when the change ends in the working tree its untracked files too (not ignored ones). A use is matched
by name, not resolved: a common name used for something else (another
class's method or instance variable of the same name, `Rack::Test::Methods`
for a constant `Methods`) blocks the operation. A method's name is searched
without its `?`, `!` or `=`. A file that reaches a method by a name built
at runtime (`send(n)`, `public_send("get_#{k}")`, `define_method`) is read
too, once per run (every file of the language is parsed for them), and held
to the same rule as in the change: a computed name that could spell the old
one (`get_*` spells `get_total`; a bare `send(n)` spells any) is a use. The
name as a whole word in any other file code may read (config such as YAML
or JSON, another language such as JavaScript, a file of no known kind; not
docs, generated, vendored or binary files) can't be parsed for a use, so it
leaves `unseen_uses`. It can't see other repositories beyond `public_api`. In
a partial clone (the GitHub cache) the language's blobs at the after
revision are fetched in one request first.

A diff on stdin names its after side only by the blob ids on its `index`
lines, so shapr searches it only where those ids say the change ends: the
index when every file goes from HEAD to the index (`git diff --staged`),
the working tree when every file goes from the index or HEAD to the working
tree (`git diff`, `git diff HEAD`). Any other diff (between two commits, or
against an older base) leaves the files it doesn't name unknown, so its
parts carry `unseen_uses`: give the range instead (`shapr A..B`).

## Restyle

A `restyle` part holds changed lines whose code differs only in spellings
of the same program. The frontend reduces each syntax with equivalent
spellings to a canonical form holding everything it means, and two aligned
subtrees whose canonical trees are equal, though their text differs, are a
restyle. Where the parser resolves a spelling to a different tree, the
trees differ and it stays `logic`: `puts foo(a) do … end` binds the block to
`puts`, `ok = a and b` assigns `a`. A line that also holds other changed code
is `logic`. A restyle relies on nothing outside the change, so it has no
`assumptions`, and no line of it is read (`review_lines` 0).

Ruby (prism), each named by the RuboCop cop that chooses among its
spellings:

| variant | the same program | not the same |
|---|---|---|
| `Style/StringLiterals` | `'a'`, `"a"`, `%q(a)`, `?a`, a plain heredoc: a string with no interpolation, compared by its value and its encoding and frozen flags | `"\u00e9"` vs `"é"` (the escape forces UTF-8); a regexp's or command's text; an interpolated string's quotes |
| `Style/QuotedSymbols` | `:a`, `:"a"` | `:"a#{b}"` |
| `Style/HashSyntax` | `{:a => 1}`, `{a: 1}`, `{"a b": 1}` (the key a symbol on both sides); `{x: x}`, `{x:}` where `x` is a local | `{"a": 1}` vs `{"a" => 1}` (a symbol vs a string key); `{x:}` reading a method |
| `Style/WordArray`, `Style/SymbolArray` | `%w[a b]`, `["a", "b"]`; `%i[a b]`, `[:a, :b]` | |
| `Style/NumericLiterals` | `1000`, `1_000`, `0x3e8`; `1.0`, `1.00`: by exact value and type | `1` vs `1.0` |
| `Style/TrailingCommaInArrayLiteral`, `Style/TrailingCommaInHashLiteral`, `Style/TrailingCommaInArguments` | a trailing comma in a literal or a call's arguments | `\|a,\|` vs `\|a\|` in block parameters (destructures) |
| `Style/RedundantParentheses` | `(expr)` with one statement, and `expr` | `(a; b)`; under `defined?` |
| `Style/MethodCallWithArgsParentheses`, `Style/ColonMethodCall`, `Style/LambdaCall`, `Style/Not` | `foo(a)`, `foo a`; `a::b`, `a.b`; `f.(x)`, `f.call(x)`; `not x`, `!x`: one call, by its name, receiver, arguments and block | `foo` vs `foo()` (an undefined `foo` raises `NameError` vs `NoMethodError`); `a&.b` vs `a.b`; `foo(a: 1)` vs `foo({a: 1})` |
| `Style/BlockDelimiters` | `{ … }`, `do … end` attached to the same call | `puts foo(a) { }` vs `puts foo(a) do end` |
| `Style/AndOr` | `and`, `&&`; `or`, `\|\|` where the parse is the same | `ok = a and b` vs `ok = a && b` |
| `Style/IfUnlessModifier`, `Style/OneLineConditional`, `Style/WhileUntilModifier` | `x if c`, `if c then x end`, `if c; x; end`, `c ? a : b`; `x while c`, `while c do x end` | `begin x end while c` (do-while); `if !c` vs `unless c`; `c ? true : false` vs `c` |
| `Style/DefWithParentheses`, `Style/EndlessMethod`, `Style/EmptyMethod`, `Style/SingleLineMethods` | `def f()`, `def f`; `def f = x`, `def f; x; end`, and either over several lines | |
| `Style/Semicolon` | `a; b`, `a` and `b` on two lines | |

Rust (tree-sitter), named by the rustfmt option or lint:

| variant | the same program | not the same |
|---|---|---|
| `use_field_init_shorthand` | `S { a: a }`, `S { a }`, in an expression or a pattern | `S { a: b }`; `S { ref a }` |
| `unused_parens` | `(e)`, `e` | `(s.f)()` (calls a field) vs `s.f()` (a method); `(e,)`, a one-element tuple |

Not normalized, so `logic`: `x = x || y` vs `x ||= y`, `unless`/`else` vs
swapped branches, `foo(**{a: 1})` vs `foo(a: 1)`, `elsif` vs `else if`,
lambda delimiters, and Rust `use` trees, derive order and literal suffixes.

`detail.variants` lists the variants of the respelled syntax on either side
of the part's code, sorted; `evidence.respelled` counts its respelled
subtrees.

## Renames

A hunk's two sides are aligned node for node: two nodes align when their
kinds, roles, child counts and source text apart from names and whitespace
match, and their children then align in turn; a pair that doesn't align
differs as a whole, and its lines are `logic`. A changed line holding a
renamed name, and no code that differs otherwise, is a `rename` part when
every name that differs passes these checks:

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
- Every definition of the new name in its scope after the change is a
  renamed one: new code defining it too (a second `def new` in another
  class, a new `let new`) would share the name with the renamed code.
- For a method, constant or field, no file outside the change holds the
  old name at the after revision ([Proof status](#proof-status)).

Scopes: a `local` (variable or parameter) lives in its enclosing method,
class or module; a `field` in its file when it is a Ruby instance or class
variable, and in every parsed file of the change when it is a Rust struct
field; a `method` or `constant` (class and module names included) in every
parsed file of the change.

`detail`:

| key | type | meaning |
|---|---|---|
| `renames` | array | One object per rename the part makes. |
| `from`, `to` | string | The old and new name. |
| `scope` | string | Where the old name lives, and so where every use had to change: `local`, `field`, `method` or `constant`. |
| `occurrences` | int | Uses renamed across the change, the definition included; a class or module name counts once. |

`evidence`, beside `comparison`:

| key | type | meaning |
|---|---|---|
| `renamed` | int | Sum of the part's renames' `occurrences`. |
| `uses` | int | Uses of the old names shapr saw in their scopes before the change. More than `renamed` when another hunk deleted some. |
| `searched` | int or null | Files the repository search read for uses of the old names outside the change (see [Proof status](#proof-status)); `null` when no name can be used outside its scope (locals), or no search ran. |

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

## Categories

Every file has one `category`, a closed vocabulary. It says what kind of
file it is, apart from whether shapr parses it: a Ruby spec is `test` and
still parsed and classified. The first rule that matches wins:

| category | rule |
|---|---|
| `binary` | git reports the file as binary. |
| `generated` | `.gitattributes` marks it `linguist-generated`; or a lockfile, by name in any directory (`Cargo.lock`, `Gemfile.lock`, `Package.resolved`, `Pipfile.lock`, `Podfile.lock`, `bun.lock`, `composer.lock`, `flake.lock`, `go.sum`, `mix.lock`, `npm-shrinkwrap.json`, `package-lock.json`, `pnpm-lock.yaml`, `poetry.lock`, `pubspec.lock`, `uv.lock`, `yarn.lock`); or a minified asset (`*.min.js`, `*.min.css`). |
| `vendored` | `.gitattributes` marks it `linguist-vendored`. |
| `docs` | `.gitattributes` marks it `linguist-documentation`; or it is under a `doc`, `docs` or `documentation` directory; or its extension is prose (`md`, `markdown`, `mdx`, `txt`, `rdoc`, `adoc`, `asciidoc`, `rst`, `org`, `textile`); or its name, extension aside, is `README`, `LICENSE`, `LICENCE`, `COPYING`, `CHANGELOG`, `CHANGES`, `HISTORY`, `NEWS`, `AUTHORS`, `CONTRIBUTORS` or `NOTICE` (any case). |
| `test` | Under a `spec`, `test`, `tests` or `__tests__` directory, or named `*_spec.rb`, `*_test.rb`, `*_test.rs`, `*_test.go`, `*_test.py`, `test_*.rb`, `test_*.py`, `*.test.*`, `*.spec.*`, `tests.rs` or `test.rs`. |
| `manifest` | A dependency manifest, by name: `Gemfile`, `gems.rb`, `*.gemspec`, `Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`, `Pipfile`, `Podfile`, `composer.json`, `mix.exs`, `pubspec.yaml`, `Package.swift`, `flake.nix`. |
| `config` | Extension `yml`, `yaml`, `toml`, `json`, `jsonc`, `json5`, `ini`, `cfg`, `conf`, `properties`, `env`, `xml` or `plist`; or a dotenv file (`.env`, `.env.*`). |
| `code` | A language shapr parses (see [Languages](#languages)). |
| `unknown` | Anything else: source in a language shapr doesn't parse, scripts, build files. |

A linguist attribute wins over the name either way: `-linguist-generated`
(or `=false`) unmarks a lockfile, and `-linguist-documentation` a `README`.
Attributes are read as `git check-attr` sees them, from the working tree's
`.gitattributes` (for a GitHub run, the head commit's), not the diffed
revisions'.

Test code inside a code file (a Rust `#[test]` fn or `#[cfg(test)]` mod) is
not a category: it is the residual facet `test` (see [Facets](#facets)).

A file's category decides whether its hunks count toward the verdict (see
[Verdicts](#verdicts)). A `docs` file's `review_lines` is 0: prose isn't
code to review. Generated and vendored files don't count toward size: their
`review_lines` is 0, and the summary's line sums, `behavior_lines`, `entropy` and
`structure_share` leave them out. Their own `added`, `deleted` and other
line counts stay as the diff has them. Generated-but-reviewed files such as
a Rails `db/schema.rb` aren't `generated` by name; mark them in
`.gitattributes` if they should be.

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
  definition moved to another file is a `move` part with context
  `module`, which isn't structure. Struct fields, enum
  variants and `macro_rules!` don't move. An `impl` is its methods' owner,
  told apart from another by its header.
- Extract and inline: a `self` parameter rides on the call's receiver, and
  parameter types are allowed. Values are dropped when their block ends, so
  moving statements into a function can move a drop: risk `drop` (see
  [Extract and inline](#extract-and-inline)).

## `meta` record

What produced the output, and from what. The first record, so a consumer
can check `schema` before reading the rest.

| field | type | meaning |
|---|---|---|
| `record` | `"meta"` | |
| `schema` | int | This schema's version. |
| `version` | string | shapr's version. |
| `source` | object or null | *Experimental.* The GitHub pull request or commit analyzed (see [GitHub input](#github-input)); `null` for a local range or stdin. |

## `file` record

| field | type | meaning |
|---|---|---|
| `record` | `"file"` | |
| `file` | string | Path after the change; the old path for a deleted file. |
| `old_file` | string or null | Path before the change; `null` for an added file. Differs from `file` on a rename. |
| `language` | string or null | The frontend that parsed it (`"ruby"` or `"rust"`, see [Languages](#languages)); `null` when none did. |
| `parsed` | bool | Every hunk in the file was compared as trees (`evidence.comparison` `"ast"`). |
| `category` | string | What kind of file: `code`, `test`, `docs`, `config`, `manifest`, `generated`, `vendored`, `binary` or `unknown`. See [Categories](#categories). |
| `lines_before` | int or null | Lines in the file before the change (Kamei's LT); 0 for an added file; `null` when its content couldn't be read. |
| `hunks` | int | Hunk records that follow for this file. |
| `added`, `deleted`, `code_added`, `code_deleted`, `comment_added`, `comment_deleted`, `review_lines` | int | Sums over the file's hunks. |

## `hunk` record

| field | type | meaning |
|---|---|---|
| `record` | `"hunk"` | |
| `id` | int | Position in the output, from 0. Kind-specific `detail` refers to other hunks by `id`. |
| `file`, `old_file`, `language`, `category` | | As in the `file` record. |
| `parsed` | bool | Compared as trees. `false` when either side has a syntax error (compared as text instead) or the file wasn't parsed at all. |
| `before`, `after` | line run | The changed region on each side. |
| `kind` | string | The one-word rollup: `logic` unless `verdict` is `structure`, else the most substantial part kind. See [Parts](#parts). |
| `verdict` | string | `structure`, `behavior`, `mixed`, `none` or `unknown`. See [Verdicts](#verdicts). |
| `added`, `deleted` | int | `+` and `-` lines (Kamei's LA and LD for this hunk). |
| `code_added`, `code_deleted` | int | Of those, lines carrying code. |
| `comment_added`, `comment_deleted` | int | Of those, lines carrying a comment, so comment growth is visible. |
| `review_lines` | int | Changed code lines a reviewer still has to read: the sum of its parts' `review_lines`. 0 in a `docs`, `generated`, `vendored` or `binary` file, and in a hunk whose `verdict` is `none`. |
| `parts` | array | The hunk's parts in the [Parts](#parts) table's order, the residual (`logic`) last. Below. |
| `style` | object or null | *Experimental.* How the hunk's new code sits against the style norms of the code around it. `null` for a file in a language with no norms (Ruby has them). See [Style](#style). |

Each part:

| field | type | meaning |
|---|---|---|
| `kind` | string | See [Parts](#parts). |
| `structure` | bool | Changes structure without changing behavior, proven: false for `logic`, for a `move` into new surroundings, and for a part with `assumptions`. |
| `lines` | object | The changed lines the part explains: `before`, the `-` lines, and `after`, the `+` lines, each an array of line runs. |
| `confidence` | number 0–1 | How strongly the evidence supports the part. See [Confidence](#confidence). |
| `reason` | string | One short human-readable sentence. Not a contract: don't match on it. A residual's names the structural kind it resembles and why it isn't one, when a pass came close. |
| `evidence` | object | What the part rests on. Always has `comparison`; other keys depend on the kind. See [Evidence](#evidence). |
| `detail` | object | Kind-specific facts about the change, such as rename pairs. Empty (`{}`) unless the kind's section documents it. A residual that resembles a structural kind it couldn't be proven to be carries `near` (see [Extract and inline](#extract-and-inline) and [Confidence](#confidence)). |
| `facets` | string array | What kind of logic change the residual is, in the table's order; empty for every other part. See [Facets](#facets). |
| `assumptions` | string array | What the part relies on that shapr couldn't verify, in the [Proof status](#proof-status) table's order; empty for a proven part and for the residual. |
| `review_lines` | int | Of its lines, the code lines a reviewer still has to read. A residual: all of them. `format_only`, `comment_only`, `rename`, and a `move` that is structure: 0. A `rename` with assumptions: all of them. A `move` into new surroundings or with assumptions: the moved definition's first code line, where the new surroundings show. `extract` and `inline`: all of them; they are structure but still need reading. Any part: 0 where the hunk's `review_lines` is 0 by its category or a `none` verdict. |

## Evidence

| key | type | when | meaning |
|---|---|---|---|
| `comparison` | string | always | `"ast"`: both sides parsed cleanly and their trees were compared. `"line"`: a side has a syntax error, so the region's text was compared, whitespace and comments aside. `"none"`: nothing was compared (no frontend for the file, a binary file, or content that couldn't be read). |
| `code_similarity` | number 0–1 | `comparison` `"ast"` and the code differs | Dice coefficient over the subtrees of the two sides' changed code: 1 when they share every subtree, 0 when none. |
| `respelled` | int | a `restyle` part | Aligned subtrees the part respells. |

## Extract and inline

An `extract` or `inline` is proven, not scored: the caller is identical on both sides once the call is swapped for the statements (the *chunk*), the new method's body equals the chunk once its parameters are renamed to the call's arguments, and the methods' container still holds the same other code, in order, apart from siblings that changed on their own. A sibling's changed lines aren't part of the refactoring: they stay `logic`, beside the refactoring's part when they share its hunk. The new method's parameters must be plain required positionals, the call's arguments plain local variables, and a value used after the chunk comes back as the call's result (`x = b(...)`). Every hunk the refactoring touches gets a part, or none does (when another part already explains a line it would).

`detail` on each part:

| key | type | meaning |
|---|---|---|
| `method` | string | The method created (`extract`) or removed (`inline`). |
| `caller` | string | The method the chunk left or joined. |
| `hunks` | int array | `id`s of every hunk the refactoring touches, this one included. |

`evidence` keys beside `comparison`:

| key | type | meaning |
|---|---|---|
| `statements` | int | Statements in the chunk. |
| `nodes` | int | Syntax nodes in the chunk: its size. |
| `params` | int | The method's parameters. |
| `verbatim` | number 0–1 | Share of the chunk's nodes that match the method's body without renaming a parameter. |
| `returns` | bool | The call's result is assigned: a value flows back to the caller. |
| `searched` | int or null | Files the repository search read for the method's name outside the change; `null` when no search ran. |

Confidence is `size × (1 + verbatim) / 2`, where `size` is `nodes / 16`, capped at 1. A chunk under 8 nodes stays `logic`: equality of a body that small says little, and a one-call method is more often an override hook than a tidying. A renamed node is an inferred correspondence, so it counts half.

A hunk that matches this shape but would change behavior stays `logic`, and its residual says why in `detail`, beside `method`, `caller` and `hunks`:

| key | type | meaning |
|---|---|---|
| `near` | string | `"extract"` or `"inline"`. |
| `risk` | string | `"jump"`: the chunk holds code bound to its method or its place: `return`, `break`, `next`, `redo`, `retry`, `yield`, `super`, `block_given?` or a blockless `proc`/`lambda`/`Proc.new` (control flow and the method's block); `binding`, `eval`, `local_variables` or a string `instance_eval`/`class_eval` (the method's locals by name); `__method__`, `caller`, `caller_locations`, `$~`, `$_` or a match reference (`$1`, `$&`) (the method's frame); `__LINE__` or Rust's `line!()`/`column!()` (the code's line); and in Rust `?`, `.await`, `break`, `continue` or a macro outside std. Also when the caller reads its frame anywhere (`binding`, `$1`): the chunk's locals and matches leave it, or arrive. `"block_parameter"`: it uses the caller's `&block`. `"wiring"`: the arguments aren't exactly the chunk's free variables in parameter order, the chunk assigns a local of the caller, or a value used after the chunk isn't returned. `"collision"`: the method's name is a [reserved name](#reserved-names), or already appears, in any file of the change, on the side where the method doesn't exist (a def, a call, a `send`, a plain literal such as `:name`, as a `method_missing` compares): defining it would capture that use, removing it would break it; or another def of the name is on the method's own side, so the method takes part in dispatch beyond its one call; or a file outside the change holds the name at the after revision (a subclass's method of that name, a caller of the removed method), which the `reason` names. `"drop"` (Rust): code follows the chunk in its block and the chunk binds a local other than the returned one, or takes arguments, so a value would be dropped at another point. `"tiny"`: under 8 nodes. |


## Facets

A residual's `facets` say what kind of logic change it is: facts the code
shows on the residual's lines, never what a commit message claims. They
don't change `kind` or the verdict, and a residual can carry several or
none. They need both sides parsed cleanly (`evidence.comparison` `"ast"`).

| facet | meaning | how it is found |
|---|---|---|
| `test` | Test code inside a code file. | Every changed code line lies inside a Rust `#[test]` (`#[tokio::test]`, `#[rstest]`) fn or `#[cfg(test)]` mod. A whole test file is the `test` [category](#categories) instead. |
| `adds_definition` | A method, class or module that didn't exist. | A definition whose own syntax (the line naming it) is on a residual line, and whose name, qualified by its enclosing definitions, the other side of the file doesn't define. A Rust `impl` is named by its header. |
| `removes_definition` | A method, class or module that no longer exists. | The same, before side. A method renamed and edited, where the rename isn't proven, both adds and removes. |
| `signature` | An existing method's interface. | A method on both sides, touched by the residual, whose parameter list or annotations (Rust: return type, generics, visibility) differ, whitespace aside. |
| `condition` | What the code branches on. | A conditional (`if`, `unless`, `while`, `until`, `case`, `when`, `in`; Rust `if`, `while`, `match`, a match arm) whose test sits on a changed line, or a comparison or boolean operator (`==`, `<`, `>=`, `&&`, `and`, `!`, …) whose operator does, is on one side and not the other: added, removed, or testing something else. A line added inside a branch's body is not a condition change. |
| `error_handling` | How errors are raised, propagated or recovered. | An error construct with its own syntax on a changed line is on one side and not the other, compared by kind, name, operator text and the classes it names. Ruby: `raise`/`fail`, `rescue` (clause and modifier), `ensure`. Rust: `?`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, `bail!`, `ensure!`, `anyhow!`, `Err(…)` built or matched, and `unwrap`, `expect`, `unwrap_err`, `expect_err`, `map_err`, `ok_or`, `ok_or_else`, `unwrap_or`, `unwrap_or_else`, `unwrap_or_default`, `context`, `with_context`. Changing an argument under a `?` is not. |
| `literal_only` | Only literal values changed: config-like. | The changed regions are equal once every literal (number, string, symbol, boolean, nil, regexp; a string's text inside an interpolation) and comment is masked, and hold the same number of literals. A symbol naming code (`send(:x)`, see `DynamicName` in the rename rules) is not a literal here. |

Code inside a definition the residual adds or removes doesn't add `condition` or
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
| `experimental` | object | *Experimental*: judgements drawn from the hints. Below. |

Each hint:

| key | type | meaning |
|---|---|---|
| `hint` | string | `type`: a conventional-commit type (`fix:`, `feat(api):`, known types only). `keyword`: a purpose word in the subject (`fix`, `bug`, `regression`, `crash`, `hotfix`; `add`, `introduce`, `support`, `implement`, `allow`; `refactor`, `rename`, `move`, `extract`, `tidy`, `cleanup`, `simplify`; `perf`, `faster`, `optimize`; `test`, `spec`, `coverage`; `docs`, `readme`, `typo`, `changelog`; `chore`, `bump`, `deps`, `upgrade`, `release`, `lint`; `revert`). `revert`: a git revert's `Revert "subject"` (the quoted subject, whose own words are not read as claims) or its `This reverts commit <id>`. `issue`: `#12`, `KEY-12` (standards such as `UTF-8` aside), or an issue or pull URL (as `#12`). `breaking`: `type!:` or a `BREAKING CHANGE:` footer. `no_behavior_change`: "no behavior change", "no functional change", "no logic change", "behavior preserving", "pure refactor" (any case, hyphens as spaces) or the word `NFC`. |
| `value` | string | What matched, lowercased for types and keywords. |
| `category` | string or null | The category the hint claims: `feat` is `feature`, `style` is `refactor`, `build` and `ci` are `chore`, `no_behavior_change` is `refactor`, an issue after `fix`/`fixes`/`fixed` is `fix`; `null` for `breaking` and other issues. |
| `source` | string | `commit`, `message` (`--message`), or `pull_request` (a GitHub pull request's title and body). |
| `commit` | string or null | The commit's id, 12 hex digits; `null` for a description. |
| `field` | string | `subject` (the first non-empty line) or `body`. Purpose words are read only in the subject, where they summarize; a body mentions too much in passing. |

Hints are facts: what a message says, where. `experimental` holds
judgements built on them by keyword rules that haven't been measured against
labelled changes; its fields and rules may change in any release before 1.0.

| field | type | meaning |
|---|---|---|
| `categories` | object | Messages carrying at least one hint of each category, every category present: `fix`, `feature`, `refactor`, `perf`, `test`, `docs`, `chore`, `revert`. |
| `conflicts` | array | Claims the code doesn't bear out. Below. |

Each conflict is a count of what the code shows, not a verdict on the author:

| key | type | meaning |
|---|---|---|
| `claim` | string | The claimed category. |
| `finding` | string | `behavior_hunks`: the claim is `refactor` or `docs`, no message claims `fix`, `feature` or `perf`, and shapr couldn't prove `count` hunks structural (their `verdict` is `behavior` or `mixed`). `non_test_behavior_hunks`: the same for a `test` claim, counting behavior hunks outside the `test` category whose residual lacks the `test` facet. `no_behavior_hunks`: the claim is `fix`, `feature` or `perf` and the change's verdict is `structure`. |
| `count` | int | Behavior hunks behind the finding (0 for `no_behavior_hunks`). Hunks outside the verdict (`none`, `unknown`) aren't counted: they aren't evidence. |

"Couldn't prove structural" is not "changes behavior": a `refactor` claim with
behavior hunks may be a refactoring shapr can't recognize. The conflict says
where to look.

## Confidence

Each part's confidence is computed from the evidence, not assigned per kind.

- `comparison` `"ast"`: 1 for `format_only`, `comment_only` and `logic`. Tree
  comparison is exact, so it settles those three. `rename`, `move`,
  `extract` and `inline` compute their own score: see
  [Renames](#renames), [Moves](#moves) and
  [Extract and inline](#extract-and-inline).
- `comparison` `"line"`: text equality can't see a change in how whitespace
  parses (`puts -x` and `puts - x`), so it never proves structure. A hunk
  whose code text matches is one `logic` part with confidence 0 and
  `detail.near` naming the kind it resembles, `"format_only"` or
  `"comment_only"`. One whose code text differs is `logic` at most **0.5**
  (the line cap), falling in proportion to the region's lines that hold a
  syntax error: `0.5 × (1 − lines with an error / lines in both regions)`.
- `comparison` `"none"`: 0. The `logic` part is the safe default, not a finding.

## Moves

A *unit* is a method, class, module or constant definition of at least 3
syntax nodes, sitting directly in a file or a class or module body (not in
a block or conditional). Each file's trees are aligned body by body, larger
nodes first; a unit the alignment leaves out was removed or added. A removed unit pairs with an
added one, in the same file or another, by equal tree or, failing that, by
equal name and kind with `similarity` at least 0.5 (a move whose body also
changed). Definitions align by name, so one that keeps its place among its
siblings was edited, not moved.

A pair is not structure, even with an unchanged body, when the code around
its new place differs. Its `context` names each difference:

| context | meaning |
|---|---|
| `module` | Rust, across files: each file is a module, so the paths it resolves and its own path change. |
| `owner` | A different enclosing class, module or `impl` (a Rust namespace's attributes count: `#[cfg(unix)] mod imp` is not `#[cfg(windows)] mod imp`). |
| `visibility` | A method in a different visibility section: it crossed a bare `private`, `protected`, `public` or `module_function`. A constant or class isn't affected by one. |
| `load_order` | A different order relative to a sibling that names it or that it names at load time (`alias_method`, `private :x`, a redefinition, a constant read, a Rust `macro_rules!` of a macro it invokes) or, for anything but a method, any statement that runs code at load time (a `require`, a DSL call, a constant whose value calls something: `X = Counter.next`). |
| `directives` | Across files, different file-level directives (`# frozen_string_literal`). |
| `position` | The unit reads its own line (`__LINE__`, Rust's `line!()`), or its own file (`__FILE__`, `__dir__`, `require_relative`, Rust's `file!()`) and moved to another file. |

The body is relocated, not rewritten, so a reviewer reads where it landed
rather than every line of it; but its meaning can change, so the part isn't
structure. Code that isn't a unit (a DSL call, a definition under the size
floor or inside a block) never pairs: reordering it is `logic`.

A hunk's `move` part holds its changed lines that lie in a paired unit or in
code the alignment kept unchanged (git may slide a removal across identical
lines), except the lines of a unit that changed in the move
(`changed_before`, `changed_after`), which stay `logic`. Both halves get a
part.

`evidence` keys for `move`:

| key | type | meaning |
|---|---|---|
| `min_similarity` | number 0–1 | The lowest similarity among the hunk's pairs: Dice coefficient over the two units' subtrees. 1 when every body is unchanged. |
| `nodes` | int | Syntax nodes in the smallest unit moved. |
| `candidates` | int | The most units, among the hunk's pairs, that matched equally well; 1 when every match is unique. Above 1, the part assumes `ambiguous_match`. |

`detail` for `move`:

| key | type | meaning |
|---|---|---|
| `context` | string array | Every `context` among the hunk's entries, in the table's order; empty for a part that is structure. |
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

## Style

*Experimental.* A norm is a style choice a project makes one way, such as
RuboCop's `Style/StringLiterals` (`single_quotes` or `double_quotes`). shapr
infers each norm from the code itself and judges only new code against it.
Deleting code chooses no style, and neither does code carried over
unchanged or edited in place: an added observation matched by a removed one with
the same text and style, or failing that the same style, is kept, not chosen.

Ruby's norms, named by the RuboCop cop whose setting configures them:

| norm | styles | observed on |
|---|---|---|
| `Style/StringLiterals` | `single_quotes`, `double_quotes` | each quoted string that could take either quote with the same value, as RuboCop counts them |
| `Layout/SpaceInsideHashLiteralBraces` | `space`, `no_space` | each non-empty hash literal with spaces inside both braces or neither; a brace beside a line break chooses nothing (RuboCop's `compact` reads as `space`) |
| `Style/TrailingCommaInArrayLiteral`, `Style/TrailingCommaInHashLiteral` | `comma`, `no_comma` | each multiline `[]` or `{}` literal with every item on its own line, after the bracket's line, and the closing bracket on a line of its own (where RuboCop's `comma`, `consistent_comma` and `diff_comma` all read as `comma`); a heredoc body before the bracket chooses nothing |
| `Style/TrailingCommaInArguments` | `comma`, `no_comma` | the same, for a call's arguments in `()` or `[]` (`Set[...]`), each keyword argument an item; a call ending in a `&block` argument, which can't take a comma, chooses nothing |
| `Style/HashSyntax` | `ruby19`, `hash_rockets` | each pair of a hash or keyword arguments whose key is a symbol: `a: 1` or `:a => 1` where the symbol can be a label, a quoted label `"a-b": 1` as `ruby19`. As in RuboCop's `ruby19`, a rocket counts only when every key in its hash can be a label; a hash with an omitted value (`{a:}`) chooses nothing. `ruby19_no_mixed_keys` reads as `ruby19` |
| `Layout/IndentationConsistency` | `normal`, `indented_internal_methods` | each method defined after a bare `private` or `protected` in a class or module body: at the modifier's column, or deeper. As in RuboCop, the method is measured from its modifier, outdented or not; `public` ends the section |
| `Layout/TrailingWhitespace` | `no_trailing_whitespace`, `trailing_whitespace` | each non-empty line up to `__END__`, but not a line whose break is inside a string, heredoc, regexp or quoted symbol, where whitespace is data. `.rubocop.yml` configures it by enabling the cop; `.editorconfig` by `trim_trailing_whitespace = true` |
| `Layout/TrailingEmptyLines` | `final_newline`, `final_blank_line`, `no_final_newline` | how each non-empty file ends: one line break, more, or none; one observation per file, so a file's own code never backs a norm and a new file's directory or the configured style does. `.editorconfig`'s `insert_final_newline` sets `final_newline` (`true`) or `no_final_newline` (`false`) |
| `Layout/IndentationWidth` | the width in spaces: `2`, `4`, ... | each body's first statement, when it starts its own line, against what opens the body: a `def`, `class`, `module` or block from the start of its line; `if`, `unless`, `while`, `until`, `case`/`when`, `else`, `begin`, `rescue` and `ensure` from their keyword when it starts its line (so `x = if` chooses nothing). Tab-indented lines choose nothing. No suggested edit: reindenting a body reaches past the line. RuboCop's `Width`, or `.editorconfig`'s `indent_size` (`tab_width` for `tab`) unless `indent_style = tab`, configures it |
| `Naming/MethodName` | `snake_case`, `camelCase` | each `def` whose name has two or more words, ignoring a leading `_` and a trailing `?`, `!` or `=`: lowercase words joined by `_`, or a lowercase start with capitals and no `_`. A one-word name chooses nothing. A rename is suggested only for a private instance method (in a `private` section, or `private def`) whose every mention in the file is its `def` or a call with no receiver or `self`, and whose new name the file doesn't already use; any other method is interface, reported with no edit |
| `Naming/VariableName` | `snake_case`, `camelCase` | each local of two or more words, once per scope that declares it (program, class or module body, method, block), at its first mention; a keyword parameter is interface and chooses nothing. A rename covers the scope and is suggested when every mention of the name there is that local (not a method call, symbol, string, comment or `{name:}` key), the new name is unused there, and nothing in the scope reads locals by name (`binding`, `eval`, `local_variables`) |
| `Layout/SpaceAroundOperators` | `space`, `no_space` | each binary operator call (`+`, `==`, `<<`, ...), `&&`, `\|\|`, and assignment (`=`, `+=`, `\|\|=`, attribute and index writes): whitespace of any width, line breaks included, on both sides is `space`, so aligned operators read as `space`; anything less is `no_space`. `**`, unary operators, `and`/`or` and default parameters (`a=1`) aren't observed. Only the edit to `space` is suggested: removing whitespace can change a parse (`a <<b`). `.rubocop.yml` configures it by enabling the cop |
| `Layout/EmptyLineBetweenDefs` | `empty_line`, `no_empty_line` | the gap between two adjacent `def`s in a body: a blank line in it, or only comments or nothing. The observation spans the first's last line to the second's first, so a line added on either side, or a deletion in the gap, judges it; a one-line def (RuboCop allows two adjacent; here one beside a longer def chooses nothing too) and defs sharing a line choose nothing. The edit to `empty_line` inserts a line break after the first `def`, inside the hunk when either side of the gap is. `.rubocop.yml` configures it by enabling the cop |

A style is a scope's norm when its share, as `(n + 1) / (total + styles)`, is
at least `--norm-threshold` (default 0.8): three of three backs a norm, two of
two abstains. The scope is the narrowest unchanged code that backs one: the
enclosing method, then class, then file, then, for a new file, its directory's
other files. When none does and the hunk's scope moved wholesale from one
style to another, that move is the norm (`renorming`). With no norm inferred,
the style the project configures is expected instead.

Configured styles come from `.rubocop.yml` (the nearest one, with local
`inherit_from`) and `.editorconfig` (each one from the file's directory up
to the first with `root = true`, nearer files and later sections winning,
section globs matched against the file as editorconfig.org specifies).
Where both set the same norm, `.rubocop.yml` wins: it is the Ruby-specific
file. The table above says which settings configure each norm.

Config and a new file's neighbors are read as of the change's after side:
from the revision a range ends in (`B` of `A..B`; a GitHub change's head),
or from the index or the working tree when the change ends there (one
revision, or a diff on stdin as [Proof status](#proof-status) reads it; the
working tree for any other diff on stdin). Nothing above the repository's top is read.

The hunk's `style`:

| field | type | meaning |
|---|---|---|
| `skipped` | string or null | Why the file isn't judged: `generated`, `vendored` or `docs` (by [category](#categories), plus `db/schema.rb` and `vendor`, `third_party` and `node_modules` directories), or `syntax_error`. |
| `direction` | string | The strongest of its norms' `direction`; `none` when no norm saw new code. |
| `flagged` | bool | Some norm is `flagged`. |
| `norms` | array | One per norm that saw new code in the hunk. Below. |

Each norm:

| field | type | meaning |
|---|---|---|
| `norm` | string | The norm, by its RuboCop name. |
| `before`, `after` | object | Per style, the observations on the hunk's `-` and `+` lines. |
| `scope` | string or null | Where the norm was inferred: `method`, `class`, `file` or `directory`; `null` when none backs one. |
| `surrounding` | object or null | Per style, the observations in that scope's unchanged code (or, when renorming, the scope's whole after side). |
| `previous` | object or null | When renorming, per style, the scope's whole before side. |
| `inferred` | string or null | The style the scope backs. |
| `confidence` | number 0–1 or null | Its share, as above. |
| `configured` | string or null | The style `.rubocop.yml` sets; `null` when it sets none or disables the cop. Reported beside `inferred` even when they differ. |
| `expected` | string or null | `inferred`, else `configured`. |
| `direction` | string | `aligned` (new code follows `expected`), `converging` (it does, and replaces code that didn't, or the scope renormed), `diverging` (some new code doesn't) or `abstain` (nothing expected). |
| `convergence` | string or null | When `converging`: `normalizing` (toward the surrounding norm) or `renorming` (the scope moved to a new one). |
| `restyle` | bool | The hunk's [`restyle`](#restyle) part respells this norm's variant (the norm's name is a variant's): a respelling, not new code. Layout and naming norms are never a restyle; their respellings are `format_only` and `rename` parts. |
| `flagged` | bool | `restyle` and `diverging`: a change that respells code away from its norm. |
| `deviations` | array | New code not in the `expected` style: `line`, `column`, `style`, `expected`, and `edit`, a unified diff that applies the expected style, `null` when there is no mechanical fix, it would reach outside the hunk's changed region, or the edited file wouldn't be the same program: each edit is applied and the file re-parsed, and it must parse cleanly with an equal canonical tree (whitespace, comments and the spellings [Restyle](#restyle) proves the same may differ); a `Naming/` rename need only parse cleanly. |

## `shapr norms`

*Experimental.* `shapr norms FILE...` reports each file's norms with no
diff, as a `meta` record, then per file a `file` record and its `norm`
records, then a `summary`. Under `-j` they are one object: `meta`, `files`,
`norms`, `summary`.

`file`: `file`, `language` (`null` when no language has norms) and
`skipped` (as in a hunk's `style`, or `unsupported`).

`norm`: `file`, `norm`, `scope` (`file`), `counts` (per style, the whole
file), `inferred`, `confidence`, `configured` and `expected` as above, and
`deviations`, the lines not in the `expected` style.

`summary`: `files`, `norms`, `inferred` (norms with a style inferred) and
`deviations`.

## `summary` record

| field | type | meaning |
|---|---|---|
| `record` | `"summary"` | |
| `verdict` | string | `structure`, `behavior`, `mixed`, `none` or `unknown`, over the hunks that count. See [Verdicts](#verdicts). |
| `verdicts` | object | Hunk count per hunk `verdict`, every verdict present. `unknown` counts what shapr didn't judge. |
| `hunks` | int | Hunk records. |
| `unparsed_hunks` | int | Hunks with `parsed: false`. |
| `categories` | object | Hunk count per [category](#categories), every category present. Generated and vendored hunks count in `hunks` and `kinds`, and in no size measure below. |
| `kinds` | object | Part count per kind, every kind present: `logic` counts residuals. A hunk that is one part counts once. |
| `move_contexts` | object | `move` part count per `context` (see [Moves](#moves)), every context present; a part with two contexts counts under both. |
| `assumptions` | object | Part count per assumption (see [Proof status](#proof-status)), every assumption present; a part with two counts under both. |
| `facets` | object | Residual count per facet (see [Facets](#facets)), every facet present; a residual with two facets counts under both. |
| `files` | int | Files changed (Kamei's NF). |
| `directories` | int | Distinct directories holding them (ND); the repo root counts as one. |
| `subsystems` | int | Distinct top-level directories (NS); files at the root count as one. |
| `entropy` | number | Kamei's change entropy, −Σ pₖ log₂ pₖ, where pₖ is file k's share of the changed lines (`added + deleted`), over files that aren't generated or vendored. 0 for one file. |
| `added`, `deleted`, `code_added`, `code_deleted`, `comment_added`, `comment_deleted`, `review_lines` | int | Sums over the hunks that aren't generated or vendored. |
| `test_review_lines` | int | Of `review_lines`, those in test code: every one in a `test` file, and a residual's carrying the `test` facet elsewhere. Reviewers weigh test changes apart from code changes. |
| `behavior_lines` | int | `review_lines` summed over parts that aren't structure (residuals, moves into new surroundings, parts with assumptions) in hunks whose verdict is `behavior` or `mixed`: the behavior change's size, comments and unchanged moved lines excluded. |
| `structure_share` | number 0–1 | Changed lines (`added + deleted`) in structure parts of hunks whose verdict is `structure` or `mixed`, over all changed lines, generated and vendored hunks left out; 0 when nothing changed. |
| `claims` | object | What the commit messages and `--message` description claim, kept apart from the code's facts. See [Claims](#claims). |

## GitHub input

*Experimental*: the forms below, `meta.source`, the `cache` record and the
cache flags (`--cache-dir`, `--status`, `--clear-cache`, `SHAPR_CACHE_DIR`)
may change in any release before 1.0.

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

`meta.source`:

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

*Experimental*, with [GitHub input](#github-input).

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

The verdict is in the output, never in the exit code: a run that classified
the change exits 0, whatever the verdict.

| code | meaning |
|---|---|
| 0 | success |
| 2 | operational error: git failed (not a repository, a bad revision), stdin unreadable, stdin holds no diff (no file header), or GitHub failed (unreachable, not found, rate limited, a fetch refused) |
| 64 | usage: a bad command line, no input (no range and an empty or blank stdin), a malformed GitHub form, an ambiguous `#N`, or a relative cache directory |

An empty diff from a range (`shapr main..main`) is the verdict `none`.
An empty stdin is not: it is what a failed `git diff` upstream of the pipe
leaves, so it exits 64. A diff whose only change is a rename or mode change
has file headers and no hunks, and is `none` too.
