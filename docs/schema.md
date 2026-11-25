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
  (`#!/usr/bin/env ruby`) are code. Type annotations in comments (RBS
  inline `#:`, YARD tags) and Sorbet's `# typed:` sigil are comments: a
  type checker reads them, Ruby doesn't. In a file shapr didn't
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
| `typing` | yes, unless it has `assumptions` | Only Sorbet type annotations change: the code is the same once they are stripped. See [Typing](#typing). |
| `reorder` | yes, unless its `detail.context` is non-empty | The hunk's elements of one container are the same subtrees, with the same comments leading them, in another order; lines of elements that also changed are `logic` beside it. An element out of order counts as moved only where the gap it leaves, or the one it enters, between elements kept in order holds a different number of elements on the two sides; where both hold as many, it reads as an edit in place. Structure where order can't be observed: Rust items and separate `use` declarations in a module, `impl`, trait or block with no `macro_rules!`, `#[macro_use]` item, item-level macro invocation or attribute macro among them (whichever element moved), and a struct literal's fields whose values are literals and names. Otherwise `detail.context` names what observes the order: `load_order` for `require`s, Gemfile `gem` lines and gemspec dependencies (Bundler requires gems in order), and for Rust items beside a `macro_rules!`, a `#[macro_use]` item, an item-level macro invocation or an attribute macro; `iteration_order` for Ruby hash entries and keyword arguments (every key a distinct plain literal); `element_order` for array elements and Rust struct field declarations and enum variants; `evaluation_order` for other statements, a struct literal's fields with a value that runs code, and beside the others where a moved element calls a method. Method definitions are a `move`; positional arguments and `match` arms are never a reorder. `evidence`: `elements` (the container's elements in the hunk found on both sides) and `moved` (those out of order). `review_lines`: 0 when structure, else each moved element's lines on the after side. |
| `default_argument` | yes, unless it has `assumptions` | A call drops or adds an argument that restates its parameter's literal default, and the method it calls is the only one the call can reach. See [Default arguments](#default-arguments). |
| `rename` | yes, unless it has `assumptions` | Only identifiers change, and every use of each old name in its scope became the new name. Any parameter added, removed, reordered or given a default is `logic`. See [Renames](#renames). |
| `move` | yes, unless its `detail.context` or `assumptions` is non-empty | Code removed in one place appears elsewhere, in the same file or another. Reordering methods within a class or module is a `move`; reordering other statements (`require`s, Gemfile `gem` lines, callbacks, validations, `include`s, other DSL calls) is a `reorder`. A moved definition whose body also changed is a `move` part plus the changed lines in the residual. One moved unchanged into different surroundings (another owner, visibility section, load order, file-level directives, or Rust module) is a `move` part that isn't structure. See [Moves](#moves). |
| `extract` | yes, unless it has `assumptions` | Statements leave one method and become the body of a new method in the same class, replaced by a call to it. See [Extract and inline](#extract-and-inline). |
| `inline` | yes, unless it has `assumptions` | The inverse of `extract`: a method and its only call disappear, and its body takes the call's place. |
| `logic` | no | The residual: anything else. A line shapr can't prove structural is `logic`. |

Each part is proven against the whole of both trees, residual included, so
new code in the residual can break a part's proof (a rename whose new name
new code also defines): the part is then not claimed and its lines stay
`logic`.

A part is structure only when it is proven: see [Proof status](#proof-status).

Code that reads its own position (`__LINE__`, Rust's `line!()`, `column!()`
and `dbg!`) or the call stack's (Ruby's `caller` and `caller_locations`, bare or on
`Kernel`, and `Thread.current.backtrace` or `backtrace_locations`;
Rust's `Location::caller()`) changes value when it moves. A hunk holding such a read, or one
above it in the file when the lines added and removed above the read don't
balance, proves no structure, unless the read keeps its line and column (an
edit after it on its own line; reads pair up in order, and when a change
adds or removes one, every read counts as moved): its lines are `logic`, with `detail.near`
naming the kind it would have been and the confidence of the comparison
that proved it. A shifted line an exception's backtrace
reports is not checked, as every line added above any raise would shift it;
nor is the caller's line a `#[track_caller]` function reports.

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
| `none` | The file is `docs`, `fixture` or `types`. |
| `none` | shapr has no parser for the file, and its changed lines differ only in trailing whitespace and blank lines (indentation counts: YAML, Python and Makefiles read it). |
| `structure`, `behavior`, `mixed` | A `code`, `test`, `config` or `manifest` file shapr compared (`evidence.comparison` `"ast"` or `"line"`): `structure` when every part is structure, `behavior` when none is, `mixed` otherwise. |
| `behavior` | A `code`, `test`, `config` or `manifest` file shapr didn't compare (`evidence.comparison` `"none"`): a config or manifest has no parser, and its text is the setting; code too big, nested too deep or unreadable is `logic` its reason names, never left out of the verdict on its own say. |
| `unknown` | Anything else: an `unknown` file, or a `test` file in a language shapr doesn't parse. |

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
when the change ends there; for a diff on stdin, see below): shapr searches its files for the old name (and a
rename's new name) as a whole word (`git grep -w`), then parses each file found and applies the same
rule as to the files in the change (a use, a dynamic reference, a plain
literal such as `:old`), so the word in a comment or a sentence doesn't
count; a file it can't parse cleanly (a template) counts. A file outside the
change that still uses the name (for a rename's new name, that mentions it
at all) makes the operation `logic`, and the residual's `reason` names the file.
What the search can't settle stays on the part:

| assumption | meaning |
|---|---|
| `unseen_uses` | The repository couldn't be searched (a diff on stdin outside a repository or not ending in the index or the working tree, a failed search), or holds the name in a file shapr can't parse for a use (YAML, JavaScript; see below), so code outside the change may still use the name. |
| `search_capped` | The repository holds more than 20,000 files of the language: they aren't searched. |
| `public_api` | The name is part of a library's public surface, so callers outside this repository can break: a renamed or inlined public definition in a gem's `lib/` (beside a `*.gemspec`), or the method a `default_argument` calls there, private or not (a subclass outside the repository may override it with another default), or a `pub` item (or a `pub` trait's item or `pub` enum's variant) in a Rust library crate (a `[lib]` target or `src/lib.rs`, not `publish = false` or `publish = []`, itself or through `publish.workspace = true`; a crate whose `Cargo.toml` can't be read counts; outside its binaries, tests, examples, benches and build script). A Ruby method is public unless `private` (a `private` section, `private def`, `private :name`); protected counts. A private module's `pub` items count, as they may be re-exported. Instance variables never do. Also a definition moved from a gem's `lib/` into a new file that other files load after the old one: another package may load the old file alone ([New files](#new-files)). |
| `runtime_check` | A `typing` part holds a type annotation a runtime may check: sorbet-runtime checks a `sig`, `T.let`, `T.cast` and `T.bind`, raises on a `nil` in `T.must`, and enforces `abstract!` and its kin, so a value the new types reject raises where the old code ran. See [Typing](#typing). |
| `ambiguous_match` | A `move` whose unit matched more than one other unit equally well (`evidence.candidates` above 1): which one it came from is a guess. |

A part with assumptions is reported with `structure: false` and its lines
to read: every code line for a `rename`, `extract` or `inline`, each moved
definition's first code line for a `move`. Its `reason` says why (`public
API rename: ...`). There is no separate verdict for assumed structure: such
a hunk is `behavior` or `mixed`.

The search reads the files a language's code can reach a name from (Ruby:
`.rb`, `.rake`, `.gemspec`, `.ru`, ERB, Haml, Slim and builder templates,
the extensionless DSL files below; Rust: `.rs`): tracked files, and when the change ends in the working tree its untracked files too (not ignored ones). A use is matched
by name, not resolved: a common name used for something else (another
class's method or instance variable of the same name, `Rack::Test::Methods`
for a constant `Methods`) blocks the operation. A method's name is searched
without its `?`, `!` or `=`. A file that reaches a method by a name built
at runtime (`send(n)`, `public_send("get_#{k}")`, `define_method`), or a
class or constant by one (`const_get(n)`, `n.constantize`), or a field
(`instance_variable_get(n)`), is read too, once per run (every file of the language is parsed for them), and held
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
tree (`git diff`, `git diff HEAD`), and both when the ids fit both (a use in
either is a use; a file found that differs between them counts as one). Any other diff (between two commits, or
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
| `reorder_imports` | `use a::{x, y}`, `use a::{y, x}`, nested lists too | separate `use` items in another order |

Not normalized, so `logic`: `x = x || y` vs `x ||= y`, `unless`/`else` vs
swapped branches, `foo(**{a: 1})` vs `foo(a: 1)`, `elsif` vs `else if`,
lambda delimiters, and Rust `use` trees, derive order and literal suffixes.

`detail.variants` lists the variants of the respelled syntax on either side
of the part's code, sorted; `evidence.respelled` counts its respelled
subtrees.

## Typing

A `typing` part holds changed lines where only Sorbet's type annotations
differ. The frontend marks two kinds of annotation in Ruby:

- Statements that only declare types, left out when comparing: a `sig { … }`
  or `sig do … end` block (also `sig(:final)`) and
  `T::Sig::WithoutRuntime.sig { … }`; `extend T::Sig`, `include T::Sig`,
  `extend T::Helpers`; `abstract!`, `interface!`, `sealed!` and `final!` in a
  file naming `T::Helpers` (Rails defines an `abstract!` of its own); and
  `T.bind(self, Type)` as a statement.
- Calls that return their argument, replaced by it when comparing:
  `T.let(x, Type)`, `T.cast(x, Type)`, `T.bind(x, Type)` (a literal
  `checked:` allowed), `T.must(x)` and `T.unsafe(x)`.

A bare `sig` counts only in a file that has a `# typed:` sigil or names
`T`, so a method of one's own named `sig` stays `logic`. A file that defines
a constant `T` of its own (`module T`, `class T`, `T = ...`) has no Sorbet
annotations. A cast whose value is `self` (directly or through another
cast) used as a call's receiver isn't an annotation: `T.unsafe(self).secret`
raises where `self.secret` calls a private method, so it stays `logic`. Two subtrees whose
hashes agree once annotations are stripped are the same code but for types;
the hunk's covers are compared this way, statements matched in order, and a
changed line touched only by annotations in such code (or, where one side
has none, by the code they wrap) is the part's. A line also holding code
that differs otherwise is `logic`, so a `sig` edit beside a body edit is a
`typing` part and a residual. A declaration above a statement the change
adds, removes or replaces whole (the `sig` of a new method) is new code
with it: `logic`.

Types only is not structure while a runtime may check them, so the part
carries `runtime_check` and its lines are read. It is structure only when
every annotation on its lines is one sorbet-runtime never checks
(`T.unsafe(x)`, `T.let`/`T.cast`/`T.bind` with `checked: false`,
`T::Sig::WithoutRuntime.sig`) and the nearest `Gemfile.lock` or
`gems.locked` above the file names `sorbet-runtime` or
`sorbet-static-and-runtime` as the project's own dependency, so `T` is its
module: a top-level entry under `DEPENDENCIES`, or a runtime dependency of
the gemspec at the lockfile's root (its `PATH` gem with `remote: .`). Another
gem's dependency (tapioca's) doesn't count. A repository whose
lockfile names neither isn't taken as unchecked: whatever defines `T` there
(a stub gem, a shim of its own) isn't seen. `T.cast` checks its value at
runtime unless `checked: false`. `review_lines`: every code line of the
part, structure or not; a type is a claim about the code a reviewer judges,
and Sorbet checks it against the code, not against intent.

`evidence.annotations` counts the annotations on the part's lines, both
sides, and `evidence.unchecked` those no runtime checks.

Files holding only type declarations (RBI, RBS) are the `types`
[category](#categories), not classified.

## Default arguments

A `default_argument` part holds changed lines where a call drops an
argument that restates its parameter's default, or adds one:
`fetch(x, retries: 3)` and `fetch(x)` where `fetch` is
`def fetch(key, retries: 3)`. Either way the method binds the same value.
Ruby only; Rust has no default arguments.

The hunk's code, around its changed region, is the same on both sides but
for calls that differ by one keyword argument, the others kept in order, or
by the last positional argument where the method takes no keywords. Each
call is proven to reach one definition:

- The call has no receiver, or `self`, and sits in a method with no block,
  lambda or other body between them (`instance_eval` can run a block with
  another `self`); that method is defined directly in a class or module
  body, not at the top level, in a block or in `class << self`.
- The method called is defined exactly once in the repository at the change's
  after revision (searched as for a rename: `git grep -w`, then parsed), in
  the same class or module, the same way nested, on the same receiver (both
  `def x` or both `def self.x`), unconditionally in a class or module body
  (not under an `if`, where it may not exist and the call reach an inherited
  method; `private def x` is fine). Nothing else may
  name it: no `send`, `define_method`, `alias`, `attr_*` or a computed name
  that could spell it, and no plain literal such as `:fetch` (a delegation or
  a memoizer). A file shapr can't parse that names it counts against it.
- Its parameters are the same on both sides, it takes no rest
  (`*args`, `**opts`, `**nil`, `...`) and no required parameter after an
  optional one, and its own body doesn't read its frame (`binding`,
  `local_variables`, `__method__`, `caller`).
- The argument and the default are the same plain literal by their
  canonical form ([Restyle](#restyle): `','` and `","` agree): an integer,
  float, symbol, string with no interpolation, `nil`, `true`, `false`, or an
  empty `{}` or `[]`, a new object either way. A default that is any other
  expression can differ from the argument at runtime. The files of the call
  and the method, on both sides, carry the same magic comments, which
  decide a literal's encoding and whether a string is frozen.
- The call passes no splat, `**`, or `...`, and names each keyword once
  with a plain symbol.

What the search can't settle is an assumption, as for a rename; a method
in a gem's `lib/` is `public_api` even when private, since a subclass
outside the repository may override it. A subclass or prepended module in
another package of an application isn't seen. Calls with another receiver
(`stub.to_return(status: 200)`) reach a method shapr can't name, so they
stay `logic`.

`detail.restated` lists each argument, in the hunk's order: `method`, the
method called; `parameter`, `name:` or `argument N` (1-based); `value`, the
argument's source; and `added`, true when the change adds it. `evidence`
holds `restated`, how many, and `searched`, as for an extract.

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
  is renamed. A constant lookup by a computed name (`const_get`,
  `qualified_const_get`, `const_defined?`, `constantize`,
  `safe_constantize`) counts the same way for a class, module or constant,
  matched on the name's last `::` segment (`const_get("Zoo::#{x}")` could be
  any constant), and a field lookup by one (`instance_variable_get`,
  `instance_variable_set`, `instance_variable_defined?`,
  `remove_instance_variable` and their `class_variable` forms) for an
  instance or class variable: `instance_variable_get("@#{n}")` could be any.
- A local isn't renamed in a scope that reads locals by name at runtime
  (`binding`, `eval`, `local_variables`, a string `instance_eval`; an ERB
  template's `result(binding)`), and a method isn't renamed when it reads its
  own name (`__method__`, `__callee__`).
- A keyword parameter keeps its name (callers pass it), a renamed method
  doesn't call `super` (it would reach the parent's method of the new name),
  and global variables aren't renamed.
- Neither name of a method rename is a [reserved name](#reserved-names): the
  language calls those without a call site shapr can see.
- A public method of a Rails application's or engine's controller, mailer,
  job or channel (a file under `app/controllers/`, `app/mailers/`,
  `app/jobs/` or `app/channels/` at any depth, in a repository holding a
  `config/application.rb` or an `engine.rb` anywhere, so packs and engines
  count wherever they sit) isn't renamed: routes, templates,
  `deliver_later` and `perform_later` reach it by name. A job or mailer
  class elsewhere, and an engine declared in a file not named `engine.rb`,
  are documented limits.
- A class or module (in Rust a struct, enum, union, trait or `mod`) renames
  like a constant, held to every rule here: in a library's public surface
  it assumes `public_api`. It isn't renamed in a Ruby repository that loads
  classes by their file's names (any mention of Zeitwerk, or a Rails
  `config/application.rb`), nor is a Rust `mod name;`, whose body is a
  file named for it. Its name is also a runtime value (Ruby's `Class#name`,
  `inspect`, Marshal and YAML dumps, a Rails STI column; Rust's
  `std::any::type_name` and `module_path!`) that any code may read: as with
  every part, structure is a shape, not a claim that no output changes.
- A renamed dynamic reference (`send(:old)` → `send(:new)`) follows a method,
  constant or field rename that passes.
- Every definition of the new name in its scope after the change is a
  renamed one: new code defining it too (a second `def new` in another
  class, a new `let new`) would share the name with the renamed code.
- For a method, constant or field, no file outside the change holds the
  old name or the new name at the after revision ([Proof status](#proof-status)):
  an unchanged file mentioning the new name (a parent's method it would now
  override, a subclass's instance variable, a call to another method of
  that name, a class it would reopen) binds to the renamed code. Common
  names (`len`, `save`, `last`) are often held elsewhere, so their renames
  stay `logic`.

Scopes: a `local` (variable or parameter) lives in its enclosing method,
class or module; a `field` in its file when it is a Ruby instance or class
variable, and in every parsed file of the change when it is a Rust struct
field; a `method` or `constant` in every parsed file of the change.

`detail`:

| key | type | meaning |
|---|---|---|
| `renames` | array | One object per rename the part makes. |
| `from`, `to` | string | The old and new name. |
| `scope` | string | Where the old name lives, and so where every use had to change: `local`, `field`, `method` or `constant`. |
| `occurrences` | int | Uses this part renames, the definition included; a class or module name counts once. A rename spanning hunks lists the pair in each, so occurrences sum over parts to the change's. |

`evidence`, beside `comparison`:

| key | type | meaning |
|---|---|---|
| `renamed` | int | Sum of the part's renames' `occurrences`. |
| `uses` | int | Uses of the old names shapr saw in their scopes before the change, across the change. More than `renamed` when another hunk renamed or deleted some. |
| `searched` | int or null | Files the repository search read for uses of the old and new names outside the change (see [Proof status](#proof-status)); `null` when no name can be used outside its scope (locals), or no search ran. |

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
  `method_added`, `const_missing`, …): pattern matching's `deconstruct` and
  `deconstruct_keys`, `pp`'s `pretty_print`, `raise`'s `exception`, the
  error printer's `message`, `detailed_message` and `backtrace`, and the IO
  methods the standard library calls on an IO-like object (`read`,
  `readpartial`, `write`, `flush`, `close`).
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
| `binary` | Its content is binary as git reads content: a NUL byte in the first 8000 bytes, in a file shapr doesn't parse. A Ruby or Rust file holding a NUL is code (Ruby runs the code before it, Rust reads it as content), and an attribute that makes git diff a text file as binary (`-diff`, `binary`) doesn't count either: shapr diffs such a file again with `--text`; a diff on stdin can't be, so the file keeps its category by name, and its hunk is `unknown` with a reason saying to pipe `git diff --text`. |
| `generated` | `.gitattributes` marks it `linguist-generated`; or a lockfile, by name in any directory (`Cargo.lock`, `Gemfile.lock`, `Package.resolved`, `Pipfile.lock`, `Podfile.lock`, `bun.lock`, `composer.lock`, `flake.lock`, `go.sum`, `mix.lock`, `npm-shrinkwrap.json`, `package-lock.json`, `pnpm-lock.yaml`, `poetry.lock`, `pubspec.lock`, `uv.lock`, `yarn.lock`); or a minified asset (`*.min.js`, `*.min.css`). |
| `vendored` | Under a `vendor`, `third_party` or `node_modules` directory at the repository's top (deeper, such a directory is often the project's own namespace), or `.gitattributes` marks it `linguist-vendored` (`-linguist-vendored` unmarks one). |
| `fixture` | Under a `fixtures`, `__fixtures__` or `testdata` directory, at any depth, unless it is binary, vendored, or `.gitattributes` marks it `linguist-generated` or `linguist-documentation`. It wins over the name rules below and over `generated` by name: `spec/fixtures/Gemfile.lock` and `tests/fixtures/a.rb` are fixtures. Test data: not parsed or compared (`parsed: false`), outside the verdict, `review_lines` 0; the tests that read it judge it. |
| `types` | Type declarations only a type checker reads: an `.rbi` or `.rbs` file at any path, unless it is binary, vendored, a fixture, docs by name or attribute, or `.gitattributes` marks it `linguist-generated` (tapioca's `sorbet/rbi/gems/`, as it suggests). Like a fixture it wins over the name rules below: `spec/a.rbi` is types. Not parsed or compared (`parsed: false`), outside the verdict, `review_lines` 0; Ruby never loads it. A directory never makes a file types: a `.rb` under `sorbet/rbi/` or `lib/rbi/`, or a `sig/` crate's `.rs`, is code. |
| `docs` | `.gitattributes` marks it `linguist-documentation`; or it is under a `doc`, `docs` or `documentation` directory; or its extension is prose (`md`, `markdown`, `mdx`, `txt`, `rdoc`, `adoc`, `asciidoc`, `rst`, `org`, `textile`); or its name, extension aside, is `README`, `LICENSE`, `LICENCE`, `COPYING`, `CHANGELOG`, `CHANGES`, `HISTORY`, `NEWS`, `AUTHORS`, `CONTRIBUTORS` or `NOTICE` (any case). |
| `test` | Under a `spec`, `test`, `tests` or `__tests__` directory, or named `*_spec.rb`, `*_test.rb`, `*_test.rs`, `*_test.go`, `*_test.py`, `test_*.rb`, `test_*.py`, `*.test.*`, `*.spec.*`, `tests.rs` or `test.rs`. |
| `manifest` | A dependency manifest, by name: `Gemfile`, `gems.rb`, `*.gemspec`, `Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`, `Pipfile`, `Podfile`, `composer.json`, `mix.exs`, `pubspec.yaml`, `Package.swift`, `flake.nix`. |
| `config` | Extension `yml`, `yaml`, `toml`, `json`, `jsonc`, `json5`, `ini`, `cfg`, `conf`, `properties`, `env`, `xml` or `plist`; or a dotenv file (`.env`, `.env.*`). |
| `code` | A language shapr parses (see [Languages](#languages)). |
| `unknown` | Anything else: source in a language shapr doesn't parse, scripts, build files. |

A linguist attribute wins over the name either way: `-linguist-generated`
(or `=false`) unmarks a lockfile, and `-linguist-documentation` a `README`.
Attributes are read as `git check-attr` sees them in the revision the
change ends in (B of `A..B`, a GitHub change's head), or in the working
tree's `.gitattributes` when it ends there (one revision, a diff on stdin). A change can't hide its own edit: a file whose
category counts toward the verdict (`code`, `test`, `config`, `manifest`)
by its old path, or by either path under the attributes of the revision
the change starts from (whatever a path limit leaves out of the diff),
keeps that category. So moving code
into `vendor/`, marking a file `linguist-generated` in the change that
edits it, or adding a file and marking it generated, vendored or
documentation at once, leaves the edit `code`; an attribute takes a file out
of the verdict only when the change's before side already said so (a diff
on stdin has no before side, so there it never does when the diff edits a
`.gitattributes`). The path rules are not guarded this way: a file a
change adds under `vendor/` or `fixtures/` is `vendored` or `fixture` by its
name, outside the verdict, and `summary.categories` counts it.

Test code inside a code file (a Rust `#[test]` fn or `#[cfg(test)]` mod) is
not a category: it is the residual facet `test` (see [Facets](#facets)).

A file's category decides whether its hunks count toward the verdict (see
[Verdicts](#verdicts)). A `docs`, `fixture` or `types` file's `review_lines` is 0:
prose and test data aren't code to review. Generated and vendored files don't count toward size: their
`review_lines` is 0, and the summary's line sums, `behavior_lines`, `entropy` and
`structure_share` leave them out. Their own `added`, `deleted` and other
line counts stay as the diff has them. Generated-but-reviewed files such as
a Rails `db/schema.rb` aren't `generated` by name; mark them in
`.gitattributes` if they should be.

## Languages

`language` is `"ruby"` for `.rb`, `.rake`, `.gemspec`, `.ru` and the
extensionless DSL files RuboCop includes by default (`Gemfile`, `Rakefile`,
`Guardfile`, `Podfile`, `Vagrantfile`, `Capfile`, `Brewfile`, `Fastfile` and
the like; a file found only by its shebang is not) (parsed with prism) and `"rust"` for `.rs` (parsed with
tree-sitter). Any other file is `null` and its hunks `logic`.

A parsed file isn't compared, and its hunks are `logic` (verdict
`behavior`) with `parsed: false` and a reason saying why, when its code nests deeper than
10,000 levels, or when it is too big to compare: its changed lines (removed
plus added) pass 8,000 (4,000 lines rewritten in place, or 8,000 added),
or its changed bytes pass 192,000 (24 bytes, about a one-line method, count
as a line, so code packed onto a few long lines costs what it would on lines
of its own), or the change's compared files' sizes (changed lines, or changed
bytes in lines, whichever is more), squared and summed, pass 256,000,000,
charged smallest file first (so files named to sort first can't push a small
edit out); or when a side is over 4 MiB, or its widest node's children times
the other side's pass 16,777,216 (aligning two bodies builds a table of
that size). Comparison grows with the square of the changed code: removed
lines are paired with added ones, and a file only added has its
declarations compared with each other; a 4,000-line file shuffled, or 8,000
one-line methods added, take seconds. The largest Ruby or Rust file seen in
real repositories is about 3 MB.

The rules above
are stated in Ruby's terms; for Rust:

- Comments: `//`, `/* */` and doc comments (`///`, `//!`, `/** */`) are
  comments, so editing one, or turning a `//` comment into a `///` doc, is
  `comment_only`. A doc comment's fenced code is a doctest, which `cargo test`
  compiles and runs: its fence lines (info string included) and the lines
  between them are code, as is an indented code block (by rustdoc's Markdown:
  four columns past the text, or past an open list item's, after a blank
  line), and a `/** */` block holding either. A doc comment a macro may
  read as a `#[doc]` attribute is code too: one in a macro's arguments, or on
  or inside an item under a derive outside std or an attribute macro (a CLI
  parser's help text, a schema's descriptions). Attributes
  (`#[...]`, `#![...]`, `#[doc = "..."]`) are code, and belong to the item
  after them. A doctest's code isn't parsed into the tree, so a method,
  constant or field's name anywhere in it, as a word, counts as a use: a
  rename leaving the old name in a doctest, or onto a name one holds, is
  `logic`, and so is a doctest line the rename edits.
- A trailing comma (before a closing bracket, or ending a match arm) never
  changes the parse, so adding or dropping one is formatting, as rustfmt
  does when it reflows a list. A one-element tuple's comma is code. In a
  macro's arguments a trailing comma is code (the macro may match it),
  except in `vec!`, which takes either.
- A macro's arguments can't be parsed: each token is compared on its own
  (`a << b` and `a < < b` differ). A macro that may spell its tokens out
  (`stringify!`, `dbg!`, a message-less `assert!`, any macro outside std)
  prints their spacing too, so whitespace in its arguments is code; an
  attribute macro's item is compared ignoring whitespace. An identifier in
  a macro's arguments, or a `{name}` in one of their string literals, may
  mean anything. A rename leaving the
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
  told apart from another by its header (attributes, generics, trait and
  type, whitespace aside). An `impl`, trait or inline `mod` that doesn't
  move whole is opened like any namespace (see [New files](#new-files)), so
  its items pair on their own: an `impl` moved to another module file with
  one method changed moves its other methods.
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
| `command` | string | What produced the records that follow: `classify` (`shapr REV`, a range, paths, or a diff on stdin) or `norms` (`shapr norms`). Check it before reading `summary.verdict`, which a `norms` output doesn't have. |
| `source` | object or null | *Experimental.* The GitHub pull request or commit analyzed (see [GitHub input](#github-input)); `null` for a local change or stdin. |
| `input` | string or null | How the change was named (see [Input](#input)): `stdin`, `commit`, `range`, `merge_base_range`, `merge_base_worktree`, `worktree`, `staged`, `github_pr` or `github_commit`; `null` for `norms`. |
| `paths` | bool or null | Paths limited the diff (`shapr main..HEAD -- lib`); `null` for `norms`. |
| `untracked` | int or null | Untracked files git doesn't ignore under the paths (the whole repository without paths), which a change ending in the working tree (`worktree`, `merge_base_worktree`) leaves out, as `git diff` does; stderr names up to three and the `git add -N` that includes them. `null` for any other form, stdin included, and for `norms`. |

## Input

Each way of naming a change is a git command, and `meta.input` names it:

| shapr | same as | `meta.input` |
|---|---|---|
| `shapr REV` | `git show REV`: the commit against its first parent (a root commit against the empty tree) | `commit` |
| `shapr A..B`, `shapr A B`, `shapr R^!`, `shapr R^-N` | `git diff A..B` | `range` |
| `shapr A...B`, `shapr main...` | `git diff A...B`: from the merge base | `range` |
| `shapr --merge-base REF REV` | `git diff --merge-base REF REV`, that is `REF...REV` | `merge_base_range` |
| `shapr --merge-base REF` | `git diff --merge-base REF`: from the merge base of REF and HEAD to the working tree, the branch's committed and uncommitted work | `merge_base_worktree` |
| `shapr PATH...`, `shapr .` | `git diff HEAD -- PATH...`: uncommitted changes, staged and not | `worktree` |
| `shapr --staged [REV]`, `--cached` | `git diff --cached [REV]`: the index against HEAD, or against REV; uses are searched in the index | `staged` |
| no arguments | the unified diff on stdin | `stdin` |
| a GitHub pull request or commit | see [GitHub input](#github-input) | `github_pr`, `github_commit` |

Any form but stdin and GitHub takes paths after its revisions, `shapr main..HEAD
lib` or `shapr main..HEAD -- lib`, as git pathspecs read from the directory shapr
runs in (`-C`); `meta.paths` is then `true`. Arguments are read as git reads
them: revisions first, then paths; a name that is both a revision and a path
is a usage error (64) asking for `--`, which makes everything after it a path.
A merge commit named alone is an error (2) naming `REV^1..REV` and
`REV^2..REV`. The commit messages read for [claims](#claims) are those of the
commits the form spans that touch the paths.

## `file` record

| field | type | meaning |
|---|---|---|
| `record` | `"file"` | |
| `file` | string | Path after the change; the old path for a deleted file. |
| `old_file` | string or null | Path before the change; `null` for an added file. Differs from `file` on a rename. |
| `language` | string or null | The frontend that parsed it (`"ruby"` or `"rust"`, see [Languages](#languages)); `null` when none did. |
| `parsed` | bool | Every hunk in the file was compared as trees (`evidence.comparison` `"ast"`). |
| `category` | string | What kind of file: `code`, `test`, `docs`, `fixture`, `types`, `config`, `manifest`, `generated`, `vendored`, `binary` or `unknown`. See [Categories](#categories). |
| `lines_before` | int or null | Lines in the file before the change (Kamei's LT); 0 for an added file; `null` when its content couldn't be read. |
| `hunks` | int | Hunk records that follow for this file. |
| `added`, `deleted`, `code_added`, `code_deleted`, `comment_added`, `comment_deleted`, `review_lines`, `relocated_lines` | int | Sums over the file's hunks. |

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
| `relocated_lines` | int | Of `review_lines`, the residual's code lines the change removes or adds verbatim in another hunk. See [Relocated lines](#relocated-lines). |
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
| `detail` | object | Kind-specific facts about the change, such as rename pairs. Empty (`{}`) unless the kind's section documents it. A residual that resembles a structural kind it couldn't be proven to be carries `near` (see [Extract and inline](#extract-and-inline) and [Confidence](#confidence)), and one with lines relocated verbatim carries `relocated` (see [Relocated lines](#relocated-lines)). |
| `facets` | string array | What kind of logic change the residual is, in the table's order; empty for every other part. See [Facets](#facets). |
| `assumptions` | string array | What the part relies on that shapr couldn't verify, in the [Proof status](#proof-status) table's order; empty for a proven part and for the residual. |
| `review_lines` | int | Of its lines, the code lines a reviewer still has to read. A residual: all of them, or every changed line when it holds no code line (comments or blank lines no part proved, such as a comment edit refused because it moves a line that reads its own position). `format_only`, `comment_only`, `rename`, `default_argument`, and a `move` that is structure: 0. A `rename` or `default_argument` with assumptions: all of them. A `move` into new surroundings or with assumptions: the moved definition's first code line, where the new surroundings show. A part that isn't structure whose picked lines aren't among its own changed ones (an arbitrary moved element left unchanged): all of its lines. `extract` and `inline`: all of them; they are structure but still need reading. Any part: 0 where the hunk's `review_lines` is 0 by its category or a `none` verdict. |

## Evidence

| key | type | when | meaning |
|---|---|---|---|
| `comparison` | string | always | `"ast"`: both sides parsed cleanly and their trees were compared. `"line"`: a side has a syntax error, so the region's text was compared, whitespace and comments aside. `"none"`: nothing was compared (no frontend for the file, a binary file, or content that couldn't be read). |
| `code_similarity` | number 0–1 | `comparison` `"ast"` and the code differs | Dice coefficient over the subtrees of the two sides' changed code: 1 when they share every subtree, 0 when none. |
| `respelled` | int | a `restyle` part | Aligned subtrees the part respells. |
| `restated` | int | a `default_argument` part | Arguments restating a default the part drops or adds. |
| `annotations` | int | a `typing` part | Type annotations on the part's lines, both sides. |
| `unchecked` | int | a `typing` part | Of `annotations`, those no runtime checks. |

## Extract and inline

An `extract` or `inline` is proven, not scored: the caller is identical on both sides once the call is swapped for the statements (the *chunk*), the new method's body equals the chunk once its parameters are renamed to the call's arguments, and the methods' container still holds the same other code, in order, apart from siblings that changed on their own. A bare `private`, `protected` or `public` line added or removed with the new method, whose section holds nothing but it, is part of the refactoring. A sibling's changed lines aren't part of the refactoring: they stay `logic`, beside the refactoring's part when they share its hunk. The new method's parameters must be plain required positionals, the call's arguments plain local variables, and a value used after the chunk comes back as the call's result (`x = b(...)`). Every hunk the refactoring touches gets a part, or none does (when another part already explains a line it would).

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
| `risk` | string | `"jump"`: the chunk holds code bound to its method or its place: `return`, `break`, `next`, `redo`, `retry`, `yield`, `super`, `block_given?` or a blockless `proc`/`lambda`/`Proc.new` (control flow and the method's block); `binding`, `eval`, `local_variables` or a string `instance_eval`/`class_eval` (the method's locals by name); `__method__`, `caller`, `caller_locations`, `$~`, `$_` or a match reference (`$1`, `$&`) (the method's frame); `__LINE__` or Rust's `line!()`/`column!()` (the code's line); and in Rust `?`, `.await`, `break`, `continue` or a macro outside std. Also when the caller reads its frame anywhere (`binding`, `$1`): the chunk's locals and matches leave it, or arrive; and in Ruby when the chunk or the call sits in a block (or another body opening its own `self`) inside the caller, since `instance_eval`, `class_exec` or a DSL may run it with another `self`. `"block_parameter"`: it uses the caller's `&block`. `"wiring"`: the arguments aren't exactly the chunk's free variables in parameter order, the chunk assigns a local of the caller, or a value used after the chunk isn't returned. `"collision"`: the method's name is a [reserved name](#reserved-names), or already appears, in any file of the change, on the side where the method doesn't exist (a def, a call, a `send`, a plain literal such as `:name`, as a `method_missing` compares): defining it would capture that use, removing it would break it; or another def of the name is on the method's own side, so the method takes part in dispatch beyond its one call; or a file outside the change holds the name at the after revision (a subclass's method of that name, a caller of the removed method), which the `reason` names. `"drop"` (Rust): code follows the chunk in its block and the chunk binds a local other than the returned one, or takes arguments, so a value would be dropped at another point. `"tiny"`: under 8 nodes. |


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
| `literal_only` | Only literal values changed: config-like. | The changed regions are equal once every literal (number, string, symbol, boolean, nil, regexp; a string's text inside an interpolation) and comment is masked, and hold the same number of literals, but not the same code reordered (swapped match arms or arguments). A symbol naming code (`send(:x)`, see `DynamicName` in the rename rules) is not a literal here. |

Code inside a definition the residual adds or removes doesn't add `condition` or
`error_handling`: `adds_definition` already says all of it is new.

Facets describe; they don't judge intent. A fix and a feature both change
conditions. Claims about intent come from messages, kept apart from the code's
facts.

## Claims

`summary.claims` holds what the messages around the change *claim*: the
commit messages of the change (`git log` over the commits its
[input](#input) spans: `shapr R` reads R, `A..B` and `A...B` the commits in B
and not A, `--merge-base REF` those from the merge base to HEAD, paths alone
none) and the description given with `--message FILE` (a PR body; `-` reads
stdin when the change is named on the command line). A piped diff has no commit
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
| `hints` | array | Each hint, in message order, once per message. A description whose subject repeats a commit's subject, case, spacing and closing punctuation aside (a pull request titled after its one commit) adds no subject hints. Below. |
| `experimental` | object | *Experimental*: judgements drawn from the hints. Below. |

Each hint:

| key | type | meaning |
|---|---|---|
| `hint` | string | `type`: a conventional-commit type (`fix:`, `feat(api):`, known types only). `keyword`: a purpose word in the subject (`fix`, `bug`, `regression`, `crash`, `hotfix`; `add`, `introduce`, `support`, `implement`, `allow`; `refactor`, `rename`, `move`, `extract`, `tidy`, `cleanup`, `simplify`, `reorder`, `alphabetize`, `dry`; `perf`, `faster`, `optimize`; `test`, `spec`, `coverage`; `docs`, `readme`, `typo`, `changelog`; `rubocop`, `autocorrect`, `lint`, `linting`, `reformat`; `chore`, `bump`, `deps`, `upgrade`, `release`; `revert`), not inside a path or identifier (`spec/support`, `spec_helper`, `A::move`) and not after `no`, `not a` or `not an` ("an edit, not a rename"). `revert`: a git revert's `Revert "subject"` (the quoted subject, whose own words are not read as claims) or its `This reverts commit <id>`. `issue`: `KEY-12` (standards such as `UTF-8` aside), an issue URL (as `#12`), or a `#12` the GitHub API says is an issue. `pull_request`: a pull URL (as `#12`), or a `#12` the API says is a pull request. `reference`: a `#12` not looked up: GitHub numbers issues and pull requests alike, so offline it could be either. For GitHub input, the first 10 distinct `#N` are looked up in the change's repository; a failed lookup leaves a `reference`. Schema change: `issue` no longer covers a bare `#N`. `breaking`: `type!:` or a `BREAKING CHANGE:` footer. `no_behavior_change`: "no behavior change", "no functional change", "no logic change", "behavior preserving", "pure refactor" (any case, hyphens as spaces), the word `NFC`, or a subject closing on ", unchanged" ("moves to cli/refs.rs, unchanged"). `test_only`: "test only", "tests only", "spec only" or "specs only" (any case, hyphens as spaces), not after `not`. |
| `value` | string | What matched, lowercased for types and keywords. |
| `category` | string or null | The category the hint claims: `feat` is `feature`, `fix`, `fixes`, `fixed` and `fixing` beside a style keyword (`rubocop`, `lint`, ...) are `style` ("Fix lint errors"), `build` and `ci` are `chore`, `no_behavior_change` is `refactor`, `test_only` is `test`, an `issue`, `pull_request` or `reference` after `fix`/`fixes`/`fixed` is `fix`; `null` for `breaking` and other references. |
| `source` | string | `commit`, `message` (`--message`), or `pull_request` (a GitHub pull request's title and body). |
| `commit` | string or null | The commit's id, 12 hex digits; `null` for a description. |
| `field` | string | `subject` (the first non-empty line) or `body`. Purpose words are read only in the subject, where they summarize; a body mentions too much in passing. |

Hints are facts: what a message says, where. `experimental` holds
judgements built on them by keyword rules that haven't been measured against
labelled changes; its fields and rules may change in any release before 1.0.

| field | type | meaning |
|---|---|---|
| `categories` | object | Messages carrying at least one hint of each category, every category present: `fix`, `feature`, `refactor`, `perf`, `test`, `docs`, `style`, `chore`, `revert`. |
| `conflicts` | array | Claims the code doesn't bear out. Below. |

Each conflict is a count of what the code shows, not a verdict on the author:

| key | type | meaning |
|---|---|---|
| `claim` | string | The claimed category. |
| `finding` | string | `behavior_hunks`: the claim is `refactor` or `docs`, no message claims `fix`, `feature`, `perf` or `test`, and shapr couldn't prove `count` hunks structural (their `verdict` is `behavior` or `mixed`). `non_test_behavior_hunks`: the same for a `test` claim, and for a `refactor` or `docs` claim beside one, counting behavior hunks outside the `test` category whose residual lacks the `test` facet. The text output says `unproven` for these two. `logic_hunks`: the claim is `style`, no message claims `fix`, `feature` or `perf`, and `count` hunks in code or test files (outside tests beside a `test` claim) aren't shown structural; config and manifest hunks, a linter's own setup, aren't counted. `no_reorder_parts`: a `reorder`, `reordered`, `reordering` or `alphabetize` keyword claims `refactor`, no message claims a behavior change, and none of the `count` hunks in the verdict holds a `reorder` or `move` part. The text output says `unproven` for these two as well. `no_behavior_hunks`: the claim is `fix`, `feature` or `perf` and the change's verdict is `structure`. |
| `count` | int | The hunks behind the finding, `hunks`' length (0 for `no_behavior_hunks`). Hunks outside the verdict (`none`, `unknown`) aren't counted: they aren't evidence. |
| `hunks` | array | The `id`s of the hunks behind the finding, for pointing at them; empty for `no_behavior_hunks`. |

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
a block or conditional). Each file's trees are aligned body by body, nodes
the diff left alone first, then larger nodes first, so of two swapped
definitions the one git shows moving is the unit; a unit the alignment
leaves out was removed or added. A removed unit pairs with an
added one, in the same file or another, by equal tree or, failing that, by
equal name and kind with `similarity` at least 0.5 (a move whose body also
changed). Definitions align by name, so one that keeps its place among its
siblings was edited, not moved.

A pair is not structure, even with an unchanged body, when the code around
its new place differs. Its `context` names each difference:

| context | meaning |
|---|---|
| `module` | Rust, across files: each file is a module, so the paths it resolves and its own path change. |
| `owner` | A different enclosing class, module or `impl` (a Rust namespace's attributes count, inner ones too: `#[cfg(unix)] mod imp` is not `#[cfg(windows)] mod imp`, nor `impl S { #![cfg(test)] }` a plain `impl S`). |
| `visibility` | A method in a different visibility section: it crossed a bare `private`, `protected`, `public` or `module_function`. A constant or class isn't affected by one. |
| `load_order` | A different order relative to a sibling that names it or that it names at load time (`alias_method`, `private :x`, a redefinition, a constant read, a Rust `macro_rules!` of a macro it invokes) or any statement that runs code at load time (a `require`, a DSL call, an `include`, a constant whose value calls something: `X = Counter.next`), except beside a Rust function, whose definition runs no code, where only an item-level macro invocation or attribute macro counts; for a Ruby method also a `method_added` or `singleton_method_added` definition and, below one, every other method of its class (the hook sees them in order). A definition moved between two containers (two openings of a class, two `impl`s or modules) is compared with the statements around each container too, so it crosses what stands between them. Also a Ruby definition moved into a file the change adds, unless the new file is shown to run where the code ran (see [New files](#new-files)). |
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
| `moves` | array | One entry per pair touching the hunk, below; in a hunk that holds a new file's directives or the statement loading it, the pairs moved into that file. |

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

### New files

A namespace the alignment leaves out that pairs with nothing as a whole is
opened, outermost first: its declarations pair on their own. So a class
moved into a new file inside `module Zoo … end`, or a method moved into a
new file that reopens its class, pairs with what it left. The opened
namespace is part of the move when every member moved, and its header must
say what the old place's does: the same kind (`class` or `module`), and the
same header, superclass included, or only the name; otherwise context
`owner`. A header that is more than a name (a superclass), or only the name
where the old one has more, holds only once the old file has declared the
namespace, so it is `load_order` unless the new file is shown to run after
the old one. A file the change adds or deletes whose every statement moved
moves its directives too.

A Ruby file the change adds runs only when something loads it, so a
definition moved into one is `load_order` unless the new file is shown to
run its code where it ran before, nothing in between depending on it (the
same test as a reorder: a statement naming it at load time, or for a class
or constant any statement that runs code), in one of two ways:

- The old file `require`s (or `require_relative`s) the new one at its top
  level: the code now runs there, and every other load of the new file
  follows that one or a load of the old file.
- Every load of the new file directly follows a load of the old one in the
  same statement list, and every load of the old file (there is at least
  one) is directly followed by one of the new: it runs right after the old
  file. In a gem's `lib/`, another package may require the old file alone,
  so the part assumes `public_api`.

Loads are found in the change and, by the files' names as words, in the
repository at the after revision. A bare `require 'a/b'` names `a/b.rb`
under a `lib`, `spec` or `test` directory or the repository's top. Either
way no Ruby file may load files by a name computed at runtime (`require f`,
a `Dir` glob, `load`, `autoload` with a computed path, any mention of
Zeitwerk), and an `autoload` or `load` of either file is not a load that
proves anything. When shown, the statements loading the new file are part
of the move too.

## Relocated lines

A run of residual (`logic`) lines whose text, whitespace aside, the change
removes in one hunk and adds in another, in the same file or another, is
*relocated verbatim*, as git's `--color-moved` shows it. It is a fact about
the residual, not a structure claim: no line changes part, kind or verdict.
A reviewer who read the code where it was reads the runs where it landed
as a comparison; `review_lines` still counts them, and `relocated_lines`
says how many of those are relocated, so a reader may discount them.

A run is consecutive residual lines on one side of a hunk (blank lines
inside it are skipped, not compared), matched line for line, in order,
against consecutive residual lines on the other side of another hunk.
Lines match as the trees compare code: string content, heredoc bodies and
other leaves byte for byte, elsewhere each run of whitespace as one space and
none at the line's ends, so indentation never counts but `offset -1` (a call)
doesn't match `offset - 1` (a subtraction), nor `", "` match `","`. It
needs 3 lines of two words or more (a lone `end`, `}`, `else` or name is
too common to show anything moved). Matching is unique or nothing: of the
removed runs matching from a line, the longest wins, and when two are as
long, or the same text is added in another place too, nothing is recorded.
A removed line pairs with one added line at most. Only files shapr parsed
have text to compare.

`detail.relocated` on the residual, one entry per run:

| key | type | meaning |
|---|---|---|
| `lines` | line run | The run, on its side of this hunk: added lines when the entry has `from`, removed lines when it has `to`. Blank lines inside it are included. |
| `from` | object | For added lines: where the change removes them, `file` (the path before the change) and `line` (the first). |
| `to` | object | For removed lines: where the change adds them, `file` and `line`. |

`relocated_lines` on hunk, file and summary records counts the runs' code
lines that are in the residual and in `review_lines` (0 where those are).
The text output notes them under the hunk (`4 lines relocated verbatim from
lib/report.rb:3`) and in the summary line.

## Style

*Experimental.* A norm is a style choice a project makes one way, such as
RuboCop's `Style/StringLiterals` (`single_quotes` or `double_quotes`). shapr
infers each norm from the code itself and judges only new code against it.
Deleting code chooses no style, and neither does code carried over
unchanged or edited in place: an added observation matched by a removed one with
the same text and style, or failing that the same style, is kept, not chosen. So is code
the change deletes anywhere and adds back, moved to another method or file:
an observation whose lines the diff also deletes somewhere as the same run
of lines, ignoring indentation (the indentation norms keep it, as it is their style).

Ruby's norms, named by the RuboCop cop whose setting configures them:

| norm | styles | observed on |
|---|---|---|
| `Style/StringLiterals` | `single_quotes`, `double_quotes` | each quoted string that could take either quote with the same value, as RuboCop counts them |
| `Layout/SpaceInsideHashLiteralBraces` | `space`, `no_space` | each non-empty hash literal with spaces inside both braces or neither; a brace beside a line break chooses nothing (RuboCop's `compact` reads as `space`) |
| `Style/TrailingCommaInArrayLiteral`, `Style/TrailingCommaInHashLiteral` | `comma`, `no_comma`, `single_line_comma` | each multiline `[]` or `{}` literal with every item on its own line, after the bracket's line, and the closing bracket on a line of its own (where RuboCop's `comma`, `consistent_comma` and `diff_comma` all read as `comma`); a heredoc body before the bracket chooses nothing. A one-line literal with a trailing comma is `single_line_comma`, an offense under every RuboCop style, weighed only where seen; its edit removes the comma |
| `Style/TrailingCommaInArguments` | `comma`, `no_comma`, `single_line_comma` | the same, for a call's arguments in `()` or `[]` (`Set[...]`), each keyword argument an item; a call ending in a `&block` argument, which can't take a comma, chooses nothing |
| `Style/HashSyntax` | `ruby19`, `hash_rockets` | each pair of a hash or keyword arguments whose key is a symbol: `a: 1` or `:a => 1` where the symbol can be a label, a quoted label `"a-b": 1` as `ruby19`. As in RuboCop's `ruby19`, a rocket counts only when every key in its hash can be a label; a hash with an omitted value (`{a:}`) chooses nothing. `ruby19_no_mixed_keys` reads as `ruby19` |
| `Layout/IndentationConsistency` | `normal`, `indented_internal_methods` | each method defined after a bare `private` or `protected` in a class or module body: at the modifier's column, or deeper. As in RuboCop, the method is measured from its modifier, outdented or not; `public` ends the section |
| `Layout/TrailingWhitespace` | `no_trailing_whitespace`, `trailing_whitespace` | each non-empty line up to `__END__`, but not a line whose break is inside a string, heredoc, regexp or quoted symbol, where whitespace is data. `.rubocop.yml` configures it by enabling the cop; `.editorconfig` by `trim_trailing_whitespace = true` |
| `Layout/TrailingEmptyLines` | `final_newline`, `final_blank_line`, `no_final_newline` | how each non-empty file ends: one line break, more, or none; one observation per file, so a file's own code never backs a norm and its directory or the configured style does. `.editorconfig`'s `insert_final_newline` sets `final_newline` (`true`) or `no_final_newline` (`false`) |
| `Layout/IndentationWidth` | the width in spaces: `2`, `4`, ... | each body's first statement, when it starts its own line, against what opens the body: a `def`, `class`, `module` or block from the start of its line; `if`, `unless`, `while`, `until`, `case`/`when`, `else`, `begin`, `rescue` and `ensure` from their keyword when it starts its line (so `x = if` chooses nothing). Tab-indented lines choose nothing. No suggested edit: reindenting a body reaches past the line. RuboCop's `Width`, or `.editorconfig`'s `indent_size` (`tab_width` for `tab`) unless `indent_style = tab`, configures it |
| `Naming/MethodName` | `snake_case`, `camelCase` | each `def` whose name has two or more words, ignoring a leading `_` and a trailing `?`, `!` or `=`: lowercase words joined by `_`, or a lowercase start with capitals and no `_`. A one-word name chooses nothing. A rename is suggested only for a private instance method (in a `private` section, or `private def`) whose every mention in the file is its `def` or a call with no receiver or `self`, and whose new name the file doesn't already use, and only when the repository's other files (searched as [Proof status](#proof-status) searches a rename's) hold neither name: a subclass elsewhere may call a private method, and a method elsewhere by the new name would be overridden; any other method is interface, reported with no edit |
| `Naming/VariableName` | `snake_case`, `camelCase` | each local of two or more words, once per scope that declares it (program, class or module body, method, block), at its first mention; a keyword parameter is interface and chooses nothing. A rename covers the scope and is suggested when every mention of the name there is that local (not a method call, symbol, string, comment or `{name:}` key), the new name is unused there, and nothing in the scope reads locals by name (`binding`, `eval`, `local_variables`) |
| `Layout/SpaceAroundOperators` | `space`, `no_space`, `extra_space` | each binary operator call (`+`, `==`, `<<`, ...), `&&`, `\|\|`, assignment (`=`, `+=`, `\|\|=`, attribute and index writes) and `=>` (a pair's or `rescue`'s): whitespace on both sides is `space`; anything less is `no_space`; two or more spaces after the operator, on its line, are `extra_space` unless they align the operand with a word on a nearby line, as RuboCop's `AllowForAlignment` reads it (`extra_space` weighs only where seen; its edit leaves one space). Extra space before an operator reads as `space`. `**`, unary operators, `and`/`or`, rational literals (`1/3r`) and default parameters (`a=1`) aren't observed. Only the edit to `space` is suggested: removing whitespace can change a parse (`a <<b`). `.rubocop.yml` configures it by enabling the cop |
| `Layout/EmptyLineBetweenDefs` | `empty_line`, `empty_lines`, `no_empty_line` | the gap between two adjacent `def`s in a body: one blank line in it, two or more, or only comments or nothing (`empty_lines` weighs as a style only where some gap has it, as `Layout/IndentationWidth` counts its widths). The observation spans the first's last line to the second's first, so a line added on either side, or a deletion in the gap, judges it; a one-line def (RuboCop allows two adjacent; here one beside a longer def chooses nothing too) and defs sharing a line choose nothing. The edit to `empty_line` inserts a line break after the first `def`, inside the hunk when either side of the gap is, or removes the extra blank lines. `.rubocop.yml` configures it by enabling the cop, with `NumberOfEmptyLines` above 1 as `empty_lines` (a range sets no style) |

The evidence is the unchanged code around the hunk, per scope from the
nearest: the enclosing method, class and file, each counting only what lies
outside the nearer scopes, then the directory's other files (the first 20 of
the language by path) as they were before the change: a file the change adds
doesn't count and one it edits counts as it was, so a change never backs the
norm it is judged by. A moved file is judged by its new directory. The scopes are pooled from the outside in: the outermost scope
holding any observations starts from the rule of succession, `(n + 1) /
(total + styles)`, and each nearer one shrinks its own share toward that,
`(k + w × p) / (n + w)`, where `p` is the share so far and `w` the weight it
lends: 4 observations' worth, or fewer when the scopes around hold less. So a
method with a few observations follows its surroundings, one with many speaks
for itself, and a lone scope reads exactly as the rule of succession: three
of three backs a norm, two of two abstains. A style is the norm when its
pooled share is at least `--norm-threshold` (default 0.8). The weight 4 is
the one that best predicted the style of new code over 30,000 corpus hunks
([decisions](decisions.md) #79). When no style reaches the threshold and the
hunk's scope moved wholesale from one style to another, that move is the norm
(`renorming`). With no norm inferred, the style the project configures is
expected instead.

Configured styles come from `.rubocop.yml` (the nearest one, with local
`inherit_from`) and `.editorconfig` (each one from the file's directory up
to the first with `root = true`, nearer files and later sections winning,
section globs matched against the file as editorconfig.org specifies).
Where both set the same norm, `.rubocop.yml` wins: it is the Ruby-specific
file. The table above says which settings configure each norm.

Where RuboCop wouldn't check a norm's cop, its config says nothing: a file
under `AllCops` or the cop's `Exclude`, or outside the cop's `Include`
(patterns matched as RuboCop's `match_path?` does: `File.fnmatch?` with
`FNM_PATHNAME` and `FNM_EXTGLOB`, so `*` skips a leading `.`, `**/` skips
hidden directories and `{a,b}` nests, and a hidden file in no hidden
directory matched with `FNM_DOTMATCH` as well; the directory of the `.rubocop*` file that lists them; a pattern shapr
can't read, such as a regexp, counts against the file), and a line under
`# rubocop:disable` or `# rubocop:todo` for the cop, its department or `all`
(to the matching `# rubocop:enable`, or that line alone after code). There
the norm has no `configured` style and its deviations no `edit`; the style
the code itself backs still judges, as it is the code's own evidence, not
RuboCop's.

Config, and the directory's files the change didn't touch, are read as of the change's after side:
from the revision a range ends in (`B` of `A..B`; a GitHub change's head),
or from the index or the working tree when the change ends there (one
revision, or a diff on stdin as [Proof status](#proof-status) reads it; the
working tree for any other diff on stdin). Nothing above the repository's top is read.

The hunk's `style`:

| field | type | meaning |
|---|---|---|
| `skipped` | string or null | Why the file isn't judged: `generated`, `vendored`, `docs` or `fixture` (by [category](#categories), plus `db/schema.rb` and `vendor`, `third_party` and `node_modules` directories), or `syntax_error`. |
| `direction` | string | The strongest of its norms' `direction` (`diverging`, then `converging`, `aligned`, `abstain`); `none` when no norm saw new code. One norm aligned beside another that abstains reads `aligned`: each norm's own `direction` says what backs it. |
| `flagged` | bool | Some norm is `flagged`. |
| `norms` | array | One per norm that saw new code in the hunk. Below. |

Each norm:

| field | type | meaning |
|---|---|---|
| `norm` | string | The norm, by its RuboCop name. |
| `before`, `after` | object | Per style, the observations on the hunk's `-` and `+` lines. |
| `scope` | string or null | The nearest scope with observations, whose pooled share backs the norm: `method`, `class`, `file` or `directory`; `null` when none backs one. |
| `evidence` | array | Per scope, nearest first: `scope` and `counts` (per style, the scope's unchanged observations outside the nearer scopes). Every scope of the hunk's chain is listed, empty ones too, so an abstention shows what it had; when renorming, the one scope's whole after side. Schema change: replaces `surrounding`. |
| `previous` | object or null | When renorming, per style, the scope's whole before side. |
| `inferred` | string or null | The style the scope backs. |
| `confidence` | number 0–1 or null | Its pooled share, as above, to 2 decimals and at most 0.99: finite counts never make a norm certain. |
| `configured` | string or null | The style `.rubocop.yml` sets; `null` when it sets none, disables the cop, or wouldn't check the hunk's new code (above). Reported beside `inferred` even when they differ. |
| `expected` | string or null | `inferred`, else `configured`. |
| `direction` | string | `aligned` (new code follows `expected`), `converging` (it does, and replaces code that didn't, or the scope renormed), `diverging` (some new code doesn't) or `abstain` (nothing expected). |
| `convergence` | string or null | When `converging`: `normalizing` (toward the surrounding norm) or `renorming` (the scope moved to a new one). |
| `restyle` | bool | The hunk's [`restyle`](#restyle) part respells this norm's variant (the norm's name is a variant's): a respelling, not new code. Layout and naming norms are never a restyle; their respellings are `format_only` and `rename` parts. |
| `flagged` | bool | `restyle` and `diverging`: a change that respells code away from its norm. |
| `deviations` | array | New code not in the `expected` style: `line`, `column`, `style`, `expected`, and `edit`, a unified diff that applies the expected style (in the file's own line endings), `null` when there is no mechanical fix, it would reach outside the hunk's changed region, or the edited file wouldn't be the same program: each edit is applied and the file re-parsed, and it must parse cleanly with an equal canonical tree (whitespace, comments and the spellings [Restyle](#restyle) proves the same may differ); a `Naming/` rename need only parse cleanly. Deviations whose edits share a line, of one norm or several, share one edit that applies them all, checked as a whole; edits that overlap keep their own. |

## `shapr norms`

*Experimental.* `shapr norms PATH...` answers "what are this code's norms,
where is it least like itself, and where do I start?" with no diff. Each
PATH is a file, or a directory standing for every file under it a language
has norms for (inside a repository, git's tracked and unignored files);
none is `.`. A file found under a directory that is generated, vendored or
docs (as in a hunk's `style.skipped`) is counted in `summary.skipped`, not
read or listed.

Records, in order: a `meta` record (`command` `norms`), a `file` record per
file named or judged, a `norm` record per norm, a `hotspot` record per file,
class and method holding a deviation, a `step` record per plan step, then a
`summary`. Under `-j` they are one object: `meta`, `files`, `norms`,
`hotspots`, `steps`, `summary`. Paths are as named, or as found under a named
directory.

`file`: `file`, `language` (`null` when no language has norms) and
`skipped` (as in a hunk's `style`, or `unsupported`: no language has norms
for it, or it isn't a regular file inside the repository, such as a symbolic
link resolving outside it; `null` when judged).

### `norm` record

Each norm pools every judged file: its counts are the whole codebase's, and
a style is the norm by the rule of succession over them, as a lone scope is
in [Style](#style). The code's own style is the reference for normalizing
it, so a file written wholly in another style is a deviation, not a norm of
its own.

| field | type | meaning |
|---|---|---|
| `record` | `"norm"` | |
| `norm` | string | The norm, by its RuboCop name. |
| `scope` | string | What the counts pool: `file` (one file named) or `paths` (every file under the paths named). |
| `files` | int | Files holding an observation of the norm. |
| `counts` | object | Per style, the observations. |
| `inferred`, `confidence` | string, number or null | The style the counts back and its share, as in [Style](#style). |
| `configured` | string or null | The style `.rubocop.yml` (or `.editorconfig`) sets, where it sets the same one in every file that sets one; `null` when none sets one or files differ. |
| `expected` | string or null | `inferred`, else `configured`. |
| `adherence` | number 0–1 or null | `matching` over `judged`, to 2 decimals; `null` when nothing is expected. |
| `matching`, `judged` | int | Observations held to the expected style (every one when a style is inferred; with only config, those config checks), and those in it. |
| `config` | object or null | How far the code is from config, when `configured` is set. Below. |
| `step` | int or null | The plan step that fixes the norm's deviations. |
| `unplanned` | string or null | Why a norm with deviations has no step: `config_disagrees` (the inferred and configured styles differ; changing the code or the config is a person's call, so the plan does neither), `withheld` (every deviation is where config doesn't check the cop), `no_edit` (no deviation has an edit that keeps the program); `null` otherwise. |
| `deviations` | array | Observations not in the style they're held to: `file`, `line`, `column`, `style`, `expected`. |

`config`, the norm's trajectory against its config. It never lists
RuboCop's offenses line by line; it says how far apart code and config are:

| field | type | meaning |
|---|---|---|
| `checked`, `matching` | int | Observations config checks, and those in the configured style. |
| `adherence` | number 0–1 or null | `matching` over `checked`, to 2 decimals. |
| `withheld` | int | Observations config doesn't check: the file is outside the cop's `Include` or under an `Exclude` (`.rubocop_todo.yml` writes these), or a `# rubocop:disable`/`todo` covers the line. |
| `todo` | bool | A `.rubocop_todo.yml` the config inherits lists the cop: offenses the project parked. |
| `agreement` | string | `agrees` or `disagrees` with `inferred`; `config_only` when nothing is inferred. |

### `hotspot` record

Where the code is least like itself. Each kind is ranked on its own: by the
norms it breaks, then deviations per line, then deviations. A deviation
where config doesn't check the norm's cop (`withheld`) isn't counted: the
project chose not to check it there.

| field | type | meaning |
|---|---|---|
| `record` | `"hotspot"` | |
| `kind` | string | `file`, `class` (a class, module or `class << self`) or `method`. |
| `rank` | int | 1-based, within its kind. |
| `file` | string | |
| `name` | string or null | As the source spells it: `Cart`, `Shop::Cart`, `Cart#total`, `Cart.build` (a singleton method); `null` for a file. |
| `lines` | object | A line run: the unit's first line and its lines. A class counts its own lines, not a nested class's. |
| `deviations` | int | Deviations in it; a class's own, not a nested class's; a method's in its innermost `def`. |
| `norms` | array of strings | The norms they break, sorted. |
| `per_100_lines` | number | `deviations` per 100 of its lines, to 1 decimal. |

### `step` record

The cleanup plan: one step per norm with deviations an edit fixes, ordered
by risk tier, then by adherence gained per line touched (fixes over
`judged`, over `lines_touched`). Tier 1 (`layout`) is whitespace and blank
lines, tier 2 (`lexical`) respellings proven to keep the program (quotes,
hash syntax, trailing commas), tier 3 (`naming`) renames. Each edit is a
deviation's own fix, offered only where a hunk's would be: config checks the
line, the file's config sets no other style, and the edited file parses
cleanly to the same canonical tree (a rename need only parse, and a
`Naming/MethodName` rename also needs neither name in another file of the
repository). Steps are written against the files as the steps before them
leave them: apply them in order, all of a tier's steps for one pull request
per tier, and each tier's change classifies as its `expect_kind`.

| field | type | meaning |
|---|---|---|
| `record` | `"step"` | |
| `step` | int | 1-based order. |
| `tier`, `risk` | int, string | 1 `layout`, 2 `lexical`, 3 `naming`. |
| `expect_kind` | string | The kind shapr gives a change made of the step's edits: `format_only`, `restyle` or `rename`. |
| `norm`, `style` | string | The norm, and the style its edits apply. |
| `fixes` | int | Deviations its edits fix. |
| `unfixed` | int | The norm's deviations it leaves: no edit, an edit the guard refused, or `withheld`. |
| `withheld` | int | Of `unfixed`, those where config doesn't check the cop, with an edit or not. |
| `adherence_before`, `adherence_after` | number 0–1 or null | The norm's adherence before and after the step, to 2 decimals. |
| `files`, `lines_touched` | int | Files and lines its edits change. |
| `edits` | array | Per file: `file`, `lines` (the lines it changes, as the steps before leave the file) and `diff`, a unified diff with three lines of context that `git apply` takes; a path holding a quote, backslash or control character is quoted in its headers as git quotes it (`"a/dir\nname/f.rb"`). |

For a pull request per tier:
`shapr norms -J | jq -r 'select(.record == "step" and .tier == 1) | .edits[].diff' | git apply`.

`summary`: `files` (judged), `skipped` (per reason: `generated`, `vendored`,
`docs`, `fixture`, `syntax_error`, `unsupported`), `norms`, `inferred` (norms with a
style inferred), `deviations`, `hotspots`, `steps`, `fixes`,
`lines_touched` (over the steps) and `config_unread`: config the files fall
under that shapr didn't read, one note each (`inherit_gem: NAME`,
`inherit_from: PATH (not found)`, `(remote)`, `(cycle)` or `(past 32 config
files)`, `FILE: lines N not read`, `FILE: pattern P not read (over 256
alternatives)`, `FILE: line N not read (a section glob over 1024 characters)`, `FILE: lines N not read
(matching its globs against a path this long passes 1000000)`: sections past a
budget of glob characters times path characters, `FILE: not read (over 1 MiB)`),
so their norms were judged without it. Each note is also one stderr line,
under `shapr norms` and a change's hunks alike.

## `summary` record

| field | type | meaning |
|---|---|---|
| `record` | `"summary"` | |
| `verdict` | string | `structure`, `behavior`, `mixed`, `none` or `unknown`, over the hunks that count. See [Verdicts](#verdicts). |
| `verdicts` | object | Hunk count per hunk `verdict`, every verdict present. `unknown` counts what shapr didn't judge. |
| `hunks` | int | Hunk records. |
| `unparsed_hunks` | int | Hunks with `parsed: false`. |
| `categories` | object | Hunk count per [category](#categories), every category present. Generated and vendored hunks count in `hunks`, and in no size measure below. |
| `kinds` | object | Part count per kind, every kind present: `logic` counts residuals. A hunk that is one part counts once. This and the three part counts below are over hunks whose verdict is `structure`, `behavior` or `mixed`: a docs or unparsed file's residual is no logic change. |
| `move_contexts` | object | `move` part count per `context` (see [Moves](#moves)), every context present; a part with two contexts counts under both. |
| `assumptions` | object | Part count per assumption (see [Proof status](#proof-status)), every assumption present; a part with two counts under both. |
| `facets` | object | Residual count per facet (see [Facets](#facets)), every facet present; a residual with two facets counts under both. |
| `files` | int | Files changed (Kamei's NF). |
| `directories` | int | Distinct directories holding them (ND); the repo root counts as one. |
| `subsystems` | int | Distinct top-level directories (NS); files at the root count as one. |
| `entropy` | number | Kamei's change entropy, −Σ pₖ log₂ pₖ, where pₖ is file k's share of the changed lines (`added + deleted`), over files that aren't generated or vendored. 0 for one file. |
| `added`, `deleted`, `code_added`, `code_deleted`, `comment_added`, `comment_deleted`, `review_lines`, `relocated_lines` | int | Sums over the hunks that aren't generated or vendored. |
| `test_review_lines` | int | Of `review_lines`, those in test code: every one in a `test` file, and a residual's carrying the `test` facet elsewhere. Reviewers weigh test changes apart from code changes. |
| `behavior_lines` | int | `review_lines` summed over parts that aren't structure (residuals, moves into new surroundings, parts with assumptions) in hunks whose verdict is `behavior` or `mixed`: the behavior change's size, comments and unchanged moved lines excluded. |
| `structure_share` | number 0–1 | Changed lines (`added + deleted`) in structure parts of hunks whose verdict is `structure` or `mixed`, over all changed lines, generated and vendored hunks left out; 0 when nothing changed. |
| `refactor_lines` | int | Changed lines in parts of a structure kind (every kind but `logic`), proven or not, in hunks whose verdict is `structure`, `behavior` or `mixed`, generated and vendored hunks left out: the refactoring's size, whether or not shapr could rule out a behavior change. A move whose surroundings may observe it, or a Sorbet signature a runtime checks, counts here and not in `structure_share`. |
| `refactor_share` | number 0–1 | `refactor_lines` over all changed lines, as `structure_share` counts them; 0 when nothing changed. At least `structure_share`. |
| `claims` | object | What the commit messages and `--message` description claim, kept apart from the code's facts. See [Claims](#claims). |
| `config_trajectory` | array | *Experimental.* Per norm `.rubocop.yml` (or `.editorconfig`) sets in a changed file, whether the change moves the code toward it: `norm`, `configured`, `before` and `after` (each `checked`, the observations config checks in the changed files, and `offenses`, those not in the configured style), `fixed` and `added` (offenses only before, and only after, in a changed file, matched by content with the style taken out, so one carried over, or moved within its file, is neither), and `direction`, `toward` (fewer offenses), `away` (more) or `level`, which nets them. Both sides are judged by the config as of the after side; unchanged files hold the same offenses on both sides, so the counts move as the file's, its directory's and the repository's do. Only norms whose counts move, or with an offense fixed or added, are listed. |

When `refactor_share` is two thirds or more and some hunk in the verdict
holds `logic`, the text output points at it under the summary line:
`refactor: 94% of changed lines; 6 logic lines inside (lib/a.rb:12,
lib/b.rb:40)`, the residuals' `review_lines` and the first three hunks'
places. A refactoring is where a behavior change is easiest to miss; below
two thirds, or with no logic, the line is left out. It names the hunks an
`unproven` refactor or style claim (see [Claims](#claims)) would count, so
that line is left out of the text when this one prints; the JSON keeps both.

## `--digest`

*Experimental.* `--digest` narrows the output to what a reviewer reads
first; it works with every input form. As text, the summary lines come
first, then the logic to read, ranked, then what shapr couldn't prove, what
to skip, the style check and the hunks outside the verdict. Under `-j` it is
one compact `digest` object; `-J` doesn't take it. It holds nothing the full
output doesn't: a consumer that needs the evidence reads the full JSON.
A place is a `"file:line"` string, the line a hunk's first (on the after
side unless it only deletes) or a style deviation's.

| field | type | meaning |
|---|---|---|
| `record` | `"digest"` | |
| `schema` | int | As in the `meta` record. |
| `subject` | string or null | The change: a GitHub change's name, or what shapr chose to compare (`meta.subject`). |
| `verdict`, `hunks`, `review_lines`, `test_review_lines`, `refactor_share` | | As in the `summary` record. |
| `claims` | string array | The categories the messages claim (`summary.claims.experimental.categories` above 0). |
| `unproven` | string array | Each claim the code doesn't bear out, as the text output's `unproven:` and `conflict:` lines say it. |
| `read_first` | array | The logic to read, ranked: a residual that nearly proved a structural kind first, then more facets, then more lines. At most 15; `more` places the rest. Below. |
| `more` | object | The logic left out of `read_first`: `items`, `review_lines`, and `at`, their places. |
| `unproven_structure` | array | Parts of a structural kind that aren't structure: `at`, `kind`, and `why`, their assumptions and move contexts. |
| `skip` | object | Proven structure, to skip: `hunks` (wholly structure), `changed_lines` in structure parts, and `kinds`, part count per kind. |
| `style_checked` | int | Hunks whose new code was judged against at least one norm (see [Style](#style)). |
| `style` | array | Each deviation from a diverging norm: `at`, `norm`, `expected`. Empty with `style_checked` above 0: the new code follows the norms around it. |
| `outside` | object | Per category, the places of the hunks outside the verdict. |

Each `read_first` item is one hunk's residual, or the residuals, in any
file, that share a reason:

| field | type | meaning |
|---|---|---|
| `at` | string array | The hunks' places. |
| `review_lines` | int | The residuals' `review_lines`, summed. |
| `facets` | string array | The residual's facets. |
| `with` | string array | The structural kinds proven in the same hunk. |
| `test` | bool | In a test file, or carrying the `test` facet. |
| `note` | string or null | The residual's reason: which structural kind it resembles and why it isn't one. |
| `relocated` | string or null | The text output's note on lines relocated verbatim. |

## GitHub input

*Experimental*: the forms below, `meta.source`, the `cache` record and the
cache flags (`--cache-dir`, `--status`, `--clear-cache`, `SHAPR_CACHE_DIR`)
may change in any release before 1.0.

The argument may name a GitHub pull request or commit instead of local revisions:

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
error. A second run on the same commits fetches nothing. A run holds a lock
on its clone (`shapr.lock` inside it) while it fetches and reads, so runs at
once on one repository take turns. `SHAPR_GITHUB_API_URL`
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

| `usage` | object | The [usage log](#usage-record): `path` (with `~`; null without a home directory), `bytes`, and `enabled`, whether runs write to it. |

`--clear-cache` removes `<cache>/github.com` and nothing else in the
directory.

## `usage` record

`shapr --usage` (after `--reset` empties the log) summarizes the usage log, one
object under `-j` or `-J`. Each `classify` or `norms` run appends a line to the
log, `$XDG_STATE_HOME/shapr/usage.jsonl`, else
`~/.local/state/shapr/usage.jsonl`: `ts` (UTC), `version`, `command`, `input`
and `paths` (as `meta`), `flags` (the long names of the flags given, never their
values), `format` (`text`, `json`, `ndjson`), `exit`, `duration_ms`, and
`counts` (`files`, `hunks`, `kinds` and `verdict` from the summary; for
`norms`, `files` and `steps`). It holds no paths, repositories, revisions,
code, messages or URLs. `SHAPR_USAGE=0` (`false`, `off`, `no`) turns logging
off, `1` (`true`, `on`, `yes`) on, an absolute path moves the log; unset, it is
on except in CI (`CI` set to `true`, `1`, `yes` or `on` in any case, or `GITHUB_ACTIONS=true`) and in a debug build. Past 5 MB the log keeps its newest half.

| field | type | meaning |
|---|---|---|
| `record` | `"usage"` | |
| `path` | string or null | The log, with the home directory as `~`. |
| `enabled` | bool | Whether runs write to it. |
| `bytes` | int | Its size. |
| `runs` | int | Lines read. |
| `first`, `last` | string or null | The earliest and latest `ts`. |
| `commands`, `inputs`, `flags`, `verdicts` | object | Runs per command, input form, flag and verdict. |
| `kinds` | object | Parts per kind, summed over runs. |

## Exit codes

The verdict is in the output, never in the exit code: a run that classified
the change exits 0, whatever the verdict.

| code | meaning |
|---|---|
| 0 | success |
| 2 | operational error: git failed (not a repository, a bad revision, a name that is neither a revision nor a path, a range reaching past a shallow clone's history: `shallow clone: the base commit isn't fetched`, naming `fetch-depth: 0` and `git fetch --unshallow`), stdin unreadable, stdin holds no diff (no file header), a merge against all its parents (`shapr R` or `R^!` on a merge, a combined `diff --cc`: diff against one parent, `R^1..R`), GitHub failed (unreachable, not found, rate limited, a fetch refused), or shapr itself failed (`internal error: ..., please report it`: a bug, never a verdict) |
| 64 | usage: a bad command line (a revision starting with `-`, one revision that isn't one commit: `REV^{tree}`, `REV:PATH`, `REV^@`, more than two revisions, a range beside another revision, a bare `--merge-base`), a name that is both a revision and a path (use `--`), a revision after a path, no input (no arguments and an empty or blank stdin), a GitHub file or directory page (`/blob/`, `/tree/`), a malformed GitHub form, an ambiguous `#N`, or a relative cache directory |

An empty diff from a range (`shapr main..main`) is the verdict `none`.
An empty stdin is not: it is what a failed `git diff` upstream of the pipe
leaves, so it exits 64. A diff whose only change is a rename or mode change
has file headers and no hunks, and is `none` too.
