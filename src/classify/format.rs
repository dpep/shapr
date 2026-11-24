//! `format_only` and `comment_only`: the same code before and after, ignoring
//! whitespace (and, for `comment_only`, comment text).

use super::site::{Side, Significance};
use super::{Confidence, Kind, LINE_CAP, Labels};
use crate::change::{Change, Trees};
use crate::tree::{NodeId, Span, Tree};
use serde_json::Value;

pub(super) fn run(change: &Change, labels: &mut Labels) {
    for (file, labels) in change.files.iter().zip(labels.iter_mut()) {
        let Ok(trees) = &file.trees else { continue };
        for (site, label) in trees.sites.iter().zip(labels.iter_mut()) {
            if label.kind != Kind::Logic {
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
                (false, _) => (Kind::Logic, "code differs beyond whitespace and comments"),
            };
            label.kind = kind;
            label.reason = reason.to_string();
            label.confidence = Confidence::new(strength(trees, &site.before, &site.after));
            if let Some(similarity) = compared.similarity {
                let rounded = Confidence::new(similarity).get();
                label
                    .evidence
                    .values
                    .insert("similarity".into(), Value::from(rounded));
            }
        }
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
    let comments = |tree: &Tree, side: &Side| -> Vec<String> {
        side.cover
            .iter()
            .flat_map(|&id| tree.comments_under(id))
            .map(|id| comment_text(tree, &tree.node(id).extent))
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
