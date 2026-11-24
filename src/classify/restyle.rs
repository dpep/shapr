//! `restyle`: code that changed between spellings of the same program (quote
//! style, hash syntax, block delimiters, redundant parentheses, ...). The
//! frontend gives each such node a canonical form; the alignment pairs
//! subtrees whose canons agree though their hashes don't, and this pass
//! claims their changed lines where nothing else differs.

use super::align::Alignments;
use super::site::CodeLines;
use super::{Claim, Confidence, Evidence, Kind, Labels, Method, Outside, Part, format};
use crate::change::Change;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

pub(super) fn run(change: &Change, alignments: &Alignments, _: &dyn Outside, labels: &mut Labels) {
    for (f, aligned) in alignments.files() {
        let Ok(trees) = &change.files[f].trees else {
            continue;
        };
        let code = (CodeLines::of(&trees.before), CodeLines::of(&trees.after));
        for (cover, label) in aligned.covers.iter().zip(labels[f].iter_mut()) {
            let free = label.unclaimed();
            if cover.restyled.is_empty() || free.is_empty() {
                continue;
            }
            let logic = format::differing(trees, cover, &cover.differs, &label.changed, &free);
            let (mut lines, mut variants, mut respelled) = (Claim::default(), BTreeSet::new(), 0);
            for pair in &cover.restyled {
                let own = Claim {
                    before: code.0.touched(&free.before, |at| pair.before.contains(&at)),
                    after: code.1.touched(&free.after, |at| pair.after.contains(&at)),
                };
                let own = own.minus(&logic);
                if own.is_empty() {
                    continue;
                }
                lines.before.extend(own.before);
                lines.after.extend(own.after);
                variants.extend(pair.variants.iter().copied());
                respelled += 1;
            }
            if lines.is_empty() {
                continue;
            }
            let variants: Vec<&'static str> = variants.into_iter().collect();
            label.claim(Part {
                kind: Kind::Restyle,
                context: Vec::new(),
                assumptions: Vec::new(),
                lines,
                review: Claim::default(),
                // A tree comparison, as for a reformat.
                confidence: Confidence::new(1.0),
                reason: "same program; only equivalent spellings differ".into(),
                evidence: Evidence {
                    method: Method::Ast,
                    values: Map::from_iter([("respelled".to_string(), Value::from(respelled))]),
                },
                detail: Map::from_iter([("variants".to_string(), json!(variants))]),
                variants,
            });
        }
    }
}
