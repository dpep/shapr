# shapr

Classify the shape of a code change. For each hunk in a diff, shapr says whether it is format-only, comment-only, a rename, a move, an extract, or logic, and gives the change an overall verdict: pure structure, pure behavior, or mixed.

Pipe-first: give it a git range or a unified diff on stdin, and it writes one JSON line per hunk.

Status: early development. Ruby first (parsed with ruby-prism); Rust, JavaScript, TypeScript and Python to follow via tree-sitter.
