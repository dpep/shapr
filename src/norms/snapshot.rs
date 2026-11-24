//! The repository state norms read beside the diff: linter config and a
//! file's neighbors, from the revision the change produced, or the index or
//! the working tree when the change ends there. Paths are relative to the
//! repository's top, `/`-separated; nothing above the top is read.

use crate::git::Git;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The largest linter config file read: past any real one, and a bound on
/// the parsing every judged file repeats.
pub const MAX_CONFIG_BYTES: usize = 1 << 20;

/// The note for a config file past [`MAX_CONFIG_BYTES`].
pub fn too_large(path: &str) -> String {
    format!("{path}: not read (over 1 MiB)")
}

pub trait Snapshot {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// The files directly in `dir` (`""` for the top), as paths.
    fn list(&self, dir: &str) -> Vec<String>;
    /// Several files, in order: a directory's neighbors.
    fn read_all(&self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
        paths.iter().map(|p| self.read(p)).collect()
    }
}

/// The working tree under a repository's top. A symbolic link is followed
/// only where it resolves inside the top.
pub struct Worktree(pub PathBuf);

impl Snapshot for Worktree {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let file = inside(&self.0, path)?;
        file.is_file().then(|| std::fs::read(file).ok()).flatten()
    }

    fn list(&self, dir: &str) -> Vec<String> {
        let Some(Ok(entries)) = inside(&self.0, dir).map(std::fs::read_dir) else {
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

/// `root/rel` with every link resolved, when that stays inside `root`.
pub fn inside(root: &Path, rel: &str) -> Option<PathBuf> {
    let top = root.canonicalize().ok()?;
    let full = root.join(rel).canonicalize().ok()?;
    full.starts_with(&top).then_some(full)
}

/// A snapshot whose reads are kept: every file's config lookup asks for the
/// same few paths.
pub struct Cached<S> {
    inner: S,
    read: RefCell<HashMap<String, Option<Vec<u8>>>>,
}

impl<S: Snapshot> Cached<S> {
    pub fn new(inner: S) -> Self {
        Cached {
            inner,
            read: RefCell::default(),
        }
    }
}

impl<S: Snapshot> Snapshot for Cached<S> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(found) = self.read.borrow().get(path) {
            return found.clone();
        }
        let found = self.inner.read(path);
        self.read
            .borrow_mut()
            .insert(path.to_string(), found.clone());
        found
    }

    fn list(&self, dir: &str) -> Vec<String> {
        self.inner.list(dir)
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

    /// Through git's object reader; in a partial clone, one `git cat-file
    /// --batch` after the blobs it lacks are fetched in one request.
    fn read_all(&self, paths: &[String]) -> Vec<Option<Vec<u8>>> {
        if !self.git.partial() {
            return paths.iter().map(|p| self.git.show(&self.rev, p)).collect();
        }
        let names: Vec<String> = paths.iter().map(|p| format!("{}:{p}", self.rev)).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut found = self.git.blobs(&names).unwrap_or_default();
        if found.iter().any(Option::is_none)
            && let Ok(ids) = self.git.object_ids(&names)
        {
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            if self.git.prefetch(&ids).is_ok() {
                found = self.git.blobs(&names).unwrap_or_default();
            }
        }
        paths
            .iter()
            .enumerate()
            .map(|(i, p)| match found.get(i) {
                Some(Some(blob)) => Some(blob.clone()),
                _ => self.read(p),
            })
            .collect()
    }
}

/// The index: what a staged diff ends in.
pub struct Index<'a>(pub &'a Git);

impl Snapshot for Index<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.show("", path)
    }

    fn list(&self, dir: &str) -> Vec<String> {
        self.0.ls_index(dir)
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

    #[test]
    fn a_cached_snapshot_reads_each_path_once() {
        struct Counted(RefCell<Vec<String>>);
        impl Snapshot for Counted {
            fn read(&self, path: &str) -> Option<Vec<u8>> {
                self.0.borrow_mut().push(path.to_string());
                (path == ".rubocop.yml").then(|| b"x".to_vec())
            }
            fn list(&self, _: &str) -> Vec<String> {
                Vec::new()
            }
        }
        let cached = Cached::new(Counted(RefCell::default()));
        for _ in 0..2 {
            assert_eq!(cached.read(".rubocop.yml").as_deref(), Some(&b"x"[..]));
            assert_eq!(cached.read("lib/.rubocop.yml"), None);
        }
        assert_eq!(
            *cached.inner.0.borrow(),
            [".rubocop.yml", "lib/.rubocop.yml"]
        );
    }
}
