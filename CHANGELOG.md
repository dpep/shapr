# Changelog

## Unreleased

- First release. `shapr RANGE` classifies the hunks of `git diff RANGE`; with no range, a unified diff is read from stdin.
- Ruby files are parsed with prism. Hunks are `format_only` (same syntax tree, ignoring whitespace), `comment_only` (same tree, comment text differs) or `logic`. Ruby magic comments and rubocop directives count as code.
- Hunks that only rename identifiers are `rename`, when every use of each old name in its scope (a local's method, a field's file, a method's or constant's whole change) became the new name and the new name wasn't already there. `detail.renames` lists the pairs; confidence falls where uses can live outside the diff (methods, constants, fields). A leftover `send(:old)`, a keyword-parameter rename, or a renamed method that calls `super` stays `logic`.
- Hunks are `move` when they only relocate method, class, module or constant definitions, within a file or across files, and `move_with_changes` (a behavior kind) when a relocated definition's body also changed. Both halves name each other by hunk `id` in `detail.moves`, with the changed lines of a `move_with_changes`. A definition that changes owner or visibility section, or moves past a statement naming it at load time, stays `logic`. Reordering other statements (callbacks, validations, `include`s) is `logic`.
- A file with a syntax error is compared line by line instead (`parsed: false`, confidence at most 0.5). Files in other languages are `logic` with confidence 0.
- `extract` and `inline` hunks: statements moved into a new method and replaced by a call to it, or the reverse. Matches must be exact after renaming parameters to arguments; a match that would change behavior (`return`, `yield`, the caller's block, mismatched arguments, a name already in use, a tiny chunk) stays `logic` with `detail.near` and `detail.risk`.
- `-j` and `-J` emit the JSON schema in docs/schema.md: file, hunk and summary records, with code and comment line counts, `review_lines`, and Kamei's diffusion measures.
- The exit code carries the verdict: 0 structure (or empty), 1 behavior, 3 mixed; 2 is an error, 64 a usage error.
- `--completions [SHELL]` prints a completion script for bash, zsh, fish, elvish or PowerShell.
