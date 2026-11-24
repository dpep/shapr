//! The repository beyond the change, at its after revision: where a part's
//! uses outside the change are searched (`git grep -w`) and its library
//! layout is read. A revision is searched in the object store, so a bare
//! partial clone works once the blobs are prefetched in one request; the
//! index or the working tree is searched when the change ends there.

use crate::change::Category;
use crate::classify::proof::{Outside, Search};
use crate::frontend::{self, Frontend};
use crate::git::{Git, Linguist, State};
use crate::norms::Snapshot;
use crate::tree::{Role, Tree, spells};
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

/// Most files of one language searched. Beyond it, a partial clone would
/// fetch too much to answer one change; the part keeps `search_capped`.
const CAP: usize = 20_000;

pub struct Repository<'a> {
    git: &'a Git,
    state: State,
    snapshot: &'a dyn Snapshot,
    cap: usize,
    files: OnceCell<Option<Vec<(String, String)>>>,
    /// Languages whose blobs are fetched (or failed to).
    prefetched: RefCell<HashMap<&'static str, bool>>,
    searches: RefCell<HashMap<(&'static str, String), Search>>,
    /// Per language, the files that name a method computed at runtime.
    computed: RefCell<HashMap<&'static str, Option<Computed>>>,
    /// Files parsed for a proof, each once however many names it's read for.
    parsed: RefCell<HashMap<String, Option<Rc<Tree>>>>,
}

/// Files holding a name computed at runtime ([`Role::DynamicName`] with a
/// `*`): such a file may use any name its pattern spells, with no literal
/// word for `git grep` to find. Each with its patterns, suffixes trimmed.
type Computed = Vec<(String, Vec<String>)>;

impl<'a> Repository<'a> {
    pub fn new(git: &'a Git, state: State, snapshot: &'a dyn Snapshot) -> Self {
        Repository {
            git,
            state,
            snapshot,
            cap: CAP,
            files: OnceCell::new(),
            prefetched: RefCell::default(),
            searches: RefCell::default(),
            computed: RefCell::default(),
            parsed: RefCell::default(),
        }
    }

    fn files(&self) -> Option<&[(String, String)]> {
        self.files
            .get_or_init(|| {
                let files = self.git.files(match &self.state {
                    State::Revision(rev) => Some(rev),
                    State::Index | State::Worktree => None,
                });
                // The working tree's untracked files have no blob id.
                let untracked = match self.state {
                    State::Worktree => self.git.untracked(),
                    State::Revision(_) | State::Index => Ok(Vec::new()),
                };
                match files.and_then(|f| Ok((f, untracked?))) {
                    Ok((mut files, untracked)) => {
                        files.extend(untracked.into_iter().map(|p| (p, String::new())));
                        Some(files)
                    }
                    Err(error) => {
                        crate::verbose!("can't list the repository's files: {error:#}");
                        None
                    }
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
        // Text of no language shapr reads that code may still use a name
        // from: config, templates, another language. Prose, tool output and
        // third-party code don't count.
        let texts: Vec<&(String, String)> = files
            .iter()
            .filter(|(p, _)| frontend::for_path(p).is_none_or(|f| !matches(f.search_globs(), p)))
            .filter(|(p, _)| {
                !matches!(
                    Category::of(p, false, Linguist::default()),
                    Category::Docs | Category::Generated | Category::Vendored | Category::Binary
                )
            })
            .collect();
        if matches!(self.state, State::Revision(_)) {
            let mut prefetched = self.prefetched.borrow_mut();
            let fetched = *prefetched.entry(frontend.language()).or_insert_with(|| {
                let ids: BTreeSet<&str> = (candidates.iter().chain(&texts))
                    .map(|(_, id)| id.as_str())
                    .collect();
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
        let found = self.git.grep(&self.state, word, &pathspecs(globs));
        let mentioned = self.mentions(word, &texts);
        let mut computed = self.computed.borrow_mut();
        let computed = computed
            .entry(frontend.language())
            .or_insert_with(|| self.computed_names(frontend, &candidates));
        match (found.and_then(|f| Ok((f, mentioned?))), &*computed) {
            (Ok((mut found, mentioned)), Some(computed)) => {
                found.extend(
                    (computed.iter())
                        .filter(|(_, patterns)| patterns.iter().any(|p| spells(p, word)))
                        .map(|(path, _)| path.clone()),
                );
                found.sort();
                found.dedup();
                Search::Done {
                    searched: candidates.len(),
                    found,
                    mentioned,
                }
            }
            (Ok(_), None) => Search::Unavailable,
            (Err(error), _) => {
                crate::verbose!("can't search for {word}: {error:#}");
                Search::Unavailable
            }
        }
    }
}

impl Repository<'_> {
    /// Which of TEXTS hold WORD as a whole word, searched by path in
    /// batches that keep the command line short.
    fn mentions(&self, word: &str, texts: &[&(String, String)]) -> anyhow::Result<Vec<String>> {
        let mut found = Vec::new();
        for batch in texts.chunks(500) {
            let paths: Vec<String> = batch
                .iter()
                .map(|(p, _)| format!(":(top,literal){p}"))
                .collect();
            found.extend(self.git.grep(&self.state, word, &paths)?);
        }
        Ok(found)
    }

    /// Parses every candidate file once for [`Computed`] names.
    fn computed_names(
        &self,
        frontend: &dyn Frontend,
        candidates: &[&(String, String)],
    ) -> Option<Computed> {
        // Only a dispatch word reaches a method by a computed name.
        if frontend.dispatch_words().is_empty() {
            return Some(Computed::new());
        }
        let sources: Vec<Option<Vec<u8>>> = match self.state {
            State::Worktree => candidates
                .iter()
                .map(|(path, _)| self.snapshot.read(path))
                .collect(),
            State::Revision(_) | State::Index => {
                let ids: Vec<&str> = candidates.iter().map(|(_, id)| id.as_str()).collect();
                match self.git.blobs(&ids) {
                    Ok(sources) => sources,
                    Err(error) => {
                        crate::verbose!("can't read files for computed names: {error:#}");
                        return None;
                    }
                }
            }
        };
        let mut computed = Computed::new();
        for ((path, _), source) in candidates.iter().zip(sources) {
            let tree = frontend.parse(source.unwrap_or_default());
            let suffixes = tree.semantics.spelling.suffixes;
            let patterns: Vec<String> = (tree.descendants(Tree::ROOT))
                .map(|id| tree.node(id))
                .filter(|n| n.role == Role::DynamicName)
                .filter_map(|n| n.name.as_deref().filter(|name| name.contains('*')))
                .map(|name| name.trim_end_matches(|c| suffixes.contains(&c)).to_string())
                .collect();
            if !patterns.is_empty() {
                computed.push((path.clone(), patterns));
            }
        }
        Some(computed)
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

    fn parse(&self, path: &str) -> Option<Rc<Tree>> {
        if let Some(tree) = self.parsed.borrow().get(path) {
            return tree.clone();
        }
        let tree = frontend::for_path(path)
            .zip(self.read(path))
            .map(|(frontend, source)| Rc::new(frontend.parse(source)));
        self.parsed
            .borrow_mut()
            .insert(path.to_string(), tree.clone());
        tree
    }
}

/// Several states a change may end in, searched as one: a diff on stdin
/// whose blob ids fit both the index and the working tree. A use in any of
/// them is a use.
pub struct Union<'a, 'r>(pub &'r [Repository<'a>]);

impl Outside for Union<'_, '_> {
    fn uses(&self, path: &str, word: &str) -> Search {
        let mut all = self.0.iter().map(|r| r.uses(path, word));
        let first = all.next().unwrap_or(Search::Unavailable);
        all.fold(first, |acc, next| match (acc, next) {
            (
                Search::Done {
                    searched,
                    mut found,
                    mut mentioned,
                },
                Search::Done {
                    searched: more,
                    found: also,
                    mentioned: too,
                },
            ) => {
                found.extend(also);
                found.sort();
                found.dedup();
                mentioned.extend(too);
                mentioned.sort();
                mentioned.dedup();
                Search::Done {
                    searched: searched.max(more),
                    found,
                    mentioned,
                }
            }
            (Search::Done { .. }, open) | (open, _) => open,
        })
    }

    fn library(&self, path: &str) -> bool {
        self.0.iter().any(|r| r.library(path))
    }

    /// The file's contents where the states agree; `None` where they differ,
    /// which a caller reads as a use.
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let mut found = self.0.iter().filter_map(|r| r.read(path));
        let first = found.next()?;
        found.all(|other| other == first).then_some(first)
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

    /// Counts reads.
    struct Counted(std::cell::Cell<usize>);
    impl Snapshot for Counted {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            self.0.set(self.0.get() + 1);
            Some(b"def go\nend\n".to_vec())
        }
        fn list(&self, _: &str) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn a_file_is_read_and_parsed_once() {
        let snapshot = Counted(Default::default());
        let shell = Git::new(None);
        let repo = Repository::new(&shell, State::Worktree, &snapshot);
        let first = repo.parse("a.rb").unwrap();
        assert!(Rc::ptr_eq(&first, &repo.parse("a.rb").unwrap()));
        assert_eq!(snapshot.0.get(), 1);
        assert!(repo.parse("notes.txt").is_none());
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
        let mut repo = Repository::new(&shell, State::Worktree, &Empty);
        assert_eq!(
            repo.uses("a.rb", "go"),
            Search::Done {
                searched: 2,
                found: vec!["a.rb".into(), "b.rb".into()],
                mentioned: Vec::new(),
            }
        );
        repo = Repository::new(&shell, State::Worktree, &Empty);
        repo.cap = 1;
        assert_eq!(repo.uses("a.rb", "go"), Search::Capped);
    }
}
