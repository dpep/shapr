//! The command line: flags, input selection, exit codes.

use crate::claims::{Claims, Message, Source};
use crate::classify::proof::{Nowhere, Outside};
use crate::outside::{Repository, Union};
use crate::report::{self, Format};
use crate::{change, classify, diff, git, github};
use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

const AFTER_HELP: &str = "\
EXAMPLES
  shapr main..HEAD           classify a branch's commits
  shapr main                 main against the working tree
  git diff --staged | shapr  classify a diff from stdin (needs `git diff` index lines)
  shapr -J main..HEAD        JSON lines: a meta record, each file and hunk, then a summary
  shapr -C app main..HEAD    run in another repository, like git -C
  shapr --message pr.md main..HEAD   also read claims from a PR description
  shapr acme/widgets#12      a GitHub pull request: the diff its Files tab shows
  shapr acme/widgets@1a2b3c4  a GitHub commit, against its (first) parent
  shapr norms lib/cart.rb    a file's style norms, no diff needed (see shapr norms --help)

  With no RANGE, shapr reads a diff from stdin; like any filter, it waits while an
  inherited stdin stays open. An empty stdin is no input (64), so a failed
  `git diff` upstream is an error, not an empty verdict.

  Ruby and Rust are parsed; hunks in other files are `logic`, confidence 0. Only
  code, test, config and manifest files count toward the verdict: a docs-only change
  is `none`, and lockfiles, vendored and binary files and unparsed languages are
  `unknown`, counted in summary.verdicts.

GITHUB (experimental: the forms, meta.source and the cache may change before 1.0)
  RANGE may name a pull request (https://github.com/OWNER/REPO/pull/N, OWNER/REPO#N,
  or #N for this repository's GitHub remote) or a commit (.../commit/SHA,
  .../pull/N/commits/SHA, OWNER/REPO@SHA). A shorthand that is also a local revision
  is the local revision. Metadata comes from the GitHub API, with a token from GH_TOKEN,
  GITHUB_TOKEN or `gh auth token` if there is one; objects are fetched with your git
  credentials into a cache of partial clones (--cache-dir), never into this repository.

EXIT CODES
  0   success, whatever the verdict: the verdict is in the output
  2   error: git failed (not a repository, a bad revision), stdin is not a diff, or GitHub
      failed (unreachable, not found, rate limited)
  64  usage: the command line is wrong, or there is no input (an empty stdin)

  Structure is a shape, not a safety claim: extracts and inlines still deserve review.";

#[derive(Parser)]
#[command(
    name = "shapr",
    version,
    about = "Classify the shape of a code change: format, comment, restyle, rename, move, extract, inline, or logic.",
    override_usage = "shapr [OPTIONS] [RANGE]\n       shapr norms [OPTIONS] <FILE>...",
    after_help = AFTER_HELP
)]
struct Cli {
    /// Revisions to compare, as `git diff` takes them (`main..HEAD`,
    /// `main...HEAD`, or one revision against the working tree), or a GitHub
    /// pull request or commit (see GITHUB below). Without it, a unified diff
    /// is read from stdin; an empty stdin is a usage error.
    #[arg(value_name = "RANGE")]
    range: Option<String>,

    /// Run as if shapr was started in DIR, like `git -C`. GIT_DIR,
    /// GIT_WORK_TREE and GIT_INDEX_FILE are ignored, so DIR's repository
    /// is the one read.
    #[arg(short = 'C', value_name = "DIR")]
    dir: Option<PathBuf>,

    /// Emit one pretty JSON object: meta, files, hunks and summary.
    #[arg(short = 'j', long)]
    json: bool,

    /// Emit newline-delimited JSON: a meta line, each file's record then its
    /// hunks, then one summary line.
    #[arg(short = 'J', long, conflicts_with = "json")]
    ndjson: bool,

    /// Log what was read and parsed to stderr.
    #[arg(short, long)]
    verbose: bool,

    /// Write nothing to stdout, not even a JSON error record; errors still go
    /// to stderr and set the exit code.
    #[arg(short, long)]
    quiet: bool,

    /// Read claims from FILE, a description of the change such as a PR body,
    /// beside the range's commit messages (see summary.claims). `-` reads
    /// stdin, when a RANGE is given. Claims never change a hunk's label.
    #[arg(long, value_name = "FILE")]
    message: Option<PathBuf>,
    /// Experimental. Where GitHub repositories are cached; also SHAPR_CACHE_DIR. Defaults
    /// to $XDG_CACHE_HOME/shapr, else ~/.cache/shapr. Must be absolute.
    #[arg(long, value_name = "DIR")]
    cache_dir: Option<PathBuf>,

    /// Experimental. Show the GitHub cache: its path, the repositories in it,
    /// and their size.
    #[arg(long, conflicts_with_all = ["range", "clear_cache"])]
    status: bool,

    /// Experimental. Remove every cached GitHub repository, then show the
    /// cache.
    #[arg(long, conflicts_with = "range")]
    clear_cache: bool,

    /// The share a style needs to be a norm of the code around a hunk, from
    /// 0.5 (exclusive) to 1 (see hunk.style; experimental).
    #[arg(long, value_name = "SHARE", default_value_t = crate::norms::DEFAULT_THRESHOLD, value_parser = crate::norms::command::threshold)]
    norm_threshold: f64,

    /// Print a shell completion script (bash, zsh, fish, elvish,
    /// powershell); defaults to $SHELL.
    #[arg(long, value_name = "SHELL", num_args = 0..=1)]
    completions: Option<Option<Shell>>,
}

pub(crate) const ERROR: u8 = 2;
pub(crate) const USAGE: u8 = 64;
const NO_INPUT: &str =
    "no input: give a range (shapr main..HEAD) or pipe a diff (git diff | shapr)";

pub fn run() -> ExitCode {
    // A subcommand beside a free RANGE: `norms` is never read as a revision,
    // and the format flags may come before it.
    let mut args: Vec<String> = std::env::args().collect();
    let first = args.iter().skip(1).position(|a| {
        !matches!(
            a.as_str(),
            "-j" | "-J" | "-v" | "-q" | "--json" | "--ndjson" | "--verbose" | "--quiet"
        )
    });
    if let Some(at) = first.map(|i| i + 1).filter(|&i| args[i] == "norms") {
        args.remove(at);
        args[0] = "shapr norms".to_string();
        return crate::norms::command::run(args);
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            if !error.use_stderr() {
                return ExitCode::SUCCESS;
            }
            let first = error.to_string();
            let first = first.lines().next().unwrap_or_default();
            emit_error(
                requested_format(),
                first.strip_prefix("error: ").unwrap_or(first),
                USAGE,
            );
            return ExitCode::from(USAGE);
        }
    };
    crate::log::set_verbose(cli.verbose);
    // -q keeps stdout empty, error records included.
    let format = if cli.quiet {
        Format::Text
    } else if cli.ndjson {
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

    if let Some(dir) = cli.dir.as_ref().filter(|d| !d.is_dir()) {
        return fail(
            format,
            &format!("-C {}: not a directory", dir.display()),
            USAGE,
        );
    }
    let stdin_message = cli.message.as_deref() == Some(std::path::Path::new("-"));
    if stdin_message && cli.range.is_none() {
        return fail(
            format,
            "--message - needs a RANGE: stdin carries the diff without one",
            USAGE,
        );
    }
    if cli.status || cli.clear_cache {
        return cache(format, cli.cache_dir, cli.clear_cache, cli.quiet);
    }
    let mut git = git::Git::new(cli.dir.clone());
    let mut remote = None;
    let text = match &cli.range {
        Some(range) if range.starts_with('-') => {
            return fail(format, &format!("`{range}` is not a range"), USAGE);
        }
        Some(range) => match github::resolve(range, &git) {
            Err(message) => return fail(format, &message, USAGE),
            Ok(None) => git.diff(range),
            Ok(Some(target)) => {
                let cache = match github::Cache::resolve(cli.cache_dir.clone()) {
                    Ok(cache) => cache,
                    Err(message) => return fail(format, &message, USAGE),
                };
                match github::fetch(&target, &cache) {
                    Err(error) => return fail(format, &format!("{error:#}"), ERROR),
                    Ok(fetched) => {
                        let text = fetched.git.diff(&fetched.range);
                        remote = Some(fetched);
                        text
                    }
                }
            }
        },
        None if std::io::stdin().is_terminal() => return fail(format, NO_INPUT, USAGE),
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

    // A combined diff has no single before side: never read it as an empty change.
    if text
        .lines()
        .any(|l| l.starts_with("diff --cc ") || l.starts_with("diff --combined "))
    {
        return fail(format, &format!("the diff is {}", git::MERGE), ERROR);
    }
    let files = diff::parse(&text);
    if cli.range.is_none() && files.is_empty() {
        // An empty pipe is usually an upstream failure (`git diff nope | shapr`).
        if text.trim().is_empty() {
            return fail(format, NO_INPUT, USAGE);
        }
        return fail(format, "stdin is not a diff: no file headers", ERROR);
    }
    let source = remote.as_ref().map(|r| r.source.clone());
    let messages = match messages(&cli, &git, stdin_message, remote.as_mut()) {
        Ok(messages) => messages,
        Err(error) => return fail(format, &format!("{error:#}"), ERROR),
    };
    // The range git diffed: a GitHub change's own, in its clone.
    let range = remote
        .as_ref()
        .map(|r| r.range.clone())
        .or(cli.range.clone());
    if let Some(remote) = remote {
        git = remote.git;
    }
    // Where uses outside the change are searched: a diff on stdin names
    // its own after side only by blob ids.
    let states = match (&cli.range, git.after_revision(cli.range.as_deref())) {
        (_, Some(rev)) => vec![git::State::Revision(rev)],
        (Some(_), None) => vec![git::State::Worktree],
        (None, None) => git.stdin_after(&files),
    };
    if cli.range.is_none() && states.is_empty() {
        crate::verbose!(
            "the diff doesn't end in the index or the working tree: uses outside it aren't searched"
        );
    }
    let before = git.before_revision(range.as_deref());
    let after = match states.first() {
        Some(git::State::Revision(rev)) => Some(rev.as_str()),
        _ => None,
    };
    let change = change::assemble(files, &git, before.as_deref(), after);
    // The repository as of the change's after side: config and neighbors
    // for norms, uses and library layout for proof.
    let revision = after.map(|rev| crate::norms::Revision::new(&git, rev.to_string()));
    let index = crate::norms::Index(&git);
    let worktree = git.toplevel().map(crate::norms::Worktree);
    let snapshot: Option<&dyn crate::norms::Snapshot> = match (states.first(), &revision, &worktree)
    {
        (_, Some(revision), _) => Some(revision),
        (Some(git::State::Index), ..) => Some(&index),
        (_, None, Some(worktree)) => Some(worktree),
        (_, None, None) => None,
    };
    let repositories: Vec<Repository> = states
        .into_iter()
        .filter_map(|state| {
            let snapshot: &dyn crate::norms::Snapshot = match &state {
                git::State::Revision(_) => revision.as_ref()?,
                git::State::Index => &index,
                git::State::Worktree => worktree.as_ref()?,
            };
            Some(Repository::new(&git, state, snapshot))
        })
        .collect();
    let union = Union(&repositories);
    let outside: &dyn Outside = match repositories.as_slice() {
        [] => &Nowhere,
        [repository] => repository,
        _ => &union,
    };
    let labels = classify::classify(&change, outside);
    let mut report = report::build(
        &change,
        &labels,
        Claims::read(&messages),
        report::Meta::new(source),
    );
    let styles = crate::norms::styles(&change, snapshot, cli.norm_threshold);
    for ((hunk, style), label) in report
        .hunks
        .iter_mut()
        .zip(styles)
        .zip(labels.iter().flatten())
    {
        hunk.style = style.map(|mut style| {
            style.restyled(&label.restyled());
            style
        });
    }
    if !cli.quiet {
        let mut stdout = std::io::stdout().lock();
        let written = report::render(&mut stdout, format, &report).and_then(|()| stdout.flush());
        // A closed pipe (`shapr … | head`) is the reader's choice, not an error.
        if let Err(error) = written.filter_broken_pipe() {
            eprintln!("shapr: writing output: {error}");
            return ExitCode::from(ERROR);
        }
    }
    ExitCode::SUCCESS
}

/// The range's commit messages, then the description: `--message`, else a
/// pull request's own.
fn messages(
    cli: &Cli,
    git: &git::Git,
    stdin: bool,
    remote: Option<&mut github::Remote>,
) -> anyhow::Result<Vec<Message>> {
    let mut messages = Vec::new();
    let mut description = None;
    if let Some(remote) = remote {
        messages.append(&mut remote.commits);
        description = remote.description.take();
    } else if let Some(range) = &cli.range {
        for (id, text) in git.messages(range)? {
            messages.push(Message {
                source: Source::Commit,
                commit: Some(id[..id.len().min(12)].to_string()),
                text,
            });
        }
    }
    if let Some(path) = &cli.message {
        let text = if stdin {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|e| anyhow::anyhow!("reading stdin: {e}"))?;
            text
        } else {
            // Relative to where shapr was started, like any file argument.
            std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("--message {}: {e}", path.display()))?
        };
        description = Some(Message {
            source: Source::Message,
            commit: None,
            text,
        });
    }
    messages.extend(description);
    Ok(messages)
}

/// `--status` and `--clear-cache`: the cache as it stands afterwards.
fn cache(format: Format, dir: Option<PathBuf>, clear: bool, quiet: bool) -> ExitCode {
    let cache = match github::Cache::resolve(dir) {
        Ok(cache) => cache,
        Err(message) => return fail(format, &message, USAGE),
    };
    if clear {
        match cache.clear() {
            Ok(n) => crate::verbose!(
                "removed {n} cached repositor{}",
                if n == 1 { "y" } else { "ies" }
            ),
            Err(error) => return fail(format, &format!("{error:#}"), ERROR),
        }
    }
    if quiet {
        return ExitCode::SUCCESS;
    }
    let status = cache.status();
    let rendered = match format {
        Format::Json => serde_json::to_string_pretty(&status).map_err(std::io::Error::from),
        Format::Ndjson => serde_json::to_string(&status).map_err(std::io::Error::from),
        Format::Text => {
            let mut text = format!(
                "{}: {} repositor{}, {}",
                status.path,
                status.repositories.len(),
                if status.repositories.len() == 1 {
                    "y"
                } else {
                    "ies"
                },
                human_bytes(status.bytes)
            );
            for repo in &status.repositories {
                text.push_str("\n  ");
                text.push_str(repo);
            }
            Ok(text)
        }
    };
    let mut stdout = std::io::stdout().lock();
    let written = rendered.and_then(|r| writeln!(stdout, "{r}"));
    if let Err(error) = written.filter_broken_pipe() {
        eprintln!("shapr: writing output: {error}");
        return ExitCode::from(ERROR);
    }
    ExitCode::SUCCESS
}

/// Two significant figures are all a directory size has.
fn human_bytes(bytes: u64) -> String {
    let mut value = bytes as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if value < 1000.0 || unit == "GB" {
            return match unit {
                "B" => format!("{bytes} B"),
                _ if value < 10.0 => format!("{value:.1} {unit}"),
                _ => format!("{value:.0} {unit}"),
            };
        }
        value /= 1000.0;
    }
    unreachable!()
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
pub(crate) fn fail(format: Format, message: &str, code: u8) -> ExitCode {
    eprintln!("shapr: {message}");
    emit_error(format, message, code);
    ExitCode::from(code)
}

pub(crate) fn emit_error(format: Format, message: &str, code: u8) {
    let error = serde_json::json!({
        "record": "error",
        "schema": report::SCHEMA,
        "error": message,
        "code": code,
    });
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
/// error reaches a JSON caller as JSON (and a `-q` caller as nothing). Every short flag but `-C` is a
/// switch, so a cluster like `-qJ` reads letter by letter up to a `C`, whose
/// value is the rest of the cluster or the next argument.
pub(crate) fn requested_format() -> Format {
    let mut format = Format::Text;
    let mut args = std::env::args().skip(1).take_while(|a| a != "--");
    while let Some(arg) = args.next() {
        let (json, ndjson) = match arg.strip_prefix("--") {
            Some("quiet") => return Format::Text,
            Some(long) => (long == "json", long == "ndjson"),
            None => match arg.strip_prefix('-') {
                Some(short) => {
                    let (switches, dir) = short.split_once('C').unwrap_or((short, "x"));
                    if dir.is_empty() {
                        args.next();
                    }
                    if switches.contains('q') {
                        return Format::Text;
                    }
                    (switches.contains('j'), switches.contains('J'))
                }
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
            let args: Vec<&str> = command.split_whitespace().collect();
            let parsed = match args.get(1) {
                Some(&"norms") => {
                    crate::norms::command::NormsCli::try_parse_from(&args[1..]).is_ok()
                }
                _ => Cli::try_parse_from(args).is_ok(),
            };
            assert!(parsed, "{command}");
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
        for code in ["0   success", "2   error", "64  usage"] {
            assert!(help.contains(code), "{code}");
        }
    }
}
