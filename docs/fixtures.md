# Fixtures

A fixture is one code change and what shapr must say about it, kept as
files. Hand-written cases, cases harvested from labelled corpus hunks, and
the verdicts grokr's review TUI records ([decisions](decisions.md) #46) all
use this format, so every correction a maintainer makes becomes a regression
test. `tests/fixtures.rs` runs every case.

This page is the contract a writer of fixtures relies on: change it
additively, and give a breaking change a row in [decisions](decisions.md).

## Layout

```
tests/fixtures/<area>/<case>/
  case.json     what the case is, where it came from, and what shapr must say
  before/       the files the change touches, as they were
  after/        the same files, as they became
  repo/         files committed unchanged on both sides (optional)
  worktree/     files written over the working tree after both commits, uncommitted (optional)
```

- `<area>` groups cases by what they exercise (`norms`, `soundness`, `repository`, `parse`,
  `parts`, `corpus`, `zoo`: author-labelled commits from two toy repos). A `case.json` anywhere else under `tests/` fails the
  run, so a case can't sit where the runner doesn't look.
- `<case>` is a short descriptive name for a hand-written case. A case
  written by a tool is named by the first 12 hex digits of its [content
  hash](#content-hash); the runner checks that a 12-hex-digit name matches.
- Paths inside each tree are repository paths: `before/lib/a.rb` is
  `lib/a.rb` before the change. They decide language and category
  (`spec/` is test code), so keep the original path.
- A file only in `before/` is deleted by the change; one only in `after/`
  is added. Either tree may be absent.

Snippets are the smallest part of the file that still parses on both
sides, with only the labelled hunk applied, so a case holds one hunk.

## Running a case

The runner builds a repository in a temporary directory, with git's
environment cleared:

1. `repo/` and `before/` are written and committed.
2. `before/`'s files are removed, `after/` is written, and the result is
   committed.
3. `worktree/` is written over the working tree and left uncommitted.
4. shapr runs there with `args` (default `["-j", "HEAD~1..HEAD"]`). It must
   exit 0 with one JSON value on stdout, the report `expect` is matched
   against.

## case.json

| key | type | meaning |
|---|---|---|
| `description` | string | Required. One sentence: what the change is and why the case exists. |
| `args` | string array | shapr's arguments; default `["-j", "HEAD~1..HEAD"]`. `["norms", "-j", "lib/a.rb"]` runs `shapr norms`. Output must be one JSON value (`-j`). |
| `norm` | string | A norm id (`Style/StringLiterals`): `hunks[].style.norms` and `norms` keep only that norm's entries before matching. A hunk's rollup `direction` and `flagged` span every norm, so a case naming one leaves them out. |
| `open` | bool | A known gap: `expect` says what shapr should say and doesn't yet. The run lists open cases as shapr's to-do list and fails when one passes, so the fix drops `open` and the case guards it. Default false. |
| `source` | object | Where the change came from, for a harvested case. [Below](#source). |
| `label` | object | Who judged the change, and what each judge said. [Below](#label). |
| `expect` | object | Required. What the report must hold. [Below](#expect). |

Any other key fails the run, so a misspelled `expect` can't pass silently.

### expect

`expect` is a pattern over shapr's `-j` report ([schema](schema.md)), in the
report's own shape:

- An object matches when each key it names matches; keys it doesn't name
  are ignored. Assert stable, meaningful fields: a hunk's `kind` and
  `verdict`, its parts' `kind`, `structure` and `lines`, `assumptions`,
  `facets`, chosen `detail` keys. Never `reason`, which isn't a contract.
- An array matches when it has as many elements and each matches in order.
- A number matches by value (`1` matches `1.0`); anything else by equality.
- `{"$not": P}` matches when `P` doesn't: `"verdict": {"$not": "structure"}`
  says a hunk must not be structure.
- `{"$some": P}` matches an array with at least one element matching `P`;
  `{"$every": P}` one whose every element matches. Use them where the
  number or order of elements isn't the point:
  `"parts": {"$some": {"kind": "logic", "detail": {"near": "extract"}}}`.
- An object whose keys are all operators matches when each does:
  `"hunks": {"$every": {"kind": "logic"}, "$some": {"parts": ...}}` says
  every hunk is logic and one has the near miss.

```json
{
  "description": "a reformat beside an edit is a format part and a residual",
  "expect": {
    "hunks": [
      {
        "kind": "logic",
        "verdict": "mixed",
        "parts": [
          {"kind": "format_only", "lines": {"before": [{"start": 2, "count": 1}], "after": [{"start": 2, "count": 1}]}},
          {"kind": "logic", "lines": {"before": [{"start": 3, "count": 1}], "after": [{"start": 3, "count": 1}]}}
        ]
      }
    ],
    "summary": {"verdict": "mixed"}
  }
}
```

### source

| key | meaning |
|---|---|
| `repo` | The repository's URL. Only public repositories, under licenses that allow a snippet (MIT, Apache-2.0, BSD). |
| `commit` | The full sha of the commit the change is. |
| `file` | The file's path at that commit. |
| `before`, `after` | The hunk's changed region on each side, as line runs in the schema's shape. |
| `lines` | The run of the before file the snippet copies. |
| `license` | The repository's license (SPDX id). |

### label

| key | meaning |
|---|---|
| `labeller` | Who settled the label: `human`, `llm` or `tool`. |
| `by` | Which one: a GitHub login, a model, a tool and version. |
| `date` | When, as `YYYY-MM-DD`. |
| `category` | The labeller's verdict, in its own vocabulary (`behavior`, `test_only`, `style`, `rename_missed`, ...). |
| `note` | The labeller's justification, one sentence. |
| `tool` | What shapr said when the change was labelled: `kind`, `verdict`, `facets`, `near`, `version`, as known. |
| `classifier` | What a classifier said, when one judged it: `category`, `confidence`. |

`expect` says the labeller's verdict in shapr's terms. Where the labeller
and shapr agree, or where the label has no shapr term (`style`: a quote
change is logic by design), `expect` holds what shapr says now and the
label keeps the disagreement for the classifier loop. Where shapr is wrong
in its own terms (a rename it missed), `expect` holds the labeller's verdict
and the case is `open`.

## Content hash

A case's content hash is SHA-256, in lowercase hex, over its trees: for each
file under `before/`, `after/`, `repo/` and `worktree/`, sorted by its path
from the case directory (`after/lib/a.rb`), that path in UTF-8, a NUL byte,
the file's length in bytes in decimal, a NUL byte, and its bytes.
`case.json` is left out: one change is one case, however it is labelled.
Two cases with the same hash fail the run; relabelling a change edits its
case.

## The runner

```
cargo test --test fixtures                            every case
SHAPR_FIXTURE=soundness/ cargo test --test fixtures   cases whose <area>/<case> contains the text
SHAPR_BLESS=1 SHAPR_FIXTURE=parts/ cargo test --test fixtures
```

A failure names each mismatch by JSON path and shows the case's `expect`
against the report shaped like it (`-` expected, `+` actual).

`SHAPR_BLESS=1` rewrites each selected case's `expect` from current output,
keeping the fields it already asserts and their operators; a case with no
`expect` gets each hunk's `file`, `kind`, `verdict` and parts (`kind`,
`structure`, `lines`, `assumptions`, `facets`, a near miss's `near` and
`risk`) and the change's `verdict`. Review the diff before committing it:
bless records what shapr says, not what it should say. It never touches an
`open` case and refuses to run when `CI` is set.
