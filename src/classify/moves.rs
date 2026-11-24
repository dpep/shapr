//! `move` and `move_with_changes`: a declaration that leaves one place and
//! reappears in another, in the same file or another.
//!
//! Each file's two trees are aligned container by container (the file, then
//! each class or module body): a longest common subsequence over the
//! children, keyed by hash, with namespaces keyed by name so their bodies
//! align in turn. A declaration (`Role::is_declaration`) left out of the
//! alignment is a *unit*, removed or added. A removed unit pairs with an
//! added one by equal hash (`move`) or by name and subtree similarity
//! (`move_with_changes`). A pair is a move only when the code around it means
//! the same in both places: the same owner, the same visibility section, the
//! same order relative to statements that name it at load time and, across
//! files, the same file-level directives. Otherwise both halves stay `logic`.
//!
//! A hunk is labeled from the trees, not from git's line placement: it is a
//! move when every changed code line lies in a paired unit or in a node the
//! alignment kept unchanged (git may slide a removal across identical lines).

use super::format::{dice, subtree_hashes};
use super::{Confidence, Kind, Labels};
use crate::change::{Change, Trees};
use crate::tree::{NameKind, NodeId, Role, Tree, Visibility};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::ops::RangeInclusive;

/// Fewest nodes a unit needs. Size evidence is `1 - 1/nodes`: one node is
/// no evidence, two a coin flip, so three is the first size whose match
/// beats chance. It keeps out a bare name or a constant set to one literal.
const FLOOR: usize = 3;

/// Least subtree similarity for a near match: at least half the subtrees
/// shared, so the bodies are more alike than not.
const NEAR: f64 = 0.5;

pub(super) fn run(change: &Change, labels: &mut Labels) {
    let mut units = Vec::new();
    let mut kept: HashMap<(usize, Side), Vec<RangeInclusive<usize>>> = HashMap::new();
    for (f, file) in change.files.iter().enumerate() {
        let Ok(trees) = &file.trees else { continue };
        if !trees.clean() || labels[f].iter().all(|l| l.kind != Kind::Logic) {
            continue;
        }
        let mut aligner = Aligner {
            file: f,
            trees,
            units: &mut units,
            kept: Vec::new(),
        };
        aligner.align(Tree::ROOT, Tree::ROOT);
        let (before, after): (Vec<_>, Vec<_>) = aligner
            .kept
            .into_iter()
            .partition(|(s, _)| *s == Side::Removed);
        kept.insert(
            (f, Side::Removed),
            before.into_iter().map(|(_, r)| r).collect(),
        );
        kept.insert(
            (f, Side::Added),
            after.into_iter().map(|(_, r)| r).collect(),
        );
    }
    let pairs = pair(change, &units);
    let ids = hunk_ids(change);
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

    for (f, file) in change.files.iter().enumerate() {
        let Ok(trees) = &file.trees else { continue };
        for (h, hunk) in file.diff.hunks.iter().enumerate() {
            if labels[f][h].kind != Kind::Logic || !kept.contains_key(&(f, Side::Removed)) {
                continue;
            }
            let sides = [
                (Side::Removed, &trees.before, &hunk.removed),
                (Side::Added, &trees.after, &hunk.added),
            ];
            let mut mine = Vec::new();
            let mut explained = true;
            for (side, tree, changed) in sides {
                let code: Vec<usize> = changed
                    .iter()
                    .copied()
                    .filter(|&l| tree.line(l).code)
                    .collect();
                let here: Vec<(usize, RangeInclusive<usize>)> = (0..units.len())
                    .filter(|&u| units[u].file == f && units[u].side == side)
                    .map(|u| (u, lines(tree, units[u].node)))
                    .filter(|(_, r)| code.iter().any(|l| r.contains(l)))
                    .collect();
                mine.extend(here.iter().filter_map(|(u, _)| by_unit.get(u).copied()));
                let ranges = &kept[&(f, side)];
                explained &= code.iter().all(|l| {
                    ranges.iter().any(|r| r.contains(l))
                        || here
                            .iter()
                            .any(|(u, r)| r.contains(l) && by_unit.contains_key(u))
                });
            }
            if !explained || mine.is_empty() {
                continue;
            }
            mine.sort_unstable();
            mine.dedup();
            let mine: Vec<&Pair> = mine.iter().map(|&p| &pairs[p]).collect();
            let label = &mut labels[f][h];
            if let Some(why) = mine.iter().find_map(|p| p.rejected.as_deref()) {
                label.reason = format!("moved, but {why}");
                continue;
            }
            let exact = mine.iter().all(|p| p.similarity == 1.0);
            label.kind = if exact {
                Kind::Move
            } else {
                Kind::MoveWithChanges
            };
            label.reason = if exact {
                "declarations moved unchanged".into()
            } else {
                "declarations moved, and their bodies changed".into()
            };
            label.confidence = Confidence::new(
                mine.iter()
                    .map(|p| p.confidence())
                    .fold(f64::INFINITY, f64::min),
            );
            let values = &mut label.evidence.values;
            values.clear();
            let similarity = mine.iter().map(|p| p.similarity).fold(1.0, f64::min);
            values.insert("similarity".into(), json!(similarity));
            values.insert("nodes".into(), json!(mine.iter().map(|p| p.nodes).min()));
            values.insert(
                "candidates".into(),
                json!(mine.iter().map(|p| p.candidates).max()),
            );
            let entries: Vec<Value> = mine
                .iter()
                .map(|p| entry(change, &units, &ids, p))
                .collect();
            label.detail = Map::from_iter([("moves".to_string(), Value::from(entries))]);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Side {
    Removed,
    Added,
}

/// A declaration the alignment left out: removed from the before tree or
/// added to the after tree.
struct Unit {
    file: usize,
    side: Side,
    node: NodeId,
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
        let range = lines(self.tree(change), self.node);
        change.files[self.file].diff.hunks.iter().position(|h| {
            let changed = match self.side {
                Side::Removed => &h.removed,
                Side::Added => &h.added,
            };
            changed.iter().any(|l| range.contains(l))
        })
    }
}

fn lines(tree: &Tree, node: NodeId) -> RangeInclusive<usize> {
    let extent = &tree.node(node).extent;
    tree.line_of(extent.start)..=tree.line_of(extent.end.saturating_sub(1).max(extent.start))
}

/// How a child lines up with its counterpart on the other side.
#[derive(PartialEq, Eq)]
enum Key {
    /// A namespace, by its name: its body is aligned in turn.
    Namespace(u64),
    /// A node with no syntax of its own (a statement list): aligned in turn.
    Transparent,
    Code(u64),
}

struct Aligner<'a> {
    file: usize,
    trees: &'a Trees,
    units: &'a mut Vec<Unit>,
    /// Line ranges of nodes left unchanged, per side.
    kept: Vec<(Side, RangeInclusive<usize>)>,
}

impl Aligner<'_> {
    fn align(&mut self, before: NodeId, after: NodeId) {
        let (tb, ta) = (&self.trees.before, &self.trees.after);
        let kids = |tree: &Tree, id: NodeId| -> Vec<NodeId> {
            tree.node(id)
                .children
                .iter()
                .copied()
                .filter(|&c| !tree.node(c).comment)
                .collect()
        };
        let (b, a) = (kids(tb, before), kids(ta, after));
        let (kb, ka): (Vec<Key>, Vec<Key>) = (
            b.iter().map(|&n| key(tb, n)).collect(),
            a.iter().map(|&n| key(ta, n)).collect(),
        );
        let weight: Vec<usize> = b.iter().map(|&n| size(tb, n)).collect();
        let matched = lcs(&kb, &ka, &weight);
        let mut gap_b = vec![0; b.len()];
        let mut gap_a = vec![0; a.len()];
        let (mut in_b, mut in_a) = (vec![false; b.len()], vec![false; a.len()]);
        for (g, &(i, j)) in matched.iter().enumerate() {
            in_b[i] = true;
            in_a[j] = true;
            gap_b.iter_mut().skip(i + 1).for_each(|x| *x = g + 1);
            gap_a.iter_mut().skip(j + 1).for_each(|x| *x = g + 1);
            let (nb, na) = (b[i], a[j]);
            if tb.node(nb).hash == ta.node(na).hash {
                self.kept.push((Side::Removed, lines(tb, nb)));
                self.kept.push((Side::Added, lines(ta, na)));
            } else if kb[i] != Key::Code(tb.node(nb).hash) {
                self.align(nb, na);
            }
        }
        // A declaration replaced by one of the same name between the same
        // anchors was edited in place, not moved.
        let named = |tree: &Tree, n: NodeId| (tree.node(n).role, tree.node(n).name.clone());
        let mut edited_b = vec![false; b.len()];
        let mut edited_a = vec![false; a.len()];
        for i in (0..b.len()).filter(|&i| !in_b[i]) {
            let found = (0..a.len()).find(|&j| {
                !in_a[j]
                    && !edited_a[j]
                    && gap_a[j] == gap_b[i]
                    && tb.node(b[i]).name.is_some()
                    && named(tb, b[i]) == named(ta, a[j])
            });
            if let Some(j) = found {
                edited_b[i] = true;
                edited_a[j] = true;
            }
        }
        for (i, &n) in b.iter().enumerate() {
            if !in_b[i] && !edited_b[i] && is_unit(tb, n) {
                self.units.push(Unit {
                    file: self.file,
                    side: Side::Removed,
                    node: n,
                });
            }
        }
        for (j, &n) in a.iter().enumerate() {
            if !in_a[j] && !edited_a[j] && is_unit(ta, n) {
                self.units.push(Unit {
                    file: self.file,
                    side: Side::Added,
                    node: n,
                });
            }
        }
    }
}

fn key(tree: &Tree, id: NodeId) -> Key {
    let node = tree.node(id);
    if matches!(
        node.role,
        Role::Definition(NameKind::Class | NameKind::Module)
    ) && let Some(name) = namespace_name(tree, id)
    {
        return Key::Namespace(name);
    }
    if transparent(tree, id) {
        return Key::Transparent;
    }
    Key::Code(node.hash)
}

/// The first child is a namespace's name (`A::B`); its hash tells `A::B`
/// from `C::B`.
fn namespace_name(tree: &Tree, id: NodeId) -> Option<u64> {
    let name = tree
        .node(id)
        .children
        .iter()
        .find(|&&c| !tree.node(c).comment)?;
    Some(tree.node(*name).hash)
}

/// Index pairs of the heaviest common subsequence, each element of `a`
/// weighing `weight[i]`. Weighing children by size anchors a big namespace
/// before a run of one-line statements that happens to be longer.
fn lcs<T: PartialEq>(a: &[T], b: &[T], weight: &[usize]) -> Vec<(usize, usize)> {
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            let take = if a[i] == b[j] {
                weight[i] + table[i + 1][j + 1]
            } else {
                0
            };
            table[i][j] = take.max(table[i + 1][j]).max(table[i][j + 1]);
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < a.len() && j < b.len() {
        if a[i] == b[j] && table[i][j] == weight[i] + table[i + 1][j + 1] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

fn is_unit(tree: &Tree, node: NodeId) -> bool {
    tree.node(node).role.is_declaration()
        && size(tree, node) >= FLOOR
        && owner(tree, node).is_some()
}

/// Non-comment nodes in the subtree.
fn size(tree: &Tree, node: NodeId) -> usize {
    tree.descendants(node)
        .filter(|&n| !tree.node(n).comment)
        .count()
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
            chain.push(namespace_name(tree, id)?);
        } else if !transparent(tree, id) {
            return None;
        }
        at = n.parent;
    }
    chain.reverse();
    Some(chain)
}

/// A node that means nothing beyond its children: no role, and no syntax
/// of its own outside them (a statement list, the file root). A role rules
/// it out even when a child's span swallows its keywords, as an implicit
/// `begin` does a `def`'s.
fn transparent(tree: &Tree, id: NodeId) -> bool {
    let node = tree.node(id);
    if node.role != Role::Other || node.children.is_empty() {
        return false;
    }
    !(node.span.start..node.span.end).any(|at| {
        !tree.source[at].is_ascii_whitespace()
            && !node
                .children
                .iter()
                .any(|&c| tree.node(c).extent.contains(&at))
    })
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
    /// Why the code around the unit means something else in its new place.
    rejected: Option<String>,
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
        let sim = round(dice(
            subtree_hashes(ur.tree(change), &[ur.node]),
            subtree_hashes(ua.tree(change), &[ua.node]),
        ));
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
    let mut ranked: Vec<(Candidate, Option<String>)> = candidates
        .into_iter()
        .map(|c| (c, check(change, &units[c.0], &units[c.1])))
        .collect();
    ranked.sort_by(|((r1, a1, s1, _), w1), ((r2, a2, s2, _), w2)| {
        let same_file = |r: &usize, a: &usize| units[*r].file == units[*a].file;
        s2.total_cmp(s1)
            .then(w1.is_some().cmp(&w2.is_some()))
            .then(same_file(r2, a2).cmp(&same_file(r1, a1)))
    });
    for ((from, to, similarity, candidates), rejected) in ranked {
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
            rejected,
        });
    }
}

/// Why the unit means something else in its new place, if it does.
fn check(change: &Change, from: &Unit, to: &Unit) -> Option<String> {
    let (tb, ta) = (from.tree(change), to.tree(change));
    if owner(tb, from.node) != owner(ta, to.node) {
        return Some("its owner (class or module) changes".into());
    }
    let (vb, va) = (visibility(tb, from.node), visibility(ta, to.node));
    if vb != va {
        return Some(format!(
            "its visibility changes from {} to {}",
            visibility_name(vb),
            visibility_name(va)
        ));
    }
    if dependencies(tb, from.node) != dependencies(ta, to.node) {
        return Some(
            "it moves past a statement that names it or that it names at load time".into(),
        );
    }
    if from.file != to.file && directives(tb) != directives(ta) {
        return Some("the files' file-level directives differ".into());
    }
    None
}

/// The section a declaration sits in: the last bare visibility statement
/// (`private` with no arguments) before it among its siblings.
fn visibility(tree: &Tree, node: NodeId) -> Option<Visibility> {
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

fn visibility_name(v: Option<Visibility>) -> &'static str {
    match v {
        None | Some(Visibility::Public) => "public",
        Some(Visibility::Protected) => "protected",
        Some(Visibility::Private) => "private",
        Some(Visibility::ModuleFunction) => "module_function",
    }
}

/// Siblings whose order relative to `node` matters, as (hash, comes first),
/// sorted: those that name what it declares at load time (`alias_method`,
/// `private :x`, a constant read), that declare the same name, or that
/// declare a name its own load-time code reads.
fn dependencies(tree: &Tree, node: NodeId) -> Vec<(u64, bool)> {
    let Some(parent) = tree.node(node).parent else {
        return Vec::new();
    };
    let declared = declared_name(tree, node);
    let reads = mentions(tree, node);
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
        let theirs = declared_name(tree, sibling);
        let depends = declared
            .is_some_and(|d| theirs == Some(d) || mentions(tree, sibling).contains(&d))
            || theirs.is_some_and(|t| reads.contains(&t));
        if depends {
            out.push((tree.node(sibling).hash, before));
        }
    }
    out.sort_unstable();
    out
}

fn declared_name(tree: &Tree, node: NodeId) -> Option<&str> {
    let n = tree.node(node);
    matches!(n.role, Role::Definition(_))
        .then_some(n.name.as_deref())
        .flatten()
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
            out.push(text.trim_matches(|c: char| !(c.is_alphanumeric() || "_?!=".contains(c))));
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
        .filter(|n| n.kind == "directive")
        .map(|n| n.hash)
        .collect();
    out.sort_unstable();
    out
}

/// Each hunk's output `id`, by `[file][hunk]`.
fn hunk_ids(change: &Change) -> Vec<usize> {
    let mut next = 0;
    change
        .files
        .iter()
        .map(|f| {
            let first = next;
            next += f.diff.hunks.len();
            first
        })
        .collect()
}

fn entry(change: &Change, units: &[Unit], ids: &[usize], pair: &Pair) -> Value {
    let (from, to) = (&units[pair.from], &units[pair.to]);
    let id = |u: &Unit| u.hunk(change).map(|h| ids[u.file] + h);
    let (changed_before, changed_after) = if pair.similarity < 1.0 {
        changed_lines(from.tree(change), from.node, to.tree(change), to.node)
    } else {
        (Vec::new(), Vec::new())
    };
    json!({
        "name": from.tree(change).node(from.node).name.as_deref(),
        "from_hunk": id(from),
        "to_hunk": id(to),
        "similarity": pair.similarity,
        "changed_before": changed_before,
        "changed_after": changed_after,
    })
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
        lines(tree, node)
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
    let common = lcs(&ta, &tb, &vec![1; ta.len()]);
    let only = |side: &[(usize, Vec<u8>)], pick: fn(&(usize, usize)) -> usize| -> Vec<usize> {
        side.iter()
            .enumerate()
            .filter(|(i, _)| !common.iter().any(|c| pick(c) == *i))
            .map(|(_, (line, _))| *line)
            .collect()
    };
    (only(&a, |c| c.0), only(&b, |c| c.1))
}

fn round(value: f64) -> f64 {
    Confidence::new(value).get()
}
