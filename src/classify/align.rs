//! A hunk's two covers aligned node for node: where the sides keep the same
//! shape, which names differ, and which code differs beyond names.
//!
//! Alignment is positional. Two nodes align when their kinds, roles, child
//! counts and own text (names masked, whitespace aside) match; their
//! children then align in turn. A pair that doesn't align differs as a
//! whole. Rename reads the names that differ; format reads what is left.

use super::site::Site;
use crate::change::Trees;
use crate::tree::{NodeId, Span, Tree, is_identifier};

/// An aligned node whose name changed.
pub(super) struct Diff {
    pub before: NodeId,
    pub after: NodeId,
}

#[derive(Default)]
pub(super) struct Alignment {
    pub diffs: Vec<Diff>,
    /// Extents of code that differs beyond names: (before, after).
    pub differs: (Vec<Span>, Vec<Span>),
    /// Code the sides share, whitespace aside, as (before, after) spans:
    /// equal subtrees, and the own text of aligned nodes whose names agree.
    pub kept: Vec<(Vec<Span>, Vec<Span>)>,
}

impl Alignment {
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

pub(super) fn align(trees: &Trees, site: &Site) -> Alignment {
    let code = |tree: &Tree, cover: &[NodeId]| -> Vec<NodeId> {
        let code = cover.iter().filter(|&&id| !tree.node(id).comment);
        code.copied().collect()
    };
    let before = code(&trees.before, &site.before.cover);
    let after = code(&trees.after, &site.after.cover);
    let mut out = Alignment::default();
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
fn pair(b: &Tree, bi: NodeId, a: &Tree, ai: NodeId, out: &mut Alignment) {
    let (bn, an) = (b.node(bi), a.node(ai));
    let differs = |out: &mut Alignment| {
        out.differs.0.push(bn.extent.clone());
        out.differs.1.push(an.extent.clone());
    };
    if bn.kind != an.kind || bn.role != an.role {
        return differs(out);
    }
    if bn.hash == an.hash {
        out.kept
            .push((vec![bn.extent.clone()], vec![an.extent.clone()]));
        return;
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
        let names = [&bn.name, &an.name].map(|n| n.as_deref().is_some_and(is_identifier));
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
