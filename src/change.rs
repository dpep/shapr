//! A change assembled for classification: each file's diff, both sides
//! parsed by its frontend, and every hunk mapped onto the trees.

use crate::classify::site::Site;
use crate::diff::{FileDiff, Hunk, Lines};
use crate::frontend;
use crate::git::{Blobs, Git, Linguist};
use crate::tree::Tree;
use serde::Serialize;
use std::collections::HashMap;

pub struct Change {
    pub files: Vec<FileChange>,
}

pub struct FileChange {
    pub diff: FileDiff,
    /// Per hunk, in `diff.hunks` order: the output id of the hunk it reports in.
    pub ids: Vec<usize>,
    pub category: Category,
    /// Per hunk, in `diff.hunks` order: in a file no frontend reads, the
    /// changed lines differ only in trailing whitespace and blank lines.
    pub whitespace_only: Vec<bool>,
    /// Line count of the file before the change, when its content was read.
    pub lines_before: Option<usize>,
    /// Why the file couldn't be parsed: no frontend, binary, a blob not found.
    pub trees: Result<Trees, String>,
}

pub struct Trees {
    pub language: &'static str,
    pub before: Tree,
    pub after: Tree,
    /// One per hunk, in `diff.hunks` order.
    pub sites: Vec<Site>,
}

impl Trees {
    /// Neither side has a syntax error, so the trees can be compared.
    pub fn clean(&self) -> bool {
        self.before.errors.is_empty() && self.after.errors.is_empty()
    }
}

impl FileChange {
    pub fn language(&self) -> Option<&'static str> {
        self.trees.as_ref().ok().map(|t| t.language)
    }

    /// Its hunks count toward the verdict, compared or not: a counted
    /// category, except a test in a language shapr doesn't parse.
    pub fn counts(&self) -> bool {
        self.category.counted()
            && (self.category != Category::Test || frontend::for_path(self.diff.path()).is_some())
    }
}

/// Deepest nesting the classification walks; deeper code isn't compared.
/// The passes recurse once per level.
pub const MAX_DEPTH: usize = 10_000;

/// Most changed lines (removed plus added) one file may hold and be
/// compared. The passes grow with their square: removed lines paired with
/// added ones, and a file only added has its declarations compared with
/// each other (a 4,000-line file shuffled, or 8,000 one-line methods added,
/// take seconds).
pub const MAX_LINES: usize = 8_000;

/// Most work one change compares: the sum of its files' changed lines
/// squared. Files past it aren't compared.
pub const MAX_CHANGE_WORK: usize = 4 * MAX_LINES * MAX_LINES;

/// Changed bytes that count as one changed line, about a one-line method:
/// code packed onto a few long lines costs what it would on lines of its own.
pub const LINE_BYTES: usize = 24;

/// Largest side a file may have and be parsed; the largest Ruby or Rust
/// file seen in real repositories is about 3 MB, and parsing and comparing
/// take about a second per megabyte.
pub const MAX_FILE_BYTES: usize = 4 << 20;

/// Most children one node may have, the widest node of each side
/// multiplied: aligning two bodies builds a table of that size.
pub const MAX_WIDTH: usize = 1 << 24;

/// DIFF's changed lines, removed plus added.
fn changed_lines(diff: &FileDiff) -> usize {
    diff.hunks
        .iter()
        .map(|h| h.removed.len() + h.added.len())
        .sum()
}

/// The bytes of DIFF's changed lines, removed in BEFORE plus added in AFTER.
fn changed_bytes(diff: &FileDiff, before: &[u8], after: &[u8]) -> usize {
    let bytes = |source: &[u8], lines: &mut dyn Iterator<Item = usize>| -> usize {
        let all: Vec<&[u8]> = source.split(|&b| b == b'\n').collect();
        lines.filter_map(|n| all.get(n - 1)).map(|l| l.len()).sum()
    };
    let removed = &mut diff.hunks.iter().flat_map(|h| h.removed.iter().copied());
    let added = &mut diff.hunks.iter().flat_map(|h| h.added.iter().copied());
    bytes(before, removed) + bytes(after, added)
}

/// Why a file of LINES changed lines holding BYTES isn't compared after
/// SPENT work; `None` when it is. Its size is its lines, or its bytes in
/// lines of [`LINE_BYTES`], whichever is more.
fn too_big(lines: usize, bytes: usize, spent: usize) -> Option<String> {
    let size = lines.max(bytes.div_ceil(LINE_BYTES));
    if lines > MAX_LINES {
        Some(format!("too big: {lines} changed lines, past {MAX_LINES}"))
    } else if size > MAX_LINES {
        Some(format!(
            "too big: {bytes} changed bytes, past {}",
            MAX_LINES * LINE_BYTES
        ))
    } else if spent + size * size > MAX_CHANGE_WORK {
        Some(format!(
            "too big: the change's files' changed lines, squared, pass {MAX_CHANGE_WORK}"
        ))
    } else {
        None
    }
}

/// Why trees this big aren't compared; `None` when they are.
fn too_wide(before: &Tree, after: &Tree) -> Option<String> {
    let widest = |t: &Tree| t.nodes.iter().map(|n| n.children.len()).max().unwrap_or(0);
    let (b, a) = (widest(before), widest(after));
    (b.saturating_mul(a) > MAX_WIDTH).then(|| {
        format!("too big: a node of {b} children before and {a} after, past {MAX_WIDTH} multiplied")
    })
}

/// Reads and parses every file the diff names. BEFORE and AFTER are the
/// revisions the change starts from and ends in, when known.
pub fn assemble(
    diffs: Vec<FileDiff>,
    git: &Git,
    before: Option<&str>,
    after: Option<&str>,
) -> Change {
    let paths: Vec<&str> = diffs.iter().map(FileDiff::path).collect();
    let attributes = git.linguist(&paths, after);
    // Each file's attributes before the change, for its category there, for
    // both its paths (a file it adds included): at the revision it starts
    // from, whatever the diff lists (a path limit can leave out the
    // .gitattributes it edits). On stdin, where it edits one, unknown, as none.
    let both: Vec<&str> = diffs
        .iter()
        .flat_map(|d| [d.old_path.as_deref(), d.new_path.as_deref()])
        .flatten()
        .collect();
    let edits_attributes = both
        .iter()
        .any(|p| p.rsplit('/').next() == Some(".gitattributes"));
    let was = match (before, edits_attributes) {
        (Some(rev), _) => git.linguist(&both, Some(rev)),
        (None, true) => HashMap::new(),
        (None, false) => git.linguist(&both, after),
    };
    let mut blobs = Blobs::new(git);
    let mut spent = 0;
    // The change's budget is spent smallest file first, so files named to
    // sort first can't push a small edit out of the verdict.
    let mut diffs: Vec<Option<FileDiff>> = diffs.into_iter().map(Some).collect();
    let mut by_size: Vec<usize> = (0..diffs.len()).collect();
    by_size.sort_by_key(|&i| diffs[i].as_ref().map_or(0, changed_lines));
    let mut made: Vec<Option<FileChange>> = diffs.iter().map(|_| None).collect();
    for i in by_size {
        let Some(mut diff) = diffs[i].take() else {
            continue;
        };
        if diff.binary && diff.hunks.is_empty() {
            // Stands for the whole file, so a binary change is never silent.
            let whole = Lines { start: 1, count: 0 };
            diff.hunks.push(Hunk {
                before: whole,
                after: whole,
                ..Hunk::default()
            });
        }
        // A pure rename or mode change: no content to classify.
        if diff.hunks.is_empty() {
            continue;
        }
        made[i] = Some(file_change(diff, &mut blobs, &attributes, &was, &mut spent));
    }
    let mut files: Vec<FileChange> = made.into_iter().flatten().collect();
    let mut next = 0;
    for file in &mut files {
        file.ids = (next..next + file.diff.hunks.len()).collect();
        next += file.diff.hunks.len();
    }
    Change { files }
}

fn file_change(
    diff: FileDiff,
    blobs: &mut Blobs,
    attributes: &HashMap<String, Linguist>,
    was: &HashMap<String, Linguist>,
    spent: &mut usize,
) -> FileChange {
    // git calls a text file binary under an attribute (`-diff`, `binary`),
    // and code holding a NUL; a range is diffed again as text, a diff on
    // stdin can't be.
    let attributed = diff.binary && !crate::git::holds_binary(blobs, &diff);
    let binary = diff.binary && !attributed;
    let category = |path: &str, attributes: &HashMap<String, Linguist>| {
        let linguist = attributes.get(path).copied().unwrap_or_default();
        Category::of(path, binary, linguist)
    };
    // A change can't hide its own edit: a file that counts toward the verdict
    // on either side (moved into `vendor/`, marked generated) counts, and
    // so does one the attributes before the change would count (added and
    // marked generated at once).
    let after = category(diff.path(), attributes);
    let before = [diff.old_path.as_deref(), diff.new_path.as_deref()]
        .into_iter()
        .flatten()
        .map(|path| category(path, was))
        .find(|c| c.counted());
    let category = match before {
        Some(before) if !after.counted() => before,
        _ => after,
    };
    let before = read(blobs, diff.old_blob.as_deref(), diff.old_path.as_deref());
    let lines_before = before.as_ref().ok().map(|b| line_count(b));
    let mut whitespace_only = vec![false; diff.hunks.len()];
    if let (false, None, Ok(before)) = (diff.binary, frontend::for_path(diff.path()), &before)
        && let Ok(after) = read(blobs, diff.new_blob.as_deref(), diff.new_path.as_deref())
    {
        whitespace_only = diff
            .hunks
            .iter()
            .map(|h| only_whitespace(h, before, &after))
            .collect();
    }
    let trees = if attributed {
        Err("git diffed it as binary (an attribute, or a NUL in code): give the range, or pipe `git diff --text`".to_string())
    } else if diff.binary {
        Err("binary file".to_string())
    } else if category == Category::Fixture {
        Err("test data, not classified".to_string())
    } else if category == Category::Types {
        Err("type declarations, not classified".to_string())
    } else {
        let lines = changed_lines(&diff);
        match frontend::for_path(diff.path()) {
            None => Err(format!("no frontend for {}", diff.path())),
            Some(_) if lines > MAX_LINES => Err(too_big(lines, 0, 0).unwrap_or_default()),
            Some(frontend) => before.and_then(|before| {
                let after = read(blobs, diff.new_blob.as_deref(), diff.new_path.as_deref())?;
                let largest = before.len().max(after.len());
                if largest > MAX_FILE_BYTES {
                    return Err(format!("too big: {largest} bytes, past {MAX_FILE_BYTES}"));
                }
                let bytes = changed_bytes(&diff, &before, &after);
                if let Some(why) = too_big(lines, bytes, *spent) {
                    return Err(why);
                }
                let size = lines.max(bytes.div_ceil(LINE_BYTES));
                *spent += size * size;
                let before = frontend.parse(before);
                let after = frontend.parse(after);
                if before.depth.max(after.depth) > MAX_DEPTH {
                    return Err(format!("code nested deeper than {MAX_DEPTH} levels"));
                }
                if let Some(why) = too_wide(&before, &after) {
                    return Err(why);
                }
                let sites = diff
                    .hunks
                    .iter()
                    .map(|h| Site::new(&before, &after, h))
                    .collect();
                Ok(Trees {
                    language: frontend.language(),
                    before,
                    after,
                    sites,
                })
            }),
        }
    };
    crate::verbose!(
        "{}: {}",
        diff.path(),
        match &trees {
            Ok(t) if t.clean() => format!("parsed as {}", t.language),
            Ok(t) => format!("parsed as {} with syntax errors", t.language),
            Err(why) => why.clone(),
        }
    );
    FileChange {
        diff,
        ids: Vec::new(),
        category,
        whitespace_only,
        lines_before,
        trees,
    }
}

/// What kind of file a change touches, one per file. Orthogonal to parsing:
/// a test file in a parsed language is parsed and classified like code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Source in a language shapr parses.
    Code,
    Test,
    Docs,
    /// Test data: under a `fixtures/`, `__fixtures__/` or `testdata/`
    /// directory. Not classified; the tests that read it judge it.
    Fixture,
    /// Type declarations only a type checker reads: Sorbet's RBI files,
    /// RBS signatures. Not classified.
    Types,
    Config,
    /// A dependency manifest.
    Manifest,
    /// Written by a tool: a lockfile, a minified asset, `linguist-generated`.
    Generated,
    /// Third-party code: under a top-level `vendor/`, `third_party/` or
    /// `node_modules/`, or `linguist-vendored`.
    Vendored,
    Binary,
    /// None of the above: source shapr doesn't parse, or anything else.
    Unknown,
}

impl Category {
    pub const ALL: [Category; 11] = [
        Category::Code,
        Category::Test,
        Category::Docs,
        Category::Fixture,
        Category::Types,
        Category::Config,
        Category::Manifest,
        Category::Generated,
        Category::Vendored,
        Category::Binary,
        Category::Unknown,
    ];

    /// Its hunks count toward the verdict.
    pub fn counted(self) -> bool {
        matches!(
            self,
            Category::Code | Category::Test | Category::Config | Category::Manifest
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Code => "code",
            Category::Test => "test",
            Category::Docs => "docs",
            Category::Fixture => "fixture",
            Category::Types => "types",
            Category::Config => "config",
            Category::Manifest => "manifest",
            Category::Generated => "generated",
            Category::Vendored => "vendored",
            Category::Binary => "binary",
            Category::Unknown => "unknown",
        }
    }

    /// A fixture directory, then a type declaration, wins over the name
    /// rules, but not over a binary, a vendored file or a set linguist
    /// attribute; a type directory's prose stays docs.
    pub fn of(path: &str, binary: bool, linguist: Linguist) -> Category {
        let category = Category::by_name(path, binary, linguist);
        match category {
            Category::Binary | Category::Vendored => category,
            Category::Generated if linguist.generated == Some(true) => category,
            Category::Docs if linguist.documentation == Some(true) => category,
            _ if fixture(path) => Category::Fixture,
            Category::Docs => category,
            _ if types(path) => Category::Types,
            _ => category,
        }
    }

    /// The first rule that matches. A linguist attribute wins over the
    /// name either way: `-linguist-generated` unmarks a lockfile.
    fn by_name(path: &str, binary: bool, linguist: Linguist) -> Category {
        let name = path.rsplit('/').next().unwrap_or(path);
        if binary {
            Category::Binary
        } else if linguist.generated.unwrap_or_else(|| generated(name)) {
            Category::Generated
        } else if linguist.vendored.unwrap_or_else(|| vendored(path)) {
            Category::Vendored
        } else if linguist.documentation.unwrap_or_else(|| docs(path, name)) {
            Category::Docs
        } else if test(path, name) {
            Category::Test
        } else if manifest(name) {
            Category::Manifest
        } else if config(name) {
            Category::Config
        } else if frontend::for_path(path).is_some() {
            Category::Code
        } else {
            Category::Unknown
        }
    }
}

/// Lockfiles a package manager writes; reviewing the manifest change that
/// produced them is the review.
const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "Gemfile.lock",
    "Package.resolved",
    "Pipfile.lock",
    "Podfile.lock",
    "bun.lock",
    "composer.lock",
    "flake.lock",
    "go.sum",
    "mix.lock",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "poetry.lock",
    "pubspec.lock",
    "uv.lock",
    "yarn.lock",
];

/// A lockfile or minified asset.
fn generated(name: &str) -> bool {
    LOCKFILES.contains(&name) || name.ends_with(".min.js") || name.ends_with(".min.css")
}

/// Under a third-party directory at the repository's top. Deeper, the name
/// is often the project's own namespace (a Rails `app/models/vendor/`).
fn vendored(path: &str) -> bool {
    path.split_once('/')
        .is_some_and(|(top, _)| matches!(top, "vendor" | "third_party" | "node_modules"))
}

/// Prose: under a docs directory, a prose extension, or a conventional
/// top-level text file (`README`, `LICENSE`, ...), as linguist reads them.
fn docs(path: &str, name: &str) -> bool {
    let dirs = path.rsplit_once('/').map_or("", |(d, _)| d);
    let in_docs = dirs.split('/').any(|d| {
        matches!(
            d.to_ascii_lowercase().as_str(),
            "doc" | "docs" | "documentation"
        )
    });
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let prose = matches!(
        ext.to_ascii_lowercase().as_str(),
        "md" | "markdown"
            | "mdx"
            | "txt"
            | "rdoc"
            | "adoc"
            | "asciidoc"
            | "rst"
            | "org"
            | "textile"
    );
    let conventional = matches!(
        stem.to_ascii_uppercase().as_str(),
        "README"
            | "LICENSE"
            | "LICENCE"
            | "COPYING"
            | "CHANGELOG"
            | "CHANGES"
            | "HISTORY"
            | "NEWS"
            | "AUTHORS"
            | "CONTRIBUTORS"
            | "NOTICE"
    );
    in_docs || prose || conventional
}

/// Test data: under a `fixtures`, `__fixtures__` or `testdata` directory, at
/// any depth.
fn fixture(path: &str) -> bool {
    let dirs = path.rsplit_once('/').map_or("", |(d, _)| d);
    dirs.split('/')
        .any(|d| matches!(d, "fixtures" | "__fixtures__" | "testdata"))
}

/// Type declarations: an `.rbi` or `.rbs` file, or under an `rbi/`
/// directory at any depth (`sorbet/rbi/`) or a top-level `sig/` (RBS).
fn types(path: &str) -> bool {
    let dirs = path.rsplit_once('/').map_or("", |(d, _)| d);
    path.ends_with(".rbi")
        || path.ends_with(".rbs")
        || dirs.split('/').any(|d| d == "rbi")
        || path.starts_with("sig/")
}

/// A test file: under a `spec`, `test`, `tests` or `__tests__` directory, or
/// named like one.
fn test(path: &str, name: &str) -> bool {
    let dirs = path.rsplit_once('/').map_or("", |(d, _)| d);
    dirs.split('/')
        .any(|d| matches!(d, "spec" | "test" | "tests" | "__tests__"))
        || ["_spec.rb", "_test.rb", "_test.rs", "_test.go", "_test.py"]
            .iter()
            .any(|s| name.ends_with(s))
        || (name.starts_with("test_") && (name.ends_with(".rb") || name.ends_with(".py")))
        || name.contains(".test.")
        || name.contains(".spec.")
        || matches!(name, "tests.rs" | "test.rs")
}

/// The manifests beside the lockfiles.
fn manifest(name: &str) -> bool {
    name.ends_with(".gemspec")
        || matches!(
            name,
            "Gemfile"
                | "gems.rb"
                | "Cargo.toml"
                | "package.json"
                | "go.mod"
                | "pyproject.toml"
                | "Pipfile"
                | "Podfile"
                | "composer.json"
                | "mix.exs"
                | "pubspec.yaml"
                | "Package.swift"
                | "flake.nix"
        )
}

/// Configuration by extension, or a dotenv file.
fn config(name: &str) -> bool {
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    name == ".env"
        || name.starts_with(".env.")
        || matches!(
            ext.to_ascii_lowercase().as_str(),
            "yml"
                | "yaml"
                | "toml"
                | "json"
                | "jsonc"
                | "json5"
                | "ini"
                | "cfg"
                | "conf"
                | "properties"
                | "env"
                | "xml"
                | "plist"
        )
}

/// A side's content; a side that doesn't exist is empty.
fn read(blobs: &mut Blobs, id: Option<&str>, path: Option<&str>) -> Result<Vec<u8>, String> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let id = id.ok_or("the diff names no blob ids (want `git diff` output)")?;
    blobs
        .read(id, path)
        .ok_or_else(|| format!("blob {id} not found"))
}

/// The hunk's removed and added lines are the same once trailing whitespace
/// and blank lines are dropped. Indentation counts: YAML, Python and
/// Makefiles read it.
fn only_whitespace(hunk: &Hunk, before: &[u8], after: &[u8]) -> bool {
    fn kept<'a>(source: &'a [u8], lines: &[usize]) -> Vec<&'a [u8]> {
        let all: Vec<&[u8]> = source.split(|&b| b == b'\n').collect();
        lines
            .iter()
            .filter_map(|&n| all.get(n - 1))
            .map(|l| l.trim_ascii_end())
            .filter(|l| !l.is_empty())
            .collect()
    }
    !(hunk.removed.is_empty() && hunk.added.is_empty())
        && kept(before, &hunk.removed) == kept(after, &hunk.added)
}

fn line_count(source: &[u8]) -> usize {
    let newlines = source.iter().filter(|&&b| b == b'\n').count();
    newlines + usize::from(source.last().is_some_and(|&b| b != b'\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn category(path: &str) -> &'static str {
        Category::of(path, false, Linguist::default()).as_str()
    }

    #[test]
    fn a_file_or_a_change_past_its_lines_isnt_compared() {
        assert_eq!(too_big(MAX_LINES, 0, 0), None);
        assert!(
            too_big(MAX_LINES + 1, 0, 0)
                .unwrap()
                .contains("lines, past")
        );
        let bytes = MAX_LINES * LINE_BYTES;
        assert_eq!(too_big(2, bytes, 0), None);
        assert!(too_big(2, bytes + 1, 0).unwrap().contains("bytes, past"));
        let rest = MAX_CHANGE_WORK - MAX_LINES * MAX_LINES;
        assert_eq!(too_big(MAX_LINES, 0, rest), None);
        assert!(
            too_big(MAX_LINES, 0, rest + 1)
                .unwrap()
                .contains("the change's")
        );
    }

    #[test]
    fn categories_by_name() {
        for (path, want) in [
            ("Gemfile.lock", "generated"),
            ("web/package-lock.json", "generated"),
            ("assets/app.min.js", "generated"),
            ("lib/lock.rb", "code"),
            ("Cargo.lock.rb", "code"),
            ("db/schema.rb", "code"),
            ("README.md", "docs"),
            ("LICENSE", "docs"),
            ("docs/index.html", "docs"),
            ("spec/a_spec.rb", "test"),
            ("lib/a_test.rs", "test"),
            ("tests/fixtures/x.json", "fixture"),
            ("spec/fixtures/cart.rb", "fixture"),
            ("tests/fixtures/repository/case/before/lib/a.rb", "fixture"),
            ("pkg/testdata/in.go", "fixture"),
            ("src/__fixtures__/README.md", "fixture"),
            ("lib/fixtures.rb", "code"),
            ("spec/fixtures_spec.rb", "test"),
            ("vendor/gem/spec/fixtures/a.rb", "vendored"),
            ("Gemfile", "manifest"),
            ("acme.gemspec", "manifest"),
            ("package.json", "manifest"),
            ("config/app.yml", "config"),
            (".rubocop.yml", "config"),
            ("Cargo.toml.orig", "unknown"),
            ("app.js", "unknown"),
            ("vendor/x.rb", "vendored"),
            ("third_party/lib/y.js", "vendored"),
            ("app/models/vendor/y.rb", "code"),
            ("lib/vendored.rb", "code"),
            ("Makefile", "unknown"),
            ("lib/a.rbi", "types"),
            ("sorbet/rbi/gems/x@1.rbi", "types"),
            ("sorbet/rbi/todo.rb", "types"),
            ("spec/a.rbi", "types"),
            ("sig/a.rbs", "types"),
            ("lib/sig/a.rb", "code"),
            ("sorbet/rbi/README.md", "docs"),
            ("spec/fixtures/a.rbi", "fixture"),
        ] {
            assert_eq!(category(path), want, "{path}");
        }
    }

    #[test]
    fn an_attribute_wins_over_the_name() {
        let unset = Linguist {
            generated: Some(false),
            documentation: Some(false),
            ..Linguist::default()
        };
        assert_eq!(Category::of("yarn.lock", false, unset), Category::Unknown);
        assert_eq!(Category::of("NOTES.md", false, unset), Category::Unknown);
        let vendored = Linguist {
            vendored: Some(true),
            ..Linguist::default()
        };
        assert_eq!(
            Category::of("lib/a.rb", false, vendored),
            Category::Vendored
        );
        assert_eq!(Category::of("a.rb", true, vendored), Category::Binary);
        let documentation = Linguist {
            documentation: Some(true),
            ..Linguist::default()
        };
        assert_eq!(
            Category::of("spec/fixtures/a.rb", false, documentation),
            Category::Docs
        );
        assert_eq!(
            Category::of("spec/fixtures/a.rb", false, unset),
            Category::Fixture
        );
    }
}
