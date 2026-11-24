//! Style norms (experimental): what the code around a change already looks
//! like, inferred by counting each style per scope, and whether new code
//! follows it. A deviation is signal, not a verdict. Learns from RuboCop's
//! style cops; never runs RuboCop. See docs/schema.md, "Style".

mod align;
pub mod command;
mod edit;
mod editorconfig;
mod ruby;
pub mod snapshot;

pub use align::{HunkStyle, styles, text};
pub use snapshot::{Index, Revision, Snapshot, Worktree, join};

use crate::change::Category;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
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
    /// Lines a linter directive turns a check off for: the check's name (a
    /// norm id, a department such as `Style`, or `all`), first and last line.
    pub disabled: Vec<(String, usize, usize)>,
    pub errors: bool,
    pub lines: usize,
}

/// A norm a language checks: its id (RuboCop's cop name) and its styles.
pub struct NormDef {
    pub id: &'static str,
    pub styles: &'static [&'static str],
    /// Its suggested edits rename identifiers, so they change the program
    /// by design: they are held to parsing cleanly, not to the same tree.
    pub renames: bool,
    /// Its renames reach a name other files can call (a private Ruby method
    /// is callable from a subclass): an edit is offered only when the
    /// repository holds neither the old name nor the new one outside the file.
    pub shared: bool,
}

/// The per-language seam: which files, which norms, how to observe them,
/// and what the project's linter config sets.
pub trait Language: Sync {
    /// The name reported in output, e.g. `ruby`.
    fn name(&self) -> &'static str;
    fn handles(&self, path: &str) -> bool;
    fn norms(&self) -> &'static [NormDef];
    fn analyze(&self, source: &[u8]) -> Analysis;
    /// What the linter config for the file at `path` says.
    fn configured(&self, path: &str, snapshot: &dyn Snapshot) -> Configured;
}

/// The linter config's word on one file.
#[derive(Default)]
pub struct Configured {
    /// Styles it sets, by norm id.
    pub styles: BTreeMap<&'static str, String>,
    /// Norms whose linter check skips the file (excluded, or outside the
    /// check's includes): no style is set for them, and no edit offered.
    pub off: BTreeSet<&'static str>,
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

/// Observations a nearer scope lends weight to its own, at most: how far a
/// scope's few observations defer to the code around it.
pub const POOLING: f64 = 4.0;

/// What one scope adds: its unchanged observations outside any nearer scope.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Evidence {
    pub scope: ScopeKind,
    pub counts: Counts,
}

/// The norm `counts` back: the top style's share by the rule of succession,
/// `(top + 1) / (total + styles)`, when it reaches `threshold`. Smoothing
/// keeps a handful of observations from passing for a norm.
pub fn infer(counts: &Counts, threshold: f64) -> Option<Norm> {
    pooled(&[counts], POOLING, threshold)
}

/// The norm `rings` back together, nearest first: each scope's share of a
/// style is shrunk toward the share of the scopes around it, which count as
/// `pooling` observations (never more than they hold), so a scope with few
/// observations follows its surroundings and one with many speaks for itself.
/// The outermost scope with any observations starts from the rule of
/// succession, so a lone scope reads exactly as `infer`.
pub fn pooled(rings: &[&Counts], pooling: f64, threshold: f64) -> Option<Norm> {
    let styles: Vec<&String> = rings
        .iter()
        .flat_map(|c| c.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let kinds = styles.len().max(2) as f64;
    let mut share: Vec<f64> = vec![1.0 / kinds; styles.len()];
    let mut weight: Option<f64> = None;
    for counts in rings.iter().rev() {
        let total: usize = counts.values().sum();
        if total == 0 {
            continue;
        }
        let prior = weight.map_or(kinds, |w| w.min(pooling));
        for (s, style) in share.iter_mut().zip(&styles) {
            let k = counts.get(*style).copied().unwrap_or_default() as f64;
            *s = (k + prior * *s) / (total as f64 + prior);
        }
        weight = Some(total as f64 + prior);
    }
    weight?;
    let best = share.iter().copied().fold(f64::MIN, f64::max);
    let mut leaders = styles.iter().zip(&share).filter(|(_, s)| **s == best);
    let (top, &confidence) = leaders.next()?;
    if leaders.next().is_some() {
        return None;
    }
    let confidence = round2(confidence);
    (confidence >= threshold).then(|| Norm {
        style: (*top).clone(),
        confidence,
    })
}

impl Analysis {
    /// A directive turns `norm`'s check off at `line`.
    pub fn disabled(&self, norm: &str, line: usize) -> bool {
        self.disabled.iter().any(|(name, first, last)| {
            (*first..=*last).contains(&line)
                && (name == "all"
                    || name == norm
                    || norm
                        .strip_prefix(name.as_str())
                        .is_some_and(|r| r.starts_with('/')))
        })
    }
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
    fn pooling_shrinks_a_scope_toward_its_surroundings() {
        let norm = |rings: &[&[(&str, usize)]]| {
            let rings: Vec<Counts> = rings.iter().map(|r| counts(r)).collect();
            let rings: Vec<&Counts> = rings.iter().collect();
            pooled(&rings, POOLING, DEFAULT_THRESHOLD).map(|n| (n.style, n.confidence))
        };
        let (a3, b0, none) = (
            &[("a", 3), ("b", 0)][..],
            &[("a", 0), ("b", 0)][..],
            &[][..],
        );
        // A lone scope is the rule of succession, wherever it sits.
        assert_eq!(norm(&[a3]), Some(("a".into(), 0.8)));
        assert_eq!(norm(&[b0, b0, a3]), norm(&[a3]));
        assert_eq!(norm(&[b0, none]), None);
        // Few observations follow strong surroundings; many speak for themselves.
        assert_eq!(
            norm(&[
                &[("a", 0), ("b", 1)],
                &[("a", 1), ("b", 0)],
                &[("a", 12), ("b", 0)]
            ]),
            None
        );
        assert_eq!(
            norm(&[b0, &[("a", 1), ("b", 0)], &[("a", 12), ("b", 0)]]),
            Some(("a".into(), 0.94))
        );
        assert_eq!(norm(&[&[("a", 0), ("b", 3)], &[("a", 20), ("b", 0)]]), None);
        assert_eq!(
            norm(&[&[("a", 0), ("b", 10)], &[("a", 6), ("b", 4)]]),
            Some(("b".into(), 0.83))
        );
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
