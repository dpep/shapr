//! The output records (the public schema, see docs/schema.md) and their
//! rendering. Everything shapr writes to stdout goes through here.

use crate::change::{Change, FileChange};
use crate::classify::{Confidence, Evidence, Kind, Label, Labels, Method};
use crate::diff::{Hunk, Lines};
use crate::tree::Tree;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

/// Bumped on any breaking change to a record's fields.
pub const SCHEMA: u32 = 1;

#[derive(Serialize)]
pub struct Report<'a> {
    pub files: Vec<FileRecord<'a>>,
    pub hunks: Vec<HunkRecord<'a>>,
    pub summary: Summary,
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

    fn code(&self) -> usize {
        self.code_added + self.code_deleted
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
    confidence: Confidence,
    reason: String,
    evidence: Evidence,
    #[serde(flatten)]
    lines: LineCounts,
    detail: Map<String, Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every hunk is a structural kind.
    Structure,
    /// Every hunk changes behavior.
    Behavior,
    Mixed,
    /// No hunks.
    Empty,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Structure => "structure",
            Verdict::Behavior => "behavior",
            Verdict::Mixed => "mixed",
            Verdict::Empty => "empty",
        }
    }

    pub fn exit_code(self) -> u8 {
        match self {
            Verdict::Structure | Verdict::Empty => 0,
            Verdict::Behavior => 1,
            Verdict::Mixed => 3,
        }
    }
}

#[derive(Serialize)]
pub struct Summary {
    record: &'static str,
    schema: u32,
    pub verdict: Verdict,
    hunks: usize,
    unparsed_hunks: usize,
    generated_hunks: usize,
    kinds: BTreeMap<&'static str, usize>,
    files: usize,
    directories: usize,
    subsystems: usize,
    entropy: f64,
    #[serde(flatten)]
    lines: LineCounts,
    behavior_lines: usize,
    structure_share: f64,
}

pub fn build<'a>(change: &'a Change, labels: &Labels) -> Report<'a> {
    let mut files = Vec::new();
    let mut hunks = Vec::new();
    for (file, labels) in change.files.iter().zip(labels) {
        let mut totals = LineCounts::default();
        for (hunk, label) in file.diff.hunks.iter().zip(labels) {
            let mut lines = count_lines(file, hunk, label, hunks.len());
            if file.generated {
                lines.review_lines = 0;
            }
            totals.add(&lines);
            hunks.push(HunkRecord {
                record: "hunk",
                id: hunks.len(),
                file: file.diff.path(),
                old_file: file.diff.old_path.as_deref(),
                language: file.language(),
                parsed: label.evidence.method == Method::Ast,
                generated: file.generated,
                before: hunk.before,
                after: hunk.after,
                kind: label.kind,
                confidence: label.confidence,
                reason: label.reason.clone(),
                evidence: label.evidence.clone(),
                lines,
                detail: label.detail.clone(),
            });
        }
        files.push(FileRecord {
            record: "file",
            file: file.diff.path(),
            old_file: file.diff.old_path.as_deref(),
            language: file.language(),
            parsed: labels
                .iter()
                .all(|l: &Label| l.evidence.method == Method::Ast),
            generated: file.generated,
            lines_before: file.lines_before,
            hunks: labels.len(),
            lines: totals,
        });
    }
    let summary = summarize(&files, &hunks);
    Report {
        files,
        hunks,
        summary,
    }
}

fn count_lines(file: &FileChange, hunk: &Hunk, label: &Label, id: usize) -> LineCounts {
    let mut counts = LineCounts {
        added: hunk.added.len(),
        deleted: hunk.removed.len(),
        ..LineCounts::default()
    };
    let Ok(trees) = &file.trees else {
        // Unparsed, every changed line is taken for code.
        counts.code_added = counts.added;
        counts.code_deleted = counts.deleted;
        counts.review_lines = counts.code();
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
    counts.review_lines = match label.kind {
        Kind::FormatOnly | Kind::CommentOnly | Kind::Rename | Kind::Move => 0,
        // Structure, but where refactoring bugs cluster: still read.
        Kind::Extract | Kind::Inline | Kind::Logic => counts.code(),
        Kind::MoveWithChanges => {
            let (before, after) = moved_changes(label, id);
            let code = |lines: &[usize], side: &Tree, changed: &BTreeSet<usize>| {
                lines
                    .iter()
                    .filter(|l| changed.contains(l) && side.line(**l).code)
                    .count()
            };
            code(&hunk.removed, &trees.before, &before) + code(&hunk.added, &trees.after, &after)
        }
    };
    counts
}

/// The lines inside the hunk's moved units that changed in the move, from
/// `detail.moves`: before-side lines of the units it removed, after-side
/// lines of the units it added.
fn moved_changes(label: &Label, id: usize) -> (BTreeSet<usize>, BTreeSet<usize>) {
    let (mut before, mut after) = (BTreeSet::new(), BTreeSet::new());
    let moves = label.detail.get("moves").and_then(Value::as_array);
    for entry in moves.into_iter().flatten() {
        let lines = |key: &str| {
            entry[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .map(|l| l as usize)
                .collect::<Vec<_>>()
        };
        let id = Some(id as u64);
        if entry["from_hunk"].as_u64() == id {
            before.extend(lines("changed_before"));
        }
        if entry["to_hunk"].as_u64() == id {
            after.extend(lines("changed_after"));
        }
    }
    (before, after)
}

fn summarize(files: &[FileRecord], hunks: &[HunkRecord]) -> Summary {
    let mut kinds: BTreeMap<&'static str, usize> =
        Kind::ALL.iter().map(|k| (k.as_str(), 0)).collect();
    let mut lines = LineCounts::default();
    let (mut structure, mut behavior, mut structure_lines, mut behavior_lines) = (0, 0, 0, 0);
    for hunk in hunks {
        *kinds.entry(hunk.kind.as_str()).or_default() += 1;
        // A generated hunk sways the verdict but not the size.
        let size = !hunk.generated;
        if size {
            lines.add(&hunk.lines);
        }
        if hunk.kind.is_structure() {
            structure += 1;
            structure_lines += hunk.lines.changed() * usize::from(size);
        } else {
            behavior += 1;
            behavior_lines += hunk.lines.review_lines;
        }
    }
    let verdict = match (structure, behavior) {
        (0, 0) => Verdict::Empty,
        (_, 0) => Verdict::Structure,
        (0, _) => Verdict::Behavior,
        _ => Verdict::Mixed,
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
    Summary {
        record: "summary",
        schema: SCHEMA,
        verdict,
        hunks: hunks.len(),
        unparsed_hunks: hunks.iter().filter(|h| !h.parsed).count(),
        generated_hunks: hunks.iter().filter(|h| h.generated).count(),
        kinds,
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
        writeln!(
            out,
            "{:width$}  {:<17} {:.2}  {}{}",
            place(hunk),
            hunk.kind.as_str(),
            hunk.confidence.get(),
            hunk.reason,
            if hunk.generated {
                " (generated: not counted in size)"
            } else {
                ""
            },
        )?;
    }
    let s = &report.summary;
    let kinds: Vec<String> = s
        .kinds
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    writeln!(
        out,
        "{}: {} hunks ({}); review {} lines{}; comments +{} -{}; structure share {:.2}",
        s.verdict.as_str(),
        s.hunks,
        if kinds.is_empty() {
            "none".to_string()
        } else {
            kinds.join(", ")
        },
        s.lines.review_lines,
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
