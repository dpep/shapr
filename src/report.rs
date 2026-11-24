//! The output records (the public schema, see docs/schema.md) and their
//! rendering. Everything shapr writes to stdout goes through here.

use crate::change::{Change, FileChange};
use crate::claims::{Claims, CodeFacts, Finding};
use crate::classify::{
    Claim, Confidence, Context, Evidence, Facet, Kind, Label, Labels, Method, Verdict,
};
use crate::diff::{Hunk, Lines};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

/// Bumped on any breaking change to a record's fields.
pub const SCHEMA: u32 = 1;

#[derive(Serialize)]
pub struct Report<'a> {
    pub meta: Meta,
    pub files: Vec<FileRecord<'a>>,
    pub hunks: Vec<HunkRecord<'a>>,
    pub summary: Summary,
}

/// What produced the report and from what: the first NDJSON line.
#[derive(Serialize)]
pub struct Meta {
    record: &'static str,
    schema: u32,
    version: &'static str,
    /// The GitHub pull request or commit analyzed; `None` for a local run.
    pub source: Option<crate::github::Source>,
}

impl Meta {
    pub fn new(source: Option<crate::github::Source>) -> Meta {
        Meta {
            record: "meta",
            schema: SCHEMA,
            version: env!("CARGO_PKG_VERSION"),
            source,
        }
    }
}

#[derive(Clone, Copy, Default, Serialize)]
pub struct LineCounts {
    pub added: usize,
    pub deleted: usize,
    pub code_added: usize,
    pub code_deleted: usize,
    pub comment_added: usize,
    pub comment_deleted: usize,
    pub review_lines: usize,
}

impl LineCounts {
    fn add(&mut self, other: &LineCounts) {
        self.added += other.added;
        self.deleted += other.deleted;
        self.code_added += other.code_added;
        self.code_deleted += other.code_deleted;
        self.comment_added += other.comment_added;
        self.comment_deleted += other.comment_deleted;
        self.review_lines += other.review_lines;
    }

    fn changed(&self) -> usize {
        self.added + self.deleted
    }
}

#[derive(Serialize)]
pub struct FileRecord<'a> {
    record: &'static str,
    file: &'a str,
    old_file: Option<&'a str>,
    language: Option<&'static str>,
    parsed: bool,
    generated: bool,
    lines_before: Option<usize>,
    hunks: usize,
    #[serde(flatten)]
    lines: LineCounts,
}

#[derive(Serialize)]
pub struct HunkRecord<'a> {
    record: &'static str,
    id: usize,
    file: &'a str,
    old_file: Option<&'a str>,
    language: Option<&'static str>,
    parsed: bool,
    generated: bool,
    before: Lines,
    after: Lines,
    kind: Kind,
    verdict: Verdict,
    #[serde(flatten)]
    lines: LineCounts,
    parts: Vec<PartRecord>,
    /// How the hunk was compared, for claims.
    #[serde(skip)]
    method: Method,
}

/// A part of a hunk, or its residual (kind `logic`).
#[derive(Serialize)]
pub struct PartRecord {
    kind: Kind,
    structure: bool,
    lines: Runs,
    confidence: Confidence,
    reason: String,
    evidence: Evidence,
    detail: Map<String, Value>,
    facets: Vec<Facet>,
    review_lines: usize,
    /// Changed lines it covers, for `structure_share`.
    #[serde(skip)]
    changed: usize,
    #[serde(skip)]
    context: Vec<Context>,
}

/// A part's changed lines on each side, as runs.
#[derive(Serialize)]
struct Runs {
    before: Vec<Lines>,
    after: Vec<Lines>,
}

impl Runs {
    fn of(claim: &Claim) -> Runs {
        let runs = |lines: &BTreeSet<usize>| {
            let mut out: Vec<Lines> = Vec::new();
            for &line in lines {
                match out.last_mut() {
                    Some(run) if run.start + run.count == line => run.count += 1,
                    _ => out.push(Lines {
                        start: line,
                        count: 1,
                    }),
                }
            }
            out
        };
        Runs {
            before: runs(&claim.before),
            after: runs(&claim.after),
        }
    }
}

#[derive(Serialize)]
pub struct Summary {
    record: &'static str,
    pub verdict: Verdict,
    hunks: usize,
    unparsed_hunks: usize,
    generated_hunks: usize,
    kinds: BTreeMap<&'static str, usize>,
    move_contexts: BTreeMap<&'static str, usize>,
    facets: BTreeMap<&'static str, usize>,
    files: usize,
    directories: usize,
    subsystems: usize,
    entropy: f64,
    #[serde(flatten)]
    lines: LineCounts,
    behavior_lines: usize,
    structure_share: f64,
    claims: Claims,
}

pub fn build<'a>(change: &'a Change, labels: &Labels, claims: Claims, meta: Meta) -> Report<'a> {
    let mut files = Vec::new();
    let mut hunks = Vec::new();
    for (file, labels) in change.files.iter().zip(labels) {
        let mut totals = LineCounts::default();
        for (hunk, label) in file.diff.hunks.iter().zip(labels) {
            let mut lines = count_lines(file, hunk);
            let mut parts = part_records(file, label);
            if file.generated {
                parts.iter_mut().for_each(|p| p.review_lines = 0);
            }
            lines.review_lines = parts.iter().map(|p| p.review_lines).sum();
            totals.add(&lines);
            hunks.push(HunkRecord {
                record: "hunk",
                id: hunks.len(),
                file: file.diff.path(),
                old_file: file.diff.old_path.as_deref(),
                language: file.language(),
                parsed: label.residual.evidence.method == Method::Ast,
                generated: file.generated,
                before: hunk.before,
                after: hunk.after,
                kind: label.kind(),
                verdict: label.verdict(),
                lines,
                parts,
                method: label.residual.evidence.method,
            });
        }
        files.push(FileRecord {
            record: "file",
            file: file.diff.path(),
            old_file: file.diff.old_path.as_deref(),
            language: file.language(),
            parsed: labels
                .iter()
                .all(|l: &Label| l.residual.evidence.method == Method::Ast),
            generated: file.generated,
            lines_before: file.lines_before,
            hunks: labels.len(),
            lines: totals,
        });
    }
    let summary = summarize(&files, &hunks, claims);
    Report {
        meta,
        files,
        hunks,
        summary,
    }
}

fn count_lines(file: &FileChange, hunk: &Hunk) -> LineCounts {
    let mut counts = LineCounts {
        added: hunk.added.len(),
        deleted: hunk.removed.len(),
        ..LineCounts::default()
    };
    let Ok(trees) = &file.trees else {
        // Unparsed, every changed line is taken for code.
        counts.code_added = counts.added;
        counts.code_deleted = counts.deleted;
        return counts;
    };
    for &line in &hunk.removed {
        let facts = trees.before.line(line);
        counts.code_deleted += usize::from(facts.code);
        counts.comment_deleted += usize::from(facts.comment);
    }
    for &line in &hunk.added {
        let facts = trees.after.line(line);
        counts.code_added += usize::from(facts.code);
        counts.comment_added += usize::from(facts.comment);
    }
    counts
}

/// The hunk's parts, in [`Kind::ALL`] order, then its residual.
fn part_records(file: &FileChange, label: &Label) -> Vec<PartRecord> {
    let trees = file.trees.as_ref().ok();
    // Unparsed, every changed line is taken for code.
    let code = |claim: &Claim| -> usize {
        let Some(trees) = trees else {
            return claim.before.len() + claim.after.len();
        };
        let before = claim.before.iter().filter(|&&l| trees.before.line(l).code);
        let after = claim.after.iter().filter(|&&l| trees.after.line(l).code);
        before.count() + after.count()
    };
    let changed = |claim: &Claim| claim.before.len() + claim.after.len();
    let first = |claim: &Claim| (claim.after.first().copied(), claim.before.first().copied());
    let mut parts: Vec<&crate::classify::Part> = label.parts.iter().collect();
    parts.sort_by_key(|p| (p.kind, first(&p.lines)));
    let mut out: Vec<PartRecord> = parts
        .into_iter()
        .map(|p| PartRecord {
            kind: p.kind,
            structure: p.structure(),
            lines: Runs::of(&p.lines),
            confidence: p.confidence,
            reason: p.reason.clone(),
            evidence: p.evidence.clone(),
            detail: p.detail.clone(),
            facets: Vec::new(),
            review_lines: code(&p.review),
            changed: changed(&p.lines),
            context: p.context.clone(),
        })
        .collect();
    if label.has_residual() {
        let r = &label.residual;
        out.push(PartRecord {
            kind: Kind::Logic,
            structure: false,
            lines: Runs::of(&r.lines),
            confidence: r.confidence,
            reason: r.reason.clone(),
            evidence: r.evidence.clone(),
            detail: r.detail.clone(),
            facets: r.facets.clone(),
            review_lines: code(&r.lines),
            changed: changed(&r.lines),
            context: Vec::new(),
        });
    }
    out
}

fn summarize(files: &[FileRecord], hunks: &[HunkRecord], mut claims: Claims) -> Summary {
    let mut kinds: BTreeMap<&'static str, usize> =
        Kind::ALL.iter().map(|k| (k.as_str(), 0)).collect();
    let mut move_contexts: BTreeMap<&'static str, usize> =
        Context::ALL.iter().map(|c| (c.as_str(), 0)).collect();
    let mut facets: BTreeMap<&'static str, usize> =
        Facet::ALL.iter().map(|f| (f.as_str(), 0)).collect();
    let mut lines = LineCounts::default();
    let (mut structure, mut behavior, mut structure_lines, mut behavior_lines) = (0, 0, 0, 0);
    for hunk in hunks {
        // A generated hunk sways the verdict but not the size.
        let size = !hunk.generated;
        if size {
            lines.add(&hunk.lines);
        }
        for part in &hunk.parts {
            *kinds.entry(part.kind.as_str()).or_default() += 1;
            for context in &part.context {
                *move_contexts.entry(context.as_str()).or_default() += 1;
            }
            for facet in &part.facets {
                *facets.entry(facet.as_str()).or_default() += 1;
            }
            if part.structure {
                structure += 1;
                structure_lines += part.changed * usize::from(size);
            } else {
                behavior += 1;
                behavior_lines += part.review_lines;
            }
        }
    }
    let verdict = Verdict::of(structure > 0, behavior > 0);
    let paths: Vec<&str> = files.iter().map(|f| f.file).collect();
    let dirs: BTreeSet<&str> = paths
        .iter()
        .map(|p| p.rsplit_once('/').map_or(".", |(d, _)| d))
        .collect();
    let subsystems: BTreeSet<&str> = paths
        .iter()
        .map(|p| p.split_once('/').map_or(".", |(s, _)| s))
        .collect();
    let total = lines.changed();
    // Only hunks shapr compared are evidence against a claim.
    let compared: Vec<&HunkRecord> = hunks
        .iter()
        .filter(|h| h.verdict != Verdict::Structure && h.method != Method::None)
        .collect();
    claims.check(&CodeFacts {
        structure: verdict == Verdict::Structure,
        behavior: compared.len(),
        non_test_behavior: compared
            .iter()
            .filter(|h| !h.parts.iter().any(|p| p.facets.contains(&Facet::Test)))
            .count(),
    });
    Summary {
        record: "summary",
        verdict,
        hunks: hunks.len(),
        unparsed_hunks: hunks.iter().filter(|h| !h.parsed).count(),
        generated_hunks: hunks.iter().filter(|h| h.generated).count(),
        kinds,
        move_contexts,
        facets,
        files: files.len(),
        directories: dirs.len(),
        subsystems: subsystems.len(),
        entropy: round2(entropy(
            files
                .iter()
                .filter(|f| !f.generated)
                .map(|f| f.lines.changed()),
        )),
        lines,
        behavior_lines,
        structure_share: if total == 0 {
            0.0
        } else {
            round2(structure_lines as f64 / total as f64)
        },
        claims,
    }
}

/// Kamei et al.'s change entropy: −Σ pₖ·log₂ pₖ over each file's share of
/// the changed lines.
fn entropy(changed: impl Iterator<Item = usize>) -> f64 {
    let changed: Vec<usize> = changed.filter(|&c| c > 0).collect();
    let total: usize = changed.iter().sum();
    changed
        .iter()
        .map(|&c| c as f64 / total as f64)
        .map(|p| -p * p.log2())
        .sum::<f64>()
        .max(0.0)
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Text,
    Json,
    Ndjson,
}

pub fn render(out: &mut impl Write, format: Format, report: &Report) -> io::Result<()> {
    match format {
        Format::Json => {
            serde_json::to_writer_pretty(&mut *out, report)?;
            writeln!(out)
        }
        Format::Ndjson => {
            line(out, &report.meta)?;
            let mut hunks = report.hunks.iter().peekable();
            for file in &report.files {
                line(out, file)?;
                while let Some(hunk) = hunks.next_if(|h| h.file == file.file) {
                    line(out, hunk)?;
                }
            }
            line(out, &report.summary)
        }
        Format::Text => text(out, report),
    }
}

fn line(out: &mut impl Write, record: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *out, record)?;
    writeln!(out)
}

fn text(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let width = report
        .hunks
        .iter()
        .map(|h| place(h).len())
        .max()
        .unwrap_or(0);
    for hunk in &report.hunks {
        // One line per part; the place names the hunk on its first.
        for (i, part) in hunk.parts.iter().enumerate() {
            let facets: Vec<&str> = part.facets.iter().map(|f| f.as_str()).collect();
            writeln!(
                out,
                "{:width$}  {:<17} {:.2}  {}{}{}",
                if i == 0 { place(hunk) } else { String::new() },
                part.kind.as_str(),
                part.confidence.get(),
                part.reason,
                if facets.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", facets.join(", "))
                },
                if hunk.generated && i == 0 {
                    " (generated: not counted in size)"
                } else {
                    ""
                },
            )?;
        }
    }
    let s = &report.summary;
    claims(out, &s.claims)?;
    let kinds: Vec<String> = s
        .kinds
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    if let Some(source) = &report.meta.source {
        write!(out, "{}", source.name())?;
        if source.merge {
            write!(out, " (a merge, against its first parent)")?;
        }
        write!(out, ": ")?;
    }
    writeln!(
        out,
        "{}: {} hunk{} ({}); review {} line{}{}; comments +{} -{}; structure share {:.2}",
        s.verdict.as_str(),
        s.hunks,
        if s.hunks == 1 { "" } else { "s" },
        if kinds.is_empty() {
            "none".to_string()
        } else {
            kinds.join(", ")
        },
        s.lines.review_lines,
        if s.lines.review_lines == 1 { "" } else { "s" },
        match s.generated_hunks {
            0 => String::new(),
            1 => " (1 generated hunk not counted)".to_string(),
            n => format!(" ({n} generated hunks not counted)"),
        },
        s.lines.comment_added,
        s.lines.comment_deleted,
        s.structure_share,
    )
}

/// What the messages claim, and where the code doesn't bear a claim out.
fn claims(out: &mut impl Write, claims: &Claims) -> io::Result<()> {
    if claims.messages == 0 {
        return Ok(());
    }
    let claimed: Vec<String> = claims
        .experimental
        .categories
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(c, n)| format!("{c} {n}"))
        .collect();
    writeln!(
        out,
        "claims: {} ({} message{})",
        if claimed.is_empty() {
            "none".to_string()
        } else {
            claimed.join(", ")
        },
        claims.messages,
        if claims.messages == 1 { "" } else { "s" },
    )?;
    for conflict in &claims.experimental.conflicts {
        let plural = if conflict.count == 1 { "" } else { "s" };
        let finding = match conflict.finding {
            Finding::BehaviorHunks => format!("{} behavior hunk{plural}", conflict.count),
            Finding::NonTestBehaviorHunks => {
                format!("{} behavior hunk{plural} outside tests", conflict.count)
            }
            Finding::NoBehaviorHunks => "every hunk is structure".to_string(),
        };
        writeln!(
            out,
            "conflict: claims {}; {finding}",
            conflict.claim.as_str()
        )?;
    }
    Ok(())
}

/// `file:first-last` on the after side; a pure deletion names its before lines.
fn place(hunk: &HunkRecord) -> String {
    let (lines, suffix) = if hunk.after.count > 0 {
        (hunk.after, "")
    } else if hunk.before.count > 0 {
        (hunk.before, " (deleted)")
    } else {
        return hunk.file.to_string();
    };
    match lines.last() {
        Some(last) if last != lines.start => {
            format!("{}:{}-{last}{suffix}", hunk.file, lines.start)
        }
        _ => format!("{}:{}{suffix}", hunk.file, lines.start),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_follows_kamei() {
        assert_eq!(entropy([10].into_iter()), 0.0);
        assert_eq!(entropy([5, 5].into_iter()), 1.0);
        assert_eq!(round2(entropy([1, 1, 2].into_iter())), 1.5);
    }
}
