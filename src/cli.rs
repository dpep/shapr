//! The command line: flags, input selection, exit codes.

use crate::report::{self, Format};
use crate::{change, classify, diff, git};
use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;

const AFTER_HELP: &str = "\
EXAMPLES
  shapr main..HEAD           classify a branch's commits
  shapr main                 main against the working tree
  git diff --staged | shapr  classify a diff from stdin (needs `git diff` index lines)
  shapr -J main..HEAD        one JSON line per file and hunk, then a summary line
  shapr -q main..HEAD        no output; the exit code is the verdict

  Only Ruby is parsed so far; hunks in other files are `logic`, confidence 0.
  To leave them out: git diff main -- '*.rb' | shapr

EXIT CODES
  0   structure: every hunk changes structure, not behavior (or there are no hunks)
  1   behavior: every hunk changes behavior
  3   mixed: both
  2   error: git failed, input unreadable
  64  usage: the command line is wrong, or there is no input

  Structure is a shape, not a safety claim: extracts and inlines still deserve review.";

#[derive(Parser)]
#[command(
    name = "shapr",
    version,
    about = "Classify the shape of a code change: format, comment, rename, move, extract, or logic.",
    after_help = AFTER_HELP
)]
struct Cli {
    /// Revisions to compare, as `git diff` takes them (`main..HEAD`,
    /// `main...HEAD`, or one revision against the working tree). Without
    /// it, a unified diff is read from stdin.
    #[arg(value_name = "RANGE")]
    range: Option<String>,

    /// Emit one pretty JSON object: files, hunks and summary.
    #[arg(short = 'j', long)]
    json: bool,

    /// Emit newline-delimited JSON: each file's record then its hunks, then
    /// one summary line.
    #[arg(short = 'J', long, conflicts_with = "json")]
    ndjson: bool,

    /// Log what was read and parsed to stderr.
    #[arg(short, long)]
    verbose: bool,

    /// Write nothing to stdout; the exit code still carries the verdict.
    #[arg(short, long)]
    quiet: bool,

    /// Print a shell completion script (bash, zsh, fish, elvish,
    /// powershell); defaults to $SHELL.
    #[arg(long, value_name = "SHELL", num_args = 0..=1)]
    completions: Option<Option<Shell>>,
}

const ERROR: u8 = 2;
const USAGE: u8 = 64;

pub fn run() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            if !error.use_stderr() {
                return ExitCode::SUCCESS;
            }
            let first = error.to_string();
            emit_error(
                requested_format(),
                first.lines().next().unwrap_or_default(),
                USAGE,
            );
            return ExitCode::from(USAGE);
        }
    };
    crate::log::set_verbose(cli.verbose);
    let format = if cli.ndjson {
        Format::Ndjson
    } else if cli.json {
        Format::Json
    } else {
        Format::Text
    };

    if let Some(shell) = cli.completions {
        let Some(shell) = shell.or_else(Shell::from_env) else {
            return fail(
                format,
                "can't tell the shell from $SHELL: name one, e.g. --completions zsh",
                USAGE,
            );
        };
        clap_complete::generate(shell, &mut Cli::command(), "shapr", &mut std::io::stdout());
        return ExitCode::SUCCESS;
    }

    let text = match &cli.range {
        Some(range) if range.starts_with('-') => {
            return fail(format, &format!("`{range}` is not a range"), USAGE);
        }
        Some(range) => git::diff(range),
        None if std::io::stdin().is_terminal() => {
            return fail(
                format,
                "no input: give a range (shapr main..HEAD) or pipe a diff (git diff | shapr)",
                USAGE,
            );
        }
        None => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map(|_| text)
                .map_err(|e| anyhow::anyhow!("reading stdin: {e}"))
        }
    };
    let text = match text {
        Ok(text) => text,
        Err(error) => return fail(format, &format!("{error:#}"), ERROR),
    };

    let change = change::assemble(diff::parse(&text), &mut git::Blobs::default());
    let labels = classify::classify(&change);
    let report = report::build(&change, &labels);
    if !cli.quiet {
        let mut stdout = std::io::stdout().lock();
        let written = report::render(&mut stdout, format, &report).and_then(|()| stdout.flush());
        // A closed pipe (`shapr … | head`) is the reader's choice, not an error.
        if let Err(error) = written.filter_broken_pipe() {
            eprintln!("shapr: writing output: {error}");
            return ExitCode::from(ERROR);
        }
    }
    ExitCode::from(report.summary.verdict.exit_code())
}

trait BrokenPipe {
    fn filter_broken_pipe(self) -> std::io::Result<()>;
}

impl BrokenPipe for std::io::Result<()> {
    fn filter_broken_pipe(self) -> std::io::Result<()> {
        match self {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            other => other,
        }
    }
}

/// The message on stderr; a structured caller also gets it on stdout.
fn fail(format: Format, message: &str, code: u8) -> ExitCode {
    eprintln!("shapr: {message}");
    emit_error(format, message, code);
    ExitCode::from(code)
}

fn emit_error(format: Format, message: &str, code: u8) {
    let error = serde_json::json!({ "record": "error", "error": message, "code": code });
    let rendered = match format {
        Format::Text => return,
        Format::Json => serde_json::to_string_pretty(&error),
        Format::Ndjson => serde_json::to_string(&error),
    };
    if let Ok(rendered) = rendered {
        println!("{rendered}");
    }
}

/// The format argv asks for, read before clap has parsed it, so a usage
/// error reaches a JSON caller as JSON. Every short flag but `--completions`
/// is a switch, so a cluster like `-qJ` reads letter by letter.
fn requested_format() -> Format {
    let mut format = Format::Text;
    for arg in std::env::args().skip(1).take_while(|a| a != "--") {
        let (json, ndjson) = match arg.strip_prefix("--") {
            Some(long) => (long == "json", long == "ndjson"),
            None => match arg.strip_prefix('-') {
                Some(short) => (short.contains('j'), short.contains('J')),
                None => (false, false),
            },
        };
        if ndjson {
            format = Format::Ndjson;
        } else if json {
            format = Format::Json;
        }
    }
    format
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_help_examples_parse() {
        let examples: Vec<&str> = AFTER_HELP
            .lines()
            .map(str::trim_start)
            .filter(|l| l.starts_with("shapr "))
            .collect();
        assert!(examples.len() >= 4);
        for example in examples {
            let command = example.split("  ").next().unwrap();
            let args = command.split_whitespace();
            assert!(Cli::try_parse_from(args).is_ok(), "{command}");
        }
    }

    #[test]
    fn help_lists_every_flag_and_the_exit_codes() {
        let help = Cli::command().render_long_help().to_string();
        for arg in Cli::command().get_arguments() {
            if let Some(long) = arg.get_long() {
                assert!(
                    help.contains(&format!("--{long}")),
                    "--{long} missing from help"
                );
            }
        }
        for code in [
            "0   structure",
            "1   behavior",
            "3   mixed",
            "2   error",
            "64  usage",
        ] {
            assert!(help.contains(code), "{code}");
        }
    }
}
