//! The output records (the public schema, see docs/schema.md) and their
//! rendering. Everything shapr writes to stdout goes through here.

use crate::change::{Category, Change, FileChange};
use crate::claims::{Claims, CodeFacts, Finding, HunkFacts};
use crate::classify::{
    self, Assumption, Claim, Confidence, Context, Evidence, Facet, Kind, Label, Labels, Method,
};
use crate::diff::{Hunk, Lines};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

/// Bumped on any breaking change to a record's fields.
pub const SCHEMA: u32 = 1;

/// What a hunk, or the whole change, does to the code that counts toward the
/// verdict (categories code, test, config and manifest).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Structure only.
    Structure,
    /// Behavior only.
    Behavior,
    Mixed,
    /// Nothing that counts changed: docs, or whitespace in a file shapr
    /// doesn't parse. For the change: no hunks, or only these.
    None,
    /// Outside the verdict: a generated, vendored, binary or unknown file,
    /// or code shapr couldn't compare. For the change: nothing counted, and
    /// something wasn't judged.
    Unknown,
}

impl Verdict {
    pub const ALL: [Verdict; 5] = [
        Verdict::Structure,
        Verdict::Behavior,
        Verdict::Mixed,
        Verdict::None,
        Verdict::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Structure => "structure",
            Verdict::Behavior => "behavior",
            Verdict::Mixed => "mixed",
            Verdict::None => "none",
            Verdict::Unknown => "unknown",
        }
    }

    /// Counts toward the change's verdict.
    fn counted(self) -> bool {
        matches!(
            self,
            Verdict::Structure | Verdict::Behavior | Verdict::Mixed
        )
    }

    fn of(shape: classify::Verdict) -> Verdict {
        match shape {
            classify::Verdict::Structure => Verdict::Structure,
            classify::Verdict::Behavior => Verdict::Behavior,
            classify::Verdict::Mixed => Verdict::Mixed,
        }
    }
}

/// The first rule that matches; see docs/schema.md, "Verdicts".
fn hunk_verdict(file: &FileChange, whitespace_only: bool, label: &Label) -> Verdict {
    let compared = label.residual.evidence.method != Method::None || !label.parts.is_empty();
    match file.category {
        Category::Binary | Category::Generated | Category::Vendored => Verdict::Unknown,
        Category::Docs | Category::Fixture => Verdict::None,
        _ if !compared && whitespace_only => Verdict::None,
        Category::Code | Category::Test | Category::Config | Category::Manifest if compared => {
            Verdict::of(label.verdict())
        }
        // Their text differs beyond whitespace, and their text is the setting.
        Category::Config | Category::Manifest => Verdict::Behavior,
        Category::Code | Category::Test | Category::Unknown => Verdict::Unknown,
    }
}

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
    /// What ran: `classify` or `norms`, whose records differ.
    command: &'static str,
    /// The GitHub pull request or commit analyzed; `None` for a local run.
    pub source: Option<crate::github::Source>,
    /// The form the change was named in; `None` for `norms`.
    input: Option<Input>,
    /// Whether paths limited the diff; `None` for `norms`.
    paths: Option<bool>,
    /// How many untracked files the paths hold, which a change ending in
    /// the working tree leaves out, as `git diff` does; `None` for any
    /// other change.
    pub untracked: Option<usize>,
    /// What was compared, for the text summary (`main...HEAD`,
    /// `uncommitted changes in lib`); a GitHub change names its `source`.
    #[serde(skip)]
    pub subject: Option<String>,
}

impl Meta {
    pub fn new(command: &'static str, source: Option<crate::github::Source>) -> Meta {
        Meta {
            record: "meta",
            schema: SCHEMA,
            version: env!("CARGO_PKG_VERSION"),
            command,
            source,
            input: None,
            paths: None,
            untracked: None,
            subject: None,
        }
    }

    /// The change's input form, and whether paths limited it.
    pub fn input(self, input: Input, paths: bool) -> Meta {
        Meta {
            input: Some(input),
            paths: Some(paths),
            ..self
        }
    }
}

/// How the command line named a change: each form is a git command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Input {
    /// A diff on stdin.
    Stdin,
    /// `shapr REV`: `git show REV`.
    Commit,
    /// `shapr A..B`, `A...B`, `A B`, `R^!`: `git diff` of them.
    Range,
    /// `shapr --merge-base REF REV`: `git diff REF...REV`.
    MergeBaseRange,
    /// `shapr --merge-base REF`: the merge base of REF and HEAD against the working tree.
    MergeBaseWorktree,
    /// `shapr PATH...`: `git diff HEAD -- PATH...`.
    Worktree,
    /// `shapr --staged [REV]`: `git diff --cached [REV]`.
    Staged,
    GithubPr,
    GithubCommit,
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
    category: Category,
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
    category: Category,
    before: Lines,
    after: Lines,
    kind: Kind,
    verdict: Verdict,
    #[serde(flatten)]
    lines: LineCounts,
    parts: Vec<PartRecord>,
    /// Against the norms of the code around it; experimental.
    pub style: Option<crate::norms::HunkStyle>,
    /// Of `review_lines`, those in test code.
    #[serde(skip)]
    test_review_lines: usize,
}

/// A part of a hunk, or its residual (kind `logic`).
#[derive(Serialize)]
pub struct PartRecord {
    kind: Kind,
    structure: bool,
    lines: Runs,
    confidence: Confidence,
    reason: String,
    evidence: EvidenceRecord,
    detail: Map<String, Value>,
    facets: Vec<Facet>,
    assumptions: Vec<Assumption>,
    review_lines: usize,
    /// Changed lines it covers, for `structure_share`.
    #[serde(skip)]
    changed: usize,
    #[serde(skip)]
    context: Vec<Context>,
}

/// What a part rests on: how the hunk was compared, beside the kind's own
/// values. `comparison`, not `method`, which kind detail uses for a method
/// name.
#[derive(Serialize)]
struct EvidenceRecord {
    comparison: Method,
    #[serde(flatten)]
    values: Map<String, Value>,
}

impl EvidenceRecord {
    fn of(evidence: &Evidence) -> EvidenceRecord {
        EvidenceRecord {
            comparison: evidence.method,
            values: evidence.values.clone(),
        }
    }
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
    verdicts: BTreeMap<&'static str, usize>,
    categories: BTreeMap<&'static str, usize>,
    kinds: BTreeMap<&'static str, usize>,
    move_contexts: BTreeMap<&'static str, usize>,
    assumptions: BTreeMap<&'static str, usize>,
    facets: BTreeMap<&'static str, usize>,
    files: usize,
    directories: usize,
    subsystems: usize,
    entropy: f64,
    #[serde(flatten)]
    lines: LineCounts,
    test_review_lines: usize,
    behavior_lines: usize,
    structure_share: f64,
    claims: Claims,
    /// Experimental: per norm config sets, whether the change moves the
    /// code toward it; filled in beside the hunks' style.
    pub config_trajectory: Vec<crate::norms::Trajectory>,
}

pub fn build<'a>(change: &'a Change, labels: &Labels, claims: Claims, meta: Meta) -> Report<'a> {
    let mut files = Vec::new();
    let mut hunks = Vec::new();
    for (file, labels) in change.files.iter().zip(labels) {
        let mut totals = LineCounts::default();
        for ((hunk, label), &whitespace_only) in file
            .diff
            .hunks
            .iter()
            .zip(labels)
            .zip(&file.whitespace_only)
        {
            let mut lines = count_lines(file, hunk);
            let mut parts = part_records(file, label);
            let verdict = hunk_verdict(file, whitespace_only, label);
            // A `none` hunk changed nothing a reviewer reads.
            if !reviewed(file.category) || verdict == Verdict::None {
                parts.iter_mut().for_each(|p| p.review_lines = 0);
            }
            lines.review_lines = parts.iter().map(|p| p.review_lines).sum();
            let test_review_lines = if file.category == Category::Test {
                lines.review_lines
            } else {
                parts
                    .iter()
                    .filter(|p| p.facets.contains(&Facet::Test))
                    .map(|p| p.review_lines)
                    .sum()
            };
            totals.add(&lines);
            hunks.push(HunkRecord {
                record: "hunk",
                id: hunks.len(),
                file: file.diff.path(),
                old_file: file.diff.old_path.as_deref(),
                language: file.language(),
                parsed: label.residual.evidence.method == Method::Ast,
                category: file.category,
                before: hunk.before,
                after: hunk.after,
                kind: label.kind(),
                verdict,
                lines,
                parts,
                style: None,
                test_review_lines,
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
            category: file.category,
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
            evidence: EvidenceRecord::of(&p.evidence),
            detail: p.detail.clone(),
            facets: Vec::new(),
            assumptions: p.assumptions.clone(),
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
            evidence: EvidenceRecord::of(&r.evidence),
            detail: r.detail.clone(),
            facets: r.facets.clone(),
            assumptions: Vec::new(),
            review_lines: code(&r.lines),
            changed: changed(&r.lines),
            context: Vec::new(),
        });
    }
    out
}

fn summarize(files: &[FileRecord], hunks: &[HunkRecord], mut claims: Claims) -> Summary {
    let mut categories: BTreeMap<&'static str, usize> =
        Category::ALL.iter().map(|c| (c.as_str(), 0)).collect();
    let mut kinds: BTreeMap<&'static str, usize> =
        Kind::ALL.iter().map(|k| (k.as_str(), 0)).collect();
    let mut move_contexts: BTreeMap<&'static str, usize> =
        Context::MOVE.iter().map(|c| (c.as_str(), 0)).collect();
    let mut facets: BTreeMap<&'static str, usize> =
        Facet::ALL.iter().map(|f| (f.as_str(), 0)).collect();
    let mut assumptions: BTreeMap<&'static str, usize> =
        Assumption::ALL.iter().map(|a| (a.as_str(), 0)).collect();
    let mut verdicts: BTreeMap<&'static str, usize> =
        Verdict::ALL.iter().map(|v| (v.as_str(), 0)).collect();
    let mut lines = LineCounts::default();
    let (mut structure, mut behavior, mut structure_lines, mut behavior_lines) = (0, 0, 0, 0);
    let mut test_review_lines = 0;
    for hunk in hunks {
        *categories.entry(hunk.category.as_str()).or_default() += 1;
        *verdicts.entry(hunk.verdict.as_str()).or_default() += 1;
        let size = sized(hunk.category);
        if size {
            lines.add(&hunk.lines);
            test_review_lines += hunk.test_review_lines;
        }
        let counted = hunk.verdict.counted();
        for part in &hunk.parts {
            *kinds.entry(part.kind.as_str()).or_default() += 1;
            for context in part.context.iter().filter(|_| part.kind == Kind::Move) {
                *move_contexts.entry(context.as_str()).or_default() += 1;
            }
            for facet in &part.facets {
                *facets.entry(facet.as_str()).or_default() += 1;
            }
            for assumption in &part.assumptions {
                *assumptions.entry(assumption.as_str()).or_default() += 1;
            }
            if !counted {
                continue;
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
    let verdict = match (structure > 0, behavior > 0) {
        (true, false) => Verdict::Structure,
        (false, true) => Verdict::Behavior,
        (true, true) => Verdict::Mixed,
        _ if hunks.iter().any(|h| h.verdict == Verdict::Unknown) => Verdict::Unknown,
        _ => Verdict::None,
    };
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
    // Only hunks in the verdict are evidence against a claim.
    claims.check(&CodeFacts {
        structure: verdict == Verdict::Structure,
        hunks: hunks
            .iter()
            .filter(|h| h.verdict.counted())
            .map(|h| HunkFacts {
                id: h.id,
                behavior: matches!(h.verdict, Verdict::Behavior | Verdict::Mixed),
                test: h.category == Category::Test
                    || h.parts.iter().any(|p| p.facets.contains(&Facet::Test)),
            })
            .collect(),
    });
    Summary {
        record: "summary",
        verdict,
        hunks: hunks.len(),
        unparsed_hunks: hunks.iter().filter(|h| !h.parsed).count(),
        verdicts,
        categories,
        kinds,
        move_contexts,
        assumptions,
        facets,
        files: files.len(),
        directories: dirs.len(),
        subsystems: subsystems.len(),
        entropy: round2(entropy(
            files
                .iter()
                .filter(|f| sized(f.category))
                .map(|f| f.lines.changed()),
        )),
        lines,
        test_review_lines,
        behavior_lines,
        structure_share: if total == 0 {
            0.0
        } else {
            round2(structure_lines as f64 / total as f64)
        },
        claims,
        config_trajectory: Vec::new(),
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

/// Counts toward size: tool-written and third-party files are reviewed
/// elsewhere.
fn sized(category: Category) -> bool {
    !matches!(category, Category::Generated | Category::Vendored)
}

/// Its code is read in review: prose, tool output and third-party code
/// aren't.
fn reviewed(category: Category) -> bool {
    sized(category) && !matches!(category, Category::Docs | Category::Binary)
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
    summary_text(out, report)?;
    if !report.hunks.is_empty() {
        writeln!(out)?;
    }
    let width = report
        .hunks
        .iter()
        .map(|h| place(h).len())
        .max()
        .unwrap_or(0);
    for hunk in &report.hunks {
        let outside = if hunk.verdict.counted() {
            ""
        } else {
            " (outside the verdict)"
        };
        if let Some(plain) = uncompared(hunk) {
            emit(out, &format!("{:width$}  {plain}{outside}", place(hunk)))?;
            continue;
        }
        // One line per part; the place names the hunk on its first.
        for (i, part) in hunk.parts.iter().enumerate() {
            let facets: Vec<&str> = part.facets.iter().map(|f| f.as_str()).collect();
            let reason = match part.reason.strip_prefix(classify::PLAIN_LOGIC) {
                Some(rest) => rest.trim_start_matches("; "),
                None => &part.reason,
            };
            let mut detail = vec![reason.to_string()];
            if !facets.is_empty() {
                detail.push(format!("[{}]", facets.join(", ")));
            }
            if !hunk.verdict.counted() && i == 0 {
                detail.push(format!("({}: outside the verdict)", hunk.category.as_str()));
            }
            detail.retain(|d| !d.is_empty());
            let line = format!(
                "{:width$}  {:<17} {:.2}  {}",
                if i == 0 { place(hunk) } else { String::new() },
                part.kind.as_str(),
                part.confidence.get(),
                detail.join(" "),
            );
            emit(out, line.trim_end())?;
        }
        for line in hunk.style.iter().flat_map(crate::norms::text) {
            emit(out, &format!("{:width$}  {line}", ""))?;
        }
    }
    Ok(())
}

/// The change's verdict and counts, then claims: the text output's first lines.
fn summary_text(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let s = &report.summary;
    // Parts in hunks inside the verdict; "outside the verdict" counts the rest.
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for part in report
        .hunks
        .iter()
        .filter(|h| h.verdict.counted())
        .flat_map(|h| &h.parts)
    {
        *kinds.entry(part.kind.as_str()).or_default() += 1;
    }
    let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect();
    let lead = if let Some(source) = &report.meta.source {
        let merge = match source.merge {
            true => " (a merge, against its first parent)",
            false => "",
        };
        format!("{}{merge}: ", source.name())
    } else if let Some(subject) = &report.meta.subject {
        format!("{subject}: ")
    } else {
        String::new()
    };
    let outside: Vec<String> = [Verdict::None, Verdict::Unknown]
        .iter()
        .map(|v| (v.as_str(), s.verdicts[v.as_str()]))
        .filter(|(_, n)| *n > 0)
        .map(|(v, n)| format!("{n} {v}"))
        .collect();
    let line = format!(
        "{lead}{}: {} hunk{} ({}){}; review {} line{}{}{}{}",
        s.verdict.as_str(),
        s.hunks,
        if s.hunks == 1 { "" } else { "s" },
        if kinds.is_empty() {
            "none".to_string()
        } else {
            kinds.join(", ")
        },
        if outside.is_empty() {
            String::new()
        } else {
            format!("; outside the verdict: {}", outside.join(", "))
        },
        s.lines.review_lines,
        if s.lines.review_lines == 1 { "" } else { "s" },
        match s.test_review_lines {
            0 => String::new(),
            n => format!(", {n} in tests"),
        },
        match (s.lines.comment_added, s.lines.comment_deleted) {
            (0, 0) => String::new(),
            (added, deleted) => format!("; comments +{added} -{deleted}"),
        },
        // Always 0 on behavior and 1 on structure.
        if s.verdict == Verdict::Mixed {
            format!("; structure share {:.2}", s.structure_share)
        } else {
            String::new()
        },
    );
    emit(out, &line)?;
    for line in crate::norms::trajectory::text(&s.config_trajectory) {
        emit(out, &line)?;
    }
    claims(out, &s.claims)?;
    Ok(())
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
        .map(|(c, n)| format!("{c} in {n}"))
        .collect();
    let of = format!(
        "{} message{}",
        claims.messages,
        if claims.messages == 1 { "" } else { "s" }
    );
    match claimed.as_slice() {
        // Nothing claimed: nothing worth a line.
        [] => {}
        [one] => emit(out, &format!("claims: {one} of {of}"))?,
        many => emit(out, &format!("claims: {} (of {of})", many.join(", ")))?,
    }
    for conflict in &claims.experimental.conflicts {
        let plural = if conflict.count == 1 { "" } else { "s" };
        // Hunks shapr couldn't prove structural aren't proof of behavior.
        let (label, finding) = match conflict.finding {
            Finding::BehaviorHunks => (
                "unproven",
                format!("{} hunk{plural} not shown structural", conflict.count),
            ),
            Finding::NonTestBehaviorHunks => (
                "unproven",
                format!(
                    "{} hunk{plural} outside tests not shown structural",
                    conflict.count
                ),
            ),
            Finding::NoBehaviorHunks => ("conflict", "every hunk is structure".to_string()),
        };
        emit(
            out,
            &format!("{label}: claims {}; {finding}", conflict.claim.as_str()),
        )?;
    }
    Ok(())
}

/// A hunk in a file shapr didn't compare, by its category alone: `docs
/// change`, `unknown file`. `None` for code or tests, whose reason says why.
fn uncompared(hunk: &HunkRecord) -> Option<String> {
    let [part] = hunk.parts.as_slice() else {
        return None;
    };
    if hunk.parsed || part.evidence.comparison != Method::None {
        return None;
    }
    Some(match hunk.category {
        Category::Code | Category::Test => return None,
        Category::Unknown | Category::Binary => format!("{} file", hunk.category.as_str()),
        // A counted file outside the verdict changed only in whitespace.
        category if category.counted() && hunk.verdict == Verdict::None => {
            format!("{} change, whitespace only", category.as_str())
        }
        category => format!("{} change", category.as_str()),
    })
}

/// `file:first-last` on the after side; a pure deletion names its before lines.
fn place(hunk: &HunkRecord) -> String {
    let (lines, suffix) = if hunk.after.count > 0 {
        (hunk.after, "")
    } else if hunk.before.count > 0 {
        (hunk.before, " (deleted)")
    } else {
        return quote_path(hunk.file).into_owned();
    };
    let file = quote_path(hunk.file);
    match lines.last() {
        Some(last) if last != lines.start => format!("{file}:{}-{last}{suffix}", lines.start),
        _ => format!("{file}:{}{suffix}", lines.start),
    }
}

/// Writes LINE as one line of text output. Every text line goes through here:
/// a control character from a file name, a reason or a message is escaped,
/// so it can't end the line (and start a `::error::` CI command) or reach a
/// terminal as an escape sequence.
pub(crate) fn emit(out: &mut impl Write, line: &str) -> io::Result<()> {
    writeln!(out, "{}", escape(line))
}

/// TEXT with each control character (C0, DEL, C1) escaped C-style, as git
/// quotes paths: `\n`, `\t`, `\033`.
pub fn escape(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return text.into();
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        push_escaped(&mut out, c);
    }
    out.into()
}

/// PATH as git shows a path with `core.quotePath=false`: as it is, or in
/// double quotes with `"`, `\` and control characters escaped.
pub fn quote_path(path: &str) -> std::borrow::Cow<'_, str> {
    if !path
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\')
    {
        return path.into();
    }
    let mut out = String::from('"');
    for c in path.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c => push_escaped(&mut out, c),
        }
    }
    out.push('"');
    out.into()
}

fn push_escaped(out: &mut String, c: char) {
    let short = match c {
        '\x07' => "\\a",
        '\x08' => "\\b",
        '\t' => "\\t",
        '\n' => "\\n",
        '\x0b' => "\\v",
        '\x0c' => "\\f",
        '\r' => "\\r",
        c if c.is_control() => {
            let mut bytes = [0; 4];
            for byte in c.encode_utf8(&mut bytes).bytes() {
                out.push_str(&format!("\\{byte:03o}"));
            }
            return;
        }
        c => {
            out.push(c);
            return;
        }
    };
    out.push_str(short);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_quote_as_git_quotes_them() {
        assert_eq!(quote_path("lib/a.rb"), "lib/a.rb");
        assert_eq!(quote_path("lib/é.rb"), "lib/é.rb");
        assert_eq!(
            quote_path("a\tb\n\u{1b}\u{85}\"\\"),
            r#""a\tb\n\033\302\205\"\\""#
        );
        assert_eq!(escape("one\ntwo \"x\""), r#"one\ntwo "x""#);
    }

    #[test]
    fn entropy_follows_kamei() {
        assert_eq!(entropy([10].into_iter()), 0.0);
        assert_eq!(entropy([5, 5].into_iter()), 1.0);
        assert_eq!(round2(entropy([1, 1, 2].into_iter())), 1.5);
    }
}
