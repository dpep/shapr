//! Style norms (experimental): what the code around a change already looks
//! like, inferred by counting each style per scope, and whether new code
//! follows it. A deviation is signal, not a verdict. Learns from RuboCop's
//! style cops; never runs RuboCop. See docs/schema.md, "Style".

mod align;
pub mod command;
mod edit;
mod ruby;
pub mod snapshot;

pub use align::{HunkStyle, styles, text};
pub use snapshot::{Revision, Snapshot, Worktree, join};

use crate::change::Category;
use serde::Serialize;
use std::collections::BTreeMap;
use std::ops::Range;

/// The share (rule of succession) a style needs to be a norm.
pub const DEFAULT_THRESHOLD: f64 = 0.8;

/// Observations per style, every style of the norm's vocabulary present.
pub type Counts = BTreeMap<String, usize>;

/// One place where code chose a style for a norm.
pub struct Observation {
    pub norm: &'static str,
    pub style: String,
    /// First and last line the choice spans (1-based).
    pub lines: (usize, usize),
    /// Where it shows, 1-based.
    pub line: usize,
    pub column: usize,
    /// The content with the style taken out: equal keys in other styles are
    /// the same code respelled.
    pub key: String,
    /// The rewrite to each other style.
    pub fix: Vec<(String, Vec<Replace>)>,
}

/// Replace `span` of the source with `text`.
#[derive(Clone, Debug, PartialEq)]
pub struct Replace {
    pub span: Range<usize>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Method,
    Class,
    File,
    Directory,
}

/// A method or class (module, singleton class) by its lines.
pub struct Scope {
    pub kind: ScopeKind,
    pub first: usize,
    pub last: usize,
}

pub struct Analysis {
    pub observations: Vec<Observation>,
    pub scopes: Vec<Scope>,
    pub errors: bool,
    pub lines: usize,
}

/// A norm a language checks: its id (RuboCop's cop name) and its styles.
pub struct NormDef {
    pub id: &'static str,
    pub styles: &'static [&'static str],
}

/// The per-language seam: which files, which norms, how to observe them,
/// and what the project's linter config sets.
pub trait Language: Sync {
    /// The name reported in output, e.g. `ruby`.
    fn name(&self) -> &'static str;
    fn handles(&self, path: &str) -> bool;
    fn norms(&self) -> &'static [NormDef];
    fn analyze(&self, source: &[u8]) -> Analysis;
    /// Styles the linter config for the file at `path` sets, by norm id.
    fn configured(&self, path: &str, snapshot: &dyn Snapshot) -> BTreeMap<&'static str, String>;
}

static LANGUAGES: &[&dyn Language] = &[&ruby::Ruby];

pub fn for_path(path: &str) -> Option<&'static dyn Language> {
    LANGUAGES.iter().copied().find(|l| l.handles(path))
}

/// A style that holds in a scope, and how strongly its counts back it.
#[derive(Debug, PartialEq)]
pub struct Norm {
    pub style: String,
    pub confidence: f64,
}

/// The norm `counts` back: the top style's share by the rule of succession,
/// `(top + 1) / (total + styles)`, when it reaches `threshold`. Smoothing
/// keeps a handful of observations from passing for a norm.
pub fn infer(counts: &Counts, threshold: f64) -> Option<Norm> {
    let top = counts.values().copied().max().filter(|&n| n > 0)?;
    let mut leaders = counts.iter().filter(|(_, n)| **n == top);
    let (style, _) = leaders.next()?;
    if leaders.next().is_some() {
        return None;
    }
    let total: usize = counts.values().sum();
    let styles = counts.len().max(2);
    let confidence = round2((top + 1) as f64 / (total + styles) as f64);
    (confidence >= threshold).then(|| Norm {
        style: style.clone(),
        confidence,
    })
}

pub fn count<'a>(def: &NormDef, observations: impl Iterator<Item = &'a Observation>) -> Counts {
    let mut counts: Counts = def.styles.iter().map(|s| (s.to_string(), 0)).collect();
    for o in observations {
        *counts.entry(o.style.clone()).or_default() += 1;
    }
    counts
}

/// Why a file's style isn't checked: tools write generated files, vendored
/// code follows its own project, documentation shows code rather than being it.
/// Beyond the file category: `db/schema.rb` and vendor directories, which
/// carry no linguist attribute in most repos.
pub fn skipped(path: &str, category: Category) -> Option<&'static str> {
    if matches!(
        category,
        Category::Generated | Category::Vendored | Category::Docs
    ) {
        return Some(category.as_str());
    }
    let mut dirs: Vec<&str> = path.split('/').collect();
    let name = dirs.pop().unwrap_or_default();
    if name == "schema.rb" && dirs.last() == Some(&"db") {
        return Some("generated");
    }
    if dirs
        .iter()
        .any(|d| matches!(*d, "vendor" | "third_party" | "node_modules"))
    {
        return Some("vendored");
    }
    None
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Line starts of a source, for 1-based line and column lookups.
pub struct LineIndex {
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    pub fn new(source: &[u8]) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .iter()
                .enumerate()
                .filter(|(_, b)| **b == b'\n')
                .map(|(i, _)| i + 1)
                .filter(|&i| i < source.len()),
        );
        LineIndex {
            starts,
            len: source.len(),
        }
    }

    pub fn count(&self) -> usize {
        if self.len == 0 { 0 } else { self.starts.len() }
    }

    pub fn line(&self, offset: usize) -> usize {
        self.starts.partition_point(|&s| s <= offset)
    }

    pub fn column(&self, offset: usize) -> usize {
        offset - self.starts[self.line(offset) - 1] + 1
    }

    pub fn start(&self, line: usize) -> usize {
        self.starts.get(line - 1).copied().unwrap_or(self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Linguist;

    fn counts(pairs: &[(&str, usize)]) -> Counts {
        pairs.iter().map(|(s, n)| (s.to_string(), *n)).collect()
    }

    #[test]
    fn a_norm_needs_its_smoothed_share_to_reach_the_threshold() {
        let norm = |pairs| infer(&counts(pairs), DEFAULT_THRESHOLD);
        // 3 of 3: (3 + 1) / (3 + 2) = 0.8.
        assert_eq!(
            norm(&[("a", 3), ("b", 0)]),
            Some(Norm {
                style: "a".into(),
                confidence: 0.8
            })
        );
        assert_eq!(norm(&[("a", 2), ("b", 0)]), None);
        assert_eq!(norm(&[("a", 9), ("b", 1)]).unwrap().confidence, 0.83);
        assert_eq!(norm(&[("a", 4), ("b", 1)]), None);
        assert_eq!(norm(&[("a", 0), ("b", 0)]), None);
        assert_eq!(norm(&[("a", 5), ("b", 5)]), None);
    }

    #[test]
    fn generated_vendored_and_docs_paths_are_skipped() {
        let of = |path| skipped(path, Category::of(path, false, Linguist::default()));
        assert_eq!(of("lib/a.rb"), None);
        assert_eq!(skipped("lib/a.rb", Category::Generated), Some("generated"));
        assert_eq!(of("db/schema.rb"), Some("generated"));
        assert_eq!(of("vendor/gems/x/a.rb"), Some("vendored"));
        assert_eq!(of("docs/example.rb"), Some("docs"));
        assert_eq!(of("lib/docs.rb"), None);
    }

    #[test]
    fn line_index_is_one_based() {
        let index = LineIndex::new(b"ab\ncd\n");
        assert_eq!(index.count(), 2);
        assert_eq!((index.line(0), index.column(0)), (1, 1));
        assert_eq!((index.line(4), index.column(4)), (2, 2));
        assert_eq!(index.start(2), 3);
    }
}
