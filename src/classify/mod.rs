//! Labels every hunk of a change: the structural parts passes prove, and the
//! residual, the changed lines no part explains, which is `logic`.
//!
//! Every hunk starts as all residual, the safe label. Passes then run in
//! order over the whole change, so a pass may match across hunks and files (a
//! move pairs a removal in one file with an addition in another). A pass
//! claims lines with [`Label::claim`], and each kind lives in its own module.

pub mod facets;
pub mod site;

mod extract;
mod format;
mod moves;
mod rename;

pub use facets::Facet;
pub use moves::Context;

use crate::change::Change;
use crate::diff::Hunk;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeSet;

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

/// Changed lines of one hunk, per side: `-` lines by their before-side
/// number, `+` lines by their after-side number.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Claim {
    pub before: BTreeSet<usize>,
    pub after: BTreeSet<usize>,
}

impl Claim {
    pub fn of(hunk: &Hunk) -> Claim {
        Claim {
            before: hunk.removed.iter().copied().collect(),
            after: hunk.added.iter().copied().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after.is_empty()
    }

    pub fn minus(&self, other: &Claim) -> Claim {
        Claim {
            before: self.before.difference(&other.before).copied().collect(),
            after: self.after.difference(&other.after).copied().collect(),
        }
    }

    pub fn intersect(&self, other: &Claim) -> Claim {
        Claim {
            before: self.before.intersection(&other.before).copied().collect(),
            after: self.after.intersection(&other.after).copied().collect(),
        }
    }
}

/// A structural transformation a pass proved, and the changed lines it
/// explains.
#[derive(Clone, Debug)]
pub struct Part {
    pub kind: Kind,
    /// Changes structure without changing behavior. False for a part whose
    /// own lines are explained but whose meaning can still change (code
    /// moved into a new context).
    pub structure: bool,
    pub lines: Claim,
    /// Of the hunk's changed lines, those a reviewer still reads for this
    /// part; only lines that carry code count.
    pub review: Claim,
    pub confidence: Confidence,
    pub reason: String,
    pub evidence: Evidence,
    /// Kind-specific facts about the change itself (e.g. rename pairs).
    pub detail: Map<String, Value>,
}

/// What no part explains: logic.
#[derive(Clone, Debug)]
pub struct Residual {
    /// The hunk's changed lines no part claims; set when the passes finish.
    pub lines: Claim,
    pub confidence: Confidence,
    pub reason: String,
    pub evidence: Evidence,
    /// Near misses: the structural kind it resembles and why it isn't one.
    pub detail: Map<String, Value>,
    /// What kind of logic change it is.
    pub facets: Vec<Facet>,
}

/// One hunk's composition: the parts passes proved, and the residual.
#[derive(Clone, Debug)]
pub struct Label {
    /// Every changed line of the hunk.
    pub changed: Claim,
    pub parts: Vec<Part>,
    pub residual: Residual,
}

impl Label {
    /// Changed lines no part has claimed yet.
    pub fn unclaimed(&self) -> Claim {
        self.parts
            .iter()
            .fold(self.changed.clone(), |left, p| left.minus(&p.lines))
    }

    /// No part claims any of the hunk yet.
    pub fn open(&self) -> bool {
        self.parts.is_empty()
    }

    /// Adds `part`, keeping only lines no earlier part claims: an earlier
    /// pass takes precedence.
    pub fn claim(&mut self, mut part: Part) {
        let free = self.unclaimed();
        part.lines = part.lines.intersect(&free);
        part.review = part.review.intersect(&part.lines);
        self.parts.push(part);
    }

    /// There is logic: lines no part explains, or no part at all.
    pub fn has_residual(&self) -> bool {
        self.parts.is_empty() || !self.residual.lines.is_empty()
    }

    /// Every part is structure and nothing is left over.
    pub fn structure(&self) -> bool {
        !self.has_residual() && self.parts.iter().all(|p| p.structure)
    }

    /// The one-word rollup: the latest structural kind (in [`Kind::ALL`]
    /// order) of a structure hunk, else `logic`.
    pub fn kind(&self) -> Kind {
        match self.decider() {
            Decider::Part(p) => p.kind,
            Decider::Residual(_) => Kind::Logic,
        }
    }

    pub fn confidence(&self) -> Confidence {
        match self.decider() {
            Decider::Part(p) if self.structure() => self
                .parts
                .iter()
                .map(|q| q.confidence)
                .fold(p.confidence, |a, b| if b < a { b } else { a }),
            Decider::Part(p) => p.confidence,
            Decider::Residual(r) => r.confidence,
        }
    }

    pub fn reason(&self) -> &str {
        match self.decider() {
            Decider::Part(p) => &p.reason,
            Decider::Residual(r) => &r.reason,
        }
    }

    pub fn evidence(&self) -> &Evidence {
        match self.decider() {
            Decider::Part(p) => &p.evidence,
            Decider::Residual(r) => &r.evidence,
        }
    }

    pub fn detail(&self) -> &Map<String, Value> {
        match self.decider() {
            Decider::Part(p) => &p.detail,
            Decider::Residual(r) => &r.detail,
        }
    }

    /// Facets of the residual; empty when there is none.
    pub fn facets(&self) -> &[Facet] {
        if self.has_residual() {
            &self.residual.facets
        } else {
            &[]
        }
    }

    /// The part or residual whose kind, reason, evidence and detail stand
    /// for the hunk: the residual when there is one, else the first part
    /// that isn't structure, else the latest structural kind.
    fn decider(&self) -> Decider<'_> {
        if self.has_residual() {
            return Decider::Residual(&self.residual);
        }
        match self.parts.iter().find(|p| !p.structure) {
            Some(p) => Decider::Part(p),
            None => Decider::Part(
                self.parts
                    .iter()
                    .max_by_key(|p| p.kind)
                    .expect("a hunk without a residual has a part"),
            ),
        }
    }
}

enum Decider<'l> {
    Part(&'l Part),
    Residual(&'l Residual),
}

/// One label per hunk: `labels[file][hunk]`, parallel to `Change::files`.
pub type Labels = Vec<Vec<Label>>;

/// A pass proves parts against the whole of both trees and claims the lines
/// they explain with [`Label::claim`]; the residual is never cut out of the
/// trees, so a check that new code would break fails and the lines stay
/// logic. A pass may also leave a near miss on the residual.
type Pass = fn(&Change, &mut Labels);

/// In precedence order: an earlier pass claims a line first.
const PASSES: &[Pass] = &[format::run, rename::run, moves::run, extract::run];

pub fn classify(change: &Change) -> Labels {
    let mut labels = defaults(change);
    for pass in PASSES {
        pass(change, &mut labels);
    }
    for label in labels.iter_mut().flatten() {
        label.residual.lines = label.unclaimed();
    }
    facets::run(change, &mut labels);
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
            let residual = Residual {
                lines: Claim::default(),
                // No comparison yet: the label is the safe default, not a finding.
                confidence: Confidence::new(0.0),
                reason,
                evidence: Evidence {
                    method,
                    values: Map::new(),
                },
                detail: Map::new(),
                facets: Vec::new(),
            };
            file.diff
                .hunks
                .iter()
                .map(|hunk| Label {
                    changed: Claim::of(hunk),
                    parts: Vec::new(),
                    residual: residual.clone(),
                })
                .collect()
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
