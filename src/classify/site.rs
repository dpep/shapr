//! Mapping a textual hunk onto tree nodes, once per hunk, for every pass.
//!
//! A hunk's changed region on each side becomes a *cover*: the smallest run
//! of sibling nodes holding every significant byte of the region. Two covers
//! that hash equal are the same code. Cross-hunk passes use covers as the
//! subtrees a hunk removed (before side) and added (after side).

use crate::diff::{Hunk, Lines};
use crate::tree::{NodeId, Span, Tree};
use std::collections::BTreeSet;

#[derive(Debug)]
pub struct Site {
    pub before: Side,
    pub after: Side,
}

#[derive(Debug)]
pub struct Side {
    pub lines: Lines,
    /// First to last significant byte of the region; `None` when the region
    /// holds nothing but insignificant whitespace.
    pub span: Option<Span>,
    /// Sibling nodes, in order, covering `span`. Can reach past the region
    /// when the region cuts through a node; never misses any of it.
    pub cover: Vec<NodeId>,
}

impl Site {
    pub fn new(before: &Tree, after: &Tree, hunk: &Hunk) -> Site {
        Site {
            before: Side::new(before, hunk.before),
            after: Side::new(after, hunk.after),
        }
    }
}

impl Side {
    fn new(tree: &Tree, lines: Lines) -> Side {
        let significance = Significance::of(tree, lines);
        let span = significance.span();
        let cover = span
            .as_ref()
            .map(|s| cover(tree, s, &significance))
            .unwrap_or_default();
        Side { lines, span, cover }
    }
}

/// Which bytes of a region matter: anything but whitespace, plus whitespace
/// inside a leaf (string content, a comment), where it is part of the value.
pub struct Significance {
    region: Span,
    marks: Vec<bool>,
}

impl Significance {
    pub fn of(tree: &Tree, lines: Lines) -> Significance {
        let Some(last) = lines.last() else {
            return Significance {
                region: 0..0,
                marks: Vec::new(),
            };
        };
        let region = tree.line_span(lines.start, last);
        let mut marks: Vec<bool> = tree
            .text(&region)
            .iter()
            .map(|b| !b.is_ascii_whitespace())
            .collect();
        // A node's extent holds its children's, so only nodes reaching into
        // the region can hold a leaf that does.
        let mut stack = vec![Tree::ROOT];
        while let Some(id) = stack.pop() {
            let node = tree.node(id);
            if node.extent.start >= region.end || node.extent.end <= region.start {
                continue;
            }
            if !node.children.is_empty() {
                stack.extend(&node.children);
                continue;
            }
            let from = node.extent.start.max(region.start);
            let to = node.extent.end.min(region.end);
            for mark in marks
                .iter_mut()
                .take(to.saturating_sub(region.start))
                .skip(from.saturating_sub(region.start))
            {
                *mark = true;
            }
        }
        Significance { region, marks }
    }

    pub fn span(&self) -> Option<Span> {
        let first = self.marks.iter().position(|&m| m)?;
        let last = self.marks.iter().rposition(|&m| m)?;
        Some(self.region.start + first..self.region.start + last + 1)
    }

    /// Whether any byte of `span` inside the region is significant.
    pub fn any(&self, span: &Span) -> bool {
        let from = span.start.max(self.region.start);
        let to = span.end.min(self.region.end);
        from < to && self.marks[from - self.region.start..to - self.region.start].contains(&true)
    }

    /// The region's significant bytes outside `skip`, in order.
    pub fn bytes(&self, tree: &Tree, skip: &[Span]) -> Vec<u8> {
        self.marks
            .iter()
            .enumerate()
            .filter(|(_, m)| **m)
            .map(|(i, _)| self.region.start + i)
            .filter(|at| !skip.iter().any(|s| s.contains(at)))
            .map(|at| tree.source[at])
            .collect()
    }
}

/// Descends while one child holds all of `span` and the node's own text
/// doesn't reach into it; then the overlapping children are the cover.
fn cover(tree: &Tree, span: &Span, sig: &Significance) -> Vec<NodeId> {
    let mut at = Tree::ROOT;
    loop {
        let node = tree.node(at);
        if tree.own_offsets(at).any(|o| span.contains(&o)) {
            return vec![at];
        }
        let overlapping: Vec<NodeId> = node
            .children
            .iter()
            .copied()
            .filter(|&c| sig.any(&clip(&tree.node(c).extent, span)))
            .collect();
        match overlapping.as_slice() {
            [only] if contains(&tree.node(*only).extent, span) => at = *only,
            [] => return vec![at],
            _ => return overlapping,
        }
    }
}

fn clip(a: &Span, b: &Span) -> Span {
    a.start.max(b.start)..a.end.min(b.end).max(a.start.max(b.start))
}

fn contains(outer: &Span, inner: &Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// Offsets of the bytes on `line` that carry code: not whitespace, not in a
/// comment.
pub fn code_bytes(tree: &Tree, comments: &[&Span], line: usize) -> Vec<usize> {
    tree.line_span(line, line)
        .filter(|&at| !tree.source[at].is_ascii_whitespace())
        .filter(|at| !comments.iter().any(|c| c.contains(at)))
        .collect()
}

pub fn comments(tree: &Tree) -> Vec<&Span> {
    tree.nodes
        .iter()
        .filter(|n| n.comment)
        .map(|n| &n.extent)
        .collect()
}

/// Of `lines`, those holding a code byte inside one of `spans`.
pub fn lines_in(tree: &Tree, lines: &BTreeSet<usize>, spans: &[Span]) -> BTreeSet<usize> {
    if spans.is_empty() {
        return BTreeSet::new();
    }
    let comments = comments(tree);
    lines
        .iter()
        .copied()
        .filter(|&line| {
            code_bytes(tree, &comments, line)
                .iter()
                .any(|at| spans.iter().any(|s| s.contains(at)))
        })
        .collect()
}
