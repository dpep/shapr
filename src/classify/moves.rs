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

mod crossing;
mod loading;

use super::align::{Alignments, Side, lcs, namespace_name, owns, qualifiers, size, transparent};
use super::format::{dice, subtree_hashes};
use super::hazard::{Op, blocks};
use super::{Assumption, Claim, Confidence, Kind, Labels, Outside, Part, rounded, site};
use crate::change::{Change, Trees};
use crate::tree::{Flags, NameKind, NodeId, Role, Tree, Visibility};
use crossing::dependencies;
use loading::Loading;
use serde::Serialize;
use serde_json::{Map, json};
use std::collections::HashMap;
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
/// a Rust attribute) and its qualifiers (an inner attribute), as hashes.
fn header(tree: &Tree, node: NodeId) -> Vec<u64> {
    let code = tree.code_children(node).filter(|&c| !transparent(tree, c));
    code.chain(qualifiers(tree, node))
        .map(|c| tree.node(c).hash)
        .collect()
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
            if proven && into.keys().any(|(_, to)| *to == f) {
                for (file, stmt) in loading.loaders(f) {
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

/// The namespaces enclosing `node` (singleton classes too), outermost
/// first, each as its name's subtree hash. `None` when some ancestor is
/// neither a namespace nor transparent: a block, a conditional, a call. A declaration there runs
/// under that construct, so moving it out or in is not a reorder.
fn owner(tree: &Tree, node: NodeId) -> Option<Vec<u64>> {
    let mut chain = Vec::new();
    let mut at = tree.node(node).parent;
    while let Some(id) = at {
        let n = tree.node(id);
        if owns(tree, id) {
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
    /// An extract's or inline's caller changed beyond the call.
    Caller,
    /// An extracted or inlined method's body differs from the statements.
    Body,
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
            Context::Caller => "caller",
            Context::Body => "body",
        }
    }

    pub(super) fn reason(self) -> &'static str {
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
            Context::Caller => "the caller changed beyond the call",
            Context::Body => "the method's body differs from the statements",
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
