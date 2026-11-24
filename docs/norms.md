# Norms catalog

The style choices shapr's norms engine could infer, ranked by how much inferring them is worth. A norm earns a place when repos really differ on it, it can be counted from the parse tree or tokens, it shows up often enough per file to infer from, fixing a deviation is a mechanical edit, and the edit never changes behavior.

**Next 15 after v1, ranked:**

1. `Style/WordArray` + `Style/SymbolArray`: `%w[a b]` vs `["a", "b"]`
2. `Layout/SpaceInsideBlockBraces`: `{ |x| x }` vs `{|x| x}`
3. `Layout/ArgumentAlignment`, `ParameterAlignment`, `ArrayAlignment`: align continuation lines with the first element, or use a fixed indent
4. `Layout/DotPosition`: leading vs trailing `.` in multi-line chains
5. `Style/HashSyntax` shorthand (`EnforcedShorthandSyntax`): `{x:}` vs `{x: x}`
6. `RSpec/NotToNot`: `not_to` vs `to_not`
7. `Layout/MultilineMethodCallIndentation` + `MultilineOperationIndentation`: aligned vs indented
8. `Layout/SpaceAroundEqualsInParameterDefault`: `a = 1` vs `a=1`
9. `Layout/EmptyLinesAroundAccessModifier`: a blank line after `private`, or only before it
10. `Layout/AccessModifierIndentation`: `private` indented to the method level, or outdented
11. `Layout/EmptyLinesAroundClassBody` / `ModuleBody`
12. `Style/PercentLiteralDelimiters`: `%w[]` vs `%w()`
13. `Layout/EndAlignment` + `Layout/CaseIndentation` for assigned `if`/`case`: `end` aligned with the variable or with the keyword
14. `Style/Lambda` (+ `Layout/SpaceInLambdaLiteral`, `Style/StabbyLambdaParentheses`)
15. `Style/StringLiteralsInInterpolation` + `Style/QuotedSymbols`: reuse v1's quote machinery

Rust has its own short list (imports granularity and grouping, inline format arguments, `Self` vs the type name). See [Rust](#rust).

Near misses: `Layout/EmptyLineAfterGuardClause` (repos differ, but files are so mixed that it would mostly abstain), `Style/MethodCallWithArgsParentheses` for receiver calls only, and `Style/RescueStandardError`.

## Evidence

**Config evidence.** Where widely used style configs make different choices for a cop. RC = RuboCop `config/default.yml`, Std = Standard Ruby `config/base.yml`, Shop = Shopify's `rubocop.yml`, GH = `rubocop-github`, Rails = rails/rails `.rubocop.yml`, Airbnb = `rubocop-airbnb`. A cop that Std or Shop *disables* is one they decline to pick a style for, so local inference adds the most there. No published survey of `EnforcedStyle` settings across many public `.rubocop.yml` files turned up.

**Measured.** A rough token and regex count, not prism. It uses Ruby's Ripper (Ruby 2.6) plus line regexes, over 12 public repos: faraday, flipper, graphql-ruby, rubocop, sidekiq, hexapdf, rails, mastodon, rspec-core, discourse, dd-trace-rb and faker. Vendor and fixture dirs were skipped, and files Ripper 2.6 can't lex were dropped. For each norm it records the repo-wide style split, and the **in-file split**: the share of files with 5 or more occurrences whose majority style is under 80% (where the engine would abstain). Treat the numbers as directional. Heuristics for indentation of access modifiers and `end` are the noisiest.

Highlights:

| Norm | Repo majorities (measured) | In-file split where not enforced |
|---|---|---|
| `Style/StringLiterals` | single: rubocop, mastodon, dd-trace, faker; double: rails, sidekiq, discourse, graphql | hexapdf 53% (its config says "we don't care"), rspec-core 57%, flipper 29% |
| `SpaceInsideHashLiteralBraces` | space: rails, rubocop, discourse; no_space: sidekiq, hexapdf | graphql 31%, rspec-core 22% |
| Trailing comma (multiline array/hash) | comma: discourse, mastodon, hexapdf, flipper; no_comma: rubocop, sidekiq, faker | graphql 33–42%, rails 18–40%, dd-trace 21–44% |
| Trailing comma (arguments) | comma: discourse; no_comma: most | graphql 64% |
| `Style/HashSyntax` | ruby19 almost everywhere; rspec-core 99% rockets | ~0% |
| `WordArray` / `SymbolArray` | percent: rubocop, discourse, faker; brackets: graphql, hexapdf, rspec-core (symbols) | rails 20% (words) |
| `SpaceInsideBlockBraces` | space almost everywhere | hexapdf 37% (41% no_space), graphql 18% |
| `DotPosition` | trailing: hexapdf, rspec-core (100%); leading: rest | rails 15% (25% trailing) |
| `SpaceAroundEqualsInParameterDefault` | rspec-core 89% no_space; rest space | rspec-core 14% |
| `EmptyLinesAroundAccessModifier` | rails 100% only_before; rest around | low count (≈1 per class) |
| `AccessModifierIndentation` | rspec-core 92% outdent; rest indent | low count |
| `EmptyLinesAroundClassBody` | hexapdf 70% empty_lines; rest no_empty_lines | hexapdf 52% |
| `IndentationConsistency` | rails 73% indented_internal_methods; rest normal | rails 24% |
| `Style/Lambda` | lambda: hexapdf, rspec-core; literal: graphql, mastodon, faker | discourse 29% lambda, flipper 43% |
| `PercentLiteralDelimiters` | rails `(` 67%, mastodon `(`; rest `[` | low |
| `EmptyLineAfterGuardClause` | hexapdf 64% no_blank; rest blank | 36–86% where not enforced (graphql 86%, rspec-core 86%) |
| `RSpec/NotToNot` | to_not: mastodon (100%), dd-trace (79%); not_to: discourse (86%), rubocop, flipper | (repo-level count) |
| `Naming/RescuedExceptionsVariableName` | `e` most; rails split e/error/ex; graphql `err` | 20–100% |
| Call parens, receiver calls (non-spec) | parens 80–100% everywhere | rspec-core 34%, flipper 21% |
| Call parens, receiver-less calls (non-spec) | omit 51–81% in most; faker parens 90% | 35–59%: macros and DSLs swamp it |

**No real split** (one style at 98% or more in every repo): `Naming/MethodName`, `Naming/VariableName` (0 camelCase), `TrailingWhitespace`, `IndentationWidth` (2), `SpaceAroundOperators`, `EmptyLineBetweenDefs`, `Style/AndOr`, `Style/RedundantReturn`, `Style/MethodDefParentheses`, `CaseIndentation` for a bare `case`, and multi-line `BlockDelimiters` (88–100% do/end).

Repos with a formatter or an enforced RuboCop config come out near-uniform (rubocop, discourse with syntax_tree, mastodon). That matches the vision: where a formatter is the authority, norms add nothing. The value is in repos that don't enforce a style (hexapdf, graphql, rspec-core, flipper).

## Ruby: layout norms

Occ/file: high = a median of 5 or more per file, med = 2–4, low = about 1 (needs file or neighbor scope).

| Norm (cop) | Styles | How to count | Occ/file | Repos differ? | Behavior-safe? | Edit mechanical? | Priority | Notes |
|---|---|---|---|---|---|---|---|---|
| `Layout/IndentationWidth` | 2 / 4 / tabs | indent delta at each nesting increase | high | No: 2 in 99–100% | Yes, except heredoc bodies and `=begin` | Yes | v1 (weak) | Effectively a fixed rule in Ruby |
| `Layout/IndentationConsistency` | normal / indented_internal_methods | def column after a bare `private` vs before it | low | Yes: Rails 73% indented | Yes | Yes (reindent section) | v1 | The real indentation split |
| `Layout/SpaceAroundOperators` | space / no_space (`**`, rationals separately) | `on_op` binary ops: whitespace on both sides | high | No: 100% space | Yes | Yes | v1 (weak) | Must allow alignment spacing (`ExtraSpacing`) or it flags aligned `=` |
| `Layout/SpaceInsideHashLiteralBraces` | space / no_space / compact | `HashNode` opening followed by a space | med | Yes: measured and Std vs RC | Yes | Yes | v1 | Strong |
| `Layout/EmptyLineBetweenDefs` | blank / none | `end` then `def`, with or without a blank line | med | No: 98–100% blank | Yes | Yes | v1 (weak) | Rule-like |
| `Layout/TrailingWhitespace` | none | trailing `[ \t]+$` | high | No: never a style | Yes, except inside heredocs and multi-line strings | Yes | v1 (weak) | A fixed check, not a norm |
| `Layout/SpaceInsideBlockBraces` | space / no_space | brace-block opening followed by a space | med | Yes: hexapdf 41% no_space, graphql 16% | Yes | Yes | next (2) | Shares code with hash braces; prism tells a block brace from a hash brace |
| `Layout/ArgumentAlignment`, `ParameterAlignment`, `ArrayAlignment` | with_first_element / with_fixed_indentation | column of continuation lines vs first element vs line indent + 2 | med | Yes: Std and Shop fixed, RC and Airbnb with_first | Yes (whitespace) | Yes (reindent continuation lines) | next (3) | Strongest config split; unmeasured. One norm family |
| `Layout/DotPosition` | leading / trailing | `.`/`&.` first on its line or last | med | Yes: Airbnb trailing; hexapdf and rspec-core 100% trailing | Yes. A comment between a trailing dot and the next line is fine; leading dot after a comment line needs Ruby ≥2.7 | Yes | next (4) | Only in multi-line chains |
| `Layout/MultilineMethodCallIndentation`, `MultilineOperationIndentation` | aligned / indented / indented_relative_to_receiver | continuation column of chain or operator lines | med | Yes: Std and Airbnb indented, RC aligned, Shop disabled | Yes | Yes (reindent) | next (7) | Unmeasured |
| `Layout/SpaceAroundEqualsInParameterDefault` | space / no_space | `=` inside a def's params | med | Yes: rspec-core 89% no_space | Yes | Yes | next (8) | Cheap |
| `Layout/EmptyLinesAroundAccessModifier` | around / only_before | blank line after a bare `private` | low | Yes: Rails only_before 100% | Yes | Yes | next (9) | Needs file or neighbor scope |
| `Layout/AccessModifierIndentation` | indent / outdent | `private` column vs def column | low | Yes: rspec-core 92% outdent | Yes | Yes | next (10) | Needs neighbor scope |
| `Layout/EmptyLinesAroundClassBody`, `ModuleBody` (+ `empty_lines_except_namespace`) | empty_lines / no_empty_lines / … | blank line after `class X` and before its `end` | low | Yes: hexapdf 70% empty_lines | Yes | Yes | next (11) | One per class |
| `Layout/EndAlignment`, `Layout/CaseIndentation` (assigned forms) | keyword / variable / start_of_line; case / end | `x = if/case/begin`: column of `end` and `when` | low | Yes: Std, Shop and Rails variable, RC and Airbnb keyword; hexapdf keyword 100%, graphql variable 99% | Yes | Yes (reindent body) | next (13) | Larger edit: the whole construct moves |
| `Layout/EmptyLineAfterGuardClause` | blank / none | guard (`return/next/raise … if`) followed by a blank line | med | Yes, by repo (hexapdf no_blank) | Yes | Yes | later | 36–86% in-file split where not enforced, so it would mostly abstain |
| `Layout/FirstArgumentIndentation`, `FirstArrayElementIndentation`, `FirstHashElementIndentation` | special_inside_parentheses / consistent / align_* | first element's column after a line break | med | Yes: Std, Shop and Airbnb consistent; RC special_* | Yes | Yes | later | Same machinery as #3 |
| `Layout/HashAlignment` | key / separator / table | values aligned in column across a multi-line hash | low | Some codebases use table | Yes | Yes, but churns whole hashes | later | Interacts with `ExtraSpacing` |
| `Layout/SpaceInLambdaLiteral` | `->(x)` / `-> (x)` | space after `->` | low | Yes: graphql require_space; rubyfmt emits `-> (x)` | Yes | Yes | later (with 14) | |
| `Layout/SpaceInsideArrayLiteralBrackets`, `SpaceInsideParens`, `SpaceInsideReferenceBrackets`, `SpaceInsideStringInterpolation`, `SpaceBeforeBlockBraces`, `SpaceAroundBlockParameters` | space / no_space | the token after the opener | med | Rarely; every config agrees on no_space | Yes | Yes | later | Cheap once the token model exists; little split |
| `Layout/MultilineArrayBraceLayout`, `MultilineHashBraceLayout`, `MultilineMethodCallBraceLayout`, `MultilineMethodDefinitionBraceLayout` | symmetrical / new_line / same_line | closing bracket line vs last element | low | No: every config symmetrical | Yes | Yes | skip | No split |
| `Layout/EmptyLinesAroundBlockBody`, `EmptyLinesAroundMethodBody` | empty / none | blank line after the opener | med | No | Yes | Yes | skip | No split |
| `Layout/LineLength` | a threshold | max columns | n/a | Yes | Yes | No (needs reflow) | skip | A threshold, not a style; reflow edits are not mechanical |
| `Layout/TrailingEmptyLines`, `EndOfLine`, `IndentationStyle` | final newline / LF / spaces | file bytes | n/a | No | Mostly (CRLF inside heredocs is content) | Yes | skip | Fixed rules |

## Ruby: style norms

| Norm (cop) | Styles | How to count | Occ/file | Repos differ? | Behavior-safe? | Edit mechanical? | Priority | Notes |
|---|---|---|---|---|---|---|---|---|
| `Style/StringLiterals` | single / double | `StringNode` opening, counting only strings where both quotes are possible (no interpolation, no escapes, no quotes inside) | high | Yes: RC single, Std, GH and Rails double, Shop and Airbnb disabled | Yes when `unescaped` bytes and flags match (refactorings §3) | Yes | v1 | Strongest norm |
| Trailing comma (`Style/TrailingCommaInArrayLiteral`, `…InHashLiteral`, `…InArguments`) | comma / consistent_comma / no_comma | multi-line literal or call: comma before the closing line | med | Yes: Shop and Airbnb comma, RC and Std no_comma; measured | Yes, except block params (`\|a,\|`) and `a, = x` | Yes | v1 | Strong. Arguments split separately from literals (mastodon: literals comma, arguments none) |
| `Style/HashSyntax` (`EnforcedStyle`) | ruby19 / hash_rockets / no_mixed_keys | `AssocNode` with a symbol key: label vs `=>` | high | Mostly ruby19; legacy code differs (rspec-core 99% rockets) | Yes for symbol keys. `"a": 1` is a symbol, not a string | Yes | v1 | Keep; the split is old vs new code |
| `Style/WordArray`, `Style/SymbolArray` | percent / brackets | `%w`/`%i` opening vs an `ArrayNode` of 2 or more plain word strings or symbols | low–med | Yes: RC percent, Shop brackets; graphql and hexapdf brackets in code | Yes (refactorings §3) when element flags match | Yes | next (1) | Respect `MinSize` |
| `Style/HashSyntax` shorthand (`EnforcedShorthandSyntax`) | always / never / either / consistent | `ImplicitNode` value vs an explicit `x: x` | med | Yes: RC always, Std and Rails either; discourse 19% and mastodon 15% shorthand | Yes when the explicit value is the same local or call prism resolves | Yes | next (5) | Gate on Ruby ≥3.1 (`TargetRubyVersion`). `foo x:` followed by a newline is a parse trap: keep parens |
| `Style/PercentLiteralDelimiters` | `[]` / `()` / `{}` per literal kind | `%w`/`%i`/`%q`/`%r` opening delimiter | low | Yes: Rails `(` 67%, most `[` | Yes, escaping delimiters inside | Yes | next (12) | Per literal kind (`PreferredDelimiters`) |
| `Style/Lambda`, `Style/StabbyLambdaParentheses` | lambda / literal / line_count_dependent | `lambda {}` call vs `->` | low | Yes: hexapdf and rspec-core lambda; graphql literal | Yes with a literal block and `lambda` not redefined or shadowed. `lambda(&blk)` is not convertible | Yes | next (14) | Infer single-line and multi-line separately (RC's default style differs by line count) |
| `Style/StringLiteralsInInterpolation`, `Style/QuotedSymbols` | single / double / same_as_string_literals | strings inside `#{}`; `:"…"` symbols | low | Yes: RC single, Std double | Yes, as quotes | Yes | next (15) | Falls out of v1 |
| `Style/MethodCallWithArgsParentheses` | require / omit (macros ignored) | `CallNode` with arguments: `opening_loc` present | high | Yes: Shop requires, RC and Std disabled; receiver calls measured parens 80–100% | Yes when it's the same `CallNode` and arguments (refactorings §3). `foo a` vs `foo(a)` changes nothing; a block's binding can (`do` vs `{}`) | Yes, but care with `do` blocks and line breaks | later | Receiver-less calls are dominated by macros and DSLs (`it`, `attr_reader`, `puts`): count receiver calls only, outside specs |
| `Style/RescueStandardError` | implicit / explicit | bare `rescue` vs `rescue StandardError` | low | Yes: Std implicit, RC explicit | Yes unless `StandardError` is shadowed lexically | Yes | later (near miss) | Unmeasured |
| `Style/BlockDelimiters` | line_count_based / semantic / braces_for_chaining | multi-line brace block vs `do` | med | Configs differ (Shop and Std disable); code is 88–100% do/end | Only when the block attaches to the same `CallNode` (refactorings §3) | Yes, after checking attachment | later | Little split in practice |
| `Style/TernaryParentheses` | require_no / require / when_complex | `IfNode` ternary predicate wrapped in parens | low | Yes: Std when_complex, RC none | Yes (single-statement parens are transparent) | Yes | later | |
| `Style/EmptyMethod` | compact / expanded | `def x; end` vs two lines | low | Yes: Std and hexapdf expanded, RC compact | Yes | Yes | later | |
| `Style/Alias` | prefer_alias / prefer_alias_method | `alias` keyword vs `alias_method` | low | Yes: Std and Shop alias_method, RC alias | Only at class-body top level with literal names. `alias` is lexical (inside `instance_eval`/`class << self` it differs); `alias_method` returns a symbol | Yes, within that scope | later | Hedge |
| `Style/SignalException` | only_raise / only_fail / semantic | `raise` vs `fail` calls | low | Rarely: rspec-core 26% fail | Yes unless `fail`/`raise` is redefined | Yes | later | |
| `Style/CollectionMethods` | map/collect, reduce/inject, find/detect, select/filter | method name | med | Yes in older code (disabled by default) | Only on Enumerable receivers; type unknown, so hedge | Yes | later | Like `PreferredHashMethods` |
| `Style/ClassCheck`, `Style/FormatString` | `is_a?`/`kind_of?`; format/sprintf/`%` | method name | low | Yes: hexapdf kind_of? and sprintf | Aliases unless overridden; `%` depends on the receiver type | Yes (hedge) | later | |
| `Style/RegexpLiteral`, `Style/CommandLiteral`, `Style/BarePercentLiterals`, `Style/PercentQLiterals` | slashes / `%r` / mixed; backticks / `%x` | literal opening | low | Shop mixed and `%x`, Std mixed | Yes, escaping delimiters | Yes | later | Rare |
| `Style/HashAsLastArrayItem`, `Style/AccessorGrouping`, `Style/MixinGrouping` | braces / no_braces; grouped / separated | node shape | low | RC sets them, Std disables some | Yes. `include A, B` = `include B; include A` (the order reverses) | Yes (mind the order) | later | |
| `Style/NumericLiterals`, `Style/NumericLiteralPrefix` | `1_000` / `1000`; `0o17` / `017` | `IntegerNode` source | low | Some | Yes (equal value) | Yes | later | `NumericLiterals` is a threshold more than a style |
| `Style/EndlessMethod`, `Style/NumberedParameters`, `Style/ItBlockParameter` | allow / disallow | node kind | low | Newer syntax; not yet settled | Yes for one-expression bodies (refactorings §3) | Yes | later | Gate on the target Ruby |
| `Style/MethodDefParentheses` | require / require_no | `DefNode` params parens | med | No (Seattle style exists but is rare) | Yes | Yes | later | No split measured |
| `Style/AndOr`, `Style/Not` | always / conditionals | `and`/`or` vs `&&`/`\|\|` | med | No: 99–100% symbols | Only when the trees are equal (precedence) | Yes | skip | No split |
| `Style/RedundantReturn`, `Style/RedundantSelf` | — | trailing `return`, `self.` reads | med | No: implicit 100% | Yes at a method tail; `self.` matters for setters and private calls | Mostly | skip | A rule, not a norm |
| `Style/FrozenStringLiteralComment`, `Style/MutableConstant`, `Style/ClassAndModuleChildren`, `Style/AccessModifierDeclarations`, `Style/ModuleFunction`, `Style/NegatedIf`/`NegatedUnless`, `Style/YodaCondition`, `Style/NilComparison`, `Style/NumericPredicate`, `Style/StringConcatenation`, `Style/SafeNavigation`, `Style/DoubleNegation`, `Style/RaiseArgs`, `Style/ConditionalAssignment`, `Style/For`, `Style/PreferredHashMethods`, `Style/SpecialGlobalVars` | — | — | — | Some do | **No**: see [Skip](#skip) | — | skip | Semantic |

### RSpec (rubocop-rspec)

Spec files are a large share of every repo and have their own style choices.

| Norm (cop) | Styles | How to count | Occ/file | Repos differ? | Behavior-safe? | Edit mechanical? | Priority | Notes |
|---|---|---|---|---|---|---|---|---|
| `RSpec/NotToNot` | not_to / to_not | `.not_to` vs `.to_not` calls | med | Yes: mastodon to_not 100%, dd-trace 79%; discourse not_to 86% | Yes (aliases in rspec-expectations) | Yes | next (6) | |
| `RSpec/BeNil` | be_nil / `be nil` | matcher call | med | Yes: flipper and dd-trace mostly `be(nil)` | Pass/fail is identical; failure messages differ | Yes | later | |
| `RSpec/HookArgument` | implicit / `:each` / `:example` | `before`/`after` args | med | A little: flipper and rspec-core use some `:each` | Yes (same hook) | Yes | later | |
| `RSpec/ItBehavesLike`, `RSpec/ClassCheck`, `RSpec/MetadataStyle`, `RSpec/VariableDefinition`, `RSpec/SharedExamples` | aliases / symbol vs hash / symbol vs string | call shape | low | Some | Yes | Yes | later | |
| `RSpec/DescribedClass` | described_class / explicit | constant vs `described_class` in an example | med | Yes | Only when the nearest `describe` names that class | Mostly | later | Hedge for nested describes |
| `RSpec/MessageSpies`, `RSpec/ReturnFromStub`, `RSpec/ImplicitExpect`, `RSpec/ImplicitSubject`, `RSpec/ExpectChange` | — | — | — | — | **No**: restructures the test, a block is lazy vs an eager value, or `should` needs config | No | skip | |

## Ruby: naming norms

| Norm (cop) | Styles | How to count | Occ/file | Repos differ? | Behavior-safe? | Edit mechanical? | Priority | Notes |
|---|---|---|---|---|---|---|---|---|
| `Naming/MethodName` | snake_case / camelCase | `DefNode` name | high | No: 0 camelCase in 12 repos | **No** for public or called methods (callers; decisions #47) | No | v1 (weak) | Fixed rule; a camelCase def usually mirrors an external API on purpose |
| `Naming/VariableName` | snake_case / camelCase | local writes | high | No: 0 camelCase | Yes for locals (scoped rename); not for ivars | Yes for locals | v1 (weak) | Fixed rule |
| `Naming/RescuedExceptionsVariableName` | `e` / `error` / `ex` / `err` / … | `rescue => name` | low | Yes: rails split e/error/ex, graphql err | Yes (a local), barring shadowing | Yes | later | 20–100% in-file split: often abstains |
| `Naming/BlockForwarding` | anonymous `&` / `&block` | block param | low | New (Ruby 3.1) | Yes when the name is only forwarded | Yes | later | |
| `Naming/VariableNumber`, `Naming/HeredocDelimiterCase`, `Naming/HeredocDelimiterNaming` | normalcase / snake_case / …; upper / lower | identifier shape | low | Rarely | Yes for locals and heredoc tags | Yes | later | |
| `Naming/MemoizedInstanceVariableName`, `Naming/PredicateName`, `Naming/FileName`, `Naming/ConstantName`, `Naming/ClassAndModuleCamelCase` | — | — | — | No, or API-changing | No (renames API or files) | No | skip | |

## Rust

rustfmt and clippy's default groups decide most Rust style. A norm is worth inferring only where both leave the choice open: rustfmt options that are unstable or default to `Preserve`, and clippy lints outside the default groups (pedantic, nursery, restriction). A probe with clippy 0.1.100 confirmed that `uninlined_format_args`, `use_self` and `unreadable_literal` don't fire by default, while `redundant_field_names` does.

Measured: a rough regex estimate over tokio, serde, syn, regex, rayon, toml, anyhow, ignore, clap_builder, ast-grep and tree-sitter.

| Norm | Styles | How to count | Occ/file | Repos differ? | Behavior-safe? | Edit mechanical? | Priority | Notes |
|---|---|---|---|---|---|---|---|---|
| Import granularity (rustfmt `imports_granularity`, unstable) | Preserve / Crate / Module / Item / One | `use_declaration` tree shape | high | Yes: share of nested `use` runs from 4% (toml) to 57% (tree-sitter); in-file split 42–83% in most | Yes per leaf with the same visibility and `cfg` (refactorings §3); `{self}` is not the bare path | Yes | next (Rust 1) | Measuring "nested vs flat" understates granularity; count leaves per `use` instead |
| Import grouping (rustfmt `group_imports`, unstable) | std / external / crate blocks / one block | blank-line-separated `use` runs, classified by root | med | Unmeasured; a common team preference | Yes (`use` order is irrelevant) | Yes | next (Rust 2) | |
| Inline format args (clippy `uninlined_format_args`, pedantic) | `{x}` / `{}`, x | format-macro string and arguments | med | Yes: serde and anyhow 100% positional; clap 84% inline; tokio 71% positional | Yes for bare identifiers only (no field access or expressions); needs Rust ≥1.58 | Yes | next (Rust 3) | In-file split is high (ast-grep 64%, tree-sitter 49%) |
| `Self` vs type name in impls (clippy `use_self`, nursery) | Self / name | type and path uses of the impl's own type | high | Yes: regex and ignore name 98–99%; serde and toml Self 95–98% | Only when generic arguments match exactly: `Self` is `Foo<T>`, a bare `Foo::new()` can infer another `T` | Yes, with the generics check | next (Rust 4) | In-file split 46–66% in rayon, syn and tokio |
| Hex literal case (rustfmt `hex_literal_case`, default Preserve) | upper / lower | hex `integer_literal` digits | low | Yes: tokio lower 94%, rayon upper 100% | Yes | Yes | later | |
| `&str`→`String` (`to_string` / `to_owned` / `String::from` / `into`) | — | method on a string literal | low | Yes: per repo | Yes for literal receivers; `.into()` needs a known `String` target | Yes, except `into` | later | clippy `str_to_string` is restriction |
| `where` clause vs inline bounds | where / inline | generic bounds placement | low | Some | Yes | Yes (rustfmt reformats after) | later | |
| `crate::` vs `super::` paths, `mod.rs` vs `foo.rs` | — | path roots; file layout | low | Some | Paths: when they resolve to the same item; files: a rename | Paths yes; files no | later | |
| Tail `foo()` vs `foo();` in unit fns (`semicolon_if_nothing_returned`, pedantic) | — | last statement | med | Some | Temporaries drop at different times (edition-dependent): hedge | Yes | later | |
| Field init shorthand, needless `return`, redundant `'static`, `let_and_return` | — | — | — | clippy `style`, on by default | — | — | skip | Already decided by clippy |
| `impl Trait` argument vs generic | — | — | — | Some | **No**: callers can't turbofish `impl Trait`; changes the API | — | skip | |
| rustfmt stable options (`max_width`, `tab_spaces`, `hard_tabs`, `use_small_heuristics`, `fn_params_layout`, `match_arm_leading_pipes`, …) | — | `rustfmt.toml` | — | Only via config | — | — | skip | Read the config and respect it; nothing to infer |

## Skip

| Cop(s) | Why |
|---|---|
| `Style/FrozenStringLiteralComment`, `Style/MutableConstant` | Changes behavior: mutation raises `FrozenError` |
| `Style/ClassAndModuleChildren` | Constant lookup (`Module.nesting`) differs; compact form needs the parent defined |
| `Style/AccessModifierDeclarations` | Visibility of dynamically defined methods differs |
| `Style/ModuleFunction` | `module_function` copies methods and makes the instance copies private; `extend self` doesn't |
| `Style/ClassMethodsDefinitions` | Moves code; `private` inside `class << self` behaves differently. Not a mechanical edit |
| `Style/NegatedIf`, `Style/NegatedUnless`, `Style/NilComparison`, `Style/NonNilCheck`, `Style/YodaCondition`, `Style/NumericPredicate`, `Style/ZeroLengthPredicate`, `Style/CaseLikeIf` | `!`, `==` and predicates are user-definable methods; receiver types are unknown (refactorings §3) |
| `Style/StringConcatenation`, `Style/RedundantInterpolation`, `Style/SafeNavigation`, `Style/DoubleNegation`, `Style/SymbolProc`, `Style/For` | Not equivalent (refactorings §3) |
| `Style/RaiseArgs` | `raise E, m` calls `E.exception(m)`; `raise E.new(m)` differs if `exception` or `initialize` is customized |
| `Style/ConditionalAssignment`, `Style/IfUnlessModifier`, `Style/GuardClause`, `Style/Next` | Restructures control flow, or depends on line length; not mechanical |
| `Style/SpecialGlobalVars` | English names need `require "English"`: not a local edit |
| `Style/MissingElse`, `Style/EmptyElse`, `Style/Documentation`, `Style/ReturnNil` | Adds or removes code, or is a policy rather than a style |
| `Style/AndOr`, `Style/RedundantReturn`, `Style/MethodDefParentheses`, `Layout/EmptyLinesAroundBlockBody`, multi-line brace layouts | No measured split; a fixed rule does as well |
| `Layout/LineLength`, `Metrics/*` | Thresholds, not styles |
| `Lint/*`, `Security/*`, `Bundler/*`, `Gemspec/*` | Correctness or project policy |

## rubyfmt

[rubyfmt](https://github.com/fables-tales/rubyfmt) is an opinionated Ruby formatter written in Rust, MIT licensed, started by Fable Tales as a personal project. Stripe funded it from 2022 and formats its whole Ruby codebase with it ([Stripe's write-up](https://stripe.dev/blog/formatting-an-entire-25-million-line-codebase-overnight-the-rubyfmt-story)). It has no configuration by design. It moved from Ripper, run through an embedded Ruby VM, to prism through the `ruby-prism` crate. Stripe checked behavior neutrality by diffing parse trees before and after formatting, allowing for expected changes such as quote conversion.

**Its fixed choices** (from its fixtures and `format_prism.rs`) amount to a list of edits a large production user treats as behavior-neutral. They back our equivalence table:

| rubyfmt does | Our norm | Takeaway |
|---|---|---|
| Rewrites every string, including `'…'` and `%q`/`%Q`, to `"…"` with re-escaping | `StringLiterals` | Its `string_escape.rs` holds the escaping rules for quote conversion |
| Removes trailing commas everywhere | Trailing comma | Confirms safety outside block params |
| `{a: 1}`: no space inside hash braces | `SpaceInsideHashLiteralBraces` | |
| Keeps hash syntax as written; converts a mixed `{x:, y => 2}` to all rockets | `HashSyntax` | Shorthand can't sit next to a rocket pair: a norm edit must not create that mix |
| Adds parens to call args (`expect(foo).to(redirect_to("bar"))`), except when parens would change meaning or for `attr_*` macros | `MethodCallWithArgsParentheses` | `use_parens_for_call_node` lists the cases where parens matter: a local of the same name in scope, a capitalized method name, a block-arg-only call. Reuse that list as our safety conditions |
| Keeps `do…end` vs `{}` as written | `BlockDelimiters` | Even rubyfmt won't swap block delimiters |
| Keeps `and`/`or`, modifiers, `unless`, `lambda` vs `->` | `AndOr`, `Lambda` | It only touches layout and literal spelling |
| `-> (arg)`: space and parens after `->` | `SpaceInLambdaLiteral`, `StabbyLambdaParentheses` | |
| `%w[…]`: rewrites word-array delimiters to brackets | `PercentLiteralDelimiters` | |
| Breaks a chain at every dot (leading dots) once it goes multi-line; 120-column limit | `DotPosition`, `LineLength` | |
| Collapses runs of blank lines to one; blank line after `private` | `EmptyLines*` | |

**Worth borrowing:**
- The safety conditions in `use_parens_for_call_node` and `string_escape.rs`: copy the logic, with MIT attribution, for the suggested edits of `StringLiterals` and the call-paren norm.
- The fixtures: about 380 `*_actual.rb`/`*_expected.rb` pairs, each a formatting-only change. They make a ready corpus for `restyle` equivalence tests and for norms' "format-only hunk" verdicts.
- `file_comments.rs` and `comment_block.rs`, which attach comments by line index, and `heredoc_string.rs` (squiggly heredoc indentation). These are the hard parts if norms ever rewrite multi-line constructs.

**Not worth borrowing:**
- Its renderer (`render_queue_writer`, breakables, a fixed 120 columns). It decides a layout from scratch; norms need minimal edits that match local style, which is the opposite job.
- A crate dependency. `rubyfmt` on crates.io is a `0.0.0-wip` placeholder, so the real library would come in as a git dependency. It pins `ruby-prism` 2.x, shapr uses 1.9, and `ruby-prism-sys` declares `links = "prism"`, so both can't be in one build: depending on it would force shapr onto prism 2 first. It would also pull in jemalloc and simplelog.
- Running it as an oracle. Its output is one fixed style (double quotes, parens everywhere), so diffing against it says nothing about a repo's own norms.
