//! Labels every hunk of a change with a kind.
//!
//! Every hunk starts as `logic`, the safe label. Passes then run in order over
//! the whole change, so a pass may match across hunks and files (a move
//! pairs a removal in one file with an addition in another). A pass only
//! relabels hunks still labeled `logic`, and each kind lives in its own
//! module.

pub mod site;

mod format;
mod rename;

use crate::change::Change;
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    FormatOnly,
    CommentOnly,
    Rename,
    Move,
    MoveWithChanges,
    Extract,
    Inline,
    Logic,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::FormatOnly,
        Kind::CommentOnly,
        Kind::Rename,
        Kind::Move,
        Kind::MoveWithChanges,
        Kind::Extract,
        Kind::Inline,
        Kind::Logic,
    ];

    /// Changes structure without changing behavior. Says nothing about risk.
    pub fn is_structure(self) -> bool {
        !matches!(self, Kind::MoveWithChanges | Kind::Logic)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::FormatOnly => "format_only",
            Kind::CommentOnly => "comment_only",
            Kind::Rename => "rename",
            Kind::Move => "move",
            Kind::MoveWithChanges => "move_with_changes",
            Kind::Extract => "extract",
            Kind::Inline => "inline",
            Kind::Logic => "logic",
        }
    }
}

/// How a hunk was compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Both sides parsed cleanly; trees compared.
    Ast,
    /// A side has a syntax error; the region's text compared instead.
    Line,
    /// Nothing to compare: no frontend for the file, or its content unread.
    None,
}

/// Highest confidence the line method can earn: text equality can't see a
/// change in how whitespace parses.
pub const LINE_CAP: f64 = 0.5;

/// A score in 0..=1, rounded to the two decimals it can honestly claim.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize)]
pub struct Confidence(f64);

impl Confidence {
    pub fn new(value: f64) -> Confidence {
        Confidence((value.clamp(0.0, 1.0) * 100.0).round() / 100.0)
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Evidence {
    pub method: Method,
    /// Kind-specific values, flattened beside `method`.
    #[serde(flatten)]
    pub values: Map<String, Value>,
}

#[derive(Clone, Debug)]
pub struct Label {
    pub kind: Kind,
    pub confidence: Confidence,
    pub reason: String,
    pub evidence: Evidence,
    /// Kind-specific facts about the change itself (e.g. rename pairs).
    pub detail: Map<String, Value>,
}

/// One label per hunk: `labels[file][hunk]`, parallel to `Change::files`.
pub type Labels = Vec<Vec<Label>>;

type Pass = fn(&Change, &mut Labels);

/// In precedence order: an earlier pass claims a hunk first.
const PASSES: &[Pass] = &[format::run, rename::run];

pub fn classify(change: &Change) -> Labels {
    let mut labels = defaults(change);
    for pass in PASSES {
        pass(change, &mut labels);
    }
    labels
}

fn defaults(change: &Change) -> Labels {
    change
        .files
        .iter()
        .map(|file| {
            let (method, reason) = match &file.trees {
                Ok(trees) if trees.clean() => (Method::Ast, "code changed".to_string()),
                Ok(_) => (Method::Line, "code changed".to_string()),
                Err(why) => (Method::None, format!("not parsed: {why}")),
            };
            let label = Label {
                kind: Kind::Logic,
                // No comparison yet: the label is the safe default, not a finding.
                confidence: Confidence::new(0.0),
                reason,
                evidence: Evidence {
                    method,
                    values: Map::new(),
                },
                detail: Map::new(),
            };
            vec![label; file.diff.hunks.len()]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_rounds_where_built() {
        assert_eq!(Confidence::new(0.123_456).get(), 0.12);
        assert_eq!(Confidence::new(2.0).get(), 1.0);
        assert_eq!(
            serde_json::to_string(&Confidence::new(2.0 / 3.0)).unwrap(),
            "0.67"
        );
    }

    #[test]
    fn kind_names_match_serde() {
        for kind in Kind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
    }
}
