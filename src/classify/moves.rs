//! `move`: a declaration that leaves one place and reappears in another, in
//! the same file or another.
//!
//! Each file's two trees are aligned container by container (the file, then
//! each class or module body): a heaviest common subsequence over the
//! children, declarations keyed by name (so one edited in place still
//! aligns, and a namespace's body aligns in turn), other statements by hash.
//! A declaration (`Role::is_declaration`) left out of the alignment is a
//! *unit*, removed or added. A removed unit pairs with an
//! added one by equal hash or by name and subtree similarity (a body that
//! also changed). A pair is structure only when the code around it means the
//! same in both places: the same owner, the same visibility section, the
//! same order relative to statements that name it at load time and, across
//! files, the same file-level directives, in a language whose files aren't
//! modules of their own. Otherwise its part names each [`Context`] that
//! differs: relocated code, but not structure.
//!
//! A hunk's part comes from the trees, not from git's line placement: it
//! holds the changed lines in a paired unit or in a node the alignment kept
//! unchanged (git may slide a removal across identical lines), except the
//! unit's lines that changed in the move, which stay logic.

use super::align::{Alignments, Side, key, lcs, namespace_name, size, transparent};
use super::format::{dice, subtree_hashes};
use super::hazard::{Op, blocks};
use super::proof::Search;
use super::{Assumption, Claim, Confidence, Kind, Labels, Outside, Part, rounded, site};
use crate::change::{Change, Trees};
use crate::frontend::{self, Load};
use crate::tree::{Flags, Hazard, NameKind, NodeId, Role, Tree, Visibility, spells};
use serde::Serialize;
use serde_json::{Map, json};
use std::collections::{BTreeSet, HashMap};
use std::ops::RangeInclusive;

/// Fewest nodes a unit needs. Size evidence is `1 - 1/nodes`: one node is
/// no evidence, two a coin flip, so three is the first size whose match
/// beats chance. It keeps out a bare name or a constant set to one literal.
const FLOOR: usize = 3;

/// Least subtree similarity for a near match: at least half the subtrees
/// shared, so the bodies are more alike than not.
const NEAR: f64 = 0.5;

pub(super) fn run(
    change: &Change,
    alignments: &Alignments,
    outside: &dyn Outside,
    labels: &mut Labels,
) {
    let mut units = Vec::new();
    // Files whose declarations take part: parsed cleanly, with lines left.
    let mut aligned = vec![false; change.files.len()];
    for (f, file) in alignments.files() {
        if labels[f].iter().all(|l| l.unclaimed().is_empty()) {
            continue;
        }
        aligned[f] = true;
        let Ok(trees) = &change.files[f].trees else {
            continue;
        };
        let units_here = file.unmatched.iter().filter(|&&(side, node)| {
            let tree = match side {
                Side::Removed => &trees.before,
                Side::Added => &trees.after,
            };
            is_unit(tree, node)
        });
        units.extend(units_here.map(|&(side, node)| Unit {
            file: f,
            side,
            node,
            wrapper: None,
        }));
    }
    let (units, wrappers, mut pairs) = open(change, units);
    let mut by_unit: HashMap<usize, usize> = HashMap::new();
    for (p, pair) in pairs.iter().enumerate() {
        // Both halves must be lines the diff changed, or the trees and the
        // diff disagree about what moved.
        if units[pair.from].hunk(change).is_none() || units[pair.to].hunk(change).is_none() {
            continue;
        }
        by_unit.insert(pair.from, p);
        by_unit.insert(pair.to, p);
    }
    let extras = surroundings(change, outside, &units, &wrappers, &by_unit, &mut pairs);

    for (f, (file, labels)) in change.files.iter().zip(labels.iter_mut()).enumerate() {
        let Ok(trees) = &file.trees else { continue };
        for (h, label) in labels.iter_mut().enumerate() {
            if label.unclaimed().is_empty() || !aligned[f] {
                continue;
            }
            let file = alignments.file(f).expect("aligned files parsed cleanly");
            let sides = [
                (Side::Removed, &trees.before, &label.changed.before),
                (Side::Added, &trees.after, &label.changed.after),
            ];
            let mut mine = Vec::new();
            // Changed lines in a paired unit or in code the alignment kept
            // (git may slide a removal across identical lines).
            let mut explained = Claim::default();
            for (side, tree, changed) in sides {
                let here: Vec<(usize, RangeInclusive<usize>)> = (0..units.len())
                    .filter(|&u| units[u].file == f && units[u].side == side)
                    .filter(|u| by_unit.contains_key(u))
                    .map(|u| (u, tree.lines(units[u].node)))
                    .filter(|(_, r)| changed.iter().any(|&l| r.contains(&l) && tree.line(l).code))
                    .collect();
                mine.extend(here.iter().filter_map(|(u, _)| by_unit.get(u).copied()));
                let mut ranges = file.kept(side).to_vec();
                ranges.extend(here.iter().map(|(_, r)| r.clone()));
                for extra in extras.iter().filter(|e| e.file == f && e.side == side) {
                    if changed
                        .iter()
                        .any(|&l| extra.lines.contains(&l) && tree.line(l).code)
                    {
                        mine.extend(&extra.pairs);
                        ranges.push(extra.lines.clone());
                    }
                }
                let lines = site::spanned(changed, &ranges);
                match side {
                    Side::Removed => explained.before.extend(lines),
                    Side::Added => explained.after.extend(lines),
                }
            }
            if mine.is_empty() {
                continue;
            }
            mine.sort_unstable();
            mine.dedup();
            let mine: Vec<&Pair> = mine.iter().map(|&p| &pairs[p]).collect();
            let changed_bodies = mine.iter().filter(|p| p.similarity < 1.0).count();
            let mut context: Vec<Context> = mine.iter().flat_map(|p| p.context.clone()).collect();
            context.sort_unstable();
            context.dedup();
            let mut reason: String = match changed_bodies {
                0 => "declarations moved unchanged".into(),
                n if n == mine.len() => "declarations moved, and their bodies changed".into(),
                n => format!(
                    "declarations moved, {n} of {} with a changed body",
                    mine.len()
                ),
            };
            if !context.is_empty() {
                let why: Vec<&str> = context.iter().map(|c| c.reason()).collect();
                reason = format!("{reason}, but {}", why.join("; "));
            }
            let confidence = Confidence::new(
                mine.iter()
                    .map(|p| p.confidence())
                    .fold(f64::INFINITY, f64::min),
            );
            let mut evidence = label.residual.evidence.clone();
            let values = &mut evidence.values;
            values.clear();
            let similarity = mine.iter().map(|p| p.similarity).fold(1.0, f64::min);
            values.insert("min_similarity".into(), json!(similarity));
            values.insert("nodes".into(), json!(mine.iter().map(|p| p.nodes).min()));
            values.insert(
                "candidates".into(),
                json!(mine.iter().map(|p| p.candidates).max()),
            );
            // Several units matched as well: which one moved is a guess.
            let mut assumptions: Vec<Assumption> = Vec::new();
            let mut why = Vec::new();
            if mine.iter().any(|p| p.public_api) {
                assumptions.push(Assumption::PublicApi);
                why.push("library code may load the old file without the new one");
            }
            if mine.iter().any(|p| p.candidates > 1) {
                assumptions.push(Assumption::AmbiguousMatch);
                why.push("several declarations match it equally well");
            }
            if !why.is_empty() {
                let joint = if context.is_empty() { ", but" } else { ";" };
                reason = format!("{reason}{joint} {}", why.join("; "));
            }
            let entries: Vec<Entry> = mine.iter().map(|p| entry(change, &units, p)).collect();
            let (changed, first) =
                moved_changes(&entries, change.files[f].ids[h], !assumptions.is_empty());
            let names: Vec<&str> = context.iter().map(|c| c.as_str()).collect();
            let lines = explained.minus(&changed);
            let part = Part {
                kind: Kind::Move,
                review: first.intersect(&lines),
                lines,
                assumptions,
                confidence,
                reason,
                evidence,
                detail: Map::from_iter([
                    ("moves".to_string(), json!(entries)),
                    ("context".to_string(), json!(names)),
                ]),
                variants: Vec::new(),
                context,
            };
            label.claim(part);
        }
    }
}

/// The lines inside the hunk's moved units that changed in the move
/// (before-side lines of the units it removed, after-side lines of the units
/// it added), and the first line of each unit moved into a new context, or
/// of every unit when the part isn't proven, where a reviewer checks it.
fn moved_changes(entries: &[Entry], id: usize, unproven: bool) -> (Claim, Claim) {
    let (mut changed, mut first) = (Claim::default(), Claim::default());
    for entry in entries {
        let start = |line: Option<usize>| line.filter(|_| unproven || !entry.context.is_empty());
        if entry.from_hunk == Some(id) {
            changed.before.extend(&entry.changed_before);
            first.before.extend(start(entry.from_line));
        }
        if entry.to_hunk == Some(id) {
            changed.after.extend(&entry.changed_after);
            first.after.extend(start(entry.to_line));
        }
    }
    (changed, first)
}

/// A declaration the alignment left out: removed from the before tree or
/// added to the after tree.
struct Unit {
    file: usize,
    side: Side,
    node: NodeId,
    /// The opened namespace it was found in, an index into the wrappers.
    wrapper: Option<usize>,
}

/// A namespace the alignment left out that paired with nothing as a whole,
/// so its declarations took part on their own: the `module Zoo` around a
/// class moved into a new file.
struct Wrapper {
    file: usize,
    side: Side,
    node: NodeId,
    parent: Option<usize>,
}

/// Lines a hunk's move part also holds beyond its paired units: an opened
/// namespace whose every member moved, a new or deleted file's directives,
/// the statement that loads a new file.
struct Extra {
    file: usize,
    side: Side,
    lines: RangeInclusive<usize>,
    pairs: Vec<usize>,
}

/// Pairs the units, then opens each namespace left unpaired and pairs again
/// with its declarations in its place, until none is left to open.
fn open(change: &Change, mut units: Vec<Unit>) -> (Vec<Unit>, Vec<Wrapper>, Vec<Pair>) {
    let mut wrappers = Vec::new();
    loop {
        let pairs = pair(change, &units);
        let mut paired = vec![false; units.len()];
        for p in &pairs {
            paired[p.from] = true;
            paired[p.to] = true;
        }
        let opens = |u: &Unit| {
            let tree = u.tree(change);
            namespace(tree, u.node) && members(tree, u.node).any(|m| is_unit(tree, m))
        };
        // The outermost first: a namespace one level in may yet pair whole.
        let depth = |u: &Unit| owners(u.tree(change), u.node).len();
        let Some(outer) = (0..units.len())
            .filter(|&u| !paired[u] && opens(&units[u]))
            .map(|u| depth(&units[u]))
            .min()
        else {
            return (units, wrappers, pairs);
        };
        let mut next = Vec::new();
        for (u, unit) in units.into_iter().enumerate() {
            if paired[u] || !opens(&unit) || depth(&unit) != outer {
                next.push(unit);
                continue;
            }
            let w = wrappers.len();
            let tree = unit.tree(change);
            next.extend(
                members(tree, unit.node)
                    .filter(|&m| is_unit(tree, m))
                    .map(|node| Unit {
                        file: unit.file,
                        side: unit.side,
                        node,
                        wrapper: Some(w),
                    }),
            );
            wrappers.push(Wrapper {
                file: unit.file,
                side: unit.side,
                node: unit.node,
                parent: unit.wrapper,
            });
        }
        units = next;
    }
}

/// Words that may start loading a file by a name computed at runtime.
const LOADER_WORDS: &[&str] = &[
    "require",
    "require_relative",
    "load",
    "autoload",
    "Zeitwerk",
];

/// One statement loading a file, as [`Loading`] finds it.
struct Point {
    /// The index of the file in the change, when it is in it.
    file: Option<usize>,
    stmt: NodeId,
    load: Load,
    /// A statement of its own in a statement list, and so for the next one
    /// in the list.
    plain: bool,
    /// At the top level of its file.
    top: bool,
    /// The load in the statement before it and after it in its list.
    prev: Option<Load>,
    next: Option<Load>,
}

/// When files load, from the statements that load them: in the change's
/// after trees and in the repository beyond it, searched once per file.
struct Loading<'c> {
    change: &'c Change,
    outside: &'c dyn Outside,
    hazard: Option<bool>,
    points: HashMap<String, Option<Vec<Point>>>,
}

impl<'c> Loading<'c> {
    fn new(change: &'c Change, outside: &'c dyn Outside) -> Self {
        Loading {
            change,
            outside,
            hazard: None,
            points: HashMap::new(),
        }
    }

    fn covered(&self) -> BTreeSet<&'c str> {
        let paths = self.change.files.iter();
        paths
            .flat_map(|f| [f.diff.old_path.as_deref(), f.diff.new_path.as_deref()])
            .flatten()
            .collect()
    }

    /// Some file of the language loads files by a computed name, or one
    /// can't be read for it: any file may load at any time.
    fn hazard(&mut self, path: &str) -> bool {
        if let Some(hazard) = self.hazard {
            return hazard;
        }
        let computed = |tree: &Tree, file: &str| {
            let frontend = frontend::for_path(file);
            !tree.errors.is_empty()
                || frontend.is_some_and(|fe| {
                    tree.descendants(Tree::ROOT)
                        .any(|id| fe.load(tree, id, file) == Some(Load::Computed))
                })
        };
        let in_change = self
            .change
            .files
            .iter()
            .any(|f| match (&f.trees, &f.diff.new_path) {
                (Ok(t), Some(file))
                    if frontend::for_path(file).is_some_and(|fe| fe.language() == t.language) =>
                {
                    computed(&t.after, file)
                }
                _ => false,
            });
        let covered = self.covered();
        let hazard = in_change
            || LOADER_WORDS
                .iter()
                .any(|word| match self.outside.uses(path, word) {
                    Search::Done { found, .. } => found
                        .iter()
                        .filter(|f| !covered.contains(f.as_str()))
                        .any(|f| self.outside.parse(f).is_none_or(|tree| computed(&tree, f))),
                    _ => true,
                });
        self.hazard = Some(hazard);
        hazard
    }

    /// Every statement loading the file at `path`; `None` when some file
    /// that may hold one can't be read for it.
    fn points(&mut self, path: &str) -> Option<&[Point]> {
        if !self.points.contains_key(path) {
            let found = self.find(path);
            self.points.insert(path.to_string(), found);
        }
        self.points[path].as_deref()
    }

    fn find(&self, path: &str) -> Option<Vec<Point>> {
        let base = path.rsplit('/').next().unwrap_or(path);
        let stem = base.split('.').next().unwrap_or(base);
        let mut out = Vec::new();
        for (f, file) in self.change.files.iter().enumerate() {
            let (Ok(trees), Some(name)) = (&file.trees, &file.diff.new_path) else {
                continue;
            };
            let Some(fe) = frontend::for_path(name) else {
                continue;
            };
            if !trees.after.errors.is_empty() {
                return None;
            }
            out.extend(scan(fe, &trees.after, name, path).map(|p| Point { file: Some(f), ..p }));
        }
        let covered = self.covered();
        match self.outside.uses(path, stem) {
            Search::Done {
                found, mentioned, ..
            } if mentioned.is_empty() => {
                for name in found.iter().filter(|f| !covered.contains(f.as_str())) {
                    let fe = frontend::for_path(name)?;
                    let tree = self.outside.parse(name)?;
                    if !tree.errors.is_empty() {
                        return None;
                    }
                    out.extend(scan(fe, &tree, name, path));
                }
            }
            _ => return None,
        }
        Some(out)
    }

    /// Whether the new file holding `to` runs `from`'s code where it ran before,
    /// nothing in between depending on it: whether it runs after the old
    /// file's own statements, and whether other files load it (after the
    /// old one) rather than the old file itself; `None` when not shown.
    ///
    /// Shown two ways. The old file loads the new one (`require_relative`)
    /// at its top level, and every other load of the new file follows it or
    /// a load of the old file. Or every load of the new file directly
    /// follows a load of the old one, and every load of the old file is
    /// followed by one of the new: it runs right after the old file. Either
    /// way no file may load files by a computed name.
    fn runs(&mut self, from: &Unit, to: &Unit) -> Option<(bool, bool)> {
        let change = self.change;
        let old = change.files[from.file].diff.new_path.as_deref()?;
        let new = change.files[to.file].diff.path();
        if self.hazard(new) {
            return None;
        }
        let is = |load: &Option<Load>, path: &str| matches!(load, Some(Load::Now(t)) if t.is(path));
        let old_points: Vec<(bool, bool)> = self
            .points(old)?
            .iter()
            .map(|p| (matches!(p.load, Load::Now(_)) && p.plain, is(&p.next, new)))
            .collect();
        let points = self.points(new)?;
        let first = points
            .iter()
            .filter(|p| p.file == Some(from.file) && p.top && matches!(p.load, Load::Now(_)))
            .min_by_key(|p| p.stmt.0);
        let attached = |p: &Point| matches!(p.load, Load::Now(_)) && p.plain && is(&p.prev, old);
        let trees = from.trees(change);
        let at = match first {
            Some(first) => {
                let rest = points
                    .iter()
                    .filter(|p| p.stmt != first.stmt || p.file != first.file);
                let fine = |p: &Point| attached(p) || (p.file == Some(from.file) && p.top);
                if !rest.into_iter().all(fine) {
                    return None;
                }
                let tops: Vec<NodeId> = members(&trees.after, Tree::ROOT).collect();
                Some(tops.iter().position(|&t| t == first.stmt)?)
            }
            None => {
                let loaded = !points.is_empty() && points.iter().all(attached);
                let follows =
                    !old_points.is_empty() && old_points.iter().all(|&(plain, next)| plain && next);
                if !loaded || !follows {
                    return None;
                }
                None
            }
        };
        let after = window(&trees.before, from.node, &trees.after, at)?;
        Some((after, at.is_none()))
    }

    /// The statements in the change that load the file `to`, which the
    /// file `from` lost code to.
    fn loaders(&mut self, _from: usize, to: usize) -> Vec<(usize, NodeId)> {
        let new = self.change.files[to].diff.path().to_string();
        let points = self.points(&new).unwrap_or_default();
        points
            .iter()
            .filter_map(|p| Some((p.file?, p.stmt)))
            .collect()
    }
}

/// The statements of `tree` (the file at `name`) that load the file at
/// `path`.
fn scan<'t>(
    fe: &'t dyn crate::frontend::Frontend,
    tree: &'t Tree,
    name: &'t str,
    path: &'t str,
) -> impl Iterator<Item = Point> + 't {
    tree.descendants(Tree::ROOT).filter_map(move |id| {
        let load = fe.load(tree, id, name)?;
        let (Load::Now(target) | Load::Other(target)) = &load else {
            return None;
        };
        if !target.is(path) {
            return None;
        }
        let parent = tree.node(id).parent?;
        let plain = transparent(tree, parent);
        let top = tree
            .ancestors(id)
            .all(|a| a == Tree::ROOT || transparent(tree, a));
        let siblings: Vec<NodeId> = tree.code_children(parent).collect();
        let at = siblings.iter().position(|&s| s == id)?;
        let near = |i: Option<usize>| {
            i.and_then(|i| siblings.get(i))
                .and_then(|&s| fe.load(tree, s, name))
                .filter(|_| plain)
        };
        Some(Point {
            file: None,
            stmt: id,
            load,
            plain,
            top,
            prev: near(at.checked_sub(1)),
            next: near(Some(at + 1)),
        })
    })
}

/// Whether `unit`, of the old file's before tree, can run where the old
/// file's after tree loads its new file: at top-level statement `at`, or
/// after the file's last statement when `None`. Every statement that now
/// runs on the other side of it must not depend on it ([`Needs`]). `Some`
/// says whether it now runs later than before.
fn window(before: &Tree, unit: NodeId, after: &Tree, at: Option<usize>) -> Option<bool> {
    let (tb, ta): (Vec<NodeId>, Vec<NodeId>) = (
        members(before, Tree::ROOT).collect(),
        members(after, Tree::ROOT).collect(),
    );
    let top = std::iter::once(unit)
        .chain(before.ancestors(unit))
        .find(|a| tb.contains(a))?;
    let t = tb.iter().position(|&n| n == top)?;
    let matched = lcs(
        tb.len(),
        ta.len(),
        |i, j| key(before, tb[i]) == key(after, ta[j]),
        |_| 1,
    );
    let r = at.unwrap_or(ta.len());
    // Where the unit's top-level statement now stands: kept, or for a unit
    // at the top level itself, the gap it left.
    let (later, span) = if top == unit {
        let slot = matched
            .iter()
            .filter(|(i, _)| *i < t)
            .map(|(_, j)| j + 1)
            .max()
            .unwrap_or(0);
        (r >= slot, if r >= slot { slot..r } else { r + 1..slot })
    } else {
        let &(_, j) = matched.iter().find(|(i, _)| *i == t)?;
        (r > j, if r > j { j + 1..r } else { r + 1..j })
    };
    let needs = Needs::of(before, unit);
    if ta[span].iter().any(|&w| needs.by(after, w)) {
        return None;
    }
    // Inside the top-level statement, the unit's siblings past it (or
    // before it) in each statement list up to it.
    let mut x = unit;
    while x != top {
        let parent = before.node(x).parent?;
        if transparent(before, parent) {
            let siblings: Vec<NodeId> = before.code_children(parent).collect();
            let at = siblings.iter().position(|&s| s == x)?;
            let side = if later {
                &siblings[at + 1..]
            } else {
                &siblings[..at]
            };
            if side.iter().any(|&w| needs.by(before, w)) {
                return None;
            }
        }
        x = parent;
    }
    Some(later)
}

fn namespace(tree: &Tree, node: NodeId) -> bool {
    matches!(
        tree.node(node).role,
        Role::Definition(NameKind::Class | NameKind::Module)
    )
}

/// A container's statements, through nodes with no syntax of their own:
/// a namespace's body, a file's top level.
fn members(tree: &Tree, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    let mut stack: Vec<NodeId> = tree
        .code_children(node)
        .filter(|&c| transparent(tree, c))
        .collect();
    if node == Tree::ROOT || transparent(tree, node) {
        stack = vec![node];
    }
    let mut out = Vec::new();
    while let Some(id) = stack.pop() {
        for c in tree.code_children(id) {
            if transparent(tree, c) {
                stack.push(c);
            } else {
                out.push(c);
            }
        }
    }
    out.sort_unstable_by_key(|&id| tree.node(id).span.start);
    out.into_iter()
}

/// A namespace's header: its code beside the body (the name, a superclass,
/// a Rust attribute), as hashes.
fn header(tree: &Tree, node: NodeId) -> Vec<u64> {
    let code = tree.code_children(node).filter(|&c| !transparent(tree, c));
    code.map(|c| tree.node(c).hash).collect()
}

/// The namespaces enclosing `node`, outermost first.
fn owners(tree: &Tree, node: NodeId) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = tree
        .ancestors(node)
        .filter(|&a| namespace(tree, a))
        .collect();
    out.reverse();
    out
}

/// What the code around each pair means in its new place beyond
/// [`check`]: the namespaces opened around it, and for a unit moved into a
/// file the change adds, when that file runs. Adds what differs to the
/// pairs' context, and returns the lines their parts hold beyond the units.
fn surroundings(
    change: &Change,
    outside: &dyn Outside,
    units: &[Unit],
    wrappers: &[Wrapper],
    by_unit: &HashMap<usize, usize>,
    pairs: &mut [Pair],
) -> Vec<Extra> {
    let tree = |file: usize, side: Side| {
        let trees = change.files[file].trees.as_ref().expect("parsed");
        match side {
            Side::Removed => &trees.before,
            Side::Added => &trees.after,
        }
    };
    let added = |file: usize| change.files[file].diff.old_path.is_none();
    let deleted = |file: usize| change.files[file].diff.new_path.is_none();
    let live: Vec<usize> = {
        let mut live: Vec<usize> = by_unit.values().copied().collect();
        live.sort_unstable();
        live.dedup();
        live
    };
    // A wrapper is explained when every member moved: a paired unit, or an
    // explained wrapper.
    let mut explained = vec![false; wrappers.len()];
    for w in (0..wrappers.len()).rev() {
        let wr = &wrappers[w];
        let t = tree(wr.file, wr.side);
        explained[w] = members(t, wr.node).all(|m| {
            let unit = units
                .iter()
                .position(|u| u.wrapper == Some(w) && u.node == m);
            let inner = (0..wrappers.len())
                .find(|&i| wrappers[i].parent == Some(w) && wrappers[i].node == m);
            unit.is_some_and(|u| by_unit.contains_key(&u)) || inner.is_some_and(|i| explained[i])
        });
    }
    let chain = |unit: &Unit| -> Vec<usize> {
        let mut out = Vec::new();
        let mut at = unit.wrapper;
        while let Some(w) = at {
            out.push(w);
            at = wrappers[w].parent;
        }
        out
    };

    let mut loading = Loading::new(change, outside);
    let mut extras = Vec::new();
    // Pairs into each file the change adds, from another file, by (from, to).
    let mut into: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for &p in &live {
        let (from, to) = (&units[pairs[p].from], &units[pairs[p].to]);
        let mut needs_after = false;
        let mut context = Vec::new();
        if !pairs[p].context.contains(&Context::Owner) {
            for (unit, other) in [(from, to), (to, from)] {
                let (t, o) = (unit.tree(change), other.tree(change));
                let (mine, theirs) = (owners(t, unit.node), owners(o, other.node));
                for w in chain(unit) {
                    let wr = &wrappers[w];
                    let Some(d) = mine.iter().position(|&n| n == wr.node) else {
                        continue;
                    };
                    let Some(&ns) = theirs.get(d) else {
                        context.push(Context::Owner);
                        continue;
                    };
                    let (hw, ho) = (header(t, wr.node), header(o, ns));
                    let same_kind = t.node(wr.node).role == o.node(ns).role;
                    match wr.side {
                        _ if !same_kind => context.push(Context::Owner),
                        // A namespace no longer declared here: what it said
                        // must still hold where the code went.
                        Side::Removed if hw != ho => context.push(Context::Owner),
                        Side::Removed => {}
                        // A name alone reopens or creates the namespace in
                        // either order; anything more (a superclass) holds
                        // only once the old file has declared it.
                        Side::Added if hw == ho && hw.len() == 1 => {}
                        Side::Added if hw == ho || hw.len() == 1 => needs_after = true,
                        Side::Added => context.push(Context::Owner),
                    }
                }
            }
        }
        let new_file = added(to.file)
            && from.file != to.file
            && !tree(to.file, Side::Added).semantics.file_scoped;
        if new_file {
            into.entry((from.file, to.file)).or_default().push(p);
            match loading.runs(from, to) {
                Some((after, alone)) if after || !needs_after => {
                    let old = change.files[from.file].diff.path();
                    pairs[p].public_api = alone && outside.library(old);
                }
                _ => context.push(Context::LoadOrder),
            }
        } else if needs_after {
            context.push(Context::LoadOrder);
        }
        let pair = &mut pairs[p];
        pair.context.extend(context);
        pair.context.sort_unstable();
        pair.context.dedup();
    }

    let pairs_under = |w: usize| -> Vec<usize> {
        let mut out: Vec<usize> = (0..units.len())
            .filter(|&u| chain(&units[u]).contains(&w))
            .filter_map(|u| by_unit.get(&u).copied())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    };
    for (w, wr) in wrappers.iter().enumerate() {
        if explained[w] {
            let lines = tree(wr.file, wr.side).lines(wr.node);
            extras.push(Extra {
                file: wr.file,
                side: wr.side,
                lines,
                pairs: pairs_under(w),
            });
        }
    }
    // A file the change adds or deletes whose every statement moved: its
    // directives move with them.
    for (f, file) in change.files.iter().enumerate() {
        let side = match (added(f), deleted(f)) {
            (true, _) => Side::Added,
            (_, true) => Side::Removed,
            _ => continue,
        };
        if file.trees.as_ref().is_ok_and(|t| !t.clean()) || file.trees.is_err() {
            continue;
        }
        let t = tree(f, side);
        let mut moved = Vec::new();
        let mut directives = Vec::new();
        let whole = members(t, Tree::ROOT).all(|m| {
            if t.node(m).flags.contains(Flags::DIRECTIVE) {
                directives.push(m);
                return true;
            }
            let unit = units
                .iter()
                .position(|u| u.file == f && u.side == side && u.wrapper.is_none() && u.node == m);
            let wrapper = wrappers
                .iter()
                .position(|w| w.file == f && w.side == side && w.parent.is_none() && w.node == m);
            if let Some(p) = unit.and_then(|u| by_unit.get(&u)) {
                moved.push(*p);
                true
            } else if let Some(w) = wrapper.filter(|&w| explained[w]) {
                moved.extend(pairs_under(w));
                true
            } else {
                false
            }
        });
        if !whole || moved.is_empty() {
            continue;
        }
        moved.sort_unstable();
        moved.dedup();
        for d in directives {
            extras.push(Extra {
                file: f,
                side,
                lines: t.lines(d),
                pairs: moved.clone(),
            });
        }
        // The statements that load a new file whose every pair is proven
        // move with it.
        if side == Side::Added {
            let into_here: Vec<usize> = into
                .iter()
                .filter(|((_, to), _)| *to == f)
                .flat_map(|(_, ps)| ps.clone())
                .collect();
            let proven = into_here.len() == moved.len()
                && moved.iter().all(|&p| pairs[p].context.is_empty());
            if proven {
                let froms: Vec<usize> = into
                    .keys()
                    .filter(|(_, to)| *to == f)
                    .map(|(from, _)| *from)
                    .collect();
                for from in froms {
                    for (file, stmt) in loading.loaders(from, f) {
                        let lines = tree(file, Side::Added).lines(stmt);
                        extras.push(Extra {
                            file,
                            side: Side::Added,
                            lines,
                            pairs: moved.clone(),
                        });
                    }
                }
            }
        }
    }
    extras
}

impl Unit {
    fn trees<'c>(&self, change: &'c Change) -> &'c Trees {
        change.files[self.file]
            .trees
            .as_ref()
            .expect("units come from parsed files")
    }

    fn tree<'c>(&self, change: &'c Change) -> &'c Tree {
        let trees = self.trees(change);
        match self.side {
            Side::Removed => &trees.before,
            Side::Added => &trees.after,
        }
    }

    /// The hunk whose changed lines on this unit's side first touch it.
    fn hunk(&self, change: &Change) -> Option<usize> {
        let range = self.tree(change).lines(self.node);
        change.files[self.file].diff.hunks.iter().position(|h| {
            let changed = match self.side {
                Side::Removed => &h.removed,
                Side::Added => &h.added,
            };
            changed.iter().any(|l| range.contains(l))
        })
    }
}

fn is_unit(tree: &Tree, node: NodeId) -> bool {
    tree.node(node).role.is_declaration()
        && size(tree, node) >= FLOOR
        && owner(tree, node).is_some()
}

/// The namespaces enclosing `node`, outermost first, each as its name's
/// subtree hash. `None` when some ancestor is neither a namespace nor
/// transparent: a block, a conditional, a call. A declaration there runs
/// under that construct, so moving it out or in is not a reorder.
fn owner(tree: &Tree, node: NodeId) -> Option<Vec<u64>> {
    let mut chain = Vec::new();
    let mut at = tree.node(node).parent;
    while let Some(id) = at {
        let n = tree.node(id);
        if matches!(n.role, Role::Definition(NameKind::Class | NameKind::Module)) {
            chain.push(namespace_name(tree, id, true)?);
        } else if !transparent(tree, id) {
            return None;
        }
        at = n.parent;
    }
    chain.reverse();
    Some(chain)
}

/// What differs around a moved unit in its new place, apart from any change
/// to its own body; for a reorder, what observes the new order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Context {
    /// Each file is a module (Rust): another file is another module.
    Module,
    /// Another enclosing class, module or `impl`.
    Owner,
    /// Another visibility section (it crossed `private`).
    Visibility,
    /// Another order relative to a sibling that names it, or that it names,
    /// at load time.
    LoadOrder,
    /// Files with different file-level directives.
    Directives,
    /// The code reads its own line (`__LINE__`), or its own file
    /// (`__FILE__`) and moved to another file.
    Position,
    /// Reordered entries of a collection whose iteration sees insertion order.
    IterationOrder,
    /// Reordered elements whose positions are part of the value.
    ElementOrder,
    /// Reordered code that runs in written order.
    EvaluationOrder,
}

impl Context {
    /// The contexts a `move` can have.
    pub const MOVE: [Context; 6] = [
        Context::Module,
        Context::Owner,
        Context::Visibility,
        Context::LoadOrder,
        Context::Directives,
        Context::Position,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Context::Module => "module",
            Context::Owner => "owner",
            Context::Visibility => "visibility",
            Context::LoadOrder => "load_order",
            Context::Directives => "directives",
            Context::Position => "position",
            Context::IterationOrder => "iteration_order",
            Context::ElementOrder => "element_order",
            Context::EvaluationOrder => "evaluation_order",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Context::Module => "its file is its module",
            Context::Owner => "its owner (class or module) changes",
            Context::Visibility => "its visibility section changes",
            Context::LoadOrder => {
                "it moves past a statement that may read it, or that it reads, at load time"
            }
            Context::Directives => "the files' file-level directives differ",
            Context::Position => "it reads its own line or file",
            Context::IterationOrder => "iteration sees insertion order",
            Context::ElementOrder => "positions are part of the value",
            Context::EvaluationOrder => "the code runs in another order",
        }
    }
}

/// A matched removal and addition.
struct Pair {
    from: usize,
    to: usize,
    similarity: f64,
    /// Units on the other side that matched equally well.
    candidates: usize,
    /// The smaller unit's node count.
    nodes: usize,
    /// What differs around the unit in its new place.
    context: Vec<Context>,
    /// Moved into a new file loaded after the old one by other files, from
    /// library code another package may load alone.
    public_api: bool,
}

impl Pair {
    fn confidence(&self) -> f64 {
        self.similarity * (1.0 - 1.0 / self.nodes as f64) / self.candidates as f64
    }
}

/// Exact matches first, then near matches among what's left. Ties go to the
/// pairing whose context checks pass, then to the same file.
fn pair(change: &Change, units: &[Unit]) -> Vec<Pair> {
    let removed: Vec<usize> = (0..units.len())
        .filter(|&u| units[u].side == Side::Removed)
        .collect();
    let added: Vec<usize> = (0..units.len())
        .filter(|&u| units[u].side == Side::Added)
        .collect();
    let hash = |u: usize| units[u].tree(change).node(units[u].node).hash;
    let nodes = |u: usize| size(units[u].tree(change), units[u].node);

    let mut pairs = Vec::new();
    let mut taken = vec![false; units.len()];

    let mut exact = Vec::new();
    for &r in &removed {
        let partners: Vec<usize> = added
            .iter()
            .copied()
            .filter(|&a| hash(a) == hash(r))
            .collect();
        for &a in &partners {
            let rivals = removed.iter().filter(|&&o| hash(o) == hash(r)).count();
            exact.push((r, a, 1.0, partners.len().max(rivals)));
        }
    }
    claim(change, units, exact, &mut taken, &mut pairs, nodes);

    let similar = |r: usize, a: usize| -> Option<f64> {
        let (ur, ua) = (&units[r], &units[a]);
        let (nr, na) = (ur.tree(change).node(ur.node), ua.tree(change).node(ua.node));
        let same = nr.role == na.role && nr.name.is_some() && nr.name == na.name;
        if !same {
            return None;
        }
        // Not identical (the exact pass took those), so never rounded up to
        // 1.0: that reads as unchanged and skips the changed lines.
        let sim = rounded(dice(
            subtree_hashes(ur.tree(change), &[ur.node]),
            subtree_hashes(ua.tree(change), &[ua.node]),
        ))
        .min(0.99);
        (sim >= NEAR).then_some(sim)
    };
    let free_r: Vec<usize> = removed.iter().copied().filter(|&u| !taken[u]).collect();
    let free_a: Vec<usize> = added.iter().copied().filter(|&u| !taken[u]).collect();
    let mut near = Vec::new();
    for &r in &free_r {
        let partners: Vec<(usize, f64)> = free_a
            .iter()
            .filter_map(|&a| similar(r, a).map(|s| (a, s)))
            .collect();
        for &(a, sim) in &partners {
            let rivals = free_r.iter().filter(|&&o| similar(o, a).is_some()).count();
            near.push((r, a, sim, partners.len().max(rivals)));
        }
    }
    claim(change, units, near, &mut taken, &mut pairs, nodes);
    pairs
}

type Candidate = (usize, usize, f64, usize);

fn claim(
    change: &Change,
    units: &[Unit],
    candidates: Vec<Candidate>,
    taken: &mut [bool],
    pairs: &mut Vec<Pair>,
    nodes: impl Fn(usize) -> usize,
) {
    let mut ranked: Vec<(Candidate, Vec<Context>)> = candidates
        .into_iter()
        .map(|c| (c, check(change, &units[c.0], &units[c.1])))
        .collect();
    ranked.sort_by(|((r1, a1, s1, _), w1), ((r2, a2, s2, _), w2)| {
        let same_file = |r: &usize, a: &usize| units[*r].file == units[*a].file;
        s2.total_cmp(s1)
            .then((!w1.is_empty()).cmp(&!w2.is_empty()))
            .then(same_file(r2, a2).cmp(&same_file(r1, a1)))
    });
    for ((from, to, similarity, candidates), context) in ranked {
        if taken[from] || taken[to] {
            continue;
        }
        taken[from] = true;
        taken[to] = true;
        pairs.push(Pair {
            from,
            to,
            similarity,
            candidates,
            nodes: nodes(from).min(nodes(to)),
            context,
            public_api: false,
        });
    }
}

/// What differs around the unit in its new place.
fn check(change: &Change, from: &Unit, to: &Unit) -> Vec<Context> {
    let (tb, ta) = (from.tree(change), to.tree(change));
    let across = from.file != to.file;
    let into_new_file =
        across && change.files[to.file].diff.old_path.is_none() && !ta.semantics.file_scoped;
    // A visibility section applies to methods only.
    let method = tb.node(from.node).role == Role::Definition(NameKind::Method);
    [
        (
            Context::Module,
            across && (tb.semantics.file_scoped || ta.semantics.file_scoped),
        ),
        (Context::Owner, owner(tb, from.node) != owner(ta, to.node)),
        (
            Context::Visibility,
            method && visibility(tb, from.node) != visibility(ta, to.node),
        ),
        (
            Context::LoadOrder,
            // Into a file the change adds, where its old siblings now run is
            // `surroundings`' window; here only the new file's own.
            match into_new_file {
                true => !dependencies(ta, to.node).is_empty(),
                false => dependencies(tb, from.node) != dependencies(ta, to.node),
            },
        ),
        (
            Context::Directives,
            across && directives(tb) != directives(ta),
        ),
        (Context::Position, positional(tb, from.node, across)),
    ]
    .into_iter()
    .filter_map(|(context, differs)| differs.then_some(context))
    .collect()
}

/// Whether the unit reads its own line, or its own file when it changed files.
fn positional(tree: &Tree, node: NodeId, across: bool) -> bool {
    let op = if across {
        Op::MoveAcrossFiles
    } else {
        Op::Move
    };
    tree.descendants(node)
        .any(|d| tree.node(d).role.hazard().is_some_and(|h| blocks(op, h)))
}

/// The section a declaration sits in: the last bare visibility statement
/// (`private` with no arguments) before it among its siblings.
pub(super) fn visibility(tree: &Tree, node: NodeId) -> Option<Visibility> {
    let parent = tree.node(tree.node(node).parent?);
    parent
        .children
        .iter()
        .take_while(|&&c| c != node)
        .filter_map(|&c| match tree.node(c).role {
            Role::Visibility(v) if tree.node(c).children.iter().all(|&g| tree.node(g).comment) => {
                Some(v)
            }
            _ => None,
        })
        .last()
}

/// Siblings whose order relative to `node` matters, as (hash, comes first),
/// sorted: those that name what it declares at load time (`alias_method`,
/// `private :x`, a constant read), that declare the same name, or that
/// declare a name its own load-time code reads. For anything but a method,
/// whose body runs later, every statement that runs code counts (not a
/// declaration or a bare `private`): a `require` or a call may read it. So
/// does it for a method where definitions run ([`crate::tree::Semantics::definitions_run`]),
/// as does a sibling defining a definition hook (`method_added`).
fn dependencies(tree: &Tree, node: NodeId) -> Vec<(u64, bool)> {
    let Some(parent) = tree.node(node).parent else {
        return Vec::new();
    };
    let needs = Needs::of(tree, node);
    let mut before = true;
    let mut out = Vec::new();
    for &sibling in &tree.node(parent).children {
        if sibling == node {
            before = false;
            continue;
        }
        if tree.node(sibling).comment {
            continue;
        }
        if needs.by(tree, sibling) {
            out.push((tree.node(sibling).hash, before));
        }
    }
    out.sort_unstable();
    out
}

/// What a unit needs of the statements around it, to tell which depend on
/// it ([`dependencies`]).
struct Needs<'t> {
    tree: &'t Tree,
    node: NodeId,
    declared: Option<&'t str>,
    reads: Vec<&'t str>,
    /// A method body runs later; anything else runs where it stands, so a
    /// statement beside it may need it (or change it) by the time it runs.
    runs_now: bool,
}

impl<'t> Needs<'t> {
    fn of(tree: &'t Tree, node: NodeId) -> Self {
        Needs {
            tree,
            node,
            declared: declared_name(tree, node),
            reads: mentions(tree, node),
            runs_now: tree.node(node).role != Role::Definition(NameKind::Method),
        }
    }

    /// Whether statement `other`, of `tree`, depends on the unit or it on
    /// `other`.
    fn by(&self, tree: &Tree, other: NodeId) -> bool {
        let theirs = declared_name(tree, other);
        let semantics = &tree.semantics;
        let hook =
            |name: Option<&str>| name.is_some_and(|n| semantics.definition_hooks.contains(&n));
        // A hook sees the method definitions after it, either way round.
        let other_method = tree.node(other).role == Role::Definition(NameKind::Method);
        ((self.runs_now || semantics.definitions_run) && !inert(tree, other))
            || (hook(theirs) && !self.runs_now)
            || expands(tree, other)
            || (hook(self.declared) && other_method)
            || self.declared.is_some_and(|d| {
                theirs == Some(d) || mentions(tree, other).iter().any(|m| spells(m, d))
            })
            || theirs.is_some_and(|t| self.reads.contains(&t))
            || macro_name(tree, other).is_some_and(|m| invokes(self.tree, self.node, m))
    }
}

/// A statement that runs no code another could depend on: a declaration
/// (but not a constant whose value calls something, as `X = Counter.next`
/// does), or a bare visibility statement (`private`), which only marks a
/// section.
fn inert(tree: &Tree, node: NodeId) -> bool {
    let n = tree.node(node);
    let calls = || {
        tree.descendants(node)
            .any(|d| matches!(tree.node(d).role, Role::Call(_) | Role::Hazard(_)))
    };
    if n.flags.contains(Flags::DIRECTIVE) {
        return true;
    }
    match n.role {
        Role::Definition(NameKind::Constant) => !calls(),
        Role::Visibility(_) => n.children.iter().all(|&c| tree.node(c).comment),
        role => role.is_declaration(),
    }
}

/// A macro invocation, or an item an attribute macro rewrites: it may
/// expand to anything, depending on what is beside it.
fn expands(tree: &Tree, node: NodeId) -> bool {
    let n = tree.node(node);
    matches!(
        n.role,
        Role::Reference(NameKind::Macro) | Role::Hazard(Hazard::Macro)
    ) || n.flags.contains(Flags::SPELLS)
}

/// The macro a sibling defines (`macro_rules!`): an invocation resolves to
/// the definition textually before it, function bodies included.
fn macro_name(tree: &Tree, node: NodeId) -> Option<&str> {
    let n = tree.node(node);
    n.name
        .as_deref()
        .filter(|_| n.role == Role::Definition(NameKind::Macro))
}

/// Whether the subtree invokes macro `name`, or may in a token tree.
fn invokes(tree: &Tree, node: NodeId, name: &str) -> bool {
    tree.descendants(node).any(|d| {
        let n = tree.node(d);
        n.name.as_deref() == Some(name)
            && matches!(
                n.role,
                Role::Reference(NameKind::Macro) | Role::Hazard(Hazard::Macro) | Role::Opaque
            )
    })
}

fn declared_name(tree: &Tree, node: NodeId) -> Option<&str> {
    let n = tree.node(node);
    n.role.defines().and(n.name.as_deref())
}

/// Names the subtree uses when it runs: references, calls, dynamic names,
/// and the text of any literal leaf (`:x` in `memoize :x`). Method bodies
/// run later, so they're skipped.
fn mentions(tree: &Tree, node: NodeId) -> Vec<&str> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(id) = stack.pop() {
        let n = tree.node(id);
        if n.comment || n.role == Role::Definition(NameKind::Method) {
            continue;
        }
        match n.role {
            Role::Reference(_) | Role::Call(_) | Role::DynamicName => {
                out.extend(n.name.as_deref());
            }
            _ => {}
        }
        if n.children.is_empty() {
            let text = std::str::from_utf8(tree.text(&n.span)).unwrap_or_default();
            out.push(tree.semantics.spelling.word(text));
        }
        stack.extend(&n.children);
    }
    out
}

/// File-level directives (a magic comment) apply to everything in the file.
fn directives(tree: &Tree) -> Vec<u64> {
    let mut out: Vec<u64> = tree
        .node(Tree::ROOT)
        .children
        .iter()
        .map(|&c| tree.node(c))
        .filter(|n| n.flags.contains(Flags::DIRECTIVE))
        .map(|n| n.hash)
        .collect();
    out.sort_unstable();
    out
}

/// One pair touching a hunk, as `detail.moves` lists it.
#[derive(Serialize)]
struct Entry {
    name: Option<String>,
    from_hunk: Option<usize>,
    to_hunk: Option<usize>,
    from_line: Option<usize>,
    to_line: Option<usize>,
    #[serde(serialize_with = "context_names")]
    context: Vec<Context>,
    similarity: f64,
    changed_before: Vec<usize>,
    changed_after: Vec<usize>,
}

fn context_names<S: serde::Serializer>(context: &[Context], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(context.iter().map(|c| c.as_str()))
}

fn entry(change: &Change, units: &[Unit], pair: &Pair) -> Entry {
    let (from, to) = (&units[pair.from], &units[pair.to]);
    let id = |u: &Unit| u.hunk(change).map(|h| change.files[u.file].ids[h]);
    let first = |u: &Unit| {
        let tree = u.tree(change);
        tree.lines(u.node).find(|&l| tree.line(l).code)
    };
    let (changed_before, changed_after) = if pair.similarity < 1.0 {
        changed_lines(from.tree(change), from.node, to.tree(change), to.node)
    } else {
        (Vec::new(), Vec::new())
    };
    Entry {
        name: from
            .tree(change)
            .node(from.node)
            .name
            .as_deref()
            .map(str::to_string),
        from_hunk: id(from),
        to_hunk: id(to),
        from_line: first(from),
        to_line: first(to),
        context: pair.context.clone(),
        similarity: pair.similarity,
        changed_before,
        changed_after,
    }
}

/// Lines of each unit, whitespace aside, that the other doesn't share: a
/// longest common subsequence over the two units' non-blank lines.
fn changed_lines(
    before: &Tree,
    from: NodeId,
    after: &Tree,
    to: NodeId,
) -> (Vec<usize>, Vec<usize>) {
    let texts = |tree: &Tree, node: NodeId| -> Vec<(usize, Vec<u8>)> {
        tree.lines(node)
            .map(|l| {
                let text: Vec<u8> = tree
                    .text(&tree.line_span(l, l))
                    .iter()
                    .copied()
                    .filter(|b| !b.is_ascii_whitespace())
                    .collect();
                (l, text)
            })
            .filter(|(_, t)| !t.is_empty())
            .collect()
    };
    let (a, b) = (texts(before, from), texts(after, to));
    let (ta, tb): (Vec<&[u8]>, Vec<&[u8]>) = (
        a.iter().map(|(_, t)| t.as_slice()).collect(),
        b.iter().map(|(_, t)| t.as_slice()).collect(),
    );
    let common = lcs(ta.len(), tb.len(), |i, j| ta[i] == tb[j], |_| 1);
    let only = |side: &[(usize, Vec<u8>)], pick: fn(&(usize, usize)) -> usize| -> Vec<usize> {
        side.iter()
            .enumerate()
            .filter(|(i, _)| !common.iter().any(|c| pick(c) == *i))
            .map(|(_, (line, _))| *line)
            .collect()
    };
    (only(&a, |c| c.0), only(&b, |c| c.1))
}
