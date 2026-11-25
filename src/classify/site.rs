//! Mapping a textual hunk onto tree nodes, once per hunk, for every pass.
//!
//! A hunk's changed region on each side becomes a *cover*: the smallest run
//! of sibling nodes holding every significant byte of the region. Two covers
//! that hash equal are the same code. Cross-hunk passes use covers as the
//! subtrees a hunk removed (before side) and added (after side).

use crate::diff::{Hunk, Lines};
use crate::tree::{NodeId, Span, Tree};
use std::collections::BTreeSet;
use std::ops::RangeInclusive;

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

    /// The two covers sit under the same chain of enclosing syntax, kinds
    /// and roles to the root; true when a side covers nothing. A hunk's
    /// enclosing syntax is unchanged context, but a run's can sit in a
    /// sibling run: a wrapper added there re-nests this run's code.
    pub fn same_nesting(&self, before: &Tree, after: &Tree) -> bool {
        let chain = |tree: &Tree, side: &Side| {
            let first = side.cover.first()?;
            let chain = tree.ancestors(*first).map(|id| {
                let node = tree.node(id);
                (node.kind, node.role)
            });
            Some(chain.collect::<Vec<_>>())
        };
        match (chain(before, &self.before), chain(after, &self.after)) {
            (Some(b), Some(a)) => b == a,
            _ => true,
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

/// Which bytes of a region matter: anything but whitespace and separators the
/// language ignores, plus whitespace inside a leaf (string content, a
/// comment), where it is part of the value.
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
        let mut marks: Vec<bool> = region
            .clone()
            .map(|at| !tree.source[at].is_ascii_whitespace() && !tree.ignored(at))
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
        // One sweep: `at` is skipped while the furthest end among spans
        // starting at or before it lies past it.
        let mut skip: Vec<&Span> = skip.iter().collect();
        skip.sort_by_key(|s| s.start);
        let (mut next, mut reach) = (0, 0);
        self.marks
            .iter()
            .enumerate()
            .filter(|(_, m)| **m)
            .map(|(i, _)| self.region.start + i)
            .filter(|&at| {
                while next < skip.len() && skip[next].start <= at {
                    reach = reach.max(skip[next].end);
                    next += 1;
                }
                at >= reach
            })
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
        if tree.own_offsets_in(at, span).next().is_some() {
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

/// Which of a hunk's changed lines a part explains, read off the code bytes
/// on each line: not whitespace, not in a comment. Three quantifiers, one
/// per kind of evidence: a byte touched, every byte inside, a whole line in
/// range.
pub struct CodeLines<'t> {
    tree: &'t Tree,
}

impl<'t> CodeLines<'t> {
    pub fn of(tree: &'t Tree) -> CodeLines<'t> {
        CodeLines { tree }
    }

    /// Offsets of the code bytes on `line`: whitespace counts in a leaf.
    fn bytes(&self, line: usize) -> Vec<usize> {
        let tree = self.tree;
        tree.line_span(line, line)
            .filter(|&at| !tree.source[at].is_ascii_whitespace() || tree.verbatim(at))
            .filter(|&at| !tree.in_comment(at))
            .collect()
    }

    /// Of `lines`, those holding a code byte `pick` accepts.
    pub fn touched(
        &self,
        lines: &BTreeSet<usize>,
        pick: impl Fn(usize) -> bool,
    ) -> BTreeSet<usize> {
        let lines = lines.iter().copied();
        lines
            .filter(|&l| self.bytes(l).into_iter().any(&pick))
            .collect()
    }

    /// Of `lines`, those whose every code byte `pick` accepts; a line
    /// without code by where it starts.
    pub fn within(&self, lines: &BTreeSet<usize>, pick: impl Fn(usize) -> bool) -> BTreeSet<usize> {
        let inside = |l: usize| match self.bytes(l)[..] {
            [] => pick(self.tree.line_start(l)),
            ref bytes => bytes.iter().all(|&at| pick(at)),
        };
        lines.iter().copied().filter(|&l| inside(l)).collect()
    }
}

/// Of `lines`, those inside one of `ranges`.
pub fn spanned<'a>(
    lines: &'a BTreeSet<usize>,
    ranges: &'a [RangeInclusive<usize>],
) -> impl Iterator<Item = usize> + 'a {
    let lines = lines.iter().copied();
    lines.filter(|l| ranges.iter().any(|r| r.contains(l)))
}

/// Whether `at` lies in one of `spans`.
pub fn in_any(spans: &[Span], at: usize) -> bool {
    spans.iter().any(|s| s.contains(&at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{Frontend, Ruby};

    #[test]
    fn code_lines_quantify_over_code_bytes() {
        let tree = Ruby.parse(b"x = 1 # one\n\ny = 2\n".to_vec());
        let code = CodeLines::of(&tree);
        let lines: BTreeSet<usize> = [1, 2, 3].into();
        let at = |s: &str| {
            tree.source
                .windows(s.len())
                .position(|w| w == s.as_bytes())
                .unwrap()
        };
        // The comment's bytes aren't code: touching `1` touches line 1.
        let one = at("1");
        assert_eq!(code.touched(&lines, |b| b == one), [1].into());
        assert!(code.touched(&lines, |b| b == at("one")).is_empty());
        // Every code byte of line 1 is before the comment; the blank line
        // goes by where it starts.
        let before_comment = |b: usize| b < at("#") || b == tree.line_start(2);
        assert_eq!(code.within(&lines, before_comment), [1, 2].into());
        assert_eq!(spanned(&lines, &[2..=5, 9..=9]).collect::<Vec<_>>(), [2, 3]);
    }
}
