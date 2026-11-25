//! `--digest`: the report narrowed to what a reviewer reads first. A view
//! over the [`Report`]; it computes nothing the full output doesn't hold.

use super::{HunkRecord, Report, Verdict, emit, quote_path, summary_text};
use crate::change::Category;
use crate::classify::{Facet, Kind};
use crate::norms::Direction;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{self, Write};

/// Logic items listed before the rest are counted.
const SHOWN: usize = 10;

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
    more: More,
    unproven_structure: Vec<Unproven<'a>>,
    skip: Skip,
    style: Vec<Style<'a>>,
    outside: BTreeMap<&'static str, usize>,
}

/// Logic to read: one hunk, or hunks of one file sharing a note.
#[derive(Serialize)]
struct Item<'a> {
    file: &'a str,
    /// Each hunk's first line, on the after side unless it only deletes.
    lines: Vec<usize>,
    review_lines: usize,
    facets: Vec<&'static str>,
    /// The structural kinds proven beside the logic.
    beside: Vec<&'static str>,
    test: bool,
    /// Why the residual isn't the structural kind it resembles.
    note: Option<String>,
    relocated: Option<String>,
    #[serde(skip)]
    risk: (bool, bool, usize),
}

#[derive(Serialize, Default)]
struct More {
    items: usize,
    review_lines: usize,
}

/// A structural kind shapr couldn't prove: what it assumed, or where it landed.
#[derive(Serialize)]
struct Unproven<'a> {
    file: &'a str,
    line: usize,
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
    file: &'a str,
    line: usize,
    norm: &'static str,
    expected: String,
}

fn first_line(h: &HunkRecord) -> usize {
    match h.after.count {
        0 => h.before.start,
        _ => h.after.start,
    }
}

pub fn build<'a>(report: &'a Report) -> Digest<'a> {
    let s = &report.summary;
    let mut items: Vec<Item> = Vec::new();
    let mut skip = Skip::default();
    let mut unproven_structure = Vec::new();
    let mut style = Vec::new();
    let mut outside = BTreeMap::new();
    for h in &report.hunks {
        if !h.verdict.counted() {
            *outside.entry(h.category.as_str()).or_insert(0) += 1;
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
                    file: h.file,
                    line: first_line(h),
                    kind: p.kind.as_str(),
                    why,
                });
            }
        }
        if h.verdict == Verdict::Structure {
            skip.hunks += 1;
        }
        for e in h.style.iter().flat_map(|st| &st.norms) {
            let (Direction::Diverging, Some(expected)) = (e.direction, &e.expected) else {
                continue;
            };
            for d in &e.deviations {
                style.push(Style {
                    file: h.file,
                    line: d.line,
                    norm: e.norm,
                    expected: expected.clone(),
                });
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
        let test = h.category == Category::Test || r.facets.contains(&Facet::Test);
        let beside: Vec<&'static str> = (h.parts.iter())
            .filter(|p| p.kind != Kind::Logic)
            .map(|p| p.kind.as_str())
            .collect();
        // Hunks of one file with the same note read as one item.
        if let Some(item) = items
            .iter_mut()
            .find(|i| note.is_some() && i.file == h.file && i.note == note && i.facets == facets)
        {
            item.lines.push(first_line(h));
            item.review_lines += r.review_lines;
            continue;
        }
        let risky = (r.facets.iter())
            .filter(|f| !matches!(f, Facet::Test | Facet::LiteralOnly))
            .count();
        items.push(Item {
            file: h.file,
            lines: vec![first_line(h)],
            review_lines: r.review_lines,
            facets,
            beside,
            test,
            relocated: h.relocated_note.clone(),
            risk: (test, !r.detail.contains_key("near"), risky),
            note,
        });
    }
    // Code before tests, a near miss of a structural kind first, then the
    // most facets and the most lines.
    items.sort_by(|a, b| {
        let key = |i: &Item| {
            (
                i.risk.0,
                i.risk.1,
                std::cmp::Reverse((i.risk.2, i.review_lines)),
            )
        };
        key(a).cmp(&key(b))
    });
    let rest = items.split_off(items.len().min(SHOWN));
    let more = More {
        items: rest.len(),
        review_lines: rest.iter().map(|i| i.review_lines).sum(),
    };
    let claims = s.claims.experimental.categories.iter();
    let unproven = super::conflicts(&s.claims, false)
        .into_iter()
        .map(|(label, line)| format!("{label}: {line}"))
        .collect();
    Digest {
        record: "digest",
        schema: super::SCHEMA,
        subject: report
            .meta
            .source
            .as_ref()
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
        style,
        outside,
    }
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
        let at: Vec<String> = i.lines.iter().map(|l| l.to_string()).collect();
        let mut line = format!(
            "  {}:{}  {} line{}",
            quote_path(i.file),
            at.join(","),
            i.review_lines,
            if i.review_lines == 1 { "" } else { "s" }
        );
        if !i.facets.is_empty() {
            line.push_str(&format!(" [{}]", i.facets.join(", ")));
        }
        if !i.beside.is_empty() {
            line.push_str(&format!(" beside {}", i.beside.join(", ")));
        }
        if i.test && !i.facets.contains(&"test") {
            line.push_str(" (test)");
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
                "  and {} more, {} review lines",
                d.more.items, d.more.review_lines
            ),
        )?;
    }
    for u in &d.unproven_structure {
        emit(
            out,
            &format!(
                "unproven {} at {}:{} ({})",
                u.kind,
                quote_path(u.file),
                u.line,
                u.why.join(", ")
            ),
        )?;
    }
    if d.skip.changed_lines > 0 {
        let kinds: Vec<String> = d
            .skip
            .kinds
            .iter()
            .map(|(k, n)| format!("{n} {k}"))
            .collect();
        emit(
            out,
            &format!(
                "skip, proven structure: {} changed lines ({}); {} hunk{} wholly structure",
                d.skip.changed_lines,
                kinds.join(", "),
                d.skip.hunks,
                if d.skip.hunks == 1 { "" } else { "s" }
            ),
        )?;
    }
    for st in &d.style {
        emit(
            out,
            &format!(
                "style {}:{} {}: expects {}",
                quote_path(st.file),
                st.line,
                st.norm,
                st.expected
            ),
        )?;
    }
    if !d.outside.is_empty() {
        let parts: Vec<String> = d.outside.iter().map(|(c, n)| format!("{n} {c}")).collect();
        emit(
            out,
            &format!("outside the verdict: {} hunks", parts.join(", ")),
        )?;
    }
    Ok(())
}
