//! A change assembled for classification: each file's diff, both sides
//! parsed by its frontend, and every hunk mapped onto the trees.

use crate::classify::site::Site;
use crate::diff::{FileDiff, Hunk, Lines};
use crate::frontend;
use crate::git::{Blobs, Git};
use crate::tree::Tree;
use std::collections::HashMap;

pub struct Change {
    pub files: Vec<FileChange>,
}

pub struct FileChange {
    pub diff: FileDiff,
    /// A lockfile or generated file: its lines don't count toward size.
    pub generated: bool,
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

/// Reads and parses every file the diff names.
pub fn assemble(diffs: Vec<FileDiff>, git: &Git) -> Change {
    let paths: Vec<&str> = diffs.iter().map(FileDiff::path).collect();
    let attributes = git.linguist_generated(&paths);
    let mut blobs = Blobs::new(git);
    let files = diffs
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
        .collect();
    Change { files }
}

fn file_change(
    diff: FileDiff,
    blobs: &mut Blobs,
    attributes: &HashMap<String, bool>,
) -> FileChange {
    let generated = attributes
        .get(diff.path())
        .copied()
        .unwrap_or_else(|| generated_by_name(diff.path()));
    let before = read(blobs, diff.old_blob.as_deref(), diff.old_path.as_deref());
    let lines_before = before.as_ref().ok().map(|b| line_count(b));
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
        generated,
        lines_before,
        trees,
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

/// A lockfile or minified asset, by its path alone.
fn generated_by_name(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    LOCKFILES.contains(&name) || name.ends_with(".min.js") || name.ends_with(".min.css")
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

fn line_count(source: &[u8]) -> usize {
    let newlines = source.iter().filter(|&&b| b == b'\n').count();
    newlines + usize::from(source.last().is_some_and(|&b| b != b'\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockfiles_and_minified_assets_are_generated_by_name() {
        for path in [
            "Gemfile.lock",
            "web/package-lock.json",
            "go.sum",
            "assets/app.min.js",
        ] {
            assert!(generated_by_name(path), "{path}");
        }
        for path in [
            "Gemfile",
            "db/schema.rb",
            "lib/lock.rb",
            "app.js",
            "Cargo.lock.rb",
        ] {
            assert!(!generated_by_name(path), "{path}");
        }
    }
}
