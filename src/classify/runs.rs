//! Per-run classification. Diff context merges edits a few lines apart into
//! one hunk, so a reformat or a comment fix beside a logic edit would be read
//! with the logic. A hunk the passes leave with logic that holds several runs
//! of changed lines is classified again with each run a hunk of its own; the
//! parts proven on its runs join the whole hunk's where they claim no line
//! those hold.

use super::site::Site;
use super::{Kind, Labels, Outside, Verdict};
use crate::change::{Change, FileChange, Trees};
use crate::diff::{FileDiff, Hunk};

pub(super) fn refine(change: &Change, outside: &dyn Outside, labels: &mut Labels) {
    // Per file and hunk, its runs when the hunk is open to refining.
    let split: Vec<Vec<Option<Vec<Hunk>>>> = change
        .files
        .iter()
        .zip(labels.iter())
        .map(|(file, labels)| {
            let clean = file.trees.as_ref().is_ok_and(Trees::clean);
            let hunks = file.diff.hunks.iter().zip(labels);
            hunks
                .map(|(hunk, label)| {
                    let runs = hunk.runs();
                    let open = clean && label.verdict() != Verdict::Structure && runs.len() > 1;
                    open.then_some(runs)
                })
                .collect()
        })
        .collect();
    if split.iter().flatten().all(Option::is_none) {
        return;
    }
    let (runs, owners) = by_runs(change, &split);
    let run_labels = super::passes(&runs, outside);
    for (f, (file, run_labels)) in runs.files.iter().zip(run_labels).enumerate() {
        let Ok(trees) = &file.trees else { continue };
        for (r, label) in run_labels.into_iter().enumerate() {
            let h = owners[f][r];
            if split[f][h].is_none() {
                continue;
            }
            let in_place = trees.sites[r].same_nesting(&trees.before, &trees.after);
            let whole = &mut labels[f][h];
            // Beside the whole hunk's parts: a run part overlapping one is refused.
            for part in label.parts {
                if in_place || !same_place(part.kind) {
                    whole.claim(part);
                }
            }
            whole.residual.lines = whole.unclaimed();
        }
    }
}

/// A kind that says the code stayed where it was: proven on a run alone, it
/// holds only when the run's code kept its nesting.
fn same_place(kind: Kind) -> bool {
    match kind {
        Kind::FormatOnly | Kind::CommentOnly | Kind::Restyle | Kind::Rename => true,
        Kind::Move | Kind::Extract | Kind::Inline | Kind::Logic => false,
    }
}

/// `change` with each split hunk replaced by its runs, and per file, the
/// hunk each of its new hunks came from.
fn by_runs(change: &Change, split: &[Vec<Option<Vec<Hunk>>>]) -> (Change, Vec<Vec<usize>>) {
    let mut owners = Vec::new();
    let files = change.files.iter().zip(split).map(|(file, split)| {
        let (mut hunks, mut ids, mut owner) = (Vec::new(), Vec::new(), Vec::new());
        for (h, (hunk, runs)) in file.diff.hunks.iter().zip(split).enumerate() {
            let pieces = runs.clone().unwrap_or_else(|| vec![hunk.clone()]);
            for piece in pieces {
                hunks.push(piece);
                ids.push(file.ids[h]);
                owner.push(h);
            }
        }
        owners.push(owner);
        let trees = match &file.trees {
            Ok(t) => Ok(Trees {
                language: t.language,
                sites: hunks
                    .iter()
                    .map(|hunk| Site::new(&t.before, &t.after, hunk))
                    .collect(),
                before: t.before.clone(),
                after: t.after.clone(),
            }),
            Err(why) => Err(why.clone()),
        };
        FileChange {
            whitespace_only: vec![false; hunks.len()],
            diff: FileDiff {
                hunks,
                ..file.diff.clone()
            },
            ids,
            category: file.category,
            lines_before: file.lines_before,
            trees,
        }
    });
    let files = files.collect();
    (Change { files }, owners)
}
