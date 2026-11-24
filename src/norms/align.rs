//! A hunk's style against its surroundings: per norm, the styles it removed
//! and added, the norm of the unchanged code around it (enclosing method,
//! class, file and directory, pooled with the nearest weighted most), and
//! whether the change converges on that norm or diverges from it.

use super::snapshot::{Snapshot, parent};
use super::{
    Analysis, Counts, Evidence, Language, LineIndex, NormDef, Observation, POOLING, Replace,
    ScopeKind, count, edit, for_path, infer, pooled, skipped,
};
use crate::change::{Change, FileChange};
use crate::classify::proof::{Outside, Search};
use crate::diff::{Hunk, Lines};
use crate::tree::Tree;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// Neighbor files a directory's evidence reads, at most.
const NEIGHBORS: usize = 20;

#[derive(Serialize)]
pub struct HunkStyle {
    pub skipped: Option<&'static str>,
    pub direction: Direction,
    pub flagged: bool,
    pub norms: Vec<Entry>,
}

impl HunkStyle {
    /// Marks each norm the hunk's `restyle` part proved respelled, its
    /// variants being norm ids, and flags those that diverge.
    pub fn restyled(&mut self, variants: &[&str]) {
        for e in &mut self.norms {
            e.restyle = variants.contains(&e.norm);
            e.flagged = e.restyle && e.direction == Direction::Diverging;
        }
        self.flagged = self.norms.iter().any(|e| e.flagged);
    }
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
    pub evidence: Vec<Evidence>,
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
        let backing = match (e.confidence, e.scope) {
            (Some(confidence), Some(scope)) => {
                let counts: Vec<String> = (e.evidence.iter())
                    .filter_map(|ev| {
                        let n: usize = ev.counts.values().sum();
                        let k = ev.counts.get(expected).copied().unwrap_or_default();
                        (n > 0).then(|| format!("{} {k} of {n}", scope_name(ev.scope)))
                    })
                    .collect();
                format!(
                    "{} expects {expected} ({confidence:.2}: {})",
                    scope_name(scope),
                    counts.join(", ")
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
/// norms or the file wasn't read. `snapshot` is the repository the change
/// ends in, for linter config and each file's neighbors.
pub fn styles(
    change: &Change,
    snapshot: Option<&dyn Snapshot>,
    outside: &dyn Outside,
    threshold: f64,
) -> Vec<Option<HunkStyle>> {
    let mut out = Vec::new();
    let directory = Directory::new(change);
    let deleted = deleted(change);
    for file in &change.files {
        let hunks = &file.diff.hunks;
        match file_styles(file, snapshot, outside, &directory, &deleted, threshold) {
            Some(styles) => out.extend(styles.into_iter().map(Some)),
            None => out.extend(hunks.iter().map(|_| None)),
        }
    }
    out
}

/// Every line the change deletes, anywhere: without its indentation, and
/// as it was.
fn deleted(change: &Change) -> [HashSet<&[u8]>; 2] {
    let mut out = [HashSet::new(), HashSet::new()];
    for file in &change.files {
        let Ok(trees) = &file.trees else { continue };
        let source = &trees.before.source[..];
        let index = LineIndex::new(source);
        for line in file.diff.hunks.iter().flat_map(|h| &h.removed) {
            out[0].insert(unindented(source, &index, *line, false));
            out[1].insert(unindented(source, &index, *line, true));
        }
    }
    out
}

/// Line `line` of `source`, without its indentation unless it `keeps` it;
/// its line break stays, and a blank line keeps its whitespace, as norms
/// read both.
fn unindented<'a>(source: &'a [u8], index: &LineIndex, line: usize, keeps: bool) -> &'a [u8] {
    let text = &source[index.start(line)..index.start(line + 1).min(source.len())];
    let code = text.trim_ascii_start();
    if keeps || code.trim_ascii().is_empty() {
        text
    } else {
        code
    }
}

fn skipped_style(reason: &'static str) -> HunkStyle {
    HunkStyle {
        skipped: Some(reason),
        direction: Direction::None,
        flagged: false,
        norms: Vec::new(),
    }
}

fn file_styles(
    file: &FileChange,
    snapshot: Option<&dyn Snapshot>,
    outside: &dyn Outside,
    directory: &Directory,
    deleted: &[HashSet<&[u8]>; 2],
    threshold: f64,
) -> Option<Vec<HunkStyle>> {
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
    let super::Configured {
        styles: configured,
        off,
    } = snapshot
        .map(|s| language.configured(path, s))
        .unwrap_or_default();
    let neighbors = snapshot.map(|s| directory.read(language, s, path));
    let touched: Vec<bool> = after
        .observations
        .iter()
        .map(|o| hunks.iter().any(|h| after_touches(h, o.lines)))
        .collect();
    let file = File {
        path,
        old_path: file.diff.old_path.as_deref(),
        outside,
        deleted,
        source: &trees.after.source,
        tree: &trees.after,
        before: &before,
        after: &after,
        touched,
        neighbors,
        configured,
        off,
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

/// A directory's files of one language, each with its observations.
type Neighbors = Vec<(String, Vec<Observation>)>;

/// Each directory's files as they were before the change, read once per run
/// however many changed files sit in it: the change's own new code never
/// backs the norm it is judged by.
struct Directory<'a> {
    read: RefCell<HashMap<(&'static str, String), Rc<Neighbors>>>,
    /// The paths the change's files have after it.
    after: Vec<&'a str>,
    /// The change's files that existed before it, by their old path, with
    /// their content then (`None` when it wasn't read).
    before: HashMap<&'a str, Option<&'a [u8]>>,
}

impl<'a> Directory<'a> {
    fn new(change: &'a Change) -> Self {
        let mut before = HashMap::new();
        let mut after = Vec::new();
        for file in &change.files {
            after.extend(file.diff.new_path.as_deref());
            if let Some(old) = file.diff.old_path.as_deref() {
                let source = file.trees.as_ref().ok().map(|t| &t.before.source[..]);
                before.insert(old, source);
            }
        }
        Directory {
            read: RefCell::default(),
            after,
            before,
        }
    }

    /// The first files of `path`'s directory before the change, enough for
    /// `NEIGHBORS` besides the file itself.
    fn read(&self, language: &dyn Language, snapshot: &dyn Snapshot, path: &str) -> Rc<Neighbors> {
        let dir = parent(path).to_string();
        let key = (language.name(), dir.clone());
        if let Some(found) = self.read.borrow().get(&key) {
            return found.clone();
        }
        // The snapshot is the after side: take out what the change put there
        // and put back what it moved or deleted away.
        let mut files: Vec<String> = snapshot
            .list(&dir)
            .into_iter()
            .filter(|p| !self.after.contains(&p.as_str()))
            .chain(
                (self.before.keys())
                    .filter(|p| parent(p) == dir)
                    .map(|p| p.to_string()),
            )
            .filter(|p| language.handles(p))
            .collect();
        files.sort();
        files.dedup();
        files.truncate(NEIGHBORS + 1);
        crate::verbose!("{dir}/: reading {} file(s) for its norms", files.len());
        let unchanged: Vec<String> = (files.iter())
            .filter(|p| !self.before.contains_key(p.as_str()))
            .cloned()
            .collect();
        let mut read_back = snapshot.read_all(&unchanged).into_iter();
        let sources: Vec<Option<Vec<u8>>> = (files.iter())
            .map(|p| match self.before.get(p.as_str()) {
                Some(source) => source.map(<[u8]>::to_vec),
                None => read_back.next().flatten(),
            })
            .collect();
        let read: Neighbors = files
            .into_iter()
            .zip(sources)
            .filter_map(|(p, source)| Some((language.analyze(&source?), p)))
            .filter(|(a, _)| !a.errors)
            .map(|(a, p)| (p, a.observations))
            .collect();
        let read = Rc::new(read);
        self.read.borrow_mut().insert(key, read.clone());
        read
    }
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
    /// Where the file was before the change, which isn't its own neighbor.
    old_path: Option<&'a str>,
    /// The repository beyond the change, for renames other files could see.
    outside: &'a dyn Outside,
    /// The lines the change deletes anywhere: unindented, and as they were.
    deleted: &'a [HashSet<&'a [u8]>; 2],
    source: &'a [u8],
    /// The after side, which a suggested edit must keep the same program.
    tree: &'a Tree,
    before: &'a Analysis,
    after: &'a Analysis,
    /// Per after-side observation: some hunk changed it.
    touched: Vec<bool>,
    /// The directory's files, this one among them, with their observations.
    neighbors: Option<Rc<Neighbors>>,
    configured: BTreeMap<&'static str, String>,
    /// Norms the linter config skips this file for.
    off: BTreeSet<&'static str>,
    threshold: f64,
    index: LineIndex,
}

/// The norm the evidence backs, and the nearest scope holding any.
struct Found {
    scope: ScopeKind,
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
        // Code the change deleted elsewhere and put here, moved or
        // re-indented, chooses no style either.
        let fresh: Vec<&Observation> = (fresh(&removed, &added).into_iter())
            .filter(|o| !self.carried(o.lines, def.indents))
            .collect();
        if fresh.is_empty() {
            return None;
        }
        // Where the linter wouldn't check the norm, its config says nothing
        // and no edit is offered; the code's own norm still judges.
        let unchecked =
            |o: &Observation| self.off.contains(def.id) || self.after.disabled(def.id, o.line);
        let configured =
            (self.configured.get(def.id).cloned()).filter(|_| !fresh.iter().all(|o| unchecked(o)));
        let (mut evidence, surrounding) = self.surrounding(def, hunk);
        let renorm = match &surrounding {
            None => self.renorm(def, hunk, &fresh),
            Some(_) => None,
        };
        if let Some((found, after, _)) = &renorm {
            evidence = vec![Evidence {
                scope: found.scope,
                counts: after.clone(),
            }];
        }
        let found = surrounding.as_ref().or(renorm.as_ref().map(|(f, _, _)| f));
        let expected = found.map(|f| f.style.clone()).or(configured.clone());
        let deviating = |o: &&&Observation| expected.as_ref().is_some_and(|e| o.style != *e);
        // Config alone never judges a line its linter wouldn't check.
        let judged: Vec<&Observation> = (fresh.iter().copied())
            .filter(|o| found.is_some() || !unchecked(o))
            .collect();
        let direction = match &expected {
            None => Direction::Abstain,
            Some(_) if judged.iter().any(|o| deviating(&o)) => Direction::Diverging,
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
        let deviations = match &expected {
            Some(expected) => judged
                .iter()
                .filter(|o| o.style != *expected)
                .map(|o| Deviation {
                    line: o.line,
                    column: o.column,
                    style: o.style.clone(),
                    expected: expected.clone(),
                    edit: (o.fix.iter())
                        .filter(|_| !unchecked(o))
                        .find(|(style, _)| style == expected)
                        .filter(|(_, replaces)| self.inside(hunk, replaces))
                        .filter(|(_, replaces)| {
                            edit::keeps(self.path, self.tree, replaces, def.renames)
                        })
                        .filter(|(_, replaces)| !def.shared || self.unshared(replaces))
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
            evidence,
            inferred: found.map(|f| f.style.clone()),
            confidence: found.map(|f| f.confidence),
            previous: renorm.map(|(_, _, previous)| previous),
            configured,
            expected,
            direction,
            convergence,
            // Set from the hunk's restyle part: `HunkStyle::restyled`.
            restyle: false,
            flagged: false,
            deviations,
        })
    }

    /// Every line of `lines` is one the change deleted somewhere, with the
    /// same indentation when it `indents`.
    fn carried(&self, (first, last): (usize, usize), indents: bool) -> bool {
        (first..=last).all(|l| {
            self.deleted[indents as usize].contains(unindented(
                self.source,
                &self.index,
                l,
                indents,
            ))
        })
    }

    /// A rename's old and new names appear in no other file of the
    /// repository: what the search can't settle counts as shared.
    fn unshared(&self, replaces: &[Replace]) -> bool {
        let Some(first) = replaces.first() else {
            return false;
        };
        // The whole identifier: a rename keeps a leading `_` outside its span.
        let start = self.source[..first.span.start]
            .iter()
            .rposition(|&b| b != b'_')
            .map_or(0, |i| i + 1);
        let prefix = String::from_utf8_lossy(&self.source[start..first.span.start]);
        let old = String::from_utf8_lossy(&self.source[start..first.span.end]);
        [old.into_owned(), format!("{prefix}{}", first.text)]
            .iter()
            .all(|word| match self.outside.uses(self.path, word) {
                Search::Done {
                    found, mentioned, ..
                } => found.iter().all(|f| f == self.path) && mentioned.is_empty(),
                Search::Capped | Search::Unavailable => false,
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
            let line = self.index.line(r.span.start);
            // Line breaks inserted at a line start sit between two lines and
            // change neither: either one in the region will do.
            let between = r.span.is_empty()
                && !r.text.is_empty()
                && r.text.bytes().all(|b| b == b'\n')
                && self.index.start(line) == r.span.start;
            if between && line > 1 && region.contains(&(line - 1)) {
                return true;
            }
            region.contains(&line)
                && region.contains(
                    &self
                        .index
                        .line(r.span.end.saturating_sub(1).max(r.span.start)),
                )
        })
    }

    /// The unchanged code around the hunk, per scope from the nearest (each
    /// scope's own, outside the nearer ones), and the norm it backs pooled.
    fn surrounding(&self, def: &NormDef, hunk: &Hunk) -> (Vec<Evidence>, Option<Found>) {
        let mut nearer: Option<(usize, usize)> = None;
        let mut evidence: Vec<Evidence> = Vec::new();
        for (scope, first, last) in chain(self.after, region(hunk.after)) {
            let observations = self.after.observations.iter().zip(&self.touched);
            let counts = count(
                def,
                observations
                    .filter(|(o, touched)| {
                        !**touched
                            && o.norm == def.id
                            && (first..=last).contains(&o.line)
                            && !nearer.is_some_and(|(a, b)| (a..=b).contains(&o.line))
                    })
                    .map(|(o, _)| o),
            );
            nearer = Some((first, last));
            evidence.push(Evidence { scope, counts });
        }
        if let Some(neighbors) = &self.neighbors {
            let observations = (neighbors.iter())
                .filter(|(p, _)| p != self.path && Some(p.as_str()) != self.old_path)
                .take(NEIGHBORS)
                .flat_map(|(_, observations)| observations)
                .filter(|o| o.norm == def.id);
            evidence.push(Evidence {
                scope: ScopeKind::Directory,
                counts: count(def, observations),
            });
        }
        let rings: Vec<&Counts> = evidence.iter().map(|e| &e.counts).collect();
        let found = pooled(&rings, POOLING, self.threshold).and_then(|norm| {
            let nearest = evidence
                .iter()
                .find(|e| e.counts.values().sum::<usize>() > 0)?;
            Some(Found {
                scope: nearest.scope,
                style: norm.style,
                confidence: norm.confidence,
            })
        });
        (evidence, found)
    }

    /// A scope that moved wholesale: before the change its code backed one
    /// norm, after it another, which the new code follows. Returns the norm
    /// after, and the counts after and before.
    fn renorm(
        &self,
        def: &NormDef,
        hunk: &Hunk,
        added: &[&Observation],
    ) -> Option<(Found, Counts, Counts)> {
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
                    style: new.style,
                    confidence: new.confidence,
                };
                return Some((found, counts, previous));
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
