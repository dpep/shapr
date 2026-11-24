//! `shapr norms FILE...`: each file's norms at file scope, no diff needed.

use super::{Counts, DEFAULT_THRESHOLD, Worktree, count, for_path, infer, skipped};
use crate::change::Category;
use crate::git::Linguist;
use crate::report::Format;
use clap::Parser;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const AFTER_HELP: &str = "\
EXAMPLES
  shapr norms lib/cart.rb        each norm: its counts, inferred style, confidence, deviating lines
  shapr norms -J lib/*.rb        one JSON line per file and norm, then a summary line

  A style is a norm when (count + 1) / (total + styles) reaches --norm-threshold;
  below it, shapr abstains. The nearest .rubocop.yml's EnforcedStyle is reported
  beside the inferred style, and expected when nothing is inferred. Experimental.

EXIT CODES
  0   success, deviations or not
  2   error: a file can't be read
  64  usage";

#[derive(Parser)]
#[command(
    name = "shapr norms",
    about = "Report a file's style norms: what its code already looks like (experimental).",
    after_help = AFTER_HELP
)]
pub(crate) struct NormsCli {
    /// Files to read.
    #[arg(value_name = "FILE", required = true)]
    files: Vec<PathBuf>,

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
struct FileRecord {
    record: &'static str,
    file: String,
    language: Option<&'static str>,
    skipped: Option<&'static str>,
}

#[derive(Serialize)]
struct NormRecord {
    record: &'static str,
    file: String,
    norm: &'static str,
    scope: &'static str,
    counts: Counts,
    inferred: Option<String>,
    confidence: Option<f64>,
    configured: Option<String>,
    expected: Option<String>,
    deviations: Vec<usize>,
}

#[derive(Serialize)]
struct Summary {
    record: &'static str,
    files: usize,
    norms: usize,
    inferred: usize,
    deviations: usize,
}

#[derive(Serialize)]
struct Report {
    meta: crate::report::Meta,
    files: Vec<FileRecord>,
    norms: Vec<NormRecord>,
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
            let first = error.to_string();
            let first = first.lines().next().unwrap_or_default();
            let message = first.strip_prefix("error: ").unwrap_or(first);
            crate::cli::emit_error(crate::cli::requested_format(), message, crate::cli::USAGE);
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
    let mut report = Report {
        meta: crate::report::Meta::new(None),
        files: Vec::new(),
        norms: Vec::new(),
        summary: Summary {
            record: "summary",
            files: 0,
            norms: 0,
            inferred: 0,
            deviations: 0,
        },
    };
    for path in &cli.files {
        let source = match std::fs::read(path) {
            Ok(source) => source,
            Err(error) => {
                return crate::cli::fail(
                    format,
                    &format!("{}: {error}", path.display()),
                    crate::cli::ERROR,
                );
            }
        };
        let (file, norms) = file_norms(path, &source, cli.norm_threshold);
        report.files.push(file);
        report.norms.extend(norms);
    }
    let s = &mut report.summary;
    s.files = report.files.len();
    s.norms = report.norms.len();
    s.inferred = report.norms.iter().filter(|n| n.inferred.is_some()).count();
    s.deviations = report.norms.iter().map(|n| n.deviations.len()).sum();
    if cli.quiet {
        return ExitCode::SUCCESS;
    }
    let mut stdout = std::io::stdout().lock();
    let written = render(&mut stdout, format, &report).and_then(|()| stdout.flush());
    match written {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
            eprintln!("shapr: writing output: {e}");
            ExitCode::from(crate::cli::ERROR)
        }
        _ => ExitCode::SUCCESS,
    }
}

fn file_norms(path: &Path, source: &[u8], threshold: f64) -> (FileRecord, Vec<NormRecord>) {
    let name = path.display().to_string();
    let language = for_path(&name);
    let analysis = language.map(|l| l.analyze(source));
    let reason = match (&language, &analysis) {
        (None, _) => Some("unsupported"),
        (_, Some(a)) if a.errors => Some("syntax_error"),
        _ => skipped(&name, Category::of(&name, false, Linguist::default())),
    };
    let file = FileRecord {
        record: "file",
        file: name.clone(),
        language: language.map(|l| l.name()),
        skipped: reason,
    };
    let (Some(language), Some(analysis), None) = (language, analysis, reason) else {
        return (file, Vec::new());
    };
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = full.parent().unwrap_or(Path::new("")).to_path_buf();
    let root = repo_root(&dir);
    let relative = full.strip_prefix(&root).unwrap_or(&full);
    let relative: Vec<String> = (relative.components())
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let configured = language.configured(&relative.join("/"), &Worktree(root));
    let norms = language
        .norms()
        .iter()
        .map(|def| {
            let observations = || analysis.observations.iter().filter(|o| o.norm == def.id);
            let counts = count(def, observations());
            let norm = infer(&counts, threshold);
            let configured = configured.get(def.id).cloned();
            let expected = norm
                .as_ref()
                .map(|n| n.style.clone())
                .or(configured.clone());
            let mut deviations: Vec<usize> = observations()
                .filter(|o| expected.as_ref().is_some_and(|e| o.style != *e))
                .map(|o| o.line)
                .collect();
            deviations.dedup();
            NormRecord {
                record: "norm",
                file: name.clone(),
                norm: def.id,
                scope: "file",
                counts,
                inferred: norm.as_ref().map(|n| n.style.clone()),
                confidence: norm.map(|n| n.confidence),
                configured,
                expected,
                deviations,
            }
        })
        .collect();
    (file, norms)
}

/// The enclosing git working tree, where a linter config search stops; the
/// filesystem root outside one.
fn repo_root(dir: &Path) -> PathBuf {
    let mut at = Some(dir);
    while let Some(d) = at {
        if d.join(".git").exists() {
            return d.to_path_buf();
        }
        at = d.parent();
    }
    dir.ancestors().last().unwrap_or(dir).to_path_buf()
}

fn render(out: &mut impl Write, format: Format, report: &Report) -> std::io::Result<()> {
    match format {
        Format::Json => {
            serde_json::to_writer_pretty(&mut *out, report)?;
            writeln!(out)
        }
        Format::Ndjson => {
            serde_json::to_writer(&mut *out, &report.meta)?;
            writeln!(out)?;
            for file in &report.files {
                serde_json::to_writer(&mut *out, file)?;
                writeln!(out)?;
                for norm in report.norms.iter().filter(|n| n.file == file.file) {
                    serde_json::to_writer(&mut *out, norm)?;
                    writeln!(out)?;
                }
            }
            serde_json::to_writer(&mut *out, &report.summary)?;
            writeln!(out)
        }
        Format::Text => {
            for file in &report.files {
                if let Some(reason) = file.skipped {
                    writeln!(out, "{}: skipped ({reason})", file.file)?;
                }
            }
            let width = report.norms.iter().map(|n| n.norm.len()).max().unwrap_or(0);
            for n in &report.norms {
                let counts: Vec<String> =
                    n.counts.iter().map(|(s, c)| format!("{s} {c}")).collect();
                let style = match (&n.inferred, n.confidence) {
                    (Some(style), Some(c)) => format!("{style} {c:.2}"),
                    _ => "abstain".to_string(),
                };
                write!(
                    out,
                    "{}  {:width$}  {style} ({})",
                    n.file,
                    n.norm,
                    counts.join(", ")
                )?;
                if let Some(configured) = &n.configured
                    && n.inferred.as_ref() != Some(configured)
                {
                    write!(out, "; config says {configured}")?;
                }
                if !n.deviations.is_empty() {
                    let lines: Vec<String> = n.deviations.iter().map(usize::to_string).collect();
                    write!(out, "; deviating lines {}", lines.join(", "))?;
                }
                writeln!(out)?;
            }
            Ok(())
        }
    }
}
