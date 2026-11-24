//! A hunk's style against its surroundings: per norm, the styles it removed
//! and added, the norm of the unchanged code around it (enclosing method,
//! then class, then file; for a new file, its directory), and whether the
//! change converges on that norm or diverges from it.

use super::{
    Analysis, Counts, Language, LineIndex, NormDef, Observation, Replace, ScopeKind, count, edit,
    for_path, infer, skipped,
};
use crate::change::{Change, FileChange};
use crate::diff::{Hunk, Lines};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Neighbor files read for a new file's directory norm, at most.
const NEIGHBORS: usize = 20;

#[derive(Serialize)]
pub struct HunkStyle {
    pub skipped: Option<&'static str>,
    pub direction: Direction,
    pub flagged: bool,
    pub norms: Vec<Entry>,
}

/// Ordered so the strongest signal is the largest: a hunk's rollup is the
/// largest of its norms'.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    None,
    Abstain,
    Aligned,
    Converging,
    Diverging,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::None => "none",
            Direction::Abstain => "abstain",
            Direction::Aligned => "aligned",
            Direction::Converging => "converging",
            Direction::Diverging => "diverging",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Convergence {
    Normalizing,
    Renorming,
}

#[derive(Serialize)]
pub struct Entry {
    pub norm: &'static str,
    pub before: Counts,
    pub after: Counts,
    pub scope: Option<ScopeKind>,
    pub surrounding: Option<Counts>,
    pub inferred: Option<String>,
    pub confidence: Option<f64>,
    pub previous: Option<Counts>,
    pub configured: Option<String>,
    pub expected: Option<String>,
    pub direction: Direction,
    pub convergence: Option<Convergence>,
    pub restyle: bool,
    pub flagged: bool,
    pub deviations: Vec<Deviation>,
}

#[derive(Serialize)]
pub struct Deviation {
    pub line: usize,
    pub column: usize,
    pub style: String,
    pub expected: String,
    pub edit: Option<String>,
}

/// Text lines for a hunk's style: diverging and converging norms only, as
/// abstaining and aligned ones have nothing to say.
pub fn text(style: &HunkStyle) -> Vec<String> {
    let mut out = Vec::new();
    for e in &style.norms {
        let (Some(expected), true) = (
            &e.expected,
            matches!(e.direction, Direction::Diverging | Direction::Converging),
        ) else {
            continue;
        };
        let backing = match (&e.surrounding, e.confidence, e.scope) {
            (Some(counts), Some(confidence), Some(scope)) => {
                let k = counts.get(expected).copied().unwrap_or_default();
                let n: usize = counts.values().sum();
                format!(
                    "{} expects {expected} ({k} of {n}, {confidence:.2})",
                    scope_name(scope)
                )
            }
            _ => format!("config expects {expected}"),
        };
        let what = match (e.direction, e.convergence) {
            (Direction::Converging, Some(Convergence::Renorming)) => "renorming".to_string(),
            (Direction::Converging, _) => "normalizing".to_string(),
            _ => {
                let lines: Vec<String> = e.deviations.iter().map(|d| d.line.to_string()).collect();
                format!("diverging at line {}", lines.join(", "))
            }
        };
        let mut line = format!("style {}: {what}; {backing}", e.norm);
        if let Some(configured) = &e.configured
            && e.inferred.as_ref().is_some_and(|i| i != configured)
        {
            line.push_str(&format!("; config says {configured}"));
        }
        if e.flagged {
            line.push_str(" (a restyle that diverges)");
        }
        out.push(line);
    }
    out
}

fn scope_name(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::Method => "method",
        ScopeKind::Class => "class",
        ScopeKind::File => "file",
        ScopeKind::Directory => "directory",
    }
}

/// Every hunk's style, in report order; `None` where no language checks
/// norms or the file wasn't read. `root` is the working tree, for linter
/// config and a new file's neighbors.
pub fn styles(change: &Change, root: Option<&Path>, threshold: f64) -> Vec<Option<HunkStyle>> {
    let mut out = Vec::new();
    for file in &change.files {
        let hunks = &file.diff.hunks;
        match file_styles(file, root, threshold) {
            Some(styles) => out.extend(styles.into_iter().map(Some)),
            None => out.extend(hunks.iter().map(|_| None)),
        }
    }
    out
}

fn skipped_style(reason: &'static str) -> HunkStyle {
    HunkStyle {
        skipped: Some(reason),
        direction: Direction::None,
        flagged: false,
        norms: Vec::new(),
    }
}

fn file_styles(file: &FileChange, root: Option<&Path>, threshold: f64) -> Option<Vec<HunkStyle>> {
    let path = file.diff.path();
    let language = for_path(path)?;
    let trees = file.trees.as_ref().ok()?;
    let hunks = &file.diff.hunks;
    if let Some(reason) = skipped(path, file.category) {
        return Some(hunks.iter().map(|_| skipped_style(reason)).collect());
    }
    let before = language.analyze(&trees.before.source);
    let after = language.analyze(&trees.after.source);
    if before.errors || after.errors {
        return Some(
            hunks
                .iter()
                .map(|_| skipped_style("syntax_error"))
                .collect(),
        );
    }
    let dir = Path::new(path).parent().unwrap_or(Path::new(""));
    let configured = root
        .map(|r| language.configured(&r.join(dir), r))
        .unwrap_or_default();
    // A new file has no unchanged code of its own.
    let neighbors = match (file.diff.old_path.is_none(), root) {
        (true, Some(root)) => Some(neighbors(language, root, path)),
        _ => None,
    };
    let touched: Vec<bool> = after
        .observations
        .iter()
        .map(|o| hunks.iter().any(|h| after_touches(h, o.lines)))
        .collect();
    let file = File {
        path,
        source: &trees.after.source,
        before: &before,
        after: &after,
        touched,
        neighbors,
        configured,
        threshold,
        index: LineIndex::new(&trees.after.source),
    };
    Some(
        hunks
            .iter()
            .map(|hunk| {
                let norms: Vec<Entry> = language
                    .norms()
                    .iter()
                    .filter_map(|def| file.entry(def, hunk))
                    .collect();
                HunkStyle {
                    skipped: None,
                    direction: norms
                        .iter()
                        .map(|e| e.direction)
                        .max()
                        .unwrap_or(Direction::None),
                    flagged: norms.iter().any(|e| e.flagged),
                    norms,
                }
            })
            .collect(),
    )
}

/// Observations in the same directory's other files, unchanged code all.
fn neighbors(language: &dyn Language, root: &Path, path: &str) -> Vec<Observation> {
    let full = root.join(path);
    let Some(dir) = full.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && *p != full)
        .filter(|p| p.to_str().is_some_and(|s| language.handles(s)))
        .collect();
    files.sort();
    files.truncate(NEIGHBORS);
    crate::verbose!("{path}: new file, reading {} neighbor(s)", files.len());
    files
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .map(|source| language.analyze(&source))
        .filter(|a| !a.errors)
        .flat_map(|a| a.observations)
        .collect()
}

/// A changed line within `lines`, or, for a choice spanning lines (the gap
/// between two definitions), a deletion inside it.
fn touches(changed: &[usize], region: Lines, opposite: bool, (a, b): (usize, usize)) -> bool {
    changed.iter().any(|l| (a..=b).contains(l))
        || (opposite && b > a && region.start <= b && region.start + region.count > a)
}

fn after_touches(hunk: &Hunk, lines: (usize, usize)) -> bool {
    touches(&hunk.added, hunk.after, !hunk.removed.is_empty(), lines)
}

fn before_touches(hunk: &Hunk, lines: (usize, usize)) -> bool {
    touches(&hunk.removed, hunk.before, !hunk.added.is_empty(), lines)
}

/// Lines (first, last) the scope chain must contain: the changed region, or
/// for an empty one the lines on either side of it.
fn region(lines: Lines) -> (usize, usize) {
    match lines.last() {
        Some(last) => (lines.start, last),
        None => (lines.start.saturating_sub(1).max(1), lines.start),
    }
}

/// The innermost method and class around `region`, then the whole file.
fn chain(analysis: &Analysis, (first, last): (usize, usize)) -> Vec<(ScopeKind, usize, usize)> {
    let mut out = Vec::new();
    for kind in [ScopeKind::Method, ScopeKind::Class] {
        let innermost = analysis
            .scopes
            .iter()
            .filter(|s| s.kind == kind && s.first <= first && last <= s.last)
            .min_by_key(|s| s.last - s.first);
        if let Some(s) = innermost {
            out.push((kind, s.first, s.last));
        }
    }
    out.push((ScopeKind::File, 1, analysis.lines.max(1)));
    out
}

struct File<'a> {
    path: &'a str,
    source: &'a [u8],
    before: &'a Analysis,
    after: &'a Analysis,
    /// Per after-side observation: some hunk changed it.
    touched: Vec<bool>,
    neighbors: Option<Vec<Observation>>,
    configured: BTreeMap<&'static str, String>,
    threshold: f64,
    index: LineIndex,
}

/// The norm a scope's counts back, and where.
struct Found {
    scope: ScopeKind,
    counts: Counts,
    style: String,
    confidence: f64,
}

impl File<'_> {
    fn entry(&self, def: &NormDef, hunk: &Hunk) -> Option<Entry> {
        let of_norm = |o: &&Observation| o.norm == def.id;
        let removed: Vec<&Observation> = (self.before.observations.iter())
            .filter(of_norm)
            .filter(|o| before_touches(hunk, o.lines))
            .collect();
        let added: Vec<&Observation> = (self.after.observations.iter())
            .filter(of_norm)
            .filter(|o| after_touches(hunk, o.lines))
            .collect();
        // Only new code is judged: deleting code, or carrying a string over
        // unchanged into new code, chooses no style.
        let fresh = fresh(&removed, &added);
        if fresh.is_empty() {
            return None;
        }
        let configured = self.configured.get(def.id).cloned();
        let surrounding = self.surrounding(def, hunk);
        let renorm = match &surrounding {
            None => self.renorm(def, hunk, &fresh),
            Some(_) => None,
        };
        let found = surrounding.as_ref().or(renorm.as_ref().map(|(f, _)| f));
        let expected = found.map(|f| f.style.clone()).or(configured.clone());
        let deviating = |o: &&&Observation| expected.as_ref().is_some_and(|e| o.style != *e);
        let direction = match &expected {
            None => Direction::Abstain,
            Some(_) if fresh.iter().any(|o| deviating(&o)) => Direction::Diverging,
            Some(_) if renorm.is_some() || removed.iter().any(|o| deviating(&o)) => {
                Direction::Converging
            }
            Some(_) => Direction::Aligned,
        };
        let convergence = (direction == Direction::Converging).then_some(if renorm.is_some() {
            Convergence::Renorming
        } else {
            Convergence::Normalizing
        });
        let restyle = respelled(&removed, &added);
        let deviations = match &expected {
            Some(expected) => fresh
                .iter()
                .filter(|o| o.style != *expected)
                .map(|o| Deviation {
                    line: o.line,
                    column: o.column,
                    style: o.style.clone(),
                    expected: expected.clone(),
                    edit: (o.fix.iter())
                        .find(|(style, _)| style == expected)
                        .filter(|(_, replaces)| self.inside(hunk, replaces))
                        .and_then(|(_, replaces)| edit::unified(self.path, self.source, replaces)),
                })
                .collect(),
            None => Vec::new(),
        };
        Some(Entry {
            norm: def.id,
            before: count(def, removed.into_iter()),
            after: count(def, added.into_iter()),
            scope: found.map(|f| f.scope),
            surrounding: found.map(|f| f.counts.clone()),
            inferred: found.map(|f| f.style.clone()),
            confidence: found.map(|f| f.confidence),
            previous: renorm.map(|(_, previous)| previous),
            configured,
            expected,
            direction,
            convergence,
            restyle,
            flagged: restyle && direction == Direction::Diverging,
            deviations,
        })
    }

    /// Every replacement lies in the hunk's changed region: a suggested edit
    /// never reaches code the change didn't touch.
    fn inside(&self, hunk: &Hunk, replaces: &[Replace]) -> bool {
        let Some(last) = hunk.after.last() else {
            return false;
        };
        let region = hunk.after.start..=last;
        replaces.iter().all(|r| {
            region.contains(&self.index.line(r.span.start))
                && region.contains(
                    &self
                        .index
                        .line(r.span.end.saturating_sub(1).max(r.span.start)),
                )
        })
    }

    /// The norm of the unchanged code around the hunk, from the narrowest
    /// scope whose counts back one.
    fn surrounding(&self, def: &NormDef, hunk: &Hunk) -> Option<Found> {
        let unchanged = |first: usize, last: usize| {
            let observations = self.after.observations.iter().zip(&self.touched);
            count(
                def,
                observations
                    .filter(|(o, touched)| {
                        !**touched && o.norm == def.id && (first..=last).contains(&o.line)
                    })
                    .map(|(o, _)| o),
            )
        };
        let mut scopes: Vec<(ScopeKind, Counts)> = chain(self.after, region(hunk.after))
            .into_iter()
            .map(|(kind, first, last)| (kind, unchanged(first, last)))
            .collect();
        if let Some(neighbors) = &self.neighbors {
            let observations = neighbors.iter().filter(|o| o.norm == def.id);
            scopes.push((ScopeKind::Directory, count(def, observations)));
        }
        scopes.into_iter().find_map(|(scope, counts)| {
            let norm = infer(&counts, self.threshold)?;
            Some(Found {
                scope,
                counts,
                style: norm.style,
                confidence: norm.confidence,
            })
        })
    }

    /// A scope that moved wholesale: before the change its code backed one
    /// norm, after it another, which the new code follows. Returns the norm
    /// after, and the counts before.
    fn renorm(
        &self,
        def: &NormDef,
        hunk: &Hunk,
        added: &[&Observation],
    ) -> Option<(Found, Counts)> {
        let all = |analysis: &Analysis, first: usize, last: usize| {
            count(
                def,
                (analysis.observations.iter())
                    .filter(|o| o.norm == def.id && (first..=last).contains(&o.line)),
            )
        };
        let before_chain = chain(self.before, region(hunk.before));
        for (kind, first, last) in chain(self.after, region(hunk.after)) {
            let Some(&(_, b_first, b_last)) = before_chain.iter().find(|(k, ..)| *k == kind) else {
                continue;
            };
            let previous = all(self.before, b_first, b_last);
            let counts = all(self.after, first, last);
            let (Some(old), Some(new)) = (
                infer(&previous, self.threshold),
                infer(&counts, self.threshold),
            ) else {
                continue;
            };
            if old.style != new.style && added.iter().all(|o| o.style == new.style) {
                let found = Found {
                    scope: kind,
                    counts,
                    style: new.style,
                    confidence: new.confidence,
                };
                return Some((found, previous));
            }
        }
        None
    }
}

/// The hunk respelled existing code: the same content on both sides, in
/// other styles.
/// The added observations that choose a style: those not matched by a
/// removed one, first by text and style (carried over), then by style alone
/// (an edited string keeps its quotes).
fn fresh<'a>(removed: &[&Observation], added: &[&'a Observation]) -> Vec<&'a Observation> {
    let mut kept: Vec<&Observation> = removed.to_vec();
    let mut take = |o: &Observation, same: fn(&Observation, &Observation) -> bool| {
        let found = kept.iter().position(|k| same(k, o));
        found.map(|i| kept.swap_remove(i)).is_some()
    };
    let new_text: Vec<&'a Observation> = (added.iter().copied())
        .filter(|o| !take(o, |k, o| k.key == o.key && k.style == o.style))
        .collect();
    new_text
        .into_iter()
        .filter(|o| !take(o, |k, o| k.style == o.style))
        .collect()
}

fn respelled(removed: &[&Observation], added: &[&Observation]) -> bool {
    !removed.is_empty()
        && multiset(removed, false) == multiset(added, false)
        && multiset(removed, true) != multiset(added, true)
}

fn multiset<'a>(observations: &[&'a Observation], style: bool) -> Vec<(&'a str, &'a str)> {
    let mut keys: Vec<(&str, &str)> = observations
        .iter()
        .map(|o| (o.key.as_str(), if style { o.style.as_str() } else { "" }))
        .collect();
    keys.sort();
    keys
}
