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
}

/// Reads and parses every file the diff names. BEFORE is the revision the
/// change starts from, when known.
pub fn assemble(diffs: Vec<FileDiff>, git: &Git, before: Option<&str>) -> Change {
    let paths: Vec<&str> = diffs.iter().map(FileDiff::path).collect();
    let mut attributes = git.linguist(&paths, None);
    // An attribute the change itself sets mustn't hide the change: where the
    // change edits a .gitattributes, a file it doesn't add keeps whichever
    // side's category is reviewed.
    let edits_attributes = diffs
        .iter()
        .flat_map(|d| [d.old_path.as_deref(), d.new_path.as_deref()])
        .flatten()
        .any(|p| p.rsplit('/').next() == Some(".gitattributes"));
    if edits_attributes {
        let old: Vec<&str> = diffs.iter().filter_map(|d| d.old_path.as_deref()).collect();
        let was = before
            .map(|rev| git.linguist(&old, Some(rev)))
            .unwrap_or_default();
        for diff in diffs.iter().filter(|d| !d.binary) {
            let Some(old) = diff.old_path.as_deref() else {
                continue;
            };
            let after = attributes.get(diff.path()).copied().unwrap_or_default();
            let before = was.get(old).copied().unwrap_or_default();
            if Category::of(old, false, before).counted()
                && !Category::of(diff.path(), false, after).counted()
            {
                attributes.insert(diff.path().to_string(), before);
            }
        }
    }
    let mut blobs = Blobs::new(git);
    let mut files = diffs
        .into_iter()
        .filter_map(|mut diff| {
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
                return None;
            }
            Some(file_change(diff, &mut blobs, &attributes))
        })
        .collect::<Vec<FileChange>>();
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
) -> FileChange {
    let linguist = attributes.get(diff.path()).copied().unwrap_or_default();
    let category = Category::of(diff.path(), diff.binary, linguist);
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
    let trees = if diff.binary {
        Err("binary file".to_string())
    } else {
        match frontend::for_path(diff.path()) {
            None => Err(format!("no frontend for {}", diff.path())),
            Some(frontend) => before.and_then(|before| {
                let after = read(blobs, diff.new_blob.as_deref(), diff.new_path.as_deref())?;
                let before = frontend.parse(before);
                let after = frontend.parse(after);
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
    Config,
    /// A dependency manifest.
    Manifest,
    /// Written by a tool: a lockfile, a minified asset, `linguist-generated`.
    Generated,
    /// Third-party code: under `vendor/`, `third_party/` or `node_modules/`, or
    /// `linguist-vendored`.
    Vendored,
    Binary,
    /// None of the above: source shapr doesn't parse, or anything else.
    Unknown,
}

impl Category {
    pub const ALL: [Category; 9] = [
        Category::Code,
        Category::Test,
        Category::Docs,
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
            Category::Config => "config",
            Category::Manifest => "manifest",
            Category::Generated => "generated",
            Category::Vendored => "vendored",
            Category::Binary => "binary",
            Category::Unknown => "unknown",
        }
    }

    /// The first rule that matches. A linguist attribute wins over the
    /// name either way: `-linguist-generated` unmarks a lockfile.
    pub fn of(path: &str, binary: bool, linguist: Linguist) -> Category {
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

/// Under a third-party directory, as linguist reads them.
fn vendored(path: &str) -> bool {
    (path.split('/').rev().skip(1)).any(|d| matches!(d, "vendor" | "third_party" | "node_modules"))
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
            ("tests/fixtures/x.json", "test"),
            ("Gemfile", "manifest"),
            ("acme.gemspec", "manifest"),
            ("package.json", "manifest"),
            ("config/app.yml", "config"),
            (".rubocop.yml", "config"),
            ("Cargo.toml.orig", "unknown"),
            ("app.js", "unknown"),
            ("vendor/x.rb", "vendored"),
            ("app/vendor/lib/y.js", "vendored"),
            ("lib/vendored.rb", "code"),
            ("Makefile", "unknown"),
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
    }
}
