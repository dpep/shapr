//! The output records (the public schema, see docs/schema.md) and their
//! rendering. Everything shapr writes to stdout goes through here.

use crate::change::{Category, Change, FileChange};
use crate::claims::{self, Claims, Finding};
use crate::classify::{
    self, Assumption, Claim, Confidence, Context, Evidence, Facet, Kind, Label, Labels, Method,
};
use crate::diff::{Hunk, Lines};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

pub mod digest;

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
    pub(crate) fn counted(self) -> bool {
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
        Category::Docs | Category::Fixture | Category::Types => Verdict::None,
        _ if !compared && whitespace_only => Verdict::None,
        _ if !file.counts() => Verdict::Unknown,
        _ if compared => Verdict::of(label.verdict()),
        // Counted but not compared (no parser for config, a cap, unreadable):
        // what shapr didn't compare is logic, never left out on its own say.
        _ => Verdict::Behavior,
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
    /// What was compared (`main...HEAD`, `uncommitted changes in lib`); a
    /// GitHub change names its `source`.
    #[serde(skip)]
    pub subject: Option<String>,
    /// Whether shapr chose the subject, which the text summary then names;
    /// the caller knows revisions it typed.
    #[serde(skip)]
    pub subject_chosen: bool,
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
            subject_chosen: false,
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
    pub relocated_lines: usize,
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
        self.relocated_lines += other.relocated_lines;
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
    pub(crate) id: usize,
    file: &'a str,
    old_file: Option<&'a str>,
    language: Option<&'static str>,
    parsed: bool,
    pub(crate) category: Category,
    before: Lines,
    after: Lines,
    kind: Kind,
    pub(crate) verdict: Verdict,
    #[serde(flatten)]
    lines: LineCounts,
    pub(crate) parts: Vec<PartRecord>,
    /// Against the norms of the code around it; experimental.
    pub style: Option<crate::norms::HunkStyle>,
    /// Of `review_lines`, those in test code.
    #[serde(skip)]
    test_review_lines: usize,
    /// The text output's note on its relocated lines.
    #[serde(skip)]
    relocated_note: Option<String>,
    /// Its file counts toward the verdict ([`FileChange::counts`]).
    #[serde(skip)]
    counted_file: bool,
    /// It changes test code: a test file, or a line in a test item.
    #[serde(skip)]
    tests: bool,
}

/// A part of a hunk, or its residual (kind `logic`).
#[derive(Serialize)]
pub struct PartRecord {
    pub(crate) kind: Kind,
    structure: bool,
    lines: Runs,
    confidence: Confidence,
    reason: String,
    evidence: EvidenceRecord,
    detail: Map<String, Value>,
    pub(crate) facets: Vec<Facet>,
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
    logic_without_tests: usize,
    files: usize,
    directories: usize,
    subsystems: usize,
    entropy: f64,
    #[serde(flatten)]
    lines: LineCounts,
    test_review_lines: usize,
    behavior_lines: usize,
    structure_share: f64,
    refactor_lines: usize,
    refactor_share: f64,
    pub claims: Claims,
    /// Experimental: per norm config sets, whether the change moves the
    /// code toward it; filled in beside the hunks' style.
    pub config_trajectory: Vec<crate::norms::Trajectory>,
    /// The bounds on the search outside the change that refused a search;
    /// filled in once the norms' searches are done too.
    pub bounds_hit: Vec<crate::outside::Hit>,
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
            if lines.review_lines > 0 {
                lines.relocated_lines = relocated_lines(file, &label.residual);
            }
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
                relocated_note: relocated_note(lines.relocated_lines, &label.residual),
                counted_file: file.counts(),
                tests: file.category == Category::Test || label.tests,
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
            detail: relocated_detail(r),
            facets: r.facets.clone(),
            assumptions: Vec::new(),
            // Comments or blank lines no part proved are read whole: a line
            // shift refused, say, reaches behavior through them.
            review_lines: match code(&r.lines) {
                0 => changed(&r.lines),
                n => n,
            },
            changed: changed(&r.lines),
            context: Vec::new(),
        });
    }
    out
}

/// The residual's detail, with its relocated runs (`detail.relocated`).
fn relocated_detail(r: &classify::Residual) -> Map<String, Value> {
    let mut detail = r.detail.clone();
    if !r.relocated.is_empty() {
        let runs = r.relocated.iter().map(|m| {
            let lines = Lines {
                start: *m.lines.start(),
                count: m.lines.end() - m.lines.start() + 1,
            };
            let end = serde_json::json!({"file": m.path, "line": m.line});
            let way = if m.added { "from" } else { "to" };
            serde_json::json!({"lines": lines, way: end})
        });
        detail.insert("relocated".into(), runs.collect());
    }
    detail
}

/// The residual's code lines in relocated runs.
fn relocated_lines(file: &FileChange, r: &classify::Residual) -> usize {
    let Ok(trees) = &file.trees else { return 0 };
    let code = |m: &classify::Relocation| {
        let (tree, lines) = match m.added {
            true => (&trees.after, &r.lines.after),
            false => (&trees.before, &r.lines.before),
        };
        let lines = m.lines.clone().filter(|l| lines.contains(l));
        lines.filter(|&l| tree.line(l).code).count()
    };
    r.relocated.iter().map(code).sum()
}

/// `N lines relocated verbatim from FILE:LINE`, naming the first run's
/// other end and how many more there are.
fn relocated_note(lines: usize, r: &classify::Residual) -> Option<String> {
    let first = r.relocated.first().filter(|_| lines > 0)?;
    let way = if first.added { "from" } else { "to" };
    let more = match r.relocated.len() - 1 {
        0 => String::new(),
        n => format!(" and {n} more place{}", if n == 1 { "" } else { "s" }),
    };
    let s = if lines == 1 { "" } else { "s" };
    let (path, line) = (quote_path(&first.path), first.line);
    Some(format!(
        "{lines} line{s} relocated verbatim {way} {path}:{line}{more}"
    ))
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
    let mut refactor_lines = 0;
    let mut test_review_lines = 0;
    for hunk in hunks {
        *categories.entry(hunk.category.as_str()).or_default() += 1;
        *verdicts.entry(hunk.verdict.as_str()).or_default() += 1;
        let size = sized(hunk.category);
        if size {
            lines.add(&hunk.lines);
            test_review_lines += hunk.test_review_lines;
        }
        // Part counts are over the hunks the verdict rolls up.
        let counted = hunk.verdict.counted();
        for part in hunk.parts.iter().filter(|_| counted) {
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
            // A structure kind is a refactoring, proven or not.
            if part.kind.is_structure() {
                refactor_lines += part.changed * usize::from(size);
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
    claims.check(verdict, hunks);
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
        logic_without_tests: logic_without_tests(hunks).len(),
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
        structure_share: share(structure_lines, total),
        refactor_lines,
        refactor_share: share(refactor_lines, total),
        claims,
        config_trajectory: Vec::new(),
        bounds_hit: Vec::new(),
    }
}

/// Code hunks holding logic, in a change that touches no test code; none
/// when a code hunk wasn't compared on its trees, which may hide a test.
fn logic_without_tests<'h, 'a>(hunks: &'h [HunkRecord<'a>]) -> Vec<&'h HunkRecord<'a>> {
    let code = |h: &&HunkRecord| h.category == Category::Code && h.verdict.counted();
    if hunks.iter().any(|h| h.tests) || hunks.iter().filter(code).any(|h| !h.parsed) {
        return Vec::new();
    }
    (hunks.iter().filter(code))
        .filter(|h| h.parts.iter().any(|p| p.kind == Kind::Logic))
        .collect()
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

/// PART over TOTAL, 0 when TOTAL is.
fn share(part: usize, total: usize) -> f64 {
    match total {
        0 => 0.0,
        _ => round2(part as f64 / total as f64),
    }
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
    summary_text(out, report, false)?;
    if !report.hunks.is_empty() {
        writeln!(out)?;
    }
    // Hunks not classified, one line per description after the rest.
    let mut outside: Vec<(String, Vec<digest::Place>)> = Vec::new();
    let mut listed = Vec::new();
    for hunk in &report.hunks {
        match uncompared(hunk).filter(|_| !hunk.verdict.counted()) {
            Some(plain) => match outside.iter_mut().find(|(p, _)| *p == plain) {
                Some((_, at)) => at.push(digest::place(hunk)),
                None => outside.push((plain, vec![digest::place(hunk)])),
            },
            None => listed.push(hunk),
        }
    }
    let width = listed.iter().map(|h| place(h).len()).max().unwrap_or(0);
    for hunk in listed {
        if let Some(plain) = uncompared(hunk) {
            emit(out, &format!("{:width$}  {plain}", place(hunk)))?;
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
                detail.push(format!(
                    "({}: verdict {})",
                    hunk.category.as_str(),
                    hunk.verdict.as_str()
                ));
            }
            detail.retain(|d| !d.is_empty());
            let line = format!(
                "{:width$}  {:<17} {:4}  {}",
                if i == 0 { place(hunk) } else { String::new() },
                part.kind.as_str(),
                confidence(part),
                detail.join(" "),
            );
            emit(out, line.trim_end())?;
        }
        if let Some(note) = &hunk.relocated_note {
            emit(out, &format!("{:width$}  {note}", ""))?;
        }
        for line in hunk.style.iter().flat_map(crate::norms::text) {
            emit(out, &format!("{:width$}  {line}", ""))?;
        }
    }
    for (plain, at) in outside {
        let n = at.len();
        let hunks = if n == 1 { "hunk" } else { "hunks" };
        let at = digest::places(&at);
        emit(out, &format!("not classified ({plain}), {n} {hunks}: {at}"))?;
    }
    Ok(())
}

/// The change's verdict and counts, then claims: the text output's first lines.
/// STANDALONE names the subject even when the caller typed it, for output
/// read without the command line.
fn summary_text(out: &mut impl Write, report: &Report, standalone: bool) -> io::Result<()> {
    let s = &report.summary;
    // Parts in hunks the verdict counts; "hunk verdicts" counts the rest.
    let kinds: Vec<String> = (s.kinds.iter())
        .filter(|(_, n)| **n > 0)
        .map(|(k, n)| format!("{n} {k}"))
        .collect();
    let lead = if let Some(source) = &report.meta.source {
        let merge = match source.merge {
            true => " (a merge, against its first parent)",
            false => "",
        };
        format!("{}{merge}: ", source.name())
    } else if let Some(subject) =
        (report.meta.subject.as_ref()).filter(|_| standalone || report.meta.subject_chosen)
    {
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
        "{lead}{}: {} hunk{} ({}){}; review {} line{}{}{}{}{}",
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
            format!("; hunk verdicts: {}", outside.join(", "))
        },
        s.lines.review_lines,
        if s.lines.review_lines == 1 { "" } else { "s" },
        match s.test_review_lines {
            0 => String::new(),
            n => format!(", {n} in tests"),
        },
        match s.lines.relocated_lines {
            0 => String::new(),
            n => format!(", {n} relocated verbatim"),
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
    if let Some(line) = crate::outside::text(&s.bounds_hit) {
        emit(out, &line)?;
    }
    let refactor = refactor_text(report);
    if let Some(line) = &refactor {
        emit(out, line)?;
    }
    let untested = logic_without_tests(&report.hunks);
    if !untested.is_empty() {
        let n = untested.len();
        let (s, verb) = if n == 1 { ("", "holds") } else { ("s", "hold") };
        let line = format!(
            "no test changed: {n} code hunk{s} {verb} logic{}",
            places(&untested)
        );
        emit(out, &line)?;
    }
    claims(out, report, refactor.is_some())?;
    Ok(())
}

/// Where logic hides in a change that is two thirds refactoring or more:
/// `refactor: 80% of changed lines; 2 logic lines inside (lib/b.rb:2)`.
/// Nothing when there is none to point at.
fn refactor_text(report: &Report) -> Option<String> {
    let s = &report.summary;
    if 3 * s.refactor_lines < 2 * s.lines.changed() {
        return None;
    }
    let inside: Vec<(&HunkRecord, usize)> = (report.hunks.iter())
        .filter(|h| h.verdict.counted())
        .map(|h| {
            let logic = h.parts.iter().filter(|p| p.kind == Kind::Logic);
            (h, logic.map(|p| p.review_lines).sum())
        })
        .filter(|(_, lines)| *lines > 0)
        .collect();
    if inside.is_empty() {
        return None;
    }
    let lines: usize = inside.iter().map(|(_, n)| n).sum();
    let hunks: Vec<&HunkRecord> = inside.iter().map(|(h, _)| *h).collect();
    Some(format!(
        "refactor: {:.0}% of changed lines; {lines} logic line{} inside{}",
        s.refactor_share * 100.0,
        if lines == 1 { "" } else { "s" },
        places(&hunks),
    ))
}

/// ` (lib/a.rb:12, lib/b.rb:40, 2 more)`: the first three hunks' first
/// lines, on the after side unless a hunk only deletes. Empty for none.
fn places(hunks: &[&HunkRecord]) -> String {
    if hunks.is_empty() {
        return String::new();
    }
    let start = |h: &HunkRecord| match h.after.count {
        0 => h.before.start,
        _ => h.after.start,
    };
    let mut places: Vec<String> = (hunks.iter().take(3))
        .map(|h| format!("{}:{}", quote_path(h.file), start(h)))
        .collect();
    if hunks.len() > 3 {
        places.push(format!("{} more", hunks.len() - 3));
    }
    format!(" ({})", places.join(", "))
}

/// What the messages claim, and where the code doesn't bear a claim out.
/// With REFACTOR, the refactor line already names the hunks an unproven
/// refactor or style claim would count.
fn claims(out: &mut impl Write, report: &Report, refactor: bool) -> io::Result<()> {
    let claims = &report.summary.claims;
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
    for (label, finding) in conflicts(report, refactor) {
        emit(out, &format!("{label}: {finding}"))?;
    }
    Ok(())
}

/// Each claim the code doesn't bear out, as a label (`unproven`,
/// `conflict`) and its line, naming the first hunks behind it. With
/// REFACTOR, the refactor line already names the hunks an unproven refactor
/// or style claim would count.
fn conflicts(report: &Report, refactor: bool) -> Vec<(&'static str, String)> {
    let claims = &report.summary.claims;
    let mut out = Vec::new();
    for conflict in &claims.experimental.conflicts {
        let unproven = matches!(
            conflict.finding,
            Finding::BehaviorHunks | Finding::NonTestBehaviorHunks | Finding::LogicHunks
        );
        if refactor
            && unproven
            && matches!(
                conflict.claim,
                claims::Category::Refactor | claims::Category::Style
            )
        {
            continue;
        }
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
            Finding::LogicHunks => (
                "unproven",
                format!(
                    "{} hunk{plural} {} logic",
                    conflict.count,
                    if conflict.count == 1 { "is" } else { "are" }
                ),
            ),
            Finding::NoReorderParts => (
                "unproven",
                format!("no reorder or move in {} hunk{plural}", conflict.count),
            ),
        };
        let claim = match conflict.finding {
            Finding::NoReorderParts => "reorder",
            _ => conflict.claim.as_str(),
        };
        let hunks: Vec<&HunkRecord> = (conflict.hunks.iter())
            .filter_map(|&id| report.hunks.get(id))
            .collect();
        out.push((
            label,
            format!("claims {claim}; {finding}{}", places(&hunks)),
        ));
    }
    out
}

/// The text column's confidence: blank for a residual the tree comparison
/// settles outright, whose 1 would rank nothing.
fn confidence(part: &PartRecord) -> String {
    let settled = part.kind == Kind::Logic
        && part.evidence.comparison == Method::Ast
        && !part.detail.contains_key("near");
    match settled {
        true => String::new(),
        false => format!("{:.2}", part.confidence.get()),
    }
}

/// Shapr compared the hunk's sides: parsed, or line by line.
fn compared(hunk: &HunkRecord) -> bool {
    match hunk.parts.as_slice() {
        [part] => hunk.parsed || part.evidence.comparison != Method::None,
        _ => true,
    }
}

/// A hunk in a file shapr didn't compare, by its category alone: `docs
/// change`, `unknown file`. `None` for code or tests, whose reason says why.
fn uncompared(hunk: &HunkRecord) -> Option<String> {
    if compared(hunk) {
        return None;
    }
    Some(match hunk.category {
        Category::Code | Category::Test => return None,
        Category::Unknown | Category::Binary => format!("{} file", hunk.category.as_str()),
        // A counted file outside the verdict changed only in whitespace.
        category if hunk.counted_file && hunk.verdict == Verdict::None => {
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

/// TEXT through `emit`, line by line.
pub(crate) fn emit_text(out: &mut impl Write, text: &str) -> io::Result<()> {
    text.lines().try_for_each(|line| emit(out, line))
}

/// MESSAGE on stderr as one `shapr: ` line, escaped. Every note, warning and
/// error message goes through here.
pub fn note(message: &str) {
    eprintln!("shapr: {}", escape(message));
}

/// WRITE against stdout, then flushed. The only handle on stdout: what
/// WRITE writes is shapr's own (`emit`, JSON, a completion script).
pub(crate) fn to_stdout(
    write: impl FnOnce(&mut io::StdoutLock<'static>) -> io::Result<()>,
) -> io::Result<()> {
    let mut out = io::stdout().lock();
    write(&mut out).and_then(|()| out.flush())
}

/// A clap message (help, version, a usage error) on clap's stream, each
/// line escaped: argv quoted back may hold anything.
pub(crate) fn clap(error: &clap::Error) {
    let text = clap_text(error);
    if error.use_stderr() {
        text.lines().for_each(|line| eprintln!("{}", escape(line)));
    } else {
        let _ = to_stdout(|out| emit_text(out, &text));
    }
}

/// A clap error rendered with the arguments it quotes escaped, so its only
/// line breaks are clap's own.
pub(crate) fn clap_text(error: &clap::Error) -> String {
    use clap::error::ErrorKind;
    // Help and version text is shapr's own, never an argument: its
    // apostrophes ("DIR's") aren't quotes, and its line breaks are real.
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        return error.render().to_string();
    }
    // clap quotes every argument it names ('…'): escape control characters
    // inside those spans, so an argument can't break a line, and leave
    // clap's own line breaks, which are never inside quotes, alone.
    let mut text = error.render().to_string();
    // An argument with more than control characters may also appear
    // unquoted (a validator's message): escape it wherever it appears.
    for arg in std::env::args_os().skip(1) {
        let arg = arg.to_string_lossy();
        let values = std::iter::once(&*arg).chain(arg.split_once('=').map(|(_, v)| v));
        for value in
            values.filter(|v| v.chars().any(char::is_control) && !v.chars().all(char::is_control))
        {
            text = text.replace(value, &escape(value));
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut quoted = false;
    for c in text.chars() {
        if c == '\'' {
            quoted = !quoted;
        }
        if quoted && c.is_control() {
            push_escaped(&mut out, c);
        } else {
            out.push(c);
        }
    }
    out
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

    /// Text reaches stdout and stderr only through this module, which
    /// escapes it (#123): a raw write elsewhere could print an untrusted path.
    #[test]
    fn every_output_write_goes_through_report() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let raw = [
            "println!",
            "eprintln!",
            "print!",
            "eprint!",
            "io::stdout()",
            "io::stderr()",
            ".print()",
        ];
        let mut dirs = vec![root.join("src")];
        let mut offenders = Vec::new();
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if name == "src/report.rs" {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let code = text.split("#[cfg(test)]").next().unwrap();
                for line in code.lines().filter(|l| !l.trim_start().starts_with("//")) {
                    if raw.iter().any(|r| line.contains(r)) {
                        offenders.push(format!("{name}: {}", line.trim()));
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "write through report: {offenders:#?}");
    }

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
