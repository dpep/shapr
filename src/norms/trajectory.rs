//! Whether a change moves the code it touches toward its config or away:
//! per norm config sets, the observations config checks in the changed
//! files and those not in the configured style (offenses), before and after.
//! Unchanged files hold the same offenses on both sides, so the difference
//! is the file's, its directory's and the repository's alike.

use super::snapshot::Snapshot;
use super::{Analysis, for_path, skipped};
use crate::change::Change;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Default, PartialEq, Serialize)]
pub struct Side {
    pub checked: usize,
    pub offenses: usize,
}

#[derive(Serialize)]
pub struct Trajectory {
    pub norm: &'static str,
    pub configured: String,
    pub before: Side,
    pub after: Side,
    /// Offenses the change fixes and adds, which `direction` nets.
    pub fixed: usize,
    pub added: usize,
    /// `toward` (fewer offenses), `away` (more) or `level`.
    pub direction: &'static str,
}

/// Per norm and configured style the change's files touch, where the
/// counts move. Both sides are judged by the config as of the change's
/// after side.
pub fn trajectory(change: &Change, snapshot: Option<&dyn Snapshot>) -> Vec<Trajectory> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    let mut sides: BTreeMap<(&'static str, String), [Side; 2]> = BTreeMap::new();
    let mut moved: BTreeMap<(&'static str, String), [usize; 2]> = BTreeMap::new();
    for file in &change.files {
        let path = file.diff.path();
        let (Some(language), Ok(trees)) = (for_path(path), &file.trees) else {
            continue;
        };
        if skipped(path, file.category).is_some() {
            continue;
        }
        let analyses = [&trees.before.source, &trees.after.source].map(|s| language.analyze(s));
        if analyses.iter().any(|a| a.errors) {
            continue;
        }
        let configured = language.configured(path, snapshot);
        for (norm, style) in &configured.styles {
            let entry = sides.entry((*norm, style.clone())).or_default();
            for (side, analysis) in entry.iter_mut().zip(&analyses) {
                count(side, analysis, norm, style);
            }
            let [fixed, added] = moved.entry((*norm, style.clone())).or_default();
            let (f, a) = offenses_moved(&analyses, norm, style);
            *fixed += f;
            *added += a;
        }
    }
    sides
        .into_iter()
        .map(|(key, sides)| {
            let [fixed, added] = moved.get(&key).copied().unwrap_or_default();
            (key, sides, fixed, added)
        })
        .filter(|(_, [before, after], fixed, added)| before != after || fixed + added > 0)
        .map(
            |((norm, configured), [before, after], fixed, added)| Trajectory {
                norm,
                configured,
                before,
                after,
                fixed,
                added,
                direction: match after.offenses.cmp(&before.offenses) {
                    std::cmp::Ordering::Less => "toward",
                    std::cmp::Ordering::Greater => "away",
                    std::cmp::Ordering::Equal => "level",
                },
            },
        )
        .collect()
}

fn count(side: &mut Side, analysis: &Analysis, norm: &str, style: &str) {
    for o in analysis.observations.iter().filter(|o| o.norm == norm) {
        if !analysis.disabled(norm, o.line) {
            side.checked += 1;
            side.offenses += (o.style != style) as usize;
        }
    }
}

/// One file's offenses only before (fixed) and only after (added), matched
/// by their content with the style taken out, so an offense carried over
/// unchanged or moved is neither.
fn offenses_moved(analyses: &[Analysis; 2], norm: &str, style: &str) -> (usize, usize) {
    let mut net: BTreeMap<&str, isize> = BTreeMap::new();
    for (analysis, sign) in analyses.iter().zip([1, -1]) {
        for o in analysis.observations.iter().filter(|o| o.norm == norm) {
            if o.style != style && !analysis.disabled(norm, o.line) {
                *net.entry(o.key.as_str()).or_default() += sign;
            }
        }
    }
    let total = |positive: bool| -> usize {
        (net.values())
            .filter(|n| (**n > 0) == positive && **n != 0)
            .map(|n| n.unsigned_abs())
            .sum()
    };
    (total(true), total(false))
}

/// Text lines: one per norm whose offenses moved.
pub fn text(trajectories: &[Trajectory]) -> Vec<String> {
    (trajectories.iter())
        .filter(|t| t.fixed + t.added > 0)
        .map(|t| {
            format!(
                "config {} {}: offenses {} -> {} ({}: {} fixed, {} added)",
                t.norm,
                t.configured,
                t.before.offenses,
                t.after.offenses,
                t.direction,
                t.fixed,
                t.added
            )
        })
        .collect()
}
