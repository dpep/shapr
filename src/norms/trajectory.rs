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
        }
    }
    sides
        .into_iter()
        .filter(|(_, [before, after])| before != after)
        .map(|((norm, configured), [before, after])| Trajectory {
            norm,
            configured,
            before,
            after,
            direction: match after.offenses.cmp(&before.offenses) {
                std::cmp::Ordering::Less => "toward",
                std::cmp::Ordering::Greater => "away",
                std::cmp::Ordering::Equal => "level",
            },
        })
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

/// Text lines: one per norm whose offenses moved.
pub fn text(trajectories: &[Trajectory]) -> Vec<String> {
    (trajectories.iter())
        .filter(|t| t.direction != "level")
        .map(|t| {
            format!(
                "config {} {}: offenses {} -> {} ({})",
                t.norm, t.configured, t.before.offenses, t.after.offenses, t.direction
            )
        })
        .collect()
}
