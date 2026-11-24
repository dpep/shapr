//! `reorder`: a hunk whose elements of one container (statements, items,
//! hash entries, array elements, fields) are the same subtrees in another
//! order. Structure only where the frontend says the order can't be
//! observed (Rust items and `use` declarations, a struct literal of plain
//! values); elsewhere the part names what the order is observed through, so
//! a reviewer reads it as a sort and checks what depends on the order.

use super::align::{Alignments, lcs};
use super::hazard::{self, Op};
use super::site::Site;
use super::{Claim, Confidence, Context, Evidence, Kind, Labels, Method, Outside, Part};
use crate::change::{Change, Trees};
use crate::frontend::order::{self, Dependence};
use crate::tree::{NodeId, Span, Tree};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};

pub(super) fn run(change: &Change, alignments: &Alignments, _: &dyn Outside, labels: &mut Labels) {
    for (f, _) in alignments.files() {
        let Ok(trees) = &change.files[f].trees else {
            continue;
        };
        for (site, label) in trees.sites.iter().zip(labels[f].iter_mut()) {
            let free = label.unclaimed();
            if free.is_empty() {
                continue;
            }
            if let Some(part) = prove(trees, site, &free) {
                label.claim(part);
            }
        }
    }
}

/// An element with the comments leading it, which move with it: a doc
/// comment left behind documents another item.
struct Group {
    span: Span,
    /// The code node; `None` for comments trailing the last element.
    code: Option<NodeId>,
    /// Equal for groups with equal trees and equal comment text.
    key: u64,
}

/// The container a side's cover sits in and the elements it covers: the
/// cover itself when it is a run of siblings, else the one node's children.
/// Whether the run is siblings, whose leading comments sit before it.
fn elements(tree: &Tree, cover: &[NodeId]) -> Option<(NodeId, Vec<NodeId>, bool)> {
    match cover {
        [] => None,
        [one] => Some((*one, tree.node(*one).children.clone(), false)),
        [first, ..] => Some((tree.node(*first).parent?, cover.to_vec(), true)),
    }
}

fn groups(tree: &Tree, elements: &[NodeId], run: bool) -> Vec<Group> {
    let mut pending: Vec<NodeId> = Vec::new();
    // A run's first element keeps the comments just before it, unchanged
    // context: they must still lead the same element.
    if run && let Some(parent) = tree.node(elements[0]).parent {
        let siblings = &tree.node(parent).children;
        let at = siblings.iter().position(|&c| c == elements[0]).unwrap_or(0);
        let leading = siblings[..at].iter().rev();
        let mut leading: Vec<NodeId> = leading
            .take_while(|&&c| tree.node(c).comment)
            .copied()
            .collect();
        leading.reverse();
        pending = leading;
    }
    let mut out = Vec::new();
    for &id in elements {
        pending.push(id);
        if !tree.node(id).comment {
            out.push(group(tree, std::mem::take(&mut pending), Some(id)));
        }
    }
    if !pending.is_empty() {
        out.push(group(tree, pending, None));
    }
    out
}

fn group(tree: &Tree, nodes: Vec<NodeId>, code: Option<NodeId>) -> Group {
    let mut h = DefaultHasher::new();
    for &id in &nodes {
        // The hash ignores comments, so their text is keyed apart.
        tree.node(id).hash.hash(&mut h);
        for d in tree.descendants(id).filter(|&d| tree.node(d).comment) {
            tree.text(&tree.node(d).extent).trim_ascii().hash(&mut h);
        }
    }
    let first = tree.node(nodes[0]).extent.start;
    let last = tree
        .node(*nodes.last().expect("a group has a node"))
        .extent
        .end;
    Group {
        span: first..last,
        code,
        key: h.finish(),
    }
}

/// What the part's contexts say a reordered element can be observed
/// through; `None` where shapr has no context to name it yet.
fn context(dependence: Dependence) -> Option<Context> {
    match dependence {
        Dependence::Load => Some(Context::LoadOrder),
        Dependence::Iteration | Dependence::Elements | Dependence::Evaluation => None,
    }
}

fn prove(trees: &Trees, site: &Site, free: &Claim) -> Option<Part> {
    let (tb, ta) = (&trees.before, &trees.after);
    let (cb, eb, run) = elements(tb, &site.before.cover)?;
    let (ca, ea, run_a) = elements(ta, &site.after.cover)?;
    let (nb, na) = (tb.node(cb), ta.node(ca));
    if run != run_a || nb.kind != na.kind || !site.same_nesting(tb, ta) {
        return None;
    }
    // One node on each side: only its children may differ.
    if !run && (nb.role != na.role || tb.own_text(cb, false) != ta.own_text(ca, false)) {
        return None;
    }
    let (gb, ga) = (groups(tb, &eb, run), groups(ta, &ea, run));
    let kept = lcs(gb.len(), ga.len(), |i, j| gb[i].key == ga[j].key, |_| 1);
    let (mut left_b, mut left_a): (Vec<usize>, Vec<usize>) = (
        (0..gb.len())
            .filter(|i| !kept.iter().any(|p| p.0 == *i))
            .collect(),
        (0..ga.len())
            .filter(|j| !kept.iter().any(|p| p.1 == *j))
            .collect(),
    );
    // Out of order: equal on both sides, but not in the common subsequence.
    let mut moved = Vec::new();
    left_b.retain(
        |&i| match left_a.iter().position(|&j| ga[j].key == gb[i].key) {
            Some(at) => {
                moved.push((i, left_a.remove(at)));
                false
            }
            None => true,
        },
    );
    let codes = |groups: &[Group], picks: &mut dyn Iterator<Item = usize>| -> Vec<NodeId> {
        picks.filter_map(|i| groups[i].code).collect()
    };
    let moved_b = codes(&gb, &mut moved.iter().map(|p| p.0));
    let moved_a = codes(&ga, &mut moved.iter().map(|p| p.1));
    if moved_b.is_empty() {
        return None;
    }
    let blocked = |tree: &Tree, ids: &[NodeId]| {
        ids.iter().flat_map(|&id| tree.descendants(id)).any(|d| {
            let hazard = tree.node(d).role.hazard();
            hazard.is_some_and(|h| hazard::blocks(Op::Move, h))
        })
    };
    if blocked(tb, &moved_b) || blocked(ta, &moved_a) {
        return None;
    }
    let language = trees.language;
    let mut dependences = order::reorder(language, tb, cb, &moved_b)?;
    dependences.extend(order::reorder(language, ta, ca, &moved_a)?);
    let contexts: BTreeSet<Context> = dependences
        .into_iter()
        .map(context)
        .collect::<Option<_>>()?;
    let contexts: Vec<Context> = contexts.into_iter().collect();

    let lines = |tree: &Tree, span: &Span| tree.line_of(span.start)..=tree.line_of(span.end - 1);
    // A pure permutation takes the blank lines between its elements too;
    // beside an edit they may belong to it, so only the elements' own lines.
    let pure = left_b.is_empty() && left_a.is_empty();
    let side = |tree: &Tree, groups: &[Group], edits: &[usize], free: &BTreeSet<usize>| {
        let first = lines(tree, &groups[0].span);
        let last = lines(tree, &groups[groups.len() - 1].span);
        let edited: BTreeSet<usize> = edits
            .iter()
            .flat_map(|&i| lines(tree, &groups[i].span))
            .collect();
        let range = *first.start()..=*last.end();
        let own = |l: &usize| {
            pure || groups
                .iter()
                .enumerate()
                .any(|(i, g)| !edits.contains(&i) && lines(tree, &g.span).contains(l))
        };
        let claimed = free.iter().copied().filter(|l| range.contains(l));
        claimed
            .filter(|l| !edited.contains(l) && own(l))
            .collect::<BTreeSet<usize>>()
    };
    let claim = Claim {
        before: side(tb, &gb, &left_b, &free.before),
        after: side(ta, &ga, &left_a, &free.after),
    };
    if claim.is_empty() {
        return None;
    }
    // The moved elements' text is unchanged: read where each landed.
    let review = match contexts.is_empty() {
        true => Claim::default(),
        false => Claim {
            before: BTreeSet::new(),
            after: moved
                .iter()
                .flat_map(|p| lines(ta, &ga[p.1].span))
                .filter(|l| claim.after.contains(l))
                .collect(),
        },
    };
    let names: Vec<&str> = contexts.iter().map(|c| c.as_str()).collect();
    let reason = match names.is_empty() {
        true => "the same elements in another order, which nothing observes".to_string(),
        false => format!(
            "the same elements in another order; order is observable ({})",
            names.join(", ")
        ),
    };
    Some(Part {
        kind: Kind::Reorder,
        context: contexts.clone(),
        assumptions: Vec::new(),
        lines: claim,
        review,
        // A tree comparison, as for a reformat: every element equal.
        confidence: Confidence::new(1.0),
        reason,
        evidence: Evidence {
            method: Method::Ast,
            values: Map::from_iter([
                (
                    "elements".to_string(),
                    Value::from(kept.len() + moved.len()),
                ),
                ("moved".to_string(), Value::from(moved.len())),
            ]),
        },
        detail: Map::from_iter([("context".to_string(), json!(names))]),
        variants: Vec::new(),
    })
}
