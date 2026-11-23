# Changelog

## Unreleased

- First release. `shapr RANGE` classifies the hunks of `git diff RANGE`; with no range, a unified diff is read from stdin.
- Ruby files are parsed with prism. Hunks are `format_only` (same syntax tree, ignoring whitespace), `comment_only` (same tree, comment text differs) or `logic`. Ruby magic comments and rubocop directives count as code.
- A file with a syntax error is compared line by line instead (`parsed: false`, confidence at most 0.5). Files in other languages are `logic` with confidence 0.
- `-j` and `-J` emit the JSON schema in docs/schema.md: file, hunk and summary records, with code and comment line counts, `review_lines`, and Kamei's diffusion measures.
- The exit code carries the verdict: 0 structure (or empty), 1 behavior, 3 mixed; 2 is an error, 64 a usage error.
- `--completions [SHELL]` prints a completion script for bash, zsh, fish, elvish or PowerShell.
