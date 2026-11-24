//! The alignment of each cleanly parsed file's two trees, computed once
//! before the passes, and the one longest-common-subsequence every
//! alignment uses.
//!
//! Two views, because detectors ask two questions of it:
//!
//! - **Declarations** ([`File::unmatched`], [`File::kept`]): each container's
//!   children (the file, then each class or module body) matched by a
//!   heaviest common subsequence, declarations keyed by name (so one edited
//!   in place still aligns, and a namespace's body aligns in turn), other
//!   statements by hash. What is left out is removed or added. Move reads
//!   it; extract matches the siblings around a candidate with [`siblings`].
//! - **Covers** ([`File::covers`]): each hunk's two covers aligned node for
//!   node, positionally. Two nodes align when their kinds, roles, child
//!   counts and own text (names masked, whitespace aside) match; their
//!   children then align in turn. A pair that doesn't align differs as a
//!   whole. A renamed declaration aligns here though not by name above, so
//!   rename reads the names that differ and format what is left.

use super::site::Site;
use crate::change::{Change, Trees};
use crate::tree::{NameKind, NodeId, Role, Span, Tree};
use std::collections::BTreeSet;
use std::ops::RangeInclusive;

/// Per file of the change, its alignment when both sides parsed cleanly.
pub(super) struct Alignments {
    files: Vec<Option<File>>,
}

impl Alignments {
    pub fn of(change: &Change) -> Alignments {
        let files = change.files.iter().map(|f| match &f.trees {
            Ok(trees) if trees.clean() => Some(File::of(trees)),
            _ => None,
        });
        Alignments {
            files: files.collect(),
        }
    }

    /// The aligned files, by index into `Change::files`.
    pub fn files(&self) -> impl Iterator<Item = (usize, &File)> {
        let files = self.files.iter().enumerate();
        files.filter_map(|(f, file)| file.as_ref().map(|file| (f, file)))
    }

    /// File `f`'s alignment; `None` when a side didn't parse cleanly.
    pub fn file(&self, f: usize) -> Option<&File> {
        self.files[f].as_ref()
    }
}

/// Which tree of a file a node is in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Side {
    /// The before tree: what the change removed.
    Removed,
    /// The after tree: what it added.
    Added,
}

pub(super) struct File {
    /// One per hunk, in `diff.hunks` order.
    pub covers: Vec<Cover>,
    /// Container children the declaration alignment left out, in the order
    /// it met them.
    pub unmatched: Vec<(Side, NodeId)>,
    /// Line ranges of nodes it kept unchanged, before and after.
    pub kept: (Vec<RangeInclusive<usize>>, Vec<RangeInclusive<usize>>),
}

impl File {
    fn of(trees: &Trees) -> File {
        let mut file = File {
            covers: trees.sites.iter().map(|site| cover(trees, site)).collect(),
            unmatched: Vec::new(),
            kept: (Vec::new(), Vec::new()),
        };
        file.declarations(trees, Tree::ROOT, Tree::ROOT);
        file
    }

    /// Lines the declaration alignment kept unchanged on `side`.
    pub fn kept(&self, side: Side) -> &[RangeInclusive<usize>] {
        match side {
            Side::Removed => &self.kept.0,
            Side::Added => &self.kept.1,
        }
    }

    fn declarations(&mut self, trees: &Trees, before: NodeId, after: NodeId) {
        let (tb, ta) = (&trees.before, &trees.after);
        let b: Vec<NodeId> = tb.code_children(before).collect();
        let a: Vec<NodeId> = ta.code_children(after).collect();
        let kb: Vec<Key> = b.iter().map(|&n| key(tb, n)).collect();
        let ka: Vec<Key> = a.iter().map(|&n| key(ta, n)).collect();
        // A child the diff left alone outweighs any it changed, so of two
        // swapped declarations the one git shows moving is the one left out.
        let sizes: Vec<usize> = b.iter().map(|&n| size(tb, n)).collect();
        let heavy = sizes.iter().sum::<usize>() + 1;
        let weight: Vec<usize> = b
            .iter()
            .zip(&sizes)
            .map(|(&n, &size)| match untouched(trees, tb.lines(n)) {
                true => size * heavy,
                false => size,
            })
            .collect();
        let matched = lcs(b.len(), a.len(), |i, j| kb[i] == ka[j], |i| weight[i]);
        let (mut in_b, mut in_a) = (vec![false; b.len()], vec![false; a.len()]);
        for &(i, j) in &matched {
            in_b[i] = true;
            in_a[j] = true;
            let (nb, na) = (b[i], a[j]);
            if tb.node(nb).hash == ta.node(na).hash {
                self.kept.0.push(tb.lines(nb));
                self.kept.1.push(ta.lines(na));
            } else if matches!(kb[i], Key::Namespace(_) | Key::Transparent) {
                self.declarations(trees, nb, na);
            }
        }
        let left = |seen: &[bool], ids: &[NodeId], side: Side| -> Vec<(Side, NodeId)> {
            let ids = ids.iter().zip(seen).filter(|(_, seen)| !**seen);
            ids.map(|(&n, _)| (side, n)).collect()
        };
        self.unmatched.extend(left(&in_b, &b, Side::Removed));
        self.unmatched.extend(left(&in_a, &a, Side::Added));
    }
}

/// Whether no hunk's changed region on the before side reaches `lines`.
fn untouched(trees: &Trees, lines: RangeInclusive<usize>) -> bool {
    trees.sites.iter().all(|site| {
        let region = site.before.lines;
        region
            .last()
            .is_none_or(|last| last < *lines.start() || *lines.end() < region.start)
    })
}

/// How a child lines up with its counterpart on the other side.
#[derive(PartialEq, Eq)]
enum Key<'t> {
    /// A namespace, by its name: its body is aligned in turn.
    Namespace(u64),
    /// Any other declaration, by its name: aligned with a different hash,
    /// it was edited in place, not moved.
    Declaration(Role, &'t str),
    /// A node with no syntax of its own (a statement list): aligned in turn.
    Transparent,
    Code(u64),
}

fn key(tree: &Tree, id: NodeId) -> Key<'_> {
    let node = tree.node(id);
    if matches!(
        node.role,
        Role::Definition(NameKind::Class | NameKind::Module)
    ) && let Some(name) = namespace_name(tree, id, false)
    {
        return Key::Namespace(name);
    }
    if node.role.is_declaration()
        && let Some(name) = node.name.as_deref()
    {
        return Key::Declaration(node.role, name);
    }
    if transparent(tree, id) {
        return Key::Transparent;
    }
    Key::Code(node.hash)
}

/// A namespace's name, as its name child's hash (`A::B` tells it from
/// `C::B`); one without a name stands by its first child (a Rust `impl`'s
/// header). With `context`, any code before the name counts too: a Rust
/// attribute (`#[cfg(unix)] mod imp` is not `#[cfg(windows)] mod imp`),
/// though not an annotation (visibility). Alignment pairs namespaces by name,
/// so one whose attributes were edited still aligns; owners compare with
/// context.
pub(super) fn namespace_name(tree: &Tree, id: NodeId, context: bool) -> Option<u64> {
    let code: Vec<NodeId> = tree.code_children(id).collect();
    let Some(name) = tree.name_child(id) else {
        return code.first().map(|&c| tree.node(c).hash);
    };
    let ids: Vec<u64> = code
        .iter()
        .take_while(|&&c| context && c != name)
        .filter(|&&c| tree.node(c).role != Role::Annotation)
        .chain([&name])
        .map(|&c| tree.node(c).hash)
        .collect();
    match ids[..] {
        [only] => Some(only),
        _ => {
            let mut h = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&ids, &mut h);
            Some(std::hash::Hasher::finish(&h))
        }
    }
}

/// A node that means nothing beyond its children: no role, and no syntax
/// of its own outside them (a statement list, the file root). A role rules
/// it out even when a child's span swallows its keywords, as an implicit
/// `begin` does a `def`'s.
pub(super) fn transparent(tree: &Tree, id: NodeId) -> bool {
    let node = tree.node(id);
    if node.role != Role::Other || node.children.is_empty() {
        return false;
    }
    tree.own_offsets(id).next().is_none()
}

/// Non-comment nodes in the subtree.
pub(super) fn size(tree: &Tree, node: NodeId) -> usize {
    tree.descendants(node)
        .filter(|&n| !tree.node(n).comment)
        .count()
}

/// Index pairs of the heaviest common subsequence of `0..n` and `0..m`
/// under `eq`, each element of the first weighing `weight(i)`. Weighing
/// children by size anchors a big namespace before a run of one-line
/// statements that happens to be longer; with unit weights it is the plain
/// longest common subsequence, an equal pair taken as soon as it is met.
pub(super) fn lcs(
    n: usize,
    m: usize,
    eq: impl Fn(usize, usize) -> bool,
    weight: impl Fn(usize) -> usize,
) -> Vec<(usize, usize)> {
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let take = if eq(i, j) {
                weight(i) + table[i + 1][j + 1]
            } else {
                0
            };
            table[i][j] = take.max(table[i + 1][j]).max(table[i][j + 1]);
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if eq(i, j) && table[i][j] == weight(i) + table[i + 1][j + 1] {
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

/// Largest sibling alignment table worth building; past it, give up.
const MAX_SIBLINGS: usize = 1 << 20;

/// The code children of `before` and `after` (skipping `skip`) with no
/// equal counterpart on the other side, aligned by longest common
/// subsequence of hashes: (before, after). `None` when the containers
/// differ in kind or own text, or too much to align.
pub(super) fn siblings(
    (tb, before, skip_b): (&Tree, NodeId, &[NodeId]),
    (ta, after, skip_a): (&Tree, NodeId, &[NodeId]),
) -> Option<(Vec<NodeId>, Vec<NodeId>)> {
    let unmarked =
        |t: Vec<u8>| -> Vec<u8> { t.into_iter().filter(|&b| b != crate::tree::MARK).collect() };
    if tb.node(before).kind != ta.node(after).kind
        || unmarked(tb.own_text(before, false)) != unmarked(ta.own_text(after, false))
    {
        return None;
    }
    let side = |tree: &Tree, id: NodeId, skip: &[NodeId]| -> Vec<NodeId> {
        tree.code_children(id)
            .filter(|c| !skip.contains(c))
            .collect()
    };
    let mut left = side(tb, before, skip_b);
    let mut right = side(ta, after, skip_a);
    let equal = |l: &NodeId, r: &NodeId| tb.node(*l).hash == ta.node(*r).hash;
    // Most siblings are untouched: trim the equal ends before the quadratic part.
    let head = left
        .iter()
        .zip(&right)
        .take_while(|(l, r)| equal(l, r))
        .count();
    left.drain(..head);
    right.drain(..head);
    let tail = left
        .iter()
        .rev()
        .zip(right.iter().rev())
        .take_while(|(l, r)| equal(l, r))
        .count();
    left.truncate(left.len() - tail);
    right.truncate(right.len() - tail);
    if left.len().saturating_mul(right.len()) > MAX_SIBLINGS {
        return None;
    }
    let matched = lcs(
        left.len(),
        right.len(),
        |i, j| equal(&left[i], &right[j]),
        |_| 1,
    );
    let unmatched = |ids: &[NodeId], pick: fn(&(usize, usize)) -> usize| -> Vec<NodeId> {
        let matched: Vec<usize> = matched.iter().map(pick).collect();
        let ids = ids.iter().enumerate().filter(|(i, _)| !matched.contains(i));
        ids.map(|(_, &id)| id).collect()
    };
    Some((unmatched(&left, |p| p.0), unmatched(&right, |p| p.1)))
}

/// An aligned node whose name changed.
pub(super) struct Diff {
    pub before: NodeId,
    pub after: NodeId,
}

#[derive(Default)]
pub(super) struct Cover {
    pub diffs: Vec<Diff>,
    /// Extents of code that differs beyond names: (before, after).
    pub differs: (Vec<Span>, Vec<Span>),
    /// Code the sides share, whitespace aside, as (before, after) spans:
    /// equal subtrees, and the own text of aligned nodes whose names agree.
    pub kept: Vec<(Vec<Span>, Vec<Span>)>,
    /// Aligned subtrees that differ only in spellings of the same program
    /// ([`Node::canon`](crate::tree::Node::canon)).
    pub restyled: Vec<Restyled>,
}

/// An aligned pair of subtrees whose canons agree though their hashes don't.
pub(super) struct Restyled {
    pub before: Span,
    pub after: Span,
    /// The variants of the respelled nodes on either side, sorted.
    pub variants: Vec<&'static str>,
}

impl Cover {
    /// Code that differs beyond whitespace on each side: what doesn't align,
    /// and what is respelled.
    pub fn changed(&self) -> (Vec<Span>, Vec<Span>) {
        let (mut before, mut after) = self.differs.clone();
        before.extend(self.restyled.iter().map(|r| r.before.clone()));
        after.extend(self.restyled.iter().map(|r| r.after.clone()));
        (before, after)
    }

    /// Where names changed, on each side: the renamed nodes' own text.
    pub fn renamed(&self, trees: &Trees) -> (Vec<Span>, Vec<Span>) {
        let own = |tree: &Tree, ids: &mut dyn Iterator<Item = NodeId>| -> Vec<Span> {
            ids.flat_map(|id| tree.own_spans(id)).collect()
        };
        (
            own(&trees.before, &mut self.diffs.iter().map(|d| d.before)),
            own(&trees.after, &mut self.diffs.iter().map(|d| d.after)),
        )
    }
}

fn cover(trees: &Trees, site: &Site) -> Cover {
    let code = |tree: &Tree, cover: &[NodeId]| -> Vec<NodeId> {
        let code = cover.iter().filter(|&&id| !tree.node(id).comment);
        code.copied().collect()
    };
    let before = code(&trees.before, &site.before.cover);
    let after = code(&trees.after, &site.after.cover);
    let mut out = Cover::default();
    if let Some(restyled) = respelled_region(trees, &before, &after) {
        out.restyled.push(restyled);
        return out;
    }
    if before.len() != after.len() {
        let extents = |tree: &Tree, ids: &[NodeId]| -> Vec<Span> {
            ids.iter().map(|&id| tree.node(id).extent.clone()).collect()
        };
        out.differs = (
            extents(&trees.before, &before),
            extents(&trees.after, &after),
        );
        return out;
    }
    for (b, a) in before.into_iter().zip(after) {
        pair(&trees.before, b, &trees.after, a, &mut out);
    }
    out
}

/// Same shape, same kinds and roles, same text but for names; collects the
/// nodes whose names differ, and the pairs that differ otherwise. Comments
/// are ignored.
fn pair(b: &Tree, bi: NodeId, a: &Tree, ai: NodeId, out: &mut Cover) {
    let (bn, an) = (b.node(bi), a.node(ai));
    let differs = |out: &mut Cover| {
        out.differs.0.push(bn.extent.clone());
        out.differs.1.push(an.extent.clone());
    };
    if bn.hash == an.hash {
        out.kept
            .push((vec![bn.extent.clone()], vec![an.extent.clone()]));
        return;
    }
    // Before kinds: redundant parentheses on one side only are respelled.
    if bn.canon == an.canon {
        let mut variants = BTreeSet::new();
        respelled(b, bi, a, ai, &mut variants);
        out.restyled.push(Restyled {
            before: bn.extent.clone(),
            after: an.extent.clone(),
            variants: variants.into_iter().collect(),
        });
        return;
    }
    if bn.kind != an.kind || bn.role != an.role {
        return differs(out);
    }
    let (bc, ac): (Vec<NodeId>, Vec<NodeId>) =
        (b.code_children(bi).collect(), a.code_children(ai).collect());
    if bc.len() != ac.len() || b.own_text(bi, true) != a.own_text(ai, true) {
        return differs(out);
    }
    // A leaf's text is its value, whitespace included (string content).
    if bc.is_empty() && bn.name == an.name {
        return differs(out);
    }
    if bn.name != an.name {
        let renamable = super::rename::renamable(bn.role);
        let spelling = b.semantics.spelling;
        let names =
            [&bn.name, &an.name].map(|n| n.as_deref().is_some_and(|n| spelling.is_identifier(n)));
        if !renamable || names.contains(&false) {
            return differs(out);
        }
        out.diffs.push(Diff {
            before: bi,
            after: ai,
        });
    } else {
        out.kept.push((b.own_spans(bi), a.own_spans(ai)));
    }
    for (b2, a2) in bc.into_iter().zip(ac) {
        pair(b, b2, a, a2, out);
    }
}

/// The two covers' regions as one respelled pair, when they differ: each
/// side's cover node (or the parent of several) lifted to the shallower
/// side's depth, as a comma the region holds on one side only puts the
/// list in that cover and an item in the other.
fn respelled_region(trees: &Trees, before: &[NodeId], after: &[NodeId]) -> Option<Restyled> {
    let (tb, ta) = (&trees.before, &trees.after);
    let region = |tree: &Tree, ids: &[NodeId]| match ids {
        [] => None,
        [one] => Some(*one),
        [first, ..] => tree.node(*first).parent,
    };
    let (mut b, mut a) = (region(tb, before)?, region(ta, after)?);
    let depth = |tree: &Tree, id: NodeId| tree.ancestors(id).count();
    while depth(tb, b) > depth(ta, a) {
        b = tb.node(b).parent?;
    }
    while depth(ta, a) > depth(tb, b) {
        a = ta.node(a).parent?;
    }
    let (bn, an) = (tb.node(b), ta.node(a));
    if bn.hash == an.hash || bn.canon != an.canon {
        return None;
    }
    let mut variants = BTreeSet::new();
    respelled(tb, b, ta, a, &mut variants);
    Some(Restyled {
        before: bn.extent.clone(),
        after: an.extent.clone(),
        variants: variants.into_iter().collect(),
    })
}

/// The variants of the nodes whose own spelling differs between two subtrees
/// with equal canons, walked in step through transparent nodes.
fn respelled(b: &Tree, bi: NodeId, a: &Tree, ai: NodeId, out: &mut BTreeSet<&'static str>) {
    let (bn, an) = (b.node(bi), a.node(ai));
    if bn.hash == an.hash {
        return;
    }
    let only = |tree: &Tree, id: NodeId| tree.code_children(id).next();
    let through = |tree: &Tree, id: NodeId| {
        let respell = tree.node(id).respell.filter(|r| r.transparent)?;
        Some((respell.variant, only(tree, id)?))
    };
    if let Some((variant, child)) = through(b, bi) {
        out.insert(variant);
        return respelled(b, child, a, ai, out);
    }
    if let Some((variant, child)) = through(a, ai) {
        out.insert(variant);
        return respelled(b, bi, a, child, out);
    }
    let (bc, ac): (Vec<NodeId>, Vec<NodeId>) =
        (b.code_children(bi).collect(), a.code_children(ai).collect());
    if bc.is_empty() || ac.is_empty() || b.own_text(bi, false) != a.own_text(ai, false) {
        out.extend(
            [bn.respell, an.respell]
                .into_iter()
                .flatten()
                .map(|r| r.variant),
        );
    }
    if bc.len() == ac.len() {
        for (b2, a2) in bc.into_iter().zip(ac) {
            respelled(b, b2, a, a2, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{Frontend, Ruby};

    fn trees(before: &str, after: &str) -> Trees {
        Trees {
            language: "ruby",
            before: Ruby.parse(before.as_bytes().to_vec()),
            after: Ruby.parse(after.as_bytes().to_vec()),
            sites: Vec::new(),
        }
    }

    fn names(trees: &Trees, file: &File, side: Side) -> Vec<String> {
        let tree = match side {
            Side::Removed => &trees.before,
            Side::Added => &trees.after,
        };
        let unmatched = file.unmatched.iter().filter(|(s, _)| *s == side);
        unmatched
            .filter_map(|&(_, n)| tree.node(n).name.as_deref().map(str::to_string))
            .collect()
    }

    #[test]
    fn lcs_takes_the_heaviest_common_run() {
        let (a, b) = ([1, 2, 3, 4], [3, 4, 1]);
        let eq = |i: usize, j: usize| a[i] == b[j];
        assert_eq!(lcs(4, 3, eq, |_| 1), [(2, 0), (3, 1)]);
        assert_eq!(lcs(4, 3, eq, |i| if a[i] == 1 { 5 } else { 1 }), [(0, 2)]);
        assert!(lcs(0, 3, eq, |_| 1).is_empty());
    }

    #[test]
    fn declarations_align_by_name_and_leave_out_what_moved() {
        let t = trees(
            "class A\n  def a; 1 + 2 + 3; end\n  def b; 2; end\nend\n",
            "class A\n  def b; 2; end\n  def a; 1 + 2 + 4; end\nend\nclass C\n  def c; end\nend\n",
        );
        let file = File::of(&t);
        // `a`, the heavier, edited in place aligns by name; `b` moved past it is left out.
        assert_eq!(names(&t, &file, Side::Removed), ["b"]);
        assert_eq!(names(&t, &file, Side::Added), ["b", "C"]);
        assert!(!file.kept(Side::Removed).is_empty());
    }

    #[test]
    fn siblings_are_what_has_no_equal_counterpart() {
        let t = trees("x = 1\ny = 2\nz = 3\n", "x = 1\ny = 5\nz = 3\n");
        let (b, a) = (&t.before, &t.after);
        // The innermost node holding all three statements.
        let stmts = |tree: &Tree| {
            let mut at = Tree::ROOT;
            while let [only] = tree.code_children(at).collect::<Vec<_>>()[..] {
                at = only;
            }
            at
        };
        let (gone, new) = siblings((b, stmts(b), &[]), (a, stmts(a), &[])).unwrap();
        let text = |tree: &Tree, ids: &[NodeId]| -> Vec<String> {
            let text = |&id: &NodeId| {
                String::from_utf8_lossy(tree.text(&tree.node(id).extent)).into_owned()
            };
            ids.iter().map(text).collect()
        };
        assert_eq!(text(b, &gone), ["y = 2"]);
        assert_eq!(text(a, &new), ["y = 5"]);
    }

    #[test]
    fn covers_align_renamed_names_positionally() {
        let before = "def a\n  x = 1\n  x\nend\n";
        let after = "def a\n  y = 1\n  y\nend\n";
        let hunk = crate::diff::Hunk {
            before: crate::diff::Lines { start: 2, count: 2 },
            after: crate::diff::Lines { start: 2, count: 2 },
            removed: vec![2, 3],
            added: vec![2, 3],
        };
        let mut t = trees(before, after);
        t.sites = vec![Site::new(&t.before, &t.after, &hunk)];
        let file = File::of(&t);
        let renamed: Vec<_> = file.covers[0]
            .diffs
            .iter()
            .map(|d| {
                (
                    t.before.node(d.before).name.clone(),
                    t.after.node(d.after).name.clone(),
                )
            })
            .collect();
        assert_eq!(renamed.len(), 2);
        assert!(
            renamed
                .iter()
                .all(|(b, a)| b.as_deref() == Some("x") && a.as_deref() == Some("y"))
        );
        assert!(file.covers[0].differs.0.is_empty());
    }
}
