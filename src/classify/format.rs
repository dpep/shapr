//! `format_only` and `comment_only`: the same code before and after, ignoring
//! whitespace (and, for `comment_only`, comment text).

use super::align::{Alignments, Cover};
use super::site::{self, CodeLines, Side, Significance};
use super::{Claim, Confidence, Evidence, Kind, LINE_CAP, Labels, Method, Outside, Part};
use crate::change::{Change, Trees};
use crate::tree::{NodeId, Span, Tree};
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};

/// The reason of a logic residual that only says the code differs; text
/// output leaves it out, as the kind says as much.
pub const PLAIN_LOGIC: &str = "code differs beyond whitespace and comments";

pub(super) fn run(change: &Change, _: &Alignments, _: &dyn Outside, labels: &mut Labels) {
    for (file, labels) in change.files.iter().zip(labels.iter_mut()) {
        let Ok(trees) = &file.trees else { continue };
        for (site, label) in trees.sites.iter().zip(labels.iter_mut()) {
            if !label.open() {
                continue;
            }
            let compared = if trees.clean() {
                by_tree(trees, &site.before, &site.after)
            } else {
                by_text(trees, &site.before, &site.after)
            };
            let (kind, reason) = match (compared.code_equal, compared.comments_equal) {
                (true, true) => (
                    Kind::FormatOnly,
                    "same code and comments; whitespace differs",
                ),
                (true, false) => (Kind::CommentOnly, "same code; comments differ"),
                (false, _) => (Kind::Logic, PLAIN_LOGIC),
            };
            // Text equality can't see a change in how whitespace parses
            // (`puts -x` vs `puts - x`): structure needs tree evidence.
            let unproven = !trees.clean() && kind.is_structure();
            let reason = if unproven {
                format!("{reason}, but text can't prove structure")
            } else {
                reason.to_string()
            };
            let reason = match broken_sides(trees) {
                Some(sides) => format!("{reason}; syntax error on {sides}; compared line by line"),
                None => reason,
            };
            let residual = &mut label.residual;
            if unproven {
                // The label is the safe default, not a finding.
                residual.reason = reason;
                residual.confidence = Confidence::new(0.0);
                residual
                    .detail
                    .insert("near".into(), Value::from(kind.as_str()));
                continue;
            }
            let confidence = Confidence::new(strength(trees, &site.before, &site.after));
            if kind == Kind::Logic {
                residual.reason = reason;
                residual.confidence = confidence;
                if let Some(similarity) = compared.similarity {
                    let rounded = super::rounded(similarity);
                    residual
                        .evidence
                        .values
                        .insert("code_similarity".into(), Value::from(rounded));
                }
                continue;
            }
            let part = Part {
                kind,
                context: Vec::new(),
                assumptions: Vec::new(),
                lines: label.changed.clone(),
                review: Claim::default(),
                confidence,
                reason,
                evidence: residual.evidence.clone(),
                detail: Map::new(),
                variants: Vec::new(),
            };
            label.claim(part);
        }
    }
}

/// What the other passes leave of a cleanly parsed hunk. A changed code line
/// whose code the alignment keeps (only whitespace differs) is reformatted.
/// With no logic in the hunk, the remaining lines (blank lines, comments)
/// join it, as `comment_only` when comment text differs. Beside logic (a
/// residual, or a part that isn't structure) a changed comment or blank line
/// is read with the logic, so only reformatted code lines without a comment
/// form a part.
pub(super) fn rest(change: &Change, alignments: &Alignments, _: &dyn Outside, labels: &mut Labels) {
    for (f, aligned) in alignments.files() {
        let Ok(trees) = &change.files[f].trees else {
            continue;
        };
        for (cover, label) in aligned.covers.iter().zip(labels[f].iter_mut()) {
            let free = label.unclaimed();
            if free.is_empty() {
                continue;
            }
            let logic = differing(trees, cover, &cover.changed(), &label.changed, &free);
            let code = |tree: &Tree, lines: &BTreeSet<usize>, comment: bool| -> BTreeSet<usize> {
                let pick = |l: &&usize| tree.line(**l).code && (comment || !tree.line(**l).comment);
                lines.iter().filter(pick).copied().collect()
            };
            // The hunk's logic: code no part explains, or a part whose
            // meaning can change.
            let beside = label.parts.iter().position(|p| !p.structure());
            if logic.is_empty() && beside.is_none() {
                let kind = match comment_texts(&trees.before, &free.before)
                    == comment_texts(&trees.after, &free.after)
                {
                    true => Kind::FormatOnly,
                    false => Kind::CommentOnly,
                };
                label.claim(reformat(kind, free));
                continue;
            }
            let reformatted = Claim {
                before: code(&trees.before, &free.before, false),
                after: code(&trees.after, &free.after, false),
            };
            let reformatted = reformatted.minus(&logic);
            let unchanged = unchanged_lines(trees, &reformatted);
            label.claim(reformat(Kind::FormatOnly, reformatted.minus(&unchanged)));
            // Blank lines and comments go with the logic.
            if let (true, Some(i)) = (logic.is_empty(), beside) {
                let left = label.unclaimed();
                let part = &mut label.parts[i];
                part.lines.before.extend(left.before);
                part.lines.after.extend(left.after);
            }
        }
    }
}

/// Of `lines`, those whose text is byte for byte the same on both sides,
/// paired in order: git deleted and re-added them around reordered code, so
/// nothing on them was reformatted.
fn unchanged_lines(trees: &Trees, lines: &Claim) -> Claim {
    let text = |tree: &Tree, lines: &BTreeSet<usize>| -> Vec<(usize, Vec<u8>)> {
        let line = |l: usize| {
            let text = tree.text(&tree.line_span(l, l));
            let text = text.strip_suffix(b"\n").unwrap_or(text);
            text.strip_suffix(b"\r").unwrap_or(text).to_vec()
        };
        lines.iter().map(|&l| (l, line(l))).collect()
    };
    let (b, a) = (
        text(&trees.before, &lines.before),
        text(&trees.after, &lines.after),
    );
    let matched = super::align::lcs(b.len(), a.len(), |i, j| b[i].1 == a[j].1, |_| 1);
    Claim {
        before: matched.iter().map(|&(i, _)| b[i].0).collect(),
        after: matched.iter().map(|&(_, j)| a[j].0).collect(),
    }
}

fn reformat(kind: Kind, lines: Claim) -> Part {
    let reason = match kind {
        Kind::FormatOnly => "same code; whitespace differs",
        _ => "same code; comments differ",
    };
    Part {
        kind,
        context: Vec::new(),
        assumptions: Vec::new(),
        lines,
        review: Claim::default(),
        confidence: Confidence::new(1.0),
        reason: reason.into(),
        evidence: Evidence {
            method: Method::Ast,
            values: Map::new(),
        },
        detail: Map::new(),
        variants: Vec::new(),
    }
}

/// Of the changed lines, those holding code that differs beyond whitespace:
/// `differs` (code the alignment couldn't match, and respelled code unless
/// the caller is proving the respelling), a renamed node no part explains, or
/// code it kept that also sits on such a line, or changed on one side only
/// (a reformat closes on both sides).
pub(super) fn differing(
    trees: &Trees,
    alignment: &Cover,
    differs: &(Vec<Span>, Vec<Span>),
    changed: &Claim,
    free: &Claim,
) -> Claim {
    let code = (CodeLines::of(&trees.before), CodeLines::of(&trees.after));
    let lines = |(b, a): &(Vec<Span>, Vec<Span>), within: &Claim| Claim {
        before: code.0.touched(&within.before, |at| site::in_any(b, at)),
        after: code.1.touched(&within.after, |at| site::in_any(a, at)),
    };
    let renamed = lines(&alignment.renamed(trees), free);
    let mut logic = lines(differs, changed);
    logic.before.extend(renamed.before);
    logic.after.extend(renamed.after);
    let kept: Vec<Claim> = alignment
        .kept
        .iter()
        .map(|pair| lines(pair, changed))
        .filter(|k| !k.is_empty())
        .collect();
    loop {
        let before = logic.before.len() + logic.after.len();
        for k in &kept {
            // Changed on one side only: code moved onto or off a line.
            let one_sided = k.before.is_empty() || k.after.is_empty();
            if one_sided || !k.intersect(&logic).is_empty() {
                logic.before.extend(&k.before);
                logic.after.extend(&k.after);
            }
        }
        if logic.before.len() + logic.after.len() == before {
            return logic.intersect(free);
        }
    }
}

/// The text of each comment on `lines`, sorted.
fn comment_texts(tree: &Tree, lines: &BTreeSet<usize>) -> Vec<String> {
    let mut texts: Vec<String> = tree
        .nodes
        .iter()
        .filter(|n| n.comment)
        .filter(|n| lines.range(tree.span_lines(&n.extent)).next().is_some())
        .map(|n| comment_text(tree, &n.extent))
        .collect();
    texts.sort();
    texts
}

/// Which sides hold a syntax error, for a reason naming the line method.
fn broken_sides(trees: &Trees) -> Option<&'static str> {
    match (
        trees.before.errors.is_empty(),
        trees.after.errors.is_empty(),
    ) {
        (true, true) => None,
        (false, true) => Some("the before side"),
        (true, false) => Some("the after side"),
        (false, false) => Some("both sides"),
    }
}

struct Compared {
    code_equal: bool,
    comments_equal: bool,
    /// Share of code subtrees the two sides have in common, when they differ.
    similarity: Option<f64>,
}

fn by_tree(trees: &Trees, before: &Side, after: &Side) -> Compared {
    let code = |tree: &Tree, side: &Side| -> Vec<u64> {
        side.cover
            .iter()
            .map(|&id| tree.node(id))
            .filter(|n| !n.comment)
            .map(|n| n.hash)
            .collect()
    };
    // Each comment with where it sits: the parent's place among the cover's
    // code nodes and the code siblings before it. When the code is equal the
    // two sides' code nodes correspond one to one, so a comment that moved to
    // another node (a doc to another item) differs.
    let comments = |tree: &Tree, side: &Side| -> Vec<(usize, usize, String)> {
        let code: HashMap<NodeId, usize> = side
            .cover
            .iter()
            .flat_map(|&id| tree.descendants(id))
            .filter(|&n| !tree.node(n).comment)
            .enumerate()
            .map(|(i, n)| (n, i))
            .collect();
        let place = |n: NodeId| code.get(&n).copied().unwrap_or(usize::MAX);
        side.cover
            .iter()
            .flat_map(|&id| tree.comments_under(id))
            .map(|id| {
                let node = tree.node(id);
                let siblings = node.parent.map_or(0, |p| {
                    let kids = &tree.node(p).children;
                    let before = kids.iter().take_while(|&&k| k != id);
                    before.filter(|k| code.contains_key(k)).count()
                });
                let text = comment_text(tree, &node.extent);
                (node.parent.map_or(usize::MAX, place), siblings, text)
            })
            .collect()
    };
    let code_equal = code(&trees.before, before) == code(&trees.after, after);
    Compared {
        code_equal,
        comments_equal: comments(&trees.before, before) == comments(&trees.after, after),
        similarity: (!code_equal).then(|| {
            dice(
                subtree_hashes(&trees.before, &before.cover),
                subtree_hashes(&trees.after, &after.cover),
            )
        }),
    }
}

/// The fallback when a side doesn't parse: the region's significant bytes,
/// comments aside, compared as text.
fn by_text(trees: &Trees, before: &Side, after: &Side) -> Compared {
    let side = |tree: &Tree, side: &Side| -> (Vec<u8>, Vec<String>) {
        let region = side
            .lines
            .last()
            .map_or(0..0, |last| tree.line_span(side.lines.start, last));
        let comments: Vec<Span> = tree
            .nodes
            .iter()
            .filter(|n| n.comment && n.extent.start < region.end && region.start < n.extent.end)
            .map(|n| n.extent.clone())
            .collect();
        let code = Significance::of(tree, side.lines).bytes(tree, &comments);
        let texts = comments.iter().map(|c| comment_text(tree, c)).collect();
        (code, texts)
    };
    let (code_b, comments_b) = side(&trees.before, before);
    let (code_a, comments_a) = side(&trees.after, after);
    Compared {
        code_equal: code_b == code_a,
        comments_equal: comments_b == comments_a,
        similarity: None,
    }
}

/// 1 for a tree comparison. A text comparison is capped, and loses more for
/// each region line holding a syntax error.
fn strength(trees: &Trees, before: &Side, after: &Side) -> f64 {
    if trees.clean() {
        return 1.0;
    }
    let lines = |tree: &Tree, side: &Side| -> (usize, usize) {
        let Some(last) = side.lines.last() else {
            return (0, 0);
        };
        let broken = (side.lines.start..=last)
            .filter(|&line| tree.errors.iter().any(|e| tree.line_of(e.start) == line))
            .count();
        (broken, side.lines.count)
    };
    let (broken_b, total_b) = lines(&trees.before, before);
    let (broken_a, total_a) = lines(&trees.after, after);
    let total = total_b + total_a;
    let clean_share = if total == 0 {
        1.0
    } else {
        1.0 - (broken_b + broken_a) as f64 / total as f64
    };
    LINE_CAP * clean_share
}

/// Trailing whitespace on each line doesn't change what a comment says.
fn comment_text(tree: &Tree, span: &Span) -> String {
    String::from_utf8_lossy(tree.text(span))
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn subtree_hashes(tree: &Tree, cover: &[NodeId]) -> Vec<u64> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = cover.to_vec();
    while let Some(id) = stack.pop() {
        let node = tree.node(id);
        if !node.comment {
            out.push(node.hash);
            stack.extend(&node.children);
        }
    }
    out.sort_unstable();
    out
}

/// Dice coefficient of two sorted multisets.
pub(super) fn dice(a: Vec<u64>, b: Vec<u64>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
        }
    }
    2.0 * common as f64 / (a.len() + b.len()) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dice_counts_shared_subtrees() {
        assert_eq!(dice(vec![1, 2, 3], vec![1, 2, 4]), 2.0 / 3.0);
        assert_eq!(dice(vec![], vec![]), 1.0);
        assert_eq!(dice(vec![1], vec![]), 0.0);
    }
}
