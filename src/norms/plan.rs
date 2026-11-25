//! A cleanup plan toward a codebase's norms: one step per norm, ordered by
//! risk tier, then by the adherence it gains per line it touches. Each
//! step's edits are the deviations' own fixes, kept only when the edited
//! file is the same program (`edit::keeps`), and written against the file
//! as the steps before it leave it, so applied in order they all apply.

use super::codebase::{Agreement, Pooled, Source, checked};
use super::{Replace, edit};
use crate::classify::proof::Outside;
use crate::tree::Tree;
use serde::Serialize;
use std::collections::BTreeSet;

/// Risk tiers, least disruptive first: what each touches, and the kind
/// shapr's classifier gives a change made of its edits alone.
const TIERS: [(&str, &str); 3] = [
    ("layout", "format_only"),
    ("lexical", "restyle"),
    ("naming", "rename"),
];

/// The tier a norm's edits belong to, 1-based; `None` for a norm the plan
/// doesn't know, which it leaves alone.
fn tier(norm: &str) -> Option<usize> {
    match norm.split('/').next()? {
        "Layout" => Some(1),
        "Style" => Some(2),
        "Naming" => Some(3),
        _ => None,
    }
}

#[derive(Serialize)]
pub struct Step {
    pub record: &'static str,
    pub step: usize,
    pub tier: usize,
    pub risk: &'static str,
    pub expect_kind: &'static str,
    pub norm: &'static str,
    pub style: String,
    pub fixes: usize,
    pub unfixed: usize,
    pub withheld: usize,
    pub adherence_before: Option<f64>,
    pub adherence_after: Option<f64>,
    pub files: usize,
    pub lines_touched: usize,
    pub edits: Vec<Edit>,
}

#[derive(Serialize)]
pub struct Edit {
    pub file: String,
    /// Lines the edit changes, in the file as the steps before leave it.
    pub lines: Vec<usize>,
    pub diff: String,
}

pub struct Plan {
    pub steps: Vec<Step>,
    /// Per pooled norm: its step number, or why it has none.
    pub planned: Vec<Result<usize, Option<Unplanned>>>,
}

/// Why a norm with deviations has no step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unplanned {
    /// Config sets a style the code disagrees with.
    ConfigDisagrees,
    /// Config doesn't check any deviation.
    Withheld,
    /// No deviation has a mechanical edit that keeps the program.
    NoEdit,
}

/// Per file, the fixes kept for one norm: each deviation's replacements.
type Kept = Vec<Vec<Vec<Replace>>>;

/// A step before its edits are written: what pass one found.
struct Draft {
    norm: usize,
    tier: usize,
    style: String,
    kept: Kept,
    unfixed: usize,
    withheld: usize,
    /// Adherence gained per line touched, for the order.
    gain: f64,
}

pub fn plan(sources: &[Source], pooled: &[Pooled], outside: &dyn Outside) -> Plan {
    let trees: Vec<std::cell::OnceCell<Option<Tree>>> =
        sources.iter().map(|_| Default::default()).collect();
    let tree = |f: usize| {
        trees[f]
            .get_or_init(|| {
                crate::frontend::for_path(&sources[f].path)
                    .map(|fe| fe.parse(sources[f].bytes.clone()))
            })
            .as_ref()
    };
    let mut planned: Vec<Result<usize, Option<Unplanned>>> = vec![Err(None); pooled.len()];
    let mut drafts: Vec<Draft> = Vec::new();
    for (n, p) in pooled.iter().enumerate() {
        let Some(tier) = tier(p.def.id) else { continue };
        if p.deviations().next().is_none() {
            continue;
        }
        let disagrees = p
            .config
            .as_ref()
            .is_some_and(|c| c.agreement == Agreement::Disagrees);
        let Some(style) = p.expected().filter(|_| !disagrees) else {
            planned[n] = Err(Some(Unplanned::ConfigDisagrees));
            continue;
        };
        let (mut unfixed, mut withheld, mut total) = (0, 0, 0);
        let mut kept: Kept = vec![Vec::new(); sources.len()];
        for (f, o, expected) in p.deviations() {
            total += 1;
            let source = &sources[f];
            let check = checked(source, p.def, o);
            if check.withheld {
                withheld += 1;
                unfixed += 1;
                continue;
            }
            let fix = (o.fix.iter())
                .find(|(s, _)| s == expected)
                .map(|(_, r)| edit::endings(&source.bytes, r));
            // A file's own config setting another style isn't overridden.
            let fights = check.style.is_some_and(|s| s != expected);
            match fix {
                Some(fix) if !fights => kept[f].push(fix),
                _ => unfixed += 1,
            }
        }
        // Each file's fixes, kept together when the whole is the same
        // program, else one by one as each keeps it.
        for (f, fixes) in kept.iter_mut().enumerate() {
            if fixes.is_empty() {
                continue;
            }
            let source = &sources[f];
            let Some(tree) = tree(f) else {
                unfixed += fixes.len();
                fixes.clear();
                continue;
            };
            let shared = |r: &[Replace]| {
                !p.def.shared || edit::unshared(outside, &source.rel, &source.bytes, r)
            };
            let all = fixes.clone();
            fixes.retain(|r| shared(r));
            let whole: Vec<Replace> = fixes.iter().flatten().cloned().collect();
            if overlaps(&whole) || !edit::keeps(&source.path, tree, &whole, p.def.renames) {
                let mut so_far: Vec<Replace> = Vec::new();
                fixes.retain(|r| {
                    let mut with = so_far.clone();
                    with.extend(r.iter().cloned());
                    let ok =
                        !overlaps(&with) && edit::keeps(&source.path, tree, &with, p.def.renames);
                    if ok {
                        so_far = with;
                    }
                    ok
                });
            }
            unfixed += all.len() - fixes.len();
        }
        let fixed: usize = kept.iter().map(Vec::len).sum();
        if fixed == 0 {
            planned[n] = Err(Some(if withheld == total {
                Unplanned::Withheld
            } else {
                Unplanned::NoEdit
            }));
            continue;
        }
        let touched: usize = (kept.iter().enumerate())
            .map(|(f, fixes)| lines(&sources[f].bytes, fixes.iter().flatten()).len())
            .sum();
        let (_, judged) = p.adherence();
        drafts.push(Draft {
            norm: n,
            tier,
            style,
            kept,
            unfixed,
            withheld,
            gain: fixed as f64 / judged.max(1) as f64 / touched.max(1) as f64,
        });
    }
    drafts.sort_by(|a, b| {
        (a.tier.cmp(&b.tier))
            .then(b.gain.total_cmp(&a.gain))
            .then(pooled[a.norm].def.id.cmp(pooled[b.norm].def.id))
    });
    // Pass two: each file's edits so far, in its original offsets; each
    // step's written against the file as they leave it.
    let mut applied: Vec<Vec<Replace>> = vec![Vec::new(); sources.len()];
    let mut renamed = vec![false; sources.len()];
    let mut steps = Vec::new();
    for draft in drafts {
        let p = &pooled[draft.norm];
        let mut edits = Vec::new();
        let (mut fixes, mut unfixed, mut touched) = (0, draft.unfixed, 0);
        for (f, kept) in draft.kept.iter().enumerate() {
            if kept.is_empty() {
                continue;
            }
            let source = &sources[f];
            let new: Vec<Vec<Replace>> = (kept.iter())
                .filter(|r| !r.iter().any(|r| applied[f].iter().any(|a| clash(a, r))))
                .cloned()
                .collect();
            let mut all = applied[f].clone();
            all.extend(new.iter().flatten().cloned());
            let renames = renamed[f] || p.def.renames;
            let ok = !new.is_empty()
                && tree(f).is_some_and(|t| edit::keeps(&source.path, t, &all, renames));
            if !ok {
                unfixed += kept.len();
                continue;
            }
            let before = edit::apply(&source.bytes, &applied[f]);
            let shifted: Vec<Replace> = (new.iter().flatten())
                .map(|r| shift(r, &applied[f]))
                .collect();
            let Some(diff) = edit::patch(&source.path, &before, &shifted) else {
                unfixed += kept.len();
                continue;
            };
            let at = lines(&before, shifted.iter());
            unfixed += kept.len() - new.len();
            fixes += new.len();
            touched += at.len();
            applied[f] = all;
            renamed[f] = renames;
            edits.push(Edit {
                file: source.path.clone(),
                lines: at.into_iter().collect(),
                diff,
            });
        }
        if fixes == 0 {
            planned[draft.norm] = Err(Some(Unplanned::NoEdit));
            continue;
        }
        let (matching, judged) = p.adherence();
        let (risk, expect_kind) = TIERS[draft.tier - 1];
        steps.push(Step {
            record: "step",
            step: steps.len() + 1,
            tier: draft.tier,
            risk,
            expect_kind,
            norm: p.def.id,
            style: draft.style,
            fixes,
            unfixed,
            withheld: draft.withheld,
            adherence_before: super::codebase::share(matching, judged),
            adherence_after: super::codebase::share(matching + fixes, judged),
            files: edits.len(),
            lines_touched: touched,
            edits,
        });
        planned[draft.norm] = Ok(steps.len());
    }
    Plan { steps, planned }
}

/// Two replacements touch the same bytes, or insert at the same place.
fn clash(a: &Replace, b: &Replace) -> bool {
    (a.span.start < b.span.end && b.span.start < a.span.end) || a.span.start == b.span.start
}

fn overlaps(replaces: &[Replace]) -> bool {
    let mut sorted = replaces.to_vec();
    sorted.sort_by_key(|r| (r.span.start, r.span.end));
    sorted.windows(2).any(|w| clash(&w[0], &w[1]))
}

/// R's span in the file after APPLIED, which it doesn't clash with.
fn shift(r: &Replace, applied: &[Replace]) -> Replace {
    let delta: isize = (applied.iter())
        .filter(|a| a.span.end <= r.span.start)
        .map(|a| a.text.len() as isize - a.span.len() as isize)
        .sum();
    let at = |offset: usize| (offset as isize + delta) as usize;
    Replace {
        span: at(r.span.start)..at(r.span.end),
        text: r.text.clone(),
    }
}

/// The lines of SOURCE the replacements change.
fn lines<'a>(source: &[u8], replaces: impl Iterator<Item = &'a Replace>) -> BTreeSet<usize> {
    let index = super::LineIndex::new(source);
    replaces
        .flat_map(|r| {
            let first = index.line(r.span.start).max(1);
            first..=index.line(r.span.end.max(r.span.start + 1) - 1).max(first)
        })
        .collect()
}
