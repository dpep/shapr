//! The repository state norms read beside the diff: linter config and a new
//! file's neighbors, from the revision the change produced, or the working
//! tree when the change ends there. Paths are relative to the repository's
//! top, `/`-separated; nothing above the top is read.

use crate::git::Git;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

pub trait Snapshot {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// The files directly in `dir` (`""` for the top), as paths.
    fn list(&self, dir: &str) -> Vec<String>;
}

pub struct Worktree(pub PathBuf);

impl Snapshot for Worktree {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }

    fn list(&self, dir: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.0.join(dir)) else {
            return Vec::new();
        };
        let mut files: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| e.file_name().into_string().ok())
            .map(|name| join(dir, &name))
            .collect();
        files.sort();
        files
    }
}

/// A revision's files, each read once: every file's config lookup asks for
/// the same few paths.
pub struct Revision<'a> {
    git: &'a Git,
    rev: String,
    read: RefCell<HashMap<String, Option<Vec<u8>>>>,
}

impl<'a> Revision<'a> {
    pub fn new(git: &'a Git, rev: String) -> Self {
        Revision {
            git,
            rev,
            read: RefCell::default(),
        }
    }
}

impl Snapshot for Revision<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let mut read = self.read.borrow_mut();
        read.entry(path.to_string())
            .or_insert_with(|| self.git.show(&self.rev, path))
            .clone()
    }

    fn list(&self, dir: &str) -> Vec<String> {
        self.git.ls(&self.rev, dir)
    }
}

/// `dir/name`, or `name` at the top.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The directory a path is in; `""` at the top.
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// `dir/rel` with `.` and `..` resolved; `None` when it climbs above the top.
pub fn resolve(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_within_the_top() {
        assert_eq!(resolve("a/b", "../c.yml").as_deref(), Some("a/c.yml"));
        assert_eq!(resolve("", "./x.yml").as_deref(), Some("x.yml"));
        assert_eq!(resolve("a", "../../x.yml"), None);
        assert_eq!(parent("a/b.rb"), "a");
        assert_eq!(parent("b.rb"), "");
    }
}
