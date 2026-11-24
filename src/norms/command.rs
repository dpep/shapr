//! `shapr norms PATH...`: a codebase's norms, its hotspots and a cleanup
//! plan, no diff needed.

use super::codebase::{
    Collected, ConfigRecord, FileRecord, Hotspot, collect, hotspots, pool, share,
};
use super::plan::{Step, plan};
use super::{Counts, DEFAULT_THRESHOLD, Worktree};
use crate::report::Format;
use clap::Parser;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const AFTER_HELP: &str = "\
EXAMPLES
  shapr norms                    the norms of everything under ., its hotspots, a cleanup plan
  shapr norms lib/ app/models    the same, over those directories
  shapr norms lib/cart.rb        one file
  shapr norms -J lib/            one JSON line per file, norm, hotspot and plan step, then a summary

  Each norm pools every file read: a style is the norm when (count + 1) / (total +
  styles) reaches --norm-threshold; below it, shapr abstains. Adherence is the share
  of observations in the expected style (the inferred one, else .rubocop.yml's).
  Hotspots rank files, classes and methods by the norms they break, then deviations
  per line. The plan's steps go layout, then lexical restyles, then renames; each
  step's edits are kept only when the file stays the same program, and apply in
  order. Where config sets a style the code doesn't follow, shapr reports the gap and
  plans neither way. Experimental.

EXIT CODES
  0   success, deviations or not
  2   error: a path can't be read
  64  usage";

#[derive(Parser)]
#[command(
    name = "shapr norms",
    about = "Report a codebase's style norms, where it is least like itself, and a cleanup plan (experimental).",
    after_help = AFTER_HELP
)]
pub(crate) struct NormsCli {
    /// Files and directories to read; a directory is every file under it
    /// shapr judges. Defaults to `.`.
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,

    /// Hotspots of each kind the text output lists; -j and -J list all.
    #[arg(long, value_name = "N", default_value_t = 5)]
    top: usize,

    /// Emit one pretty JSON object: files, norms and summary.
    #[arg(short = 'j', long)]
    json: bool,

    /// Emit newline-delimited JSON: each file's record then its norms, then
    /// one summary line.
    #[arg(short = 'J', long, conflicts_with = "json")]
    ndjson: bool,

    /// Log what was read to stderr.
    #[arg(short, long)]
    verbose: bool,

    /// Write nothing to stdout; errors still go to stderr.
    #[arg(short, long)]
    quiet: bool,

    /// The share a style needs to be a norm, from 0.5 (exclusive) to 1.
    #[arg(long, value_name = "SHARE", default_value_t = DEFAULT_THRESHOLD, value_parser = threshold)]
    norm_threshold: f64,
}

pub fn threshold(value: &str) -> Result<f64, String> {
    let share: f64 = value
        .parse()
        .map_err(|_| format!("`{value}` is not a number"))?;
    if share > 0.5 && share <= 1.0 {
        Ok(share)
    } else {
        Err("must be above 0.5 and at most 1".to_string())
    }
}

#[derive(Serialize)]
struct NormRecord {
    record: &'static str,
    norm: &'static str,
    scope: &'static str,
    files: usize,
    counts: Counts,
    inferred: Option<String>,
    confidence: Option<f64>,
    configured: Option<String>,
    expected: Option<String>,
    adherence: Option<f64>,
    matching: usize,
    judged: usize,
    config: Option<ConfigRecord>,
    step: Option<usize>,
    unplanned: Option<&'static str>,
    deviations: Vec<Deviation>,
}

/// As a hunk's deviation, with its file and without an edit.
#[derive(Serialize)]
struct Deviation {
    file: String,
    line: usize,
    column: usize,
    style: String,
    expected: String,
}

#[derive(Serialize)]
struct Summary {
    record: &'static str,
    files: usize,
    skipped: BTreeMap<&'static str, usize>,
    norms: usize,
    inferred: usize,
    deviations: usize,
    hotspots: usize,
    steps: usize,
    fixes: usize,
    lines_touched: usize,
}

#[derive(Serialize)]
struct Report {
    meta: crate::report::Meta,
    files: Vec<FileRecord>,
    norms: Vec<NormRecord>,
    hotspots: Vec<Hotspot>,
    steps: Vec<Step>,
    summary: Summary,
}

/// Runs `shapr norms`, given the arguments after `shapr`.
pub fn run(args: Vec<String>) -> ExitCode {
    let cli = match NormsCli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            if !error.use_stderr() {
                return ExitCode::SUCCESS;
            }
            let message = crate::cli::usage_message(&error);
            crate::cli::emit_error(crate::cli::requested_format(), &message, crate::cli::USAGE);
            return ExitCode::from(crate::cli::USAGE);
        }
    };
    crate::log::set_verbose(cli.verbose);
    let format = if cli.quiet {
        Format::Text
    } else if cli.ndjson {
        Format::Ndjson
    } else if cli.json {
        Format::Json
    } else {
        Format::Text
    };
    let paths = match cli.paths.is_empty() {
        true => vec![PathBuf::from(".")],
        false => cli.paths.clone(),
    };
    if let Some(missing) = paths.iter().find(|p| !p.exists()) {
        let message = format!("{}: no such file or directory", missing.display());
        return crate::cli::fail(format, &message, crate::cli::ERROR);
    }
    let collected = match collect(&paths) {
        Ok(collected) => collected,
        Err(message) => return crate::cli::fail(format, &message, crate::cli::ERROR),
    };
    let report = build(&paths, collected, cli.norm_threshold);
    if cli.quiet {
        return ExitCode::SUCCESS;
    }
    let mut stdout = std::io::stdout().lock();
    let written =
        render(&mut stdout, format, &report, &paths, cli.top).and_then(|()| stdout.flush());
    match written {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
            eprintln!("shapr: writing output: {e}");
            ExitCode::from(crate::cli::ERROR)
        }
        _ => ExitCode::SUCCESS,
    }
}

fn build(paths: &[PathBuf], collected: Collected, threshold: f64) -> Report {
    let Collected {
        sources,
        files,
        skipped,
    } = collected;
    let pooled = pool(&sources, threshold);
    let hotspots = hotspots(&sources, &pooled);
    // Renames reach other files: the repository around the first file.
    let git = sources
        .first()
        .map(|s| crate::git::Git::new(Some(s.root.clone())));
    let worktree = sources.first().map(|s| Worktree(s.root.clone()));
    let repository = match (&git, &worktree) {
        (Some(git), Some(worktree)) if worktree.0.join(".git").exists() => Some(
            crate::outside::Repository::new(git, crate::git::State::Worktree, worktree),
        ),
        _ => None,
    };
    let outside: &dyn crate::classify::proof::Outside = match &repository {
        Some(r) => r,
        None => &crate::classify::proof::Nowhere,
    };
    let plan = plan(&sources, &pooled, outside);
    let one_file = paths.len() == 1 && paths[0].is_file();
    let norms: Vec<NormRecord> = (pooled.iter().zip(&plan.planned))
        .map(|(p, planned)| {
            let (matching, judged) = p.adherence();
            let expected = p.expected();
            NormRecord {
                record: "norm",
                norm: p.def.id,
                scope: if one_file { "file" } else { "paths" },
                files: p.files,
                counts: p.counts.clone(),
                inferred: p.inferred.as_ref().map(|n| n.style.clone()),
                confidence: p.inferred.as_ref().map(|n| n.confidence),
                configured: p.configured.clone(),
                adherence: expected.as_ref().and(share(matching, judged)),
                expected,
                matching,
                judged,
                config: None,
                step: planned.ok(),
                unplanned: planned.err().flatten(),
                deviations: p
                    .deviations()
                    .map(|(f, o, e)| Deviation {
                        file: sources[f].path.clone(),
                        line: o.line,
                        column: o.column,
                        style: o.style.clone(),
                        expected: e.to_string(),
                    })
                    .collect(),
            }
        })
        .collect();
    let mut norms = norms;
    for (record, p) in norms.iter_mut().zip(pooled) {
        record.config = p.config;
    }
    let summary = Summary {
        record: "summary",
        files: sources.len(),
        skipped,
        norms: norms.len(),
        inferred: norms.iter().filter(|n| n.inferred.is_some()).count(),
        deviations: norms.iter().map(|n| n.deviations.len()).sum(),
        hotspots: hotspots.len(),
        steps: plan.steps.len(),
        fixes: plan.steps.iter().map(|s| s.fixes).sum(),
        lines_touched: plan.steps.iter().map(|s| s.lines_touched).sum(),
    };
    Report {
        meta: crate::report::Meta::new("norms", None),
        files,
        norms,
        hotspots,
        steps: plan.steps,
        summary,
    }
}

fn render(
    out: &mut impl Write,
    format: Format,
    report: &Report,
    paths: &[PathBuf],
    top: usize,
) -> std::io::Result<()> {
    match format {
        Format::Json => {
            serde_json::to_writer_pretty(&mut *out, report)?;
            writeln!(out)
        }
        Format::Ndjson => {
            let mut line = |record: &dyn erased::Record| -> std::io::Result<()> {
                record.write(out)?;
                writeln!(out)
            };
            line(&report.meta)?;
            for r in &report.files {
                line(r)?;
            }
            for r in &report.norms {
                line(r)?;
            }
            for r in &report.hotspots {
                line(r)?;
            }
            for r in &report.steps {
                line(r)?;
            }
            line(&report.summary)
        }
        Format::Text => text(out, report, paths, top),
    }
}

/// One serializable record of any type, for NDJSON's mixed lines.
mod erased {
    pub trait Record {
        fn write(&self, out: &mut dyn std::io::Write) -> std::io::Result<()>;
    }

    impl<T: serde::Serialize> Record for T {
        fn write(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
            serde_json::to_writer(out, self).map_err(std::io::Error::from)
        }
    }
}

fn text(
    out: &mut impl Write,
    report: &Report,
    paths: &[PathBuf],
    top: usize,
) -> std::io::Result<()> {
    let s = &report.summary;
    let named: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let skipped: Vec<String> = (s.skipped.iter())
        .filter(|(_, n)| **n > 0)
        .map(|(why, n)| format!("{n} {why}"))
        .collect();
    write!(
        out,
        "norms over {} file{} in {}",
        s.files,
        if s.files == 1 { "" } else { "s" },
        named.join(" ")
    )?;
    if !skipped.is_empty() {
        write!(out, " (skipped: {})", skipped.join(", "))?;
    }
    writeln!(out)?;
    let width = report.norms.iter().map(|n| n.norm.len()).max().unwrap_or(0);
    for n in &report.norms {
        let total: usize = n.counts.values().sum();
        if total == 0 {
            continue;
        }
        let style = match (&n.expected, &n.inferred, n.adherence) {
            (Some(e), Some(_), Some(a)) => {
                format!("{e} {:.0}% ({}/{})", a * 100.0, n.matching, n.judged)
            }
            (Some(e), None, Some(a)) => format!(
                "abstain; config {e} {:.0}% ({}/{})",
                a * 100.0,
                n.matching,
                n.judged
            ),
            _ => {
                let counts: Vec<String> =
                    n.counts.iter().map(|(s, c)| format!("{s} {c}")).collect();
                format!("abstain ({})", counts.join(", "))
            }
        };
        write!(out, "  {:width$}  {style}", n.norm)?;
        if let (Some(c), Some(configured), Some(_)) = (&n.config, &n.configured, &n.inferred) {
            match c.adherence {
                Some(a) if c.agreement == "disagrees" => write!(
                    out,
                    "; config says {configured}: {:.0}% ({}/{})",
                    a * 100.0,
                    c.matching,
                    c.checked
                )?,
                _ => {}
            }
        }
        if let Some(c) = &n.config {
            if c.todo {
                write!(out, "; in .rubocop_todo.yml")?;
            }
            if c.withheld > 0 {
                write!(out, "; {} unchecked by config", c.withheld)?;
            }
        }
        writeln!(out)?;
    }
    if !report.hotspots.is_empty() {
        writeln!(out, "hotspots (deviations per 100 lines; norms broken)")?;
    }
    let mut more = 0;
    for (kind, title) in [
        ("file", "files"),
        ("class", "classes"),
        ("method", "methods"),
    ] {
        let of: Vec<&Hotspot> = report.hotspots.iter().filter(|h| h.kind == kind).collect();
        if of.is_empty() {
            continue;
        }
        writeln!(out, "  {title}")?;
        for h in of.iter().take(top) {
            let place = match &h.name {
                Some(name) => format!("{}:{} {name}", h.file, h.lines.start),
                None => h.file.clone(),
            };
            writeln!(
                out,
                "    {place}  {:.1} ({} in {} line{}; {})",
                h.per_100_lines,
                h.deviations,
                h.lines.count,
                if h.lines.count == 1 { "" } else { "s" },
                h.norms.join(", ")
            )?;
        }
        more += of.len().saturating_sub(top);
    }
    if more > 0 {
        writeln!(out, "  ... {more} more (--top N, or -j)")?;
    }
    if report.steps.is_empty() && s.deviations == 0 {
        writeln!(out, "no deviations: nothing to plan")?;
    }
    if !report.steps.is_empty() {
        writeln!(out, "plan (apply in order; diffs under -j)")?;
    }
    for step in &report.steps {
        let pct = |a: Option<f64>| a.map_or("-".to_string(), |a| format!("{:.0}%", a * 100.0));
        writeln!(
            out,
            "  {}. tier {} {}  {} -> {}: {} fix{} on {} line{} in {} file{} ({} -> {}{}), a {} change",
            step.step,
            step.tier,
            step.risk,
            step.norm,
            step.style,
            step.fixes,
            if step.fixes == 1 { "" } else { "es" },
            step.lines_touched,
            if step.lines_touched == 1 { "" } else { "s" },
            step.files,
            if step.files == 1 { "" } else { "s" },
            pct(step.adherence_before),
            pct(step.adherence_after),
            match step.unfixed {
                0 => String::new(),
                n => format!(", {n} left without an edit"),
            },
            step.expect_kind
        )?;
    }
    let unplanned: Vec<String> = (report.norms.iter())
        .filter_map(|n| {
            let why = match n.unplanned? {
                "config_disagrees" => "config disagrees with the code: change one",
                "withheld" => "config doesn't check where it deviates",
                _ => "no mechanical edit, or none that keeps the program",
            };
            Some(format!("{} ({why})", n.norm))
        })
        .collect();
    if !unplanned.is_empty() {
        writeln!(out, "  not planned: {}", unplanned.join("; "))?;
    }
    Ok(())
}
