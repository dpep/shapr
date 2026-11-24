//! Labels every hunk of a change: the structural parts passes prove, and the
//! residual, the changed lines no part explains, which is `logic`.
//!
//! Every hunk starts as all residual, the safe label. Passes then run in
//! order over the whole change, so a pass may match across hunks and files (a
//! move pairs a removal in one file with an addition in another). A pass
//! claims lines with [`Label::claim`], and each kind lives in its own module.

pub mod facets;
pub mod site;

mod align;
mod extract;
mod format;
mod hazard;
mod moves;
mod rename;

pub use facets::Facet;
pub use moves::Context;

use crate::change::Change;
use crate::diff::Hunk;
use align::Alignments;
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
    Extract,
    Inline,
    Logic,
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::FormatOnly,
        Kind::CommentOnly,
        Kind::Rename,
        Kind::Move,
        Kind::Extract,
        Kind::Inline,
        Kind::Logic,
    ];

    /// Changes structure without changing behavior. Says nothing about risk.
    pub fn is_structure(self) -> bool {
        self != Kind::Logic
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::FormatOnly => "format_only",
            Kind::CommentOnly => "comment_only",
            Kind::Rename => "rename",
            Kind::Move => "move",
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

/// A score in 0..=1, rounded to two decimals where it is built: the
/// precision evidence from a handful of counts honestly has.
pub fn rounded(value: f64) -> f64 {
    (value.clamp(0.0, 1.0) * 100.0).round() / 100.0
}

impl Confidence {
    pub fn new(value: f64) -> Confidence {
        Confidence(rounded(value))
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
    /// What differs around the part's code in its new place (a move into
    /// another class): its lines are explained, but its meaning can change.
    pub context: Vec<Context>,
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

impl Part {
    /// Changes structure without changing behavior.
    pub fn structure(&self) -> bool {
        self.context.is_empty()
    }
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

    /// Adds `part` when no other part claims any of its lines: parts are
    /// disjoint, so a contested line is never silently given to one of
    /// them. Returns whether it was added.
    pub fn claim(&mut self, part: Part) -> bool {
        let taken = self.changed.minus(&self.unclaimed());
        let fits = !part.lines.is_empty()
            && part.lines.intersect(&taken).is_empty()
            && part.lines.minus(&self.changed).is_empty()
            && part.review.minus(&part.lines).is_empty();
        if fits {
            self.parts.push(part);
        }
        fits
    }

    /// There is logic: lines no part explains, or no part at all.
    pub fn has_residual(&self) -> bool {
        self.parts.is_empty() || !self.residual.lines.is_empty()
    }

    /// Structure parts only, behavior only (a residual, or a part whose
    /// meaning can change), or both.
    pub fn verdict(&self) -> Verdict {
        let structure = self.parts.iter().any(|p| p.structure());
        let behavior = self.has_residual() || self.parts.iter().any(|p| !p.structure());
        // Without a structure part there is a residual or a part that isn't
        // structure: behavior.
        match (structure, behavior) {
            (true, false) => Verdict::Structure,
            (true, true) => Verdict::Mixed,
            (false, _) => Verdict::Behavior,
        }
    }

    /// The one-word rollup: `logic` unless the hunk is all structure, else
    /// its most substantial part kind, in [`Kind::ALL`] order (a rename
    /// beside a reformat is a `rename`).
    pub fn kind(&self) -> Kind {
        match self.verdict() {
            Verdict::Structure => self
                .parts
                .iter()
                .map(|p| p.kind)
                .max()
                .expect("a structure hunk has a part"),
            _ => Kind::Logic,
        }
    }
}

/// Whether a hunk, or a whole change, changes structure, behavior or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Structure only.
    Structure,
    /// Behavior only.
    Behavior,
    Mixed,
}

/// One label per hunk: `labels[file][hunk]`, parallel to `Change::files`.
pub type Labels = Vec<Vec<Label>>;

/// A pass proves parts against the whole of both trees and claims the lines
/// they explain with [`Label::claim`], which refuses a part overlapping
/// another. The residual is never cut out of the trees, so a check that new
/// code would break fails and the lines stay logic. A pass may also leave a
/// near miss on the residual.
type Pass = fn(&Change, &Alignments, &mut Labels);

/// In order. A pass sees which lines earlier passes claimed and proposes
/// parts for the rest; `format::rest` takes what is left.
const PASSES: &[Pass] = &[
    format::run,
    rename::run,
    moves::run,
    extract::run,
    format::rest,
];

pub fn classify(change: &Change) -> Labels {
    let mut labels = defaults(change);
    let alignments = Alignments::of(change);
    for pass in PASSES {
        pass(change, &alignments, &mut labels);
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

    fn part(kind: Kind, before: &[usize], after: &[usize]) -> Part {
        Part {
            kind,
            context: Vec::new(),
            lines: Claim {
                before: before.iter().copied().collect(),
                after: after.iter().copied().collect(),
            },
            review: Claim::default(),
            confidence: Confidence::new(1.0),
            reason: String::new(),
            evidence: Evidence {
                method: Method::Ast,
                values: Map::new(),
            },
            detail: Map::new(),
        }
    }

    #[test]
    fn parts_never_share_a_line() {
        let hunk = Hunk {
            removed: vec![1, 2],
            added: vec![1, 2],
            ..Hunk::default()
        };
        let mut label = Label {
            changed: Claim::of(&hunk),
            parts: Vec::new(),
            residual: Residual {
                lines: Claim::default(),
                confidence: Confidence::new(1.0),
                reason: String::new(),
                evidence: Evidence {
                    method: Method::Ast,
                    values: Map::new(),
                },
                detail: Map::new(),
                facets: Vec::new(),
            },
        };
        assert!(label.claim(part(Kind::Rename, &[1], &[1])));
        // Contested: refused whole, not trimmed to line 2.
        assert!(!label.claim(part(Kind::FormatOnly, &[1, 2], &[2])));
        assert!(!label.claim(part(Kind::FormatOnly, &[], &[])));
        assert!(!label.claim(part(Kind::FormatOnly, &[3], &[])));
        assert_eq!(label.parts.len(), 1);
        label.residual.lines = label.unclaimed();
        assert_eq!(label.verdict(), Verdict::Mixed);
        assert_eq!(label.kind(), Kind::Logic);
        assert!(label.claim(part(Kind::FormatOnly, &[2], &[2])));
        label.residual.lines = label.unclaimed();
        assert_eq!(label.verdict(), Verdict::Structure);
        assert_eq!(label.kind(), Kind::Rename);
    }

    #[test]
    fn kind_names_match_serde() {
        for kind in Kind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
    }
}
