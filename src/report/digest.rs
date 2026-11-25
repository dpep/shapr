//! `--digest`: the report narrowed to what a reviewer reads first. A view
//! over the [`Report`]; it computes nothing the full output doesn't hold.

use super::{
    HunkRecord, Report, Verdict, calls_text, compared, emit, facet_labels, quote_path, summary_text,
};
use crate::change::Category;
use crate::classify::{Facet, Kind};
use crate::norms::Direction;
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::io::{self, Write};

/// Items listed before the rest are only placed.
const SHOWN: usize = 15;

#[derive(Serialize)]
pub struct Digest<'a> {
    record: &'static str,
    schema: u32,
    subject: Option<String>,
    input: Option<super::Input>,
    verdict: Verdict,
    hunks: usize,
    review_lines: usize,
    test_review_lines: usize,
    refactor_share: f64,
    claims: Vec<&'static str>,
    unproven: Vec<Claim<'a>>,
    read_first: Vec<Item<'a>>,
    more: More<'a>,
    unproven_structure: Vec<Unproven<'a>>,
    skip: Skip,
    style_checked: usize,
    style: Vec<Style<'a>>,
    outside: BTreeMap<&'static str, Outside<'a>>,
}

/// `file:line`, its line on the before side when DELETED: the hunk only deletes.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Place<'a>(&'a str, usize, bool);

impl Serialize for Place<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{}:{}{}", self.0, self.1, deleted(self.2)))
    }
}

fn deleted(deleted: bool) -> &'static str {
    if deleted { " (deleted)" } else { "" }
}

/// PATH in text: as git quotes it, and quoted too where it holds a
/// separator places use, so `a b.rb:3` can't read as two places.
fn text_path(path: &str) -> std::borrow::Cow<'_, str> {
    match quote_path(path) {
        std::borrow::Cow::Borrowed(p) if p.contains([' ', ',', ':']) => format!("\"{p}\"").into(),
        quoted => quoted,
    }
}

/// A claim the code doesn't bear out, as `summary.claims` has it, with
/// every hunk behind it placed.
#[derive(Serialize)]
struct Claim<'a> {
    claim: crate::claims::Category,
    finding: crate::claims::Finding,
    hunks: Vec<Place<'a>>,
}

/// Lines to read: one hunk's residual or proven part that still needs
/// reading, or several of one kind sharing a reason.
#[derive(Serialize)]
struct Item<'a> {
    at: Vec<Place<'a>>,
    /// `logic`, or the structural kind proven but still read (extract).
    kind: &'static str,
    review_lines: usize,
    facets: Vec<&'static str>,
    /// Changed lines per structural kind proven beside the logic, over its hunks.
    with: BTreeMap<&'static str, usize>,
    test: bool,
    /// The part's reason: for a residual, why it isn't the kind it resembles.
    note: Option<String>,
    relocated: Option<String>,
    #[serde(skip)]
    relocated_lines: usize,
    #[serde(skip)]
    risk: (bool, usize),
    /// The facets with their detail, for text.
    #[serde(skip)]
    labels: Vec<String>,
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
    review_lines: usize,
    why: Vec<&'static str>,
}

/// The hunks of one category the verdict doesn't count.
#[derive(Serialize, Default)]
struct Outside<'a> {
    review_lines: usize,
    at: Vec<Place<'a>>,
    /// The verdict of a hunk shapr compared, if any was.
    #[serde(skip)]
    compared: Option<Verdict>,
}

/// Proven structure with no line to read, as counts: what a reviewer can skip.
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
pub(super) fn place<'a>(h: &HunkRecord<'a>) -> Place<'a> {
    match h.after.count {
        0 => Place(h.file, h.before.start, true),
        _ => Place(h.file, h.after.start, false),
    }
}

pub fn build<'a>(report: &'a Report) -> Digest<'a> {
    let s = &report.summary;
    let mut items: Vec<Item> = Vec::new();
    let mut skip = Skip::default();
    let mut unproven_structure = Vec::new();
    let mut style = Vec::new();
    let mut style_checked = 0;
    let mut outside: BTreeMap<&'static str, Outside> = BTreeMap::new();
    for h in &report.hunks {
        if !h.verdict.counted() {
            let group = outside.entry(h.category.as_str()).or_default();
            group.review_lines += h.lines.review_lines;
            group.at.push(place(h));
            if compared(h) {
                group.compared = Some(h.verdict);
            }
            continue;
        }
        for p in h.parts.iter().filter(|p| p.kind != Kind::Logic) {
            if p.structure && p.review_lines == 0 {
                *skip.kinds.entry(p.kind.as_str()).or_insert(0) += 1;
                skip.changed_lines += p.changed;
            } else if !p.structure {
                let mut why: Vec<&'static str> = p.assumptions.iter().map(|a| a.as_str()).collect();
                why.extend(p.context.iter().map(|c| c.as_str()));
                unproven_structure.push(Unproven {
                    at: place(h),
                    kind: p.kind.as_str(),
                    review_lines: p.review_lines,
                    why,
                });
            }
        }
        if h.verdict == Verdict::Structure && h.lines.review_lines == 0 {
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
                        at: Place(h.file, d.line, false),
                        norm: e.norm,
                        expected: expected.clone(),
                    });
                }
            }
        }
        // Placement follows review_lines: proven structure still read is listed.
        for r in (h.parts.iter())
            .filter(|p| p.review_lines > 0 && (p.kind == Kind::Logic || p.structure))
        {
            let facets: Vec<&'static str> = r.facets.iter().map(|f| f.as_str()).collect();
            let mut labels = facet_labels(r);
            labels.extend(calls_text(r));
            let reason = r
                .reason
                .strip_prefix(crate::classify::PLAIN_LOGIC)
                .map_or(r.reason.as_str(), |rest| rest.trim_start_matches("; "));
            let note = (!reason.is_empty()).then(|| reason.to_string());
            let kind = r.kind.as_str();
            let logic = r.kind == Kind::Logic;
            let test = h.category == Category::Test || r.facets.contains(&Facet::Test);
            let mut with = BTreeMap::new();
            for p in (h.parts.iter()).filter(|p| logic && p.kind != Kind::Logic) {
                *with.entry(p.kind.as_str()).or_insert(0) += p.changed;
            }
            let relocated = if logic { h.lines.relocated_lines } else { 0 };
            // Items sharing a reason read as one, across files; tests apart,
            // as a reviewer weighs them apart.
            if let Some(item) = items.iter_mut().find(|i| {
                note.is_some()
                    && (i.kind, &i.note, &i.labels, i.test) == (kind, &note, &labels, test)
            }) {
                item.at.push(place(h));
                item.review_lines += r.review_lines;
                for (k, n) in with {
                    *item.with.entry(k).or_insert(0) += n;
                }
                item.relocated_lines += relocated;
                item.relocated = match item.relocated_lines {
                    0 => None,
                    n => Some(format!(
                        "{n} line{} relocated verbatim across {} hunks",
                        plural(n),
                        item.at.len()
                    )),
                };
                continue;
            }
            let risky = (r.facets.iter())
                .filter(|f| !matches!(f, Facet::Test | Facet::LiteralOnly))
                .count();
            items.push(Item {
                at: vec![place(h)],
                kind,
                review_lines: r.review_lines,
                facets,
                with,
                test,
                relocated: h.relocated_note.clone().filter(|_| logic),
                relocated_lines: relocated,
                risk: (!r.detail.contains_key("near"), risky),
                note,
                labels,
            });
        }
    }
    // A near miss of a structural kind first, then the most lines, then the
    // most facets: a facet alone marks a rewrite as readily as a risk.
    items.sort_by_key(|i| (i.risk.0, std::cmp::Reverse((i.review_lines, i.risk.1))));
    let rest = items.split_off(items.len().min(SHOWN));
    let more = More {
        items: rest.len(),
        review_lines: rest.iter().map(|i| i.review_lines).sum(),
        at: rest.iter().flat_map(|i| i.at.iter().copied()).collect(),
    };
    let claims = s.claims.experimental.categories.iter();
    let unproven = (s.claims.experimental.conflicts.iter())
        .map(|c| Claim {
            claim: c.claim,
            finding: c.finding,
            hunks: (c.hunks.iter())
                .filter_map(|&id| report.hunks.get(id))
                .map(place)
                .collect(),
        })
        .collect();
    Digest {
        record: "digest",
        schema: super::SCHEMA,
        subject: (report.meta.source.as_ref())
            .map(|s| s.name())
            .or(report.meta.subject.clone()),
        input: report.meta.input,
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

/// Places as text, every one, each file's lines together in the order the
/// files first appear, deletions apart: `a.rb:3,9 a.rb:20 (deleted) b.rb:1`.
pub(super) fn places(at: &[Place]) -> String {
    let mut files: Vec<(&str, bool, Vec<String>)> = Vec::new();
    for p in at {
        match files
            .iter_mut()
            .find(|(file, d, _)| (*file, *d) == (p.0, p.2))
        {
            Some((.., lines)) => lines.push(p.1.to_string()),
            None => files.push((p.0, p.2, vec![p.1.to_string()])),
        }
    }
    (files.iter())
        .map(|(file, d, lines)| format!("{}:{}{}", text_path(file), lines.join(","), deleted(*d)))
        .collect::<Vec<_>>()
        .join(" ")
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
    summary_text(out, report, true)?;
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
        if i.kind != Kind::Logic.as_str() {
            line.push_str(&format!(", proven {}, still read", i.kind));
        }
        if !i.labels.is_empty() {
            line.push_str(&format!(" [{}]", i.labels.join(", ")));
        }
        if i.test && !i.facets.contains(&"test") {
            line.push_str(" (test)");
        }
        if !i.with.is_empty() {
            let with: Vec<String> = (i.with.iter())
                .map(|(k, n)| format!("{n} {k} line{}", plural(*n)))
                .collect();
            line.push_str(&format!("; beside it {}", with.join(", ")));
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
                "unproven {} at {}, {} line{} ({})",
                u.kind,
                places(&[u.at]),
                u.review_lines,
                plural(u.review_lines),
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
                "skip, proven structure: {} changed line{} ({}); {} hunk{} wholly structure",
                d.skip.changed_lines,
                plural(d.skip.changed_lines),
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
    for (category, group) in &d.outside {
        let lines = match group.review_lines {
            0 => String::new(),
            n => format!(", {n} review line{}", plural(n)),
        };
        let what = match group.compared {
            Some(verdict) => format!("verdict {}", verdict.as_str()),
            None => "not classified".to_string(),
        };
        emit(
            out,
            &format!("{what} ({category}){lines}: {}", places(&group.at)),
        )?;
    }
    Ok(())
}
