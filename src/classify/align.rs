//! A hunk's two covers aligned node for node: where the sides keep the same
//! shape, which names differ, and which code differs beyond names.
//!
//! Alignment is positional. Two nodes align when their kinds, roles, child
//! counts and own text (names masked, whitespace aside) match; their
//! children then align in turn. A pair that doesn't align differs as a
//! whole. Rename reads the names that differ; format reads what is left.

use super::site::Site;
use crate::change::Trees;
use crate::tree::{NodeId, Span, Tree};

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
            ids.flat_map(|id| own_spans(tree, id)).collect()
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
    let code = |t: &Tree, id: NodeId| -> Vec<NodeId> {
        t.node(id)
            .children
            .iter()
            .copied()
            .filter(|&c| !t.node(c).comment)
            .collect()
    };
    let (bc, ac) = (code(b, bi), code(a, ai));
    if bc.len() != ac.len() || own_text(b, bi) != own_text(a, ai) {
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
        out.kept.push((own_spans(b, bi), own_spans(a, ai)));
    }
    for (b2, a2) in bc.into_iter().zip(ac) {
        pair(b, b2, a, a2, out);
    }
}

/// The node's own source, outside its children, with whitespace dropped and
/// its name (where the text spells it) masked, so two nodes differing only
/// in name compare equal.
fn own_text(tree: &Tree, id: NodeId) -> Vec<u8> {
    let node = tree.node(id);
    let mut text = Vec::new();
    let mut cursor = node.span.start;
    for &child in &node.children {
        let c = tree.node(child);
        let end = c.extent.start.min(node.span.end);
        if cursor < end {
            text.extend_from_slice(&tree.source[cursor..end]);
        }
        if !c.comment {
            text.push(0x01);
        }
        cursor = cursor.max(c.extent.end);
    }
    if cursor < node.span.end {
        text.extend_from_slice(&tree.source[cursor..node.span.end]);
    }
    if let Some(name) = &node.name {
        mask(&mut text, name.as_bytes());
    }
    text.retain(|b| !b.is_ascii_whitespace());
    text
}

/// The stretches of a node's own span outside its children.
fn own_spans(tree: &Tree, id: NodeId) -> Vec<Span> {
    let node = tree.node(id);
    let mut out = Vec::new();
    let mut cursor = node.span.start;
    for &child in &node.children {
        let c = tree.node(child);
        let end = c.extent.start.min(node.span.end);
        if cursor < end {
            out.push(cursor..end);
        }
        cursor = cursor.max(c.extent.end);
    }
    if cursor < node.span.end {
        out.push(cursor..node.span.end);
    }
    out
}

/// Replaces the first occurrence of `name` that stands as a whole token.
pub(super) fn mask(text: &mut Vec<u8>, name: &[u8]) {
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let suffixed = name.last().is_some_and(|b| b"?!=".contains(b));
    let at = (0..text.len()).find(|&i| {
        text[i..].starts_with(name)
            && (i == 0 || !(word(text[i - 1]) || b"@$".contains(&text[i - 1])))
            && text
                .get(i + name.len())
                .is_none_or(|&n| !word(n) && (suffixed || !b"?!".contains(&n)))
    });
    if let Some(i) = at {
        text.splice(i..i + name.len(), [0]);
    }
}

fn is_identifier(name: &str) -> bool {
    let bare = name.trim_start_matches(['@', '$']);
    let bare = bare.strip_suffix(['?', '!', '=']).unwrap_or(bare);
    let mut chars = bare.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked(text: &str, name: &str) -> String {
        let mut bytes = text.as_bytes().to_vec();
        mask(&mut bytes, name.as_bytes());
        String::from_utf8_lossy(&bytes).replace('\0', "_")
    }

    #[test]
    fn mask_takes_whole_tokens_only() {
        assert_eq!(masked("def f(", "f"), "def _(");
        assert_eq!(masked("def de", "de"), "def _");
        assert_eq!(masked("x += ", "x"), "_ += ");
        assert_eq!(masked("x? x", "x"), "x? _");
        assert_eq!(masked(":@x", "@x"), ":_");
        assert_eq!(masked("@x", "x"), "@x");
    }

    #[test]
    fn identifiers_exclude_operators() {
        for ok in ["a", "@b", "@@c", "d?", "e=", "Foo", "_x1", "é"] {
            assert!(is_identifier(ok), "{ok}");
        }
        for bad in ["+", "[]", "<=>", "", "1a", "a b"] {
            assert!(!is_identifier(bad), "{bad}");
        }
    }
}
