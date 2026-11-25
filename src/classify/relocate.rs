//! Relocated verbatim: a run of residual lines whose text, whitespace aside,
//! the change removes in one hunk and adds in another, as git's
//! `--color-moved` shows it. A fact about the residual, never a part: it
//! claims no line and changes no label, so a reader can discount lines it
//! has already read where they were.
//!
//! Lines compare as text the way the trees compare code: a leaf (string
//! content, a heredoc body) and a verbatim span byte for byte, elsewhere
//! each run of whitespace as one space, none at the line's ends. Where
//! whitespace sits still counts, as it may change the parse (`offset -1` is
//! a call, `offset - 1` a subtraction). A block needs
//! [`MIN_LINES`] lines with two words or more (a lone `end`, `}`, `else` or
//! name is too common to show anything moved), must match one removed run
//! and one added run only, and never pairs a hunk with itself (a reindent
//! in place isn't a relocation).

use super::{Labels, Relocation};
use crate::change::Change;
use crate::tree::{Span, Tree};
use std::collections::HashMap;

/// Lines of two words or more a block needs. Two such lines recur by
/// chance across a large change (a guard clause and its `return`); three
/// in a row, in the same order, rarely do.
const MIN_LINES: usize = 3;

/// Removed lines sharing one text past which a position isn't searched:
/// only bounds time on a change repeating one line thousands of times.
const MAX_CANDIDATES: usize = 64;

/// One non-blank residual line.
struct Line {
    file: usize,
    hunk: usize,
    line: usize,
    text: Vec<u8>,
    significant: bool,
}

pub(super) fn run(change: &Change, labels: &mut Labels) {
    let removed = segments(change, labels, false);
    let added = segments(change, labels, true);
    let mut index: HashMap<&[u8], Vec<(usize, usize)>> = HashMap::new();
    for (s, seg) in removed.iter().enumerate() {
        for (j, line) in seg.iter().enumerate() {
            index.entry(&line.text).or_default().push((s, j));
        }
    }
    let mut starts: HashMap<&[u8], Vec<(usize, usize)>> = HashMap::new();
    for (s, seg) in added.iter().enumerate() {
        for (i, line) in seg.iter().enumerate() {
            starts.entry(&line.text).or_default().push((s, i));
        }
    }
    let mut used: Vec<Vec<bool>> = removed.iter().map(|s| vec![false; s.len()]).collect();
    for (a, seg) in added.iter().enumerate() {
        let mut i = 0;
        while i < seg.len() {
            let Some((s, j, len)) = best(&removed, &used, seg, i, &index) else {
                i += 1;
                continue;
            };
            let block = &seg[i..i + len];
            let unique = starts[&block[0].text[..]]
                .iter()
                .filter(|&&(a2, i2)| (a2, i2) != (a, i))
                .all(|&(a2, i2)| !same(&added[a2], i2, block));
            if !unique || block.iter().filter(|l| l.significant).count() < MIN_LINES {
                i += 1;
                continue;
            }
            used[s][j..j + len].fill(true);
            let gone = &removed[s][j..j + len];
            record(change, labels, block, gone, true);
            record(change, labels, gone, block, false);
            i += len;
        }
    }
}

/// The longest run of removed lines, in another hunk and not yet taken,
/// matching `seg` from `i`: (segment, start, length), or `None` when no
/// run matches or two match as long.
fn best(
    removed: &[Vec<Line>],
    used: &[Vec<bool>],
    seg: &[Line],
    i: usize,
    index: &HashMap<&[u8], Vec<(usize, usize)>>,
) -> Option<(usize, usize, usize)> {
    let here = &seg[i];
    let candidates = index.get(&here.text[..])?;
    if candidates.len() > MAX_CANDIDATES {
        return None;
    }
    let mut found: Option<(usize, usize, usize)> = None;
    let mut ties = 0;
    for &(s, j) in candidates {
        let other = &removed[s];
        if (other[j].file, other[j].hunk) == (here.file, here.hunk) {
            continue;
        }
        let len = (0..)
            .take_while(|&k| {
                i + k < seg.len()
                    && j + k < other.len()
                    && !used[s][j + k]
                    && seg[i + k].text == other[j + k].text
            })
            .count();
        match found {
            _ if len == 0 => {}
            Some((_, _, best)) if len < best => {}
            Some((_, _, best)) if len == best => ties += 1,
            _ => {
                found = Some((s, j, len));
                ties = 0;
            }
        }
    }
    found.filter(|_| ties == 0)
}

/// Whether `seg` holds `block`'s texts from `at`.
fn same(seg: &[Line], at: usize, block: &[Line]) -> bool {
    seg.len() >= at + block.len() && seg[at..].iter().zip(block).all(|(l, b)| l.text == b.text)
}

/// Notes on the residual of `block`'s hunk that it came from (or went to)
/// `other`.
fn record(change: &Change, labels: &mut Labels, block: &[Line], other: &[Line], added: bool) {
    let (first, last) = (&block[0], &block[block.len() - 1]);
    let diff = &change.files[other[0].file].diff;
    // The other end's path on its own side: a removed line's file as it was.
    let path = match added {
        true => diff.old_path.as_deref().unwrap_or(diff.path()),
        false => diff.path(),
    };
    let residual = &mut labels[first.file][first.hunk].residual;
    residual.relocated.push(Relocation {
        added,
        lines: first.line..=last.line,
        path: path.to_string(),
        line: other[0].line,
    });
}

/// Each hunk's residual lines on one side, split into runs of consecutive
/// lines, blank lines dropped from each run but not breaking it. Files
/// without trees have no text to compare.
fn segments(change: &Change, labels: &Labels, added: bool) -> Vec<Vec<Line>> {
    let mut out = Vec::new();
    for (f, (file, labels)) in change.files.iter().zip(labels).enumerate() {
        let Ok(trees) = &file.trees else { continue };
        let tree = match added {
            true => &trees.after,
            false => &trees.before,
        };
        let leaves = leaves(tree);
        for (h, label) in labels.iter().enumerate() {
            let lines = match added {
                true => &label.residual.lines.after,
                false => &label.residual.lines.before,
            };
            let mut seg: Vec<Line> = Vec::new();
            let mut prev = None;
            for &line in lines {
                if prev.is_some_and(|p| p + 1 != line) && !seg.is_empty() {
                    out.push(std::mem::take(&mut seg));
                }
                prev = Some(line);
                let text = normalized(tree, &leaves, line);
                if text.is_empty() {
                    continue;
                }
                seg.push(Line {
                    file: f,
                    hunk: h,
                    line,
                    significant: words(&text) >= 2,
                    text,
                });
            }
            if !seg.is_empty() {
                out.push(seg);
            }
        }
    }
    out
}

/// The spans of a tree's leaves, whose bytes compare exactly: sorted and
/// disjoint.
fn leaves(tree: &Tree) -> Vec<Span> {
    let mut spans: Vec<Span> = tree
        .nodes
        .iter()
        .filter(|n| n.children.is_empty())
        .map(|n| n.span.clone())
        .collect();
    spans.sort_by_key(|s| s.start);
    let mut out: Vec<Span> = Vec::new();
    for s in spans {
        match out.last_mut() {
            Some(last) if s.start <= last.end => last.end = last.end.max(s.end),
            _ => out.push(s),
        }
    }
    out
}

/// A line's bytes, whitespace in a leaf or verbatim span kept, elsewhere
/// each run made one space and the line's ends trimmed.
fn normalized(tree: &Tree, leaves: &[Span], line: usize) -> Vec<u8> {
    let exact = |at: usize| {
        let i = leaves.partition_point(|s| s.end <= at);
        leaves.get(i).is_some_and(|s| s.contains(&at)) || tree.verbatim(at)
    };
    let span = tree.line_span(line, line);
    let mut out = Vec::new();
    let mut gap = false;
    for at in span {
        let b = tree.source[at];
        if b.is_ascii_whitespace() && !exact(at) {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push(b' ');
        }
        gap = false;
        out.push(b);
    }
    out
}

/// Runs of identifier bytes (letters, digits, `_`, anything non-ASCII).
fn words(text: &[u8]) -> usize {
    let word = |b: &u8| b.is_ascii_alphanumeric() || *b == b'_' || !b.is_ascii();
    let mut count = 0;
    let mut inside = false;
    for b in text {
        let w = word(b);
        count += usize::from(w && !inside);
        inside = w;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_of_one_word_is_not_significant() {
        assert_eq!(words(b"end"), 1);
        assert_eq!(words(b"});"), 0);
        assert_eq!(words(b"}else{"), 1);
        assert_eq!(words(b"Ok(())"), 1);
        assert_eq!(words(b"self.count+=1;"), 3);
    }
}
