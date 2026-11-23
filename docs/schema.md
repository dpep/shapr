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
| `move` | yes | Code removed in one place appears unchanged elsewhere, in the same file or another. Reordering methods within a class or module is a `move`; reordering anything else (callbacks, validations, `include`s, other DSL calls) is `logic`. *Not emitted yet.* |
| `move_with_changes` | no | A move whose body also changed. *Not emitted yet.* |
| `extract` | yes | Statements leave one method and become another method's body, replaced by a call. *Not emitted yet.* |
| `inline` | yes | The inverse of `extract`. *Not emitted yet.* |
| `logic` | no | Anything else. The default: a hunk shapr can't prove structural is `logic`. |

"Structure" describes the shape of a change, not its risk: extracts and
inlines in particular are linked to later bug fixes and still deserve review.

Tree equality is exact: node kinds, child order, and every non-whitespace
byte of each node's source must match, and leaves (string content, heredoc
bodies, symbols, numbers) match byte for byte, whitespace included. So
`do…end` versus `{…}`, a change of quote style, a trailing comma, or any
reordering is `logic`.

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
  (`send(:old)`, `const_get`, and for a field an `attr_*` of the same name). A
  leftover `send(:old)` is `logic`: it would now raise.
- A keyword parameter keeps its name (callers pass it), a renamed method
  doesn't call `super` (it would reach the parent's method of the new name),
  and global variables aren't renamed.
- A renamed dynamic reference (`send(:old)` → `send(:new)`) follows a method,
  constant or field rename that passes.

Scopes: a `local` (variable or parameter) lives in its enclosing method,
class or module; a `field` (instance or class variable) in its file; a
`method` or `constant` (class and module names included) in every parsed file
of the change.

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

## `file` record

| field | type | meaning |
|---|---|---|
| `record` | `"file"` | |
| `file` | string | Path after the change; the old path for a deleted file. |
| `old_file` | string or null | Path before the change; `null` for an added file. Differs from `file` on a rename. |
| `language` | string or null | The frontend that parsed it (`"ruby"`); `null` when none did. |
| `parsed` | bool | Every hunk in the file was compared as trees (`evidence.method` `"ast"`). |
| `lines_before` | int or null | Lines in the file before the change (Kamei's LT); 0 for an added file; `null` when its content couldn't be read. |
| `hunks` | int | Hunk records that follow for this file. |
| `added`, `deleted`, `code_added`, `code_deleted`, `comment_added`, `comment_deleted`, `review_lines` | int | Sums over the file's hunks. |

## `hunk` record

| field | type | meaning |
|---|---|---|
| `record` | `"hunk"` | |
| `id` | int | Position in the output, from 0. Kind-specific `detail` refers to other hunks by `id`. |
| `file`, `old_file`, `language` | | As in the `file` record. |
| `parsed` | bool | Compared as trees. `false` when either side has a syntax error (compared as text instead) or the file wasn't parsed at all. |
| `before`, `after` | line run | The changed region on each side. |
| `kind` | string | See [Hunk kinds](#hunk-kinds). |
| `confidence` | number 0–1 | How strongly the evidence supports `kind`. See [Confidence](#confidence). |
| `reason` | string | One short human-readable sentence. Not a contract: don't match on it. |
| `evidence` | object | What the label rests on. Always has `method`; other keys depend on the kind. See [Evidence](#evidence). |
| `added`, `deleted` | int | `+` and `-` lines (Kamei's LA and LD for this hunk). |
| `code_added`, `code_deleted` | int | Of those, lines carrying code. |
| `comment_added`, `comment_deleted` | int | Of those, lines carrying a comment, so comment growth is visible. |
| `review_lines` | int | Lines a reviewer still has to read once formatting, comments, renames and the unchanged bodies of moves are discounted. For `logic`, `code_added + code_deleted`; for structural kinds, 0. |
| `detail` | object | Kind-specific facts about the change, such as rename pairs. Empty (`{}`) unless the kind's section documents it. |

## Evidence

| key | type | when | meaning |
|---|---|---|---|
| `method` | string | always | `"ast"`: both sides parsed cleanly and their trees were compared. `"line"`: a side has a syntax error, so the region's text was compared, whitespace and comments aside. `"none"`: nothing was compared (no frontend for the file, a binary file, or content that couldn't be read). |
| `similarity` | number 0–1 | `method` `"ast"` and the code differs | Dice coefficient over the subtrees of the two sides' changed code: 1 when they share every subtree, 0 when none. |

## Confidence

Confidence is computed from the evidence, not assigned per kind.

- `method` `"ast"`: 1. Tree comparison is exact, so it settles
  `format_only`/`comment_only` versus `logic`. Detectors for rename, move,
  extract and inline will lower a `logic` hunk's confidence when they find
  partial evidence of a structural kind.
- `method` `"line"`: at most **0.5** (the line cap): text equality can't see a
  change in how whitespace parses. It falls in proportion to the region's lines
  that hold a syntax error: `0.5 × (1 − lines with an error / lines in both
  regions)`.
- `method` `"none"`: 0. The `logic` label is the safe default, not a finding.

## `summary` record

| field | type | meaning |
|---|---|---|
| `record` | `"summary"` | |
| `schema` | int | This schema's version. |
| `verdict` | string | `"structure"`: every hunk is a structural kind. `"behavior"`: none is. `"mixed"`: both. `"empty"`: no hunks. The exit code carries it too. |
| `hunks` | int | Hunk records. |
| `unparsed_hunks` | int | Hunks with `parsed: false`. |
| `kinds` | object | Hunk count per kind, every kind present. |
| `files` | int | Files changed (Kamei's NF). |
| `directories` | int | Distinct directories holding them (ND); the repo root counts as one. |
| `subsystems` | int | Distinct top-level directories (NS); files at the root count as one. |
| `entropy` | number | Kamei's change entropy, −Σ pₖ log₂ pₖ, where pₖ is file k's share of the changed lines (`added + deleted`). 0 for one file. |
| `added`, `deleted`, `code_added`, `code_deleted`, `comment_added`, `comment_deleted`, `review_lines` | int | Sums over all hunks. |
| `behavior_lines` | int | Code lines (`code_added + code_deleted`) in hunks of behavior kinds (`logic`, `move_with_changes`): the behavior change's size, comments excluded. |
| `structure_share` | number 0–1 | Changed lines (`added + deleted`) in structural hunks over all changed lines; 0 when nothing changed. |

## Exit codes

| code | meaning |
|---|---|
| 0 | `structure` or `empty` |
| 1 | `behavior` |
| 3 | `mixed` |
| 2 | operational error: git failed, input unreadable |
| 64 | usage: a bad command line, or no input |
