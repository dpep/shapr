//! The repository beyond the change, at its after revision: where a part's
//! uses outside the change are searched (`git grep -w`) and its library
//! layout is read. A revision is searched in the object store, so a bare
//! partial clone works once the blobs are prefetched in one request; the
//! index or the working tree is searched when the change ends there.

use crate::change::Category;
use crate::classify::proof::{Outside, Search};
use crate::diff::FileDiff;
use crate::frontend::{self, Frontend};
use crate::git::{Git, Linguist, State};
use crate::norms::{Index, Revision, Snapshot, Worktree};
use crate::report::Input;
use crate::tree::{Role, Tree, spells};
use serde::Serialize;
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

/// Most files of one language searched. Beyond it, a partial clone would
/// fetch too much to answer one change; the part keeps `search_capped`.
const CAP: usize = 20_000;

/// Most files one run's searches read, each search counting every file it
/// greps: a search reads the whole repository, so its cost grows with it.
const MAX_SEARCHED_FILES: usize = 4_000_000;

/// Most bytes of files outside the change one run reads to parse for
/// proof: the files that call by a computed name, then each search's files
/// found. Parsing costs time, and kept trees memory, in proportion.
const MAX_PARSED_BYTES: usize = 128 << 20;

/// What bounds the search of the repository outside the change. A search
/// past one isn't run, and the parts it would settle keep `search_capped`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Bound {
    /// Files of the language searched, per search ([`CAP`]).
    LanguageFiles,
    /// Files read by the run's searches ([`MAX_SEARCHED_FILES`]).
    SearchedFiles,
    /// Bytes read to parse for proof ([`MAX_PARSED_BYTES`]).
    ParsedBytes,
}

/// A bound over one run: how much of `limit` was `used` (for
/// `LanguageFiles`, the most one search asked for), the most a refused
/// search would have brought it to (`needed`, else `used`), and how many
/// searches it `refused`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Budget {
    pub used: usize,
    pub needed: usize,
    pub limit: usize,
    pub refused: usize,
}

/// A bound some search of the run hit, for `summary.bounds_hit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Hit {
    pub bound: Bound,
    #[serde(flatten)]
    pub budget: Budget,
}

impl Bound {
    pub fn as_str(self) -> &'static str {
        match self {
            Bound::LanguageFiles => "language_files",
            Bound::SearchedFiles => "searched_files",
            Bound::ParsedBytes => "parsed_bytes",
        }
    }
}

/// Each bound's limit: its default, or as `SHAPR_BOUNDS` sets it
/// (`parsed_bytes=1000000,searched_files=50000`), to try a tighter one.
fn limits() -> BTreeMap<Bound, usize> {
    let mut limits = BTreeMap::from([
        (Bound::LanguageFiles, CAP),
        (Bound::SearchedFiles, MAX_SEARCHED_FILES),
        (Bound::ParsedBytes, MAX_PARSED_BYTES),
    ]);
    let setting = std::env::var("SHAPR_BOUNDS").unwrap_or_default();
    for entry in setting.split(',').filter(|e| !e.is_empty()) {
        let parsed = entry.split_once('=').and_then(|(name, value)| {
            let bound = *limits.keys().find(|b| b.as_str() == name)?;
            Some((bound, value.parse().ok()?))
        });
        match parsed {
            Some((bound, limit)) => {
                limits.insert(bound, limit);
            }
            None => crate::report::note(&format!("SHAPR_BOUNDS: `{entry}` isn't BOUND=N, ignored")),
        }
    }
    limits
}

/// Every repository's budgets as one: the most any used, the searches all refused.
pub fn budgets(repositories: &[Repository]) -> BTreeMap<Bound, Budget> {
    let mut all = BTreeMap::new();
    for (bound, budget) in repositories.iter().flat_map(Repository::budgets) {
        all.entry(bound)
            .and_modify(|b: &mut Budget| {
                b.used = b.used.max(budget.used);
                b.needed = b.needed.max(budget.needed);
                b.refused += budget.refused;
            })
            .or_insert(budget);
    }
    all
}

/// The bounds that refused a search.
pub fn hits(budgets: &BTreeMap<Bound, Budget>) -> Vec<Hit> {
    (budgets.iter())
        .filter(|(_, b)| b.refused > 0)
        .map(|(&bound, &budget)| Hit { bound, budget })
        .collect()
}

/// `bounds hit: parsed_bytes (2 searches past 134217728); their parts keep
/// search_capped`, when any was.
pub fn text(hits: &[Hit]) -> Option<String> {
    let each: Vec<String> = (hits.iter())
        .map(|h| {
            let n = h.budget.refused;
            let s = if n == 1 { "" } else { "es" };
            format!(
                "{} ({n} search{s} past {})",
                h.bound.as_str(),
                h.budget.limit
            )
        })
        .collect();
    (!each.is_empty()).then(|| {
        format!(
            "bounds hit: {}; their parts keep search_capped",
            each.join(", ")
        )
    })
}

impl Budget {
    fn new(limit: usize) -> Self {
        Budget {
            used: 0,
            needed: 0,
            limit,
            refused: 0,
        }
    }

    /// Spends N, or refuses when it would pass the limit.
    fn spend(&mut self, n: usize) -> bool {
        let total = self.used.saturating_add(n);
        self.needed = self.needed.max(total);
        if total > self.limit {
            self.refused += 1;
            return false;
        }
        self.used = total;
        true
    }
}

pub struct Repository<'a> {
    git: &'a Git,
    state: State,
    snapshot: &'a dyn Snapshot,
    budgets: RefCell<BTreeMap<Bound, Budget>>,
    files: OnceCell<Option<Vec<(String, String)>>>,
    /// Languages whose blobs are fetched (or failed to).
    prefetched: RefCell<HashMap<&'static str, bool>>,
    /// By language, word, and whether computed names count.
    searches: RefCell<HashMap<(&'static str, String, bool), Search>>,
    /// Per language, the files that name a method computed at runtime, or
    /// why there are none to read (`Unavailable`, `Capped`).
    computed: RefCell<HashMap<&'static str, Result<Computed, Search>>>,
    /// Files parsed for a proof, each once however many names it's read for.
    parsed: RefCell<HashMap<String, Option<Rc<Tree>>>>,
    /// Files a search found, read and charged to [`Bound::ParsedBytes`],
    /// until parsed.
    sources: RefCell<HashMap<String, Option<Vec<u8>>>>,
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
            budgets: RefCell::new(
                (limits().into_iter())
                    .map(|(bound, limit)| (bound, Budget::new(limit)))
                    .collect(),
            ),
            files: OnceCell::new(),
            prefetched: RefCell::default(),
            searches: RefCell::default(),
            computed: RefCell::default(),
            parsed: RefCell::default(),
            sources: RefCell::default(),
        }
    }

    /// Each bound with what the run spent of it.
    pub fn budgets(&self) -> BTreeMap<Bound, Budget> {
        self.budgets.borrow().clone()
    }

    #[cfg(test)]
    fn limit(&mut self, bound: Bound, limit: usize) {
        self.budgets.get_mut().insert(bound, Budget::new(limit));
    }

    fn spend(&self, bound: Bound, n: usize) -> bool {
        let mut budgets = self.budgets.borrow_mut();
        let budget = budgets.get_mut(&bound).expect("every bound has a budget");
        if bound == Bound::LanguageFiles {
            // Per search: the most any one asked for.
            let fits = n <= budget.limit;
            budget.used = budget.used.max(n);
            budget.needed = budget.used;
            budget.refused += usize::from(!fits);
            return fits;
        }
        budget.spend(n)
    }

    fn files(&self) -> Option<&[(String, String)]> {
        self.files
            .get_or_init(|| {
                let _span = crate::log::span("outside.files");
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

    /// Files holding `word`, and with `computed`, files whose computed
    /// names could spell it, cached per language and word.
    fn cached(&self, path: &str, word: &str, computed: bool) -> Search {
        let Some(frontend) = frontend::for_path(path) else {
            return Search::Unavailable;
        };
        let key = (frontend.language(), word.to_string(), computed);
        if let Some(found) = self.searches.borrow().get(&key) {
            return found.clone();
        }
        let found = self.search(frontend, word, computed);
        self.searches.borrow_mut().insert(key, found.clone());
        found
    }

    fn search(&self, frontend: &dyn Frontend, word: &str, with_computed: bool) -> Search {
        let _span = crate::log::span("outside.search");
        let Some(files) = self.files() else {
            return Search::Unavailable;
        };
        let globs = frontend.search_globs();
        let candidates: Vec<&(String, String)> =
            files.iter().filter(|(p, _)| matches(globs, p)).collect();
        if !self.spend(Bound::LanguageFiles, candidates.len()) {
            crate::verbose!(
                "{} {} files: more than {} to search",
                candidates.len(),
                frontend.language(),
                CAP
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
        if !self.spend(Bound::SearchedFiles, candidates.len() + texts.len()) {
            crate::verbose!("{word}: not searched, the run's searches read too many files");
            return Search::Capped;
        }
        if matches!(self.state, State::Revision(_)) {
            let mut prefetched = self.prefetched.borrow_mut();
            let fetched = *prefetched.entry(frontend.language()).or_insert_with(|| {
                let _span = crate::log::span("outside.prefetch");
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
        crate::log::count("search.candidates", candidates.len() as u64);
        let hits = self.grep(word, globs, &texts);
        if let Ok((found, _)) = &hits {
            crate::log::count("search.found", found.len() as u64);
        }
        let (mut found, mentioned) = match hits {
            Ok(hits) => hits,
            Err(error) => {
                crate::verbose!("can't search for {word}: {error:#}");
                return Search::Unavailable;
            }
        };
        if with_computed {
            let mut computed = self.computed.borrow_mut();
            let computed = computed
                .entry(frontend.language())
                .or_insert_with(|| self.computed_names(frontend, &candidates));
            match computed {
                Ok(computed) => found.extend(
                    (computed.iter())
                        .filter(|(_, patterns)| patterns.iter().any(|p| spells(p, word)))
                        .map(|(path, _)| path.clone()),
                ),
                Err(open) => return open.clone(),
            }
            found.sort();
            found.dedup();
        }
        if !self.read_ahead(&found) {
            crate::verbose!("{word}: not settled, its files found pass the run's bytes to parse");
            return Search::Capped;
        }
        Search::Done {
            searched: candidates.len(),
            found,
            mentioned,
        }
    }

    /// Reads the files in FOUND a caller may parse and that aren't read yet,
    /// charging their bytes to [`Bound::ParsedBytes`]; false, reading
    /// nothing, when they don't fit.
    fn read_ahead(&self, found: &[String]) -> bool {
        let fresh: Vec<&String> = (found.iter())
            .filter(|p| frontend::for_path(p).is_some())
            .filter(|p| !self.parsed.borrow().contains_key(*p))
            .filter(|p| !self.sources.borrow().contains_key(*p))
            .collect();
        let read: Vec<Option<Vec<u8>>> = fresh.iter().map(|p| self.read(p)).collect();
        let bytes = read.iter().flatten().map(Vec::len).sum();
        if !self.spend(Bound::ParsedBytes, bytes) {
            return false;
        }
        let mut sources = self.sources.borrow_mut();
        sources.extend(fresh.into_iter().cloned().zip(read));
        true
    }
}

impl Repository<'_> {
    /// The files of the language (by GLOBS) and of TEXTS that hold WORD as a
    /// whole word. A repository holding every blob is searched whole and the
    /// hits sorted by path; a partial clone holds only the blobs prefetched
    /// for these files, so they are searched by pathspec.
    fn grep(
        &self,
        word: &str,
        globs: &[&str],
        texts: &[&(String, String)],
    ) -> anyhow::Result<(Vec<String>, Vec<String>)> {
        if matches!(self.state, State::Revision(_)) && self.git.partial() {
            let found = self
                .git
                .grep(&self.state, &[word], &pathspecs(globs), false)?;
            return Ok((found, self.mentions(word, texts)?));
        }
        let texts: BTreeSet<&str> = texts.iter().map(|(p, _)| p.as_str()).collect();
        let (found, rest): (Vec<String>, Vec<String>) = (self.git)
            .grep(&self.state, &[word], &[":(top)".to_string()], false)?
            .into_iter()
            .partition(|p| matches(globs, p));
        let mentioned = rest.into_iter().filter(|p| texts.contains(p.as_str()));
        Ok((found, mentioned.collect()))
    }

    /// Which of TEXTS hold WORD as a whole word, searched by path in
    /// batches that keep the command line short.
    fn mentions(&self, word: &str, texts: &[&(String, String)]) -> anyhow::Result<Vec<String>> {
        let mut found = Vec::new();
        for batch in texts.chunks(500) {
            let paths: Vec<String> = batch
                .iter()
                .map(|(p, _)| format!(":(top,literal){p}"))
                .collect();
            found.extend(self.git.grep(&self.state, &[word], &paths, false)?);
        }
        Ok(found)
    }

    /// Parses once each candidate file that calls one of the frontend's
    /// dispatch words, for [`Computed`] names.
    fn computed_names(
        &self,
        frontend: &dyn Frontend,
        candidates: &[&(String, String)],
    ) -> Result<Computed, Search> {
        let _span = crate::log::span("outside.computed");
        // Only a dispatch word reaches a method by a computed name, and its
        // call spells it out. `grep -w` can't end a word on `?` or `!`; the
        // stem still matches the call.
        let words: Vec<&str> = (frontend.dispatch_words().iter())
            .map(|w| w.trim_end_matches(['?', '!']))
            .collect();
        if words.is_empty() {
            return Ok(Computed::new());
        }
        let globs = pathspecs(frontend.search_globs());
        let callers: BTreeSet<String> = match self.git.grep(&self.state, &words, &globs, true) {
            Ok(found) => found.into_iter().collect(),
            Err(error) => {
                crate::verbose!("can't find files that call by a computed name: {error:#}");
                return Err(Search::Unavailable);
            }
        };
        let candidates: Vec<&(String, String)> = (candidates.iter().copied())
            .filter(|(path, _)| callers.contains(path))
            .collect();
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
                        return Err(Search::Unavailable);
                    }
                }
            }
        };
        let bytes = sources.iter().flatten().map(Vec::len).sum();
        if !self.spend(Bound::ParsedBytes, bytes) {
            crate::verbose!("{bytes} bytes call by a computed name: past the run's bytes to parse");
            return Err(Search::Capped);
        }
        let mut computed = Computed::new();
        crate::log::count("computed.files", candidates.len() as u64);
        for ((path, _), source) in candidates.iter().zip(sources) {
            let read = source.is_some();
            let source = source.unwrap_or_default();
            crate::log::count("computed.bytes", source.len() as u64);
            let tree = Rc::new(frontend.parse(source));
            let suffixes = tree.semantics.spelling.suffixes;
            let patterns: Vec<String> = (tree.descendants(Tree::ROOT))
                .map(|id| tree.node(id))
                .filter(|n| n.role == Role::DynamicName)
                .filter_map(|n| n.name.as_deref().filter(|name| name.contains('*')))
                .map(|name| name.trim_end_matches(|c| suffixes.contains(&c)).to_string())
                .collect();
            // A bare `*` spells every name, so every search finds and parses
            // the file: keep the tree, as `parse` would have built it.
            let same =
                frontend::for_path(path).is_some_and(|f| f.language() == frontend.language());
            if read && same && patterns.iter().any(|p| p == "*") {
                self.parsed.borrow_mut().insert(path.clone(), Some(tree));
            }
            if !patterns.is_empty() {
                computed.push((path.clone(), patterns));
            }
        }
        Ok(computed)
    }
}

impl Outside for Repository<'_> {
    fn uses(&self, path: &str, word: &str) -> Search {
        self.cached(path, word, true)
    }

    fn literal(&self, path: &str, word: &str) -> Search {
        self.cached(path, word, false)
    }

    fn library(&self, path: &str) -> bool {
        frontend::for_path(path).is_some_and(|f| f.library(path, self.snapshot))
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.snapshot.read(path)
    }

    fn called_by_name(&self, path: &str) -> bool {
        let (Some(frontend), Some(files)) = (frontend::for_path(path), self.files()) else {
            return false;
        };
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        frontend.called_by_name(path, &paths)
    }

    fn parse(&self, path: &str) -> Option<Rc<Tree>> {
        if let Some(tree) = self.parsed.borrow().get(path) {
            return tree.clone();
        }
        let _span = crate::log::span("outside.parse");
        let read = match self.sources.borrow_mut().remove(path) {
            Some(read) => read,
            None => self.read(path),
        };
        let tree = frontend::for_path(path)
            .zip(read)
            .map(|(frontend, source)| {
                crate::log::count("parsed.bytes", source.len() as u64);
                Rc::new(frontend.parse(source))
            });
        self.parsed
            .borrow_mut()
            .insert(path.to_string(), tree.clone());
        tree
    }
}

/// The repository as of a change's after side: each state the change may
/// end in, and the snapshot norms read config and neighbors from.
pub struct AfterSide<'g> {
    git: &'g Git,
    states: Vec<State>,
    revision: Option<Revision<'g>>,
    index: Index<'g>,
    worktree: Option<Worktree>,
}

impl<'g> AfterSide<'g> {
    /// Where a change named in FORM (diffed over RANGE, parsed into FILES)
    /// ends: its after revision, else the index for a staged diff and the
    /// working tree for a local one. A diff on stdin names its own after
    /// side only by blob ids.
    pub fn new(git: &'g Git, form: Input, range: Option<&str>, files: &[FileDiff]) -> Self {
        let stdin = form == Input::Stdin;
        let states = match (stdin, git.after_revision(range)) {
            (_, Some(rev)) => vec![State::Revision(rev)],
            (false, None) if form == Input::Staged => vec![State::Index],
            (false, None) => vec![State::Worktree],
            (true, None) => git.stdin_after(files),
        };
        if stdin && states.is_empty() {
            crate::verbose!(
                "the diff doesn't end in the index or the working tree: uses outside it aren't searched"
            );
        }
        let revision = match states.first() {
            Some(State::Revision(rev)) => Some(Revision::new(git, rev.clone())),
            _ => None,
        };
        AfterSide {
            git,
            states,
            revision,
            index: Index(git),
            worktree: git.toplevel().map(Worktree),
        }
    }

    pub fn states(&self) -> &[State] {
        &self.states
    }

    /// The after revision, when the change ends in one.
    pub fn revision(&self) -> Option<&str> {
        match self.states.first() {
            Some(State::Revision(rev)) => Some(rev),
            _ => None,
        }
    }

    /// The after revision's files, else the index's when the change ends
    /// there, else the working tree's.
    pub fn snapshot(&self) -> Option<&dyn Snapshot> {
        match (self.states.first(), &self.revision, &self.worktree) {
            (_, Some(revision), _) => Some(revision),
            (Some(State::Index), ..) => Some(&self.index),
            (_, None, Some(worktree)) => Some(worktree),
            (_, None, None) => None,
        }
    }

    /// One searchable repository per state, each over its own files.
    pub fn repositories(&self) -> Vec<Repository<'_>> {
        self.states
            .iter()
            .filter_map(|state| {
                let snapshot: &dyn Snapshot = match state {
                    State::Revision(_) => self.revision.as_ref()?,
                    State::Index => &self.index,
                    State::Worktree => self.worktree.as_ref()?,
                };
                Some(Repository::new(self.git, state.clone(), snapshot))
            })
            .collect()
    }
}

/// Several states a change may end in, searched as one: a diff on stdin
/// whose blob ids fit both the index and the working tree. A use in any of
/// them is a use.
pub struct Union<'a, 'r>(pub &'r [Repository<'a>]);

impl Outside for Union<'_, '_> {
    fn uses(&self, path: &str, word: &str) -> Search {
        union(self.0.iter().map(|r| r.uses(path, word)))
    }

    fn literal(&self, path: &str, word: &str) -> Search {
        union(self.0.iter().map(|r| r.literal(path, word)))
    }

    fn library(&self, path: &str) -> bool {
        self.0.iter().any(|r| r.library(path))
    }

    fn called_by_name(&self, path: &str) -> bool {
        self.0.iter().any(|r| r.called_by_name(path))
    }

    /// The file's contents where the states agree; `None` where they differ,
    /// which a caller reads as a use.
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let mut found = self.0.iter().filter_map(|r| r.read(path));
        let first = found.next()?;
        found.all(|other| other == first).then_some(first)
    }
}

/// Several states' searches as one: a use in any of them is a use.
fn union(mut all: impl Iterator<Item = Search>) -> Search {
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

    /// Runs git in DIR, isolated from any repository around the test run.
    fn git_in(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }

    #[test]
    fn the_after_side_follows_the_input_form() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| git_in(dir.path(), args);
        let write = |text: &str| std::fs::write(dir.path().join("a.rb"), text).unwrap();
        git(&["init", "-q"]);
        for text in ["v0\n", "v1\n"] {
            write(text);
            git(&["add", "-A"]);
            git(&["commit", "-qm", text]);
        }
        write("v2\n");
        git(&["add", "-A"]);
        write("v3\n");
        let shell = Git::new(Some(dir.path().to_path_buf()));
        let revision = |rev: &str| vec![State::Revision(rev.to_string())];
        // A diff on stdin whose blob ids fit neither the index nor the working tree.
        let foreign = crate::diff::parse(
            "diff --git a/a.rb b/a.rb\nindex 1111111..2222222 100644\n--- a/a.rb\n+++ b/a.rb\n@@ -1 +1 @@\n-x\n+y\n",
        )
        .unwrap();
        let none: &[FileDiff] = &[];
        let cases = [
            (
                Input::Commit,
                Some("HEAD^!"),
                none,
                revision("HEAD"),
                Some("v1\n"),
                1,
            ),
            (
                Input::Range,
                Some("HEAD^..HEAD"),
                none,
                revision("HEAD"),
                Some("v1\n"),
                1,
            ),
            (
                Input::Staged,
                None,
                none,
                vec![State::Index],
                Some("v2\n"),
                1,
            ),
            (
                Input::Worktree,
                Some("HEAD"),
                none,
                vec![State::Worktree],
                Some("v3\n"),
                1,
            ),
            // Nothing to search, but norms still read the working tree.
            (Input::Stdin, None, &foreign, vec![], Some("v3\n"), 0),
        ];
        for (form, range, files, states, read, repositories) in cases {
            let after = AfterSide::new(&shell, form, range, files);
            assert_eq!(after.states(), states, "{form:?}");
            let text = after.snapshot().and_then(|s| s.read("a.rb"));
            assert_eq!(text.as_deref(), read.map(str::as_bytes), "{form:?}");
            assert_eq!(after.repositories().len(), repositories, "{form:?}");
        }
    }

    /// The working tree's files, each read counted.
    struct Disk(std::path::PathBuf, RefCell<Vec<String>>);
    impl Snapshot for Disk {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.1.borrow_mut().push(path.to_string());
            std::fs::read(self.0.join(path)).ok()
        }
        fn list(&self, _: &str) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn a_file_calling_by_any_name_is_parsed_once() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| git_in(dir.path(), args);
        git(&["init", "-q"]);
        std::fs::write(dir.path().join("a.rb"), "def go\nend\n").unwrap();
        std::fs::write(dir.path().join("b.rb"), "x.send(name)\n").unwrap();
        git(&["add", "-A"]);
        let shell = Git::new(Some(dir.path().to_path_buf()));
        let disk = Disk(dir.path().to_path_buf(), RefCell::default());
        let repo = Repository::new(&shell, State::Worktree, &disk);
        let Search::Done { found, .. } = repo.uses("a.rb", "go") else {
            panic!("not searched");
        };
        assert_eq!(found, ["a.rb", "b.rb"]);
        assert!(repo.parse("b.rb").is_some());
        let reads = disk.1.borrow();
        assert_eq!(
            reads.iter().filter(|p| *p == "b.rb").count(),
            1,
            "{reads:?}"
        );
    }

    /// A repository of `a.rb` defining `go` and `b.rb` calling it (and,
    /// with `computed`, calling anything by a computed name).
    fn caller(computed: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| git_in(dir.path(), args);
        git(&["init", "-q"]);
        std::fs::write(dir.path().join("a.rb"), "def go\nend\n").unwrap();
        let b = if computed {
            "go\nx.send(name)\n"
        } else {
            "go\n"
        };
        std::fs::write(dir.path().join("b.rb"), b).unwrap();
        git(&["add", "-A"]);
        dir
    }

    #[test]
    fn searches_past_the_runs_files_are_not_run() {
        let dir = caller(false);
        let shell = Git::new(Some(dir.path().to_path_buf()));
        let mut repo = Repository::new(&shell, State::Worktree, &Empty);
        // Two files a search: the first fits, the second doesn't.
        repo.limit(Bound::SearchedFiles, 3);
        assert!(matches!(repo.uses("a.rb", "go"), Search::Done { .. }));
        assert_eq!(repo.uses("a.rb", "other"), Search::Capped);
        // A search already run is answered from the cache, at no cost.
        assert!(matches!(repo.uses("a.rb", "go"), Search::Done { .. }));
        let budget = repo.budgets()[&Bound::SearchedFiles];
        assert_eq!((budget.used, budget.refused), (2, 1));
    }

    #[test]
    fn files_found_past_the_runs_bytes_are_not_parsed() {
        let dir = caller(false);
        let shell = Git::new(Some(dir.path().to_path_buf()));
        let disk = Disk(dir.path().to_path_buf(), RefCell::default());
        let mut repo = Repository::new(&shell, State::Worktree, &disk);
        // `def go\nend\n` and `go\n`: 14 bytes found.
        repo.limit(Bound::ParsedBytes, 13);
        assert_eq!(repo.uses("a.rb", "go"), Search::Capped);
        let budget = repo.budgets()[&Bound::ParsedBytes];
        assert_eq!((budget.used, budget.needed, budget.refused), (0, 14, 1));
        let mut repo = Repository::new(&shell, State::Worktree, &disk);
        repo.limit(Bound::ParsedBytes, 14);
        assert!(matches!(repo.uses("a.rb", "go"), Search::Done { .. }));
        // What the search read is what `parse` parses, read once.
        disk.1.borrow_mut().clear();
        assert!(repo.parse("b.rb").is_some());
        assert!(disk.1.borrow().is_empty());
        assert_eq!(repo.budgets()[&Bound::ParsedBytes].used, 14);
    }

    #[test]
    fn computed_names_past_the_runs_bytes_leave_the_search_capped() {
        let dir = caller(true);
        let shell = Git::new(Some(dir.path().to_path_buf()));
        let disk = Disk(dir.path().to_path_buf(), RefCell::default());
        let mut repo = Repository::new(&shell, State::Worktree, &disk);
        repo.limit(Bound::ParsedBytes, 5);
        assert_eq!(repo.uses("a.rb", "go"), Search::Capped);
        // Without computed names nothing more is parsed than the files found.
        assert_eq!(
            repo.literal("a.rb", "nothing"),
            Search::Done {
                searched: 2,
                found: Vec::new(),
                mentioned: Vec::new(),
            }
        );
    }

    #[test]
    fn a_repository_beyond_the_cap_is_not_searched() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| git_in(dir.path(), args);
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
        repo.limit(Bound::LanguageFiles, 1);
        assert_eq!(repo.uses("a.rb", "go"), Search::Capped);
        assert_eq!(
            repo.budgets()[&Bound::LanguageFiles],
            Budget {
                used: 2,
                needed: 2,
                limit: 1,
                refused: 1
            }
        );
    }
}
