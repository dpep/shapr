# shapr development conventions

`shapr` classifies the **shape of a code change**. For each hunk in a diff it
says what kind of change it is (`format_only`, `comment_only`, `rename`,
`move`, `move_with_changes`, `extract`, `inline`, `logic`), and it gives the
whole change a verdict: structure, behavior, or mixed (structure and
behavior changes belong apart; see the README's References). It is a narrow, deterministic, pipe-first evidence tool; review
agents and classifier prompts consume its JSON. shapr never calls an LLM.

[docs/schema.md](docs/schema.md) is the output contract.
[docs/research.md](docs/research.md) is the prior-art survey behind the
design.

## First principles

- **Unknown is `logic`.** A hunk is structural only when the evidence proves
  it. A detector that isn't sure leaves the hunk alone.
- **Structure is not safety.** Say "structure", never "safe", in code, docs,
  help and output. Extracts and inlines lead bug-linked refactorings.
- **Evidence creates confidence.** `confidence` is computed from what backs the
  label (comparison method, similarity, consistency counts) and the evidence
  values are in the output. No per-kind constants. Scores are rounded to 2
  decimals where they are built.
- **Exact tree equality, no token heuristics.** Two subtrees are equal when
  their node kinds, child order and every non-whitespace byte of each node's
  own source match, with leaves (string content, heredoc bodies) compared
  byte for byte. Reordering is never format-invariant.
- **The JSON schema is public.** Field names and closed vocabularies change
  only with a `schema` bump and a CHANGELOG line saying what a consumer must
  do. Every field is documented in docs/schema.md
  (`every_output_field_is_documented` enforces it).
- **stdout is data.** Logs go to stderr (`verbose!`), errors are a JSON
  object under `-j`/`-J`, the exit code carries the verdict.

## Layout

```
src/
  main.rs, lib.rs   entry; lib exposes the modules to the integration tests
  cli.rs            clap, input selection (range or stdin, -C), exit codes, after-help
  log.rs            -v telemetry (the verbose! macro)
  git.rs            the git calls, run in -C's directory or a bare clone: `git diff RANGE`, `git log`
                    messages, blob reads (worktree fallback), linguist-generated attributes, remotes
  github.rs         GitHub input: PR/commit forms, REST metadata (token from env or gh), the cache of
                    bare partial clones the diff runs in (--status, --clear-cache)
  diff.rs           unified diff parser: FileDiff, Hunk (narrowed to its changed region), Lines
  tree.rs           the language-neutral Tree and the TreeBuilder that normalizes it
                    (extents, comment placement, whitespace-insensitive hashes, per-line facts)
  frontend/
    mod.rs          the Frontend trait and the registry (for_path)
    ruby/mod.rs     prism → TreeBuilder; magic comments become code leaves
    ruby/roles.rs   which prism nodes play which Role
    ruby/generated.rs   prism node kinds and names (script/gen-ruby-kinds.sh; don't edit)
    rust/mod.rs     tree-sitter → TreeBuilder: own text, folded `;`/braces, attributes into their item,
                    macro token trees as leaves, trailing commas ignored
    rust/roles.rs   which tree-sitter nodes play which Role; local bindings; derive-observed members
  change.rs         assembles a Change: per file, both trees, a Site per hunk, and whether
                    it is generated (lockfiles, minified assets, linguist-generated)
  classify/
    mod.rs          Kind, Method, Confidence, Label, the pass pipeline
    site.rs         hunk → tree mapping: each side's significant span and cover
    format.rs       the format_only / comment_only pass (tree or line method)
    rename.rs       the rename pass: aligned sides differing only in names, checked per scope
    moves.rs        the move / move_with_changes pass: unaligned declarations paired across files
    extract.rs      the extract / inline pass: a chunk of statements swapped for a call
    facets.rs       facets on logic hunks (condition, signature, ...), from Frontend::construct;
                    runs after the passes and never relabels
  claims.rs         summary.claims: hints read from commit messages and --message, and
                    conflicts with the code's facts; never feeds back into labels
  report.rs         output records, review_lines and summary metrics, text/JSON/NDJSON
tests/              end to end against throwaway git repos: cli.rs (input, output,
                    size measures), one file per detector, rust_lang.rs (Rust end to end), github.rs (offline:
                    a canned API on 127.0.0.1 and a file:// remote)
```

### The language seam

A frontend implements `Frontend` (`language`, `handles`, `parse`, and
`construct`: which nodes are a branch, comparison, literal, error construct,
parameter list or test item, for facets) and reports
raw structure to a `TreeBuilder`: `open`/`close` for nodes with children,
`leaf` for nodes compared byte for byte, `comment`, `directive` (a comment the
language reads as code), `error`. The builder computes everything the core
relies on, so adding a language needs no core change. The classification core
never imports a parser's types; test classification through a frontend.

Language facts the core acts on travel as `Node::role` (a `tree::Role`), with
the identifier in `Node::name`. Roles are not in the hash or the output.
Besides `Node::role`, a frontend may set `Tree::semantics`
(`TreeBuilder::semantics`) for language-wide facts: `file_scoped` (each file
is a module: cross-file moves change owner) and `scoped_drop` (locals drop at
block end: extract risk `drop`). `TreeBuilder::ignore(span)` marks a
separator the language ignores (Rust's trailing comma), hashed like
whitespace.

| Role | Ruby | Rust | For |
|---|---|---|---|
| `Definition(NameKind)` | `def`, `class`, `module`, constant and variable writes, parameters | `fn`, struct/enum/union (`Class`), trait/mod (`Module`), const/static/type alias/variant (`Constant`), struct field (`Member`), `macro_rules!` (`Macro`), params and `self` (`SelfParameter`), complex `let` and pattern bindings (`Variable`) | rename: what a name declares. move: `Role::is_declaration()` is true for method, class, module and constant definitions, the statements whose order doesn't change behavior |
| `Bind` | `x = 1` (local write) | `let x = v;` (simple, untyped), `x = v` | extract: the call's result bound to a local; otherwise a local definition (`Role::defines`) |
| `Reference(NameKind)` | variable reads, constant reads and paths | locals, uppercase paths (`Constant`), types (`Class`), path prefixes (`Module`), `a.b` (`Member`), a fn named as a value (`Method`) | rename: every use of a name |
| `Call(Receiver)` | a method call; `Implicit` (`go`), `SelfRef` (`self.go`), `Explicit` (receiver is the first child) | `f()` `Implicit`; `self.f()`, `Self::f()` `SelfRef`; `x.f()`, `a::f()` `Explicit` | extract/inline: a call to the new method replacing the statements |
| `DynamicName` | a string or symbol argument to `send`, `public_send`, `__send__`, `method`, `define_method`, `respond_to?`, `instance_variable_get`/`set`, `const_get`, `alias_method`, `attr*`, a visibility call, and both names of `alias`; `name` is the literal's value | none | rename: an old name still mentioned here lowers confidence |
| `Opaque` | none | identifiers in macro token trees and attributes, `{name}` in their strings, `S { a }` | rename: conflicts with any scope, follows a passing rename; extract: reads a same-named local |
| `Annotation` | none | types, generics, where clauses, visibility | extract: skipped in a new method's signature; typed params are plain |
| `Visibility(Visibility)` | `public`, `protected`, `private`, `module_function` with no receiver: bare, `private def x`, or `private :x` | none (per item, part of its text; a changed `pub` is logic) | move: a def crossing a visibility boundary |
| `Jump(Jump)` | `return`, `break`, `next`, `redo`, `retry`, `yield`, `super` (both forms), `block_given?`/`iterator?` and a blockless `proc`/`lambda`/`Proc.new` (`BlockGiven`); `binding`, `eval`, `local_variables`, a string `*_eval` (`Binding`); `__method__`, `caller`, `$~`, `$1` (`Frame`); `__LINE__` (`Line`); `__FILE__`, `__dir__`, `require_relative` (`File`) | `return`, `?`, `break`, `continue` (`Next`), `.await` (`Await`), `yield`, a macro outside std (`Macro`), and those keywords inside macro tokens; `line!`/`column!` (`Line`), `file!`/`module_path!` (`File`) | extract/inline: code that changes meaning in a new method (all but `File`, as B stays in the file); rename: `Binding` in a local's scope, `Frame` in a renamed method; move: `Line`, and `File` across files (context `position`). Block-parameter use is a `Reference` whose name matches a `Definition(BlockParameter)` |

`NameKind` says what sort of name a definition or reference is, and so its
scope and rename rules:

| NameKind | Ruby | Rust |
|---|---|---|
| `Method`, `Class`, `Module`, `Constant` | `def`; `class`/`module`; constant writes, reads and paths | see the `Definition` and `Reference` rows above |
| `Variable` | locals only: `x = 1`, `x` | locals and pattern bindings |
| `Field` | instance and class variables: `@x`, `@@x` | none |
| `Member` | none | struct field: reached from any file, so scoped like a method (reported as `"field"`) |
| `Global` | `$x` | none |
| `Parameter` | `x`, `x = 1`, `*rest`, `**opts`; read back as `Reference(Variable)` | fn parameters |
| `KeywordParameter` | `key:` and `key: 1`; callers pass it by name, so it is interface | none |
| `BlockParameter` | `&block` | none |
| `SelfParameter` | none | `self`: passed by the call's receiver |
| `Macro` | none | `macro_rules!` and its invocations: never a declaration, never renamed |

A namespace's name is a child `Reference` to its own name; `Tree::name_child`
finds it even after attributes. Alignment keys by it; owners add the code
before it (attributes).

Walk a subtree with `Tree::descendants(id)` and filter on `role`. A
frontend's mapping is unit-tested through `Frontend::parse`.

### The pass pipeline

Every hunk starts as `logic`. `classify::PASSES` run in order over the whole
`Change`; a pass may relabel only hunks still labeled `logic`. Cross-hunk
passes (move, extract, inline, rename consistency) see every file's trees and
every hunk's `Site`: the cover on the before side is what the hunk removed,
on the after side what it added. Each kind gets its own module under
`src/classify/`; kind-specific output goes in `Label::evidence.values` and
`Label::detail`, documented in docs/schema.md.

## Gate

`make check` (or `script/check.sh`): `cargo fmt --check`, `cargo clippy
--all-targets -- -D warnings`, `cargo test`, `cargo doc` with warnings
denied. CI runs the same on ubuntu. Run it before every commit.

## Testing

- End-to-end tests build tiny git repos in a tempdir and assert the JSON.
  Anything that runs git clears `GIT_DIR`, `GIT_WORK_TREE` and
  `GIT_INDEX_FILE`, so a run under `git rebase --exec` can't touch this repo.
- Fixtures are synthetic Ruby or Rust, with generic names.
- Prefer a test over a one-off CLI probe; a behavior worth checking once is
  worth keeping.

## Landing changes

Small, logically connected commits, structure or behavior but not both.
`CHANGELOG.md` gets its entry under `## Unreleased` in the commit that earns
it. Pre-1.0, prefer the clean break over a compatibility shim, but a schema
change still bumps `schema` and says what a consumer must do.
