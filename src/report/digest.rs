//! `--digest`: the report narrowed to what a reviewer reads first. A view
//! over the [`Report`]; it computes nothing the full output doesn't hold.

use super::{HunkRecord, Report, Verdict, emit, quote_path, summary_text};
use crate::change::Category;
use crate::classify::{Facet, Kind};
use crate::norms::Direction;
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::io::{self, Write};

/// Logic items listed before the rest are only placed.
const SHOWN: usize = 15;
/// Places a text line names before counting the rest.
const PLACES: usize = 12;

#[derive(Serialize)]
pub struct Digest<'a> {
    record: &'static str,
    schema: u32,
    subject: Option<String>,
    verdict: Verdict,
    hunks: usize,
    review_lines: usize,
    test_review_lines: usize,
    refactor_share: f64,
    claims: Vec<&'static str>,
    unproven: Vec<String>,
    read_first: Vec<Item<'a>>,
    more: More<'a>,
    unproven_structure: Vec<Unproven<'a>>,
    skip: Skip,
    style_checked: usize,
    style: Vec<Style<'a>>,
    outside: BTreeMap<&'static str, Vec<Place<'a>>>,
}

/// `file:line`.
#[derive(Clone, Copy, PartialEq)]
struct Place<'a>(&'a str, usize);

impl Serialize for Place<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{}:{}", self.0, self.1))
    }
}

/// Logic to read: one hunk's residual, or the residuals sharing a reason.
#[derive(Serialize)]
struct Item<'a> {
    at: Vec<Place<'a>>,
    review_lines: usize,
    facets: Vec<&'static str>,
    /// The structural kinds proven in the same hunk.
    with: Vec<&'static str>,
    test: bool,
    /// Why the residual isn't the structural kind it resembles.
    note: Option<String>,
    relocated: Option<String>,
    #[serde(skip)]
    risk: (bool, usize),
}

#[derive(Serialize, Default)]
struct More<'a> {
    items: usize,
    review_lines: usize,
    at: Vec<Place<'a>>,
}

/// A structural kind shapr couldn't prove: what it assumed, or where it landed.
#[derive(Serialize)]
struct Unproven<'a> {
    at: Place<'a>,
    kind: &'static str,
    why: Vec<&'static str>,
}

/// Proven structure, as counts: what a reviewer can skip.
#[derive(Serialize, Default)]
struct Skip {
    hunks: usize,
    changed_lines: usize,
    kinds: BTreeMap<&'static str, usize>,
}

#[derive(Serialize)]
struct Style<'a> {
    at: Place<'a>,
    norm: &'static str,
    expected: String,
}

/// The hunk's first line, on the after side unless it only deletes.
fn place<'a>(h: &HunkRecord<'a>) -> Place<'a> {
    Place(
        h.file,
        match h.after.count {
            0 => h.before.start,
            _ => h.after.start,
        },
    )
}

pub fn build<'a>(report: &'a Report) -> Digest<'a> {
    let s = &report.summary;
    let mut items: Vec<Item> = Vec::new();
    let mut skip = Skip::default();
    let mut unproven_structure = Vec::new();
    let mut style = Vec::new();
    let mut style_checked = 0;
    let mut outside: BTreeMap<&'static str, Vec<Place>> = BTreeMap::new();
    for h in &report.hunks {
        if !h.verdict.counted() {
            outside
                .entry(h.category.as_str())
                .or_default()
                .push(place(h));
            continue;
        }
        for p in h.parts.iter().filter(|p| p.kind != Kind::Logic) {
            if p.structure {
                *skip.kinds.entry(p.kind.as_str()).or_insert(0) += 1;
                skip.changed_lines += p.changed;
            } else {
                let mut why: Vec<&'static str> = p.assumptions.iter().map(|a| a.as_str()).collect();
                why.extend(p.context.iter().map(|c| c.as_str()));
                unproven_structure.push(Unproven {
                    at: place(h),
                    kind: p.kind.as_str(),
                    why,
                });
            }
        }
        if h.verdict == Verdict::Structure {
            skip.hunks += 1;
        }
        if let Some(st) = h
            .style
            .as_ref()
            .filter(|st| st.skipped.is_none() && !st.norms.is_empty())
        {
            style_checked += 1;
            for e in &st.norms {
                let (Direction::Diverging, Some(expected)) = (e.direction, &e.expected) else {
                    continue;
                };
                for d in &e.deviations {
                    style.push(Style {
                        at: Place(h.file, d.line),
                        norm: e.norm,
                        expected: expected.clone(),
                    });
                }
            }
        }
        let Some(r) = h.parts.iter().find(|p| p.kind == Kind::Logic) else {
            continue;
        };
        if r.review_lines == 0 {
            continue;
        }
        let facets: Vec<&'static str> = r.facets.iter().map(|f| f.as_str()).collect();
        let reason = r
            .reason
            .strip_prefix(crate::classify::PLAIN_LOGIC)
            .map_or(r.reason.as_str(), |rest| rest.trim_start_matches("; "));
        let note = (!reason.is_empty()).then(|| reason.to_string());
        // Residuals sharing a reason read as one item, across files.
        if let Some(item) = items
            .iter_mut()
            .find(|i| note.is_some() && i.note == note && i.facets == facets)
        {
            item.at.push(place(h));
            item.review_lines += r.review_lines;
            continue;
        }
        let risky = (r.facets.iter())
            .filter(|f| !matches!(f, Facet::Test | Facet::LiteralOnly))
            .count();
        items.push(Item {
            at: vec![place(h)],
            review_lines: r.review_lines,
            facets,
            with: (h.parts.iter())
                .filter(|p| p.kind != Kind::Logic)
                .map(|p| p.kind.as_str())
                .collect(),
            test: h.category == Category::Test || r.facets.contains(&Facet::Test),
            relocated: h.relocated_note.clone(),
            risk: (!r.detail.contains_key("near"), risky),
            note,
        });
    }
    // A near miss of a structural kind first, then the most facets and the
    // most lines.
    items.sort_by_key(|i| (i.risk.0, std::cmp::Reverse((i.risk.1, i.review_lines))));
    let rest = items.split_off(items.len().min(SHOWN));
    let more = More {
        items: rest.len(),
        review_lines: rest.iter().map(|i| i.review_lines).sum(),
        at: rest.iter().flat_map(|i| i.at.iter().copied()).collect(),
    };
    let claims = s.claims.experimental.categories.iter();
    let unproven = super::conflicts(report, false)
        .into_iter()
        .map(|(label, line)| format!("{label}: {line}"))
        .collect();
    Digest {
        record: "digest",
        schema: super::SCHEMA,
        subject: (report.meta.source.as_ref())
            .map(|s| s.name())
            .or(report.meta.subject.clone()),
        verdict: s.verdict,
        hunks: s.hunks,
        review_lines: s.lines.review_lines,
        test_review_lines: s.test_review_lines,
        refactor_share: s.refactor_share,
        claims: claims.filter(|(_, n)| **n > 0).map(|(c, _)| *c).collect(),
        unproven,
        read_first: items,
        more,
        unproven_structure,
        skip,
        style_checked,
        style,
        outside,
    }
}

/// Places as text, one file's lines together: `a.rb:3,9 b.rb:1`, the rest
/// counted past `PLACES`.
fn places(at: &[Place]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut last = None;
    for p in at.iter().take(PLACES) {
        match (last, out.last_mut()) {
            (Some(file), Some(s)) if file == p.0 => s.push_str(&format!(",{}", p.1)),
            _ => out.push(format!("{}:{}", quote_path(p.0), p.1)),
        }
        last = Some(p.0);
    }
    if at.len() > PLACES {
        out.push(format!("and {} more", at.len() - PLACES));
    }
    out.join(" ")
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

pub fn json(out: &mut impl Write, report: &Report) -> io::Result<()> {
    serde_json::to_writer(&mut *out, &build(report))?;
    writeln!(out)
}

pub fn text(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let d = build(report);
    summary_text(out, report)?;
    if !d.read_first.is_empty() {
        emit(out, "read first:")?;
    }
    for i in &d.read_first {
        let mut line = format!(
            "  {}  {} line{}",
            places(&i.at),
            i.review_lines,
            plural(i.review_lines)
        );
        if !i.facets.is_empty() {
            line.push_str(&format!(" [{}]", i.facets.join(", ")));
        }
        if i.test && !i.facets.contains(&"test") {
            line.push_str(" (test)");
        }
        if !i.with.is_empty() {
            line.push_str(&format!("; the hunk also proves {}", i.with.join(", ")));
        }
        if let Some(note) = &i.note {
            line.push_str(&format!("; {note}"));
        }
        if let Some(r) = &i.relocated {
            line.push_str(&format!("; {r}"));
        }
        emit(out, &line)?;
    }
    if d.more.items > 0 {
        emit(
            out,
            &format!(
                "  and {} more, {} review line{}: {}",
                d.more.items,
                d.more.review_lines,
                plural(d.more.review_lines),
                places(&d.more.at)
            ),
        )?;
    }
    for u in &d.unproven_structure {
        emit(
            out,
            &format!(
                "unproven {} at {} ({})",
                u.kind,
                places(&[u.at]),
                u.why.join(", ")
            ),
        )?;
    }
    if d.skip.changed_lines > 0 {
        let kinds: Vec<String> = (d.skip.kinds.iter())
            .map(|(k, n)| format!("{n} {k}"))
            .collect();
        emit(
            out,
            &format!(
                "skip, proven structure: {} changed lines ({}); {} hunk{} wholly structure",
                d.skip.changed_lines,
                kinds.join(", "),
                d.skip.hunks,
                plural(d.skip.hunks)
            ),
        )?;
    }
    if d.style.is_empty() && d.style_checked > 0 {
        emit(
            out,
            &format!(
                "style: no divergence in {} hunk{} checked",
                d.style_checked,
                plural(d.style_checked)
            ),
        )?;
    }
    for st in &d.style {
        emit(
            out,
            &format!(
                "style {} {}: expects {}",
                places(&[st.at]),
                st.norm,
                st.expected
            ),
        )?;
    }
    for (category, at) in &d.outside {
        emit(
            out,
            &format!("outside the verdict, {category}: {}", places(at)),
        )?;
    }
    Ok(())
}
