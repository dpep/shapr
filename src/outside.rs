//! The repository beyond the change, at its after revision: where a part's
//! uses outside the change are searched (`git grep -w`) and its library
//! layout is read. A revision is searched in the object store, so a bare
//! partial clone works once the blobs are prefetched in one request; the
//! working tree is searched when the change ends there.

use crate::classify::proof::{Outside, Search};
use crate::frontend::{self, Frontend};
use crate::git::Git;
use crate::norms::Snapshot;
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeSet, HashMap};

/// Most files of one language searched. Beyond it, a partial clone would
/// fetch too much to answer one change; the part keeps `search_capped`.
const CAP: usize = 20_000;

pub struct Repository<'a> {
    git: &'a Git,
    /// `None`: the working tree.
    rev: Option<String>,
    snapshot: &'a dyn Snapshot,
    cap: usize,
    files: OnceCell<Option<Vec<(String, String)>>>,
    /// Languages whose blobs are fetched (or failed to).
    prefetched: RefCell<HashMap<&'static str, bool>>,
    searches: RefCell<HashMap<(&'static str, String), Search>>,
}

impl<'a> Repository<'a> {
    pub fn new(git: &'a Git, rev: Option<String>, snapshot: &'a dyn Snapshot) -> Self {
        Repository {
            git,
            rev,
            snapshot,
            cap: CAP,
            files: OnceCell::new(),
            prefetched: RefCell::default(),
            searches: RefCell::default(),
        }
    }

    fn files(&self) -> Option<&[(String, String)]> {
        self.files
            .get_or_init(|| match self.git.files(self.rev.as_deref()) {
                Ok(files) => Some(files),
                Err(error) => {
                    crate::verbose!("can't list the repository's files: {error:#}");
                    None
                }
            })
            .as_deref()
    }

    fn search(&self, frontend: &dyn Frontend, word: &str) -> Search {
        let Some(files) = self.files() else {
            return Search::Unavailable;
        };
        let globs = frontend.search_globs();
        let candidates: Vec<&(String, String)> =
            files.iter().filter(|(p, _)| matches(globs, p)).collect();
        if candidates.len() > self.cap {
            crate::verbose!(
                "{} {} files: more than {} to search",
                candidates.len(),
                frontend.language(),
                self.cap
            );
            return Search::Capped;
        }
        if self.rev.is_some() {
            let mut prefetched = self.prefetched.borrow_mut();
            let fetched = *prefetched.entry(frontend.language()).or_insert_with(|| {
                let ids: BTreeSet<&str> = candidates.iter().map(|(_, id)| id.as_str()).collect();
                let ids: Vec<&str> = ids.into_iter().collect();
                self.git
                    .prefetch(&ids)
                    .inspect_err(|error| crate::verbose!("can't fetch blobs to search: {error:#}"))
                    .is_ok()
            });
            if !fetched {
                return Search::Unavailable;
            }
        }
        match self.git.grep(self.rev.as_deref(), word, &pathspecs(globs)) {
            Ok(found) => Search::Done {
                searched: candidates.len(),
                found,
            },
            Err(error) => {
                crate::verbose!("can't search for {word}: {error:#}");
                Search::Unavailable
            }
        }
    }
}

impl Outside for Repository<'_> {
    fn uses(&self, path: &str, word: &str) -> Search {
        let Some(frontend) = frontend::for_path(path) else {
            return Search::Unavailable;
        };
        let key = (frontend.language(), word.to_string());
        if let Some(found) = self.searches.borrow().get(&key) {
            return found.clone();
        }
        let found = self.search(frontend, word);
        self.searches.borrow_mut().insert(key, found.clone());
        found
    }

    fn library(&self, path: &str) -> bool {
        frontend::for_path(path).is_some_and(|f| f.library(path, self.snapshot))
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.snapshot.read(path)
    }
}

/// Whether `path` matches one of `globs`: `*.ext` in any directory, or a
/// file name.
fn matches(globs: &[&str], path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    globs.iter().any(|g| match g.strip_prefix('*') {
        Some(ext) => name.ends_with(ext),
        None => name == *g,
    })
}

/// `globs` as git pathspecs from the top: the same files [`matches()`] picks
/// (a pathspec's `*` crosses directories).
fn pathspecs(globs: &[&str]) -> Vec<String> {
    globs
        .iter()
        .flat_map(|g| match g.starts_with('*') {
            true => vec![format!(":(top){g}")],
            false => vec![format!(":(top){g}"), format!(":(top)*/{g}")],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_pick_extensions_anywhere_and_names_exactly() {
        let globs = ["*.rb", "Gemfile"];
        assert!(matches(&globs, "a/b/c.rb"));
        assert!(matches(&globs, "Gemfile") && matches(&globs, "sub/Gemfile"));
        assert!(!matches(&globs, "a.rbx") && !matches(&globs, "MyGemfile"));
        assert_eq!(
            pathspecs(&globs),
            [":(top)*.rb", ":(top)Gemfile", ":(top)*/Gemfile"]
        );
    }

    struct Empty;
    impl Snapshot for Empty {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            None
        }
        fn list(&self, _: &str) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn a_repository_beyond_the_cap_is_not_searched() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(dir.path())
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
        };
        git(&["init", "-q"]);
        for name in ["a.rb", "b.rb"] {
            std::fs::write(dir.path().join(name), "go\n").unwrap();
        }
        git(&["add", "-A"]);
        let shell = Git::new(Some(dir.path().to_path_buf()));
        let mut repo = Repository::new(&shell, None, &Empty);
        assert_eq!(
            repo.uses("a.rb", "go"),
            Search::Done {
                searched: 2,
                found: vec!["a.rb".into(), "b.rb".into()]
            }
        );
        repo = Repository::new(&shell, None, &Empty);
        repo.cap = 1;
        assert_eq!(repo.uses("a.rb", "go"), Search::Capped);
    }
}
