# shapr development conventions

`shapr` classifies the **shape of a code change**. For each hunk in a diff it
says what kind of change it is (`format_only`, `comment_only`, `rename`,
`move`, `move_with_changes`, `extract`, `inline`, `logic`), and it gives the
whole change a verdict: structure, behavior, or mixed (Kent Beck's *Tidy
First*). It is a narrow, deterministic, pipe-first evidence tool; review
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
  cli.rs            clap, input selection (range or stdin), exit codes, after-help
  log.rs            -v telemetry (the verbose! macro)
  git.rs            `git diff RANGE`, blob reads (worktree fallback)
  diff.rs           unified diff parser: FileDiff, Hunk (narrowed to its changed region), Lines
  tree.rs           the language-neutral Tree and the TreeBuilder that normalizes it
                    (extents, comment placement, whitespace-insensitive hashes, per-line facts)
  frontend/
    mod.rs          the Frontend trait and the registry (for_path)
    ruby/mod.rs     prism → TreeBuilder; magic comments become code leaves
    ruby/generated.rs   prism node kinds and names (script/gen-ruby-kinds.sh; don't edit)
  change.rs         assembles a Change: per file, both trees and a Site per hunk
  classify/
    mod.rs          Kind, Method, Confidence, Label, the pass pipeline
    site.rs         hunk → tree mapping: each side's significant span and cover
    format.rs       the format_only / comment_only pass (tree or line method)
  report.rs         output records, summary metrics, text/JSON/NDJSON rendering
tests/cli.rs        end to end against throwaway git repos
```

### The language seam

A frontend implements `Frontend` (`language`, `handles`, `parse`) and reports
raw structure to a `TreeBuilder`: `open`/`close` for nodes with children,
`leaf` for nodes compared byte for byte, `comment`, `directive` (a comment the
language reads as code), `error`. The builder computes everything the core
relies on, so a second language needs no core change. The classification core
never imports a parser's types; test classification through a frontend.

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
- Fixtures are synthetic Ruby written for the test, with generic names.
- Prefer a test over a one-off CLI probe; a behavior worth checking once is
  worth keeping.

## Landing changes

Small, logically connected commits, structure or behavior but not both.
`CHANGELOG.md` gets its entry under `## Unreleased` in the commit that earns
it. Pre-1.0, prefer the clean break over a compatibility shim, but a schema
change still bumps `schema` and says what a consumer must do.
