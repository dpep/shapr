//! The command line: flags, input selection, exit codes.

use crate::claims::{Claims, Message, Source};
use crate::classify::proof::{Nowhere, Outside};
use crate::outside::{Repository, Union};
use crate::report::{self, Format};
use crate::{change, classify, diff, git, github};
use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const AFTER_HELP: &str = "\
EXAMPLES                          each is a git command: shapr X is git show/diff X
  shapr HEAD                      the last commit (git show HEAD)
  shapr main..HEAD                a range (git diff main..HEAD); shapr main HEAD is the same
  shapr main...                   a branch's commits, from where it left main
  shapr --merge-base main         everything on the branch, committed or not
  shapr .                         uncommitted changes (git diff HEAD -- .)
  shapr --staged                  staged changes (git diff --cached); --cached is the same
  shapr main..HEAD -- lib         any form, limited to paths
  git diff --staged | shapr       a diff on stdin (needs `git diff` index lines)
  shapr -J main..HEAD             JSON lines: a meta record, each file and hunk, then a summary
  shapr -C app main..HEAD         run in another repository, like git -C
  shapr --message pr.md main...   also read claims from a PR description
  shapr acme/widgets#12           a GitHub pull request: the diff its Files tab shows
  shapr acme/widgets@1a2b3c4      a GitHub commit, against its (first) parent
  shapr norms lib/cart.rb         a file's style norms, no diff needed (see shapr norms --help)

  Arguments are read as git reads them: revisions, then paths. A name that is
  both needs `--`: shapr lib -- (the revision), shapr -- lib (the path). A
  merge commit alone is refused: name one parent, REV^1..REV.

  With no arguments, shapr reads a diff from stdin; like any filter, it waits while
  an inherited stdin stays open. An empty stdin is no input (64), so a failed
  `git diff` upstream is an error, not an empty verdict.

  Ruby and Rust are parsed; hunks in other files are `logic`, confidence 0. Only
  code, test, config and manifest files count toward the verdict: a docs-only change
  is `none` (so is test data under fixtures/ or testdata/), and lockfiles, vendored and binary files and unparsed languages are
  `unknown`, counted in summary.verdicts.

GITHUB (experimental: the forms, meta.source and the cache may change before 1.0)
  The argument may name a pull request (https://github.com/OWNER/REPO/pull/N, OWNER/REPO#N,
  or #N for this repository's GitHub remote) or a commit (.../commit/SHA,
  .../pull/N/commits/SHA, OWNER/REPO@SHA). A shorthand that is also a local revision
  is the local revision. Metadata comes from the GitHub API, with a token from GH_TOKEN,
  GITHUB_TOKEN or `gh auth token` if there is one; objects are fetched with your git
  credentials into a cache of partial clones (--cache-dir), never into this repository.

USAGE LOG
  Each classify or norms run appends one JSON line to $XDG_STATE_HOME/shapr/usage.jsonl
  (else ~/.local/state/shapr/usage.jsonl): the command, input form, flag names, format,
  exit code, duration and counts. Never paths, repositories, revisions, code, messages
  or URLs. SHAPR_USAGE=0 (false, off, no) turns it off, 1 (true, on, yes) on, and an
  absolute path moves it; CI (CI=true, 1, yes or on; GITHUB_ACTIONS=true) turns it off
  unless SHAPR_USAGE is set. The log is created private (0600), never through a
  symlink. --usage
  summarizes it, --usage --reset empties it, --status shows where it is.

EXIT CODES
  0   success, whatever the verdict: the verdict is in the output
  2   error: git failed (not a repository, a bad revision, a name that is neither a
      revision nor a path, a merge commit alone), stdin is not a diff, or GitHub failed
      (unreachable, not found, rate limited)
  64  usage: the command line is wrong (a name both a revision and a path, more than
      two revisions), or there is no input (an empty stdin)

  Structure is a shape, not a safety claim: extracts and inlines still deserve review.";

#[derive(Parser)]
#[command(
    name = "shapr",
    version,
    about = "Classify the shape of a code change: format, comment, restyle, rename, move, extract, inline, or logic.",
    override_usage = "shapr [OPTIONS] [REV [REV]] [[--] PATH...]\n       shapr [OPTIONS] --merge-base REF [REV] [[--] PATH...]\n       shapr [OPTIONS] --staged [REV] [[--] PATH...]\n       shapr norms [OPTIONS] [PATH]...",
    after_help = AFTER_HELP
)]
struct Cli {
    /// What to classify, as git reads it: one revision is that commit
    /// (`git show REV`); a range or two revisions are `git diff A..B`;
    /// paths alone are their uncommitted changes (`git diff HEAD -- PATH`),
    /// and after revisions limit them. A name that is both a revision and a
    /// path needs `--`. Or a GitHub pull request or commit (see GITHUB
    /// below). With none, a unified diff is read from stdin.
    #[arg(value_name = "REV|PATH")]
    args: Vec<String>,

    /// Paths, after `--`.
    #[arg(last = true, value_name = "PATH")]
    paths: Vec<String>,

    /// Diff from the merge base of REF and HEAD to the working tree, like
    /// `git diff --merge-base REF`: everything on the branch, committed or
    /// not. With a REV, from the merge base of REF and REV to REV
    /// (`REF...REV`).
    #[arg(long, value_name = "REF")]
    merge_base: Option<String>,

    /// The staged changes: the index against HEAD, or against REV, like
    /// `git diff --cached [REV]`.
    #[arg(long, visible_alias = "cached", conflicts_with = "merge_base")]
    staged: bool,

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
    /// beside the commit messages (see summary.claims). `-` reads stdin,
    /// when the change isn't on stdin. Claims never change a hunk's label.
    #[arg(long, value_name = "FILE")]
    message: Option<PathBuf>,
    /// Experimental. Where GitHub repositories are cached; also SHAPR_CACHE_DIR. Defaults
    /// to $XDG_CACHE_HOME/shapr, else ~/.cache/shapr. Must be absolute.
    #[arg(long, value_name = "DIR")]
    cache_dir: Option<PathBuf>,

    /// Experimental. Show the GitHub cache: its path, the repositories in it,
    /// and their size; and the usage log's path and size.
    #[arg(long, conflicts_with_all = ["args", "paths", "merge_base", "staged", "clear_cache"])]
    status: bool,

    /// Summarize the usage log: runs by command, input form, flag, kind and
    /// verdict (see USAGE LOG below).
    #[arg(long, conflicts_with_all = ["args", "paths", "merge_base", "staged", "status", "clear_cache"])]
    usage: bool,

    /// With --usage, empty the usage log first.
    #[arg(long, requires = "usage")]
    reset: bool,

    /// Experimental. Remove every cached GitHub repository, then show the
    /// cache.
    #[arg(long, conflicts_with_all = ["args", "paths", "merge_base", "staged"])]
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
const NO_INPUT: &str = "no input: name a change (shapr HEAD, shapr main..HEAD, shapr .) or pipe a diff (git diff | shapr)";

/// A clap error's message as one line: its first paragraph, which may
/// continue onto indented lines (the missing arguments), without the tips.
pub(crate) fn usage_message(error: &clap::Error) -> String {
    let text = error.to_string();
    let message: Vec<&str> = text
        .lines()
        .take_while(|l| !l.trim().is_empty())
        .map(str::trim)
        .collect();
    let message = message.join(" ");
    message
        .strip_prefix("error: ")
        .unwrap_or(&message)
        .to_string()
}

pub fn run() -> ExitCode {
    let started = std::time::Instant::now();
    // A panic is reported as an error by `guarded`, in one line.
    std::panic::set_hook(Box::new(|info| {
        if let Ok(mut at) = PANICKED_AT.lock() {
            *at = info.location().map(|l| l.to_string());
        }
    }));
    let code = guarded(dispatch);
    crate::usage::finish(started, flag_names(std::env::args().skip(1)));
    code
}

static PANICKED_AT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// RUN's exit code; a panic is an error (2), with a JSON error record under
/// `-j`/`-J` like any other, so a gate reading the exit code fails closed.
fn guarded(run: impl FnOnce() -> ExitCode + std::panic::UnwindSafe) -> ExitCode {
    let payload = match std::panic::catch_unwind(run) {
        Ok(code) => return code,
        Err(payload) => payload,
    };
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic".to_string());
    let at = PANICKED_AT.lock().ok().and_then(|mut at| at.take());
    fail(
        requested_format(),
        &internal_error(&message, at.as_deref()),
        ERROR,
    )
}

fn internal_error(message: &dyn std::fmt::Display, at: Option<&str>) -> String {
    let at = at.map(|at| format!(" at {at}")).unwrap_or_default();
    format!("internal error: {message}{at}, please report it")
}

/// The canonical names of the flags in ARGS (`json` for `-j`), never their
/// values, as either command knows them; sorted, once each.
fn flag_names(args: impl Iterator<Item = String>) -> Vec<String> {
    let commands = [Cli::command(), crate::norms::command::NormsCli::command()];
    let known: Vec<&clap::Arg> = commands.iter().flat_map(|c| c.get_arguments()).collect();
    let name = |arg: &clap::Arg| arg.get_long().unwrap_or(arg.get_id().as_str()).to_string();
    let mut names = std::collections::BTreeSet::new();
    for word in args.take_while(|a| a != "--") {
        if let Some(long) = word.strip_prefix("--") {
            let long = long.split('=').next().unwrap_or_default();
            names.extend(
                known
                    .iter()
                    .find(|a| a.get_long() == Some(long))
                    .map(|a| name(a)),
            );
        } else if let Some(shorts) = word.strip_prefix('-') {
            for short in shorts.chars() {
                let Some(arg) = known.iter().find(|a| a.get_short() == Some(short)) else {
                    break;
                };
                names.insert(name(arg));
                // The rest of the cluster is this flag's value (`-Capp`).
                if arg.get_action().takes_values() {
                    break;
                }
            }
        }
    }
    names.into_iter().collect()
}

fn dispatch() -> ExitCode {
    // A subcommand beside free revisions and paths: `norms` is never read as
    // either, and the global flags (format, -C) may come before it.
    let mut args: Vec<String> = std::env::args().collect();
    let mut at = 1;
    while let Some(arg) = args.get(at) {
        let switches = |s: &str| s.chars().all(|c| "jJvq".contains(c));
        match arg.as_str() {
            "--json" | "--ndjson" | "--verbose" | "--quiet" => at += 1,
            // A cluster of switches, perhaps ending in -C and its value.
            short if short.starts_with('-') && !short.starts_with("--") => {
                match short[1..].split_once('C') {
                    Some((before, "")) if switches(before) => at += 2,
                    Some((before, _)) if switches(before) => at += 1,
                    None if short.len() > 1 && switches(&short[1..]) => at += 1,
                    _ => break,
                }
            }
            _ => break,
        }
    }
    if args.get(at).is_some_and(|a| a == "norms") {
        args.remove(at);
        args[0] = "shapr norms".to_string();
        return crate::norms::command::run(args);
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            report::clap(&error);
            if !error.use_stderr() {
                return ExitCode::SUCCESS;
            }
            crate::usage::command("classify");
            crate::usage::format(requested_format());
            emit_error(requested_format(), &usage_message(&error), USAGE);
            return exit(USAGE);
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
        let _ = report::to_stdout(|out| {
            clap_complete::generate(shell, &mut Cli::command(), "shapr", out);
            Ok(())
        });
        return ExitCode::SUCCESS;
    }

    if let Some(dir) = cli.dir.as_ref().filter(|d| !d.is_dir()) {
        return fail(
            format,
            &format!("-C {}: not a directory", dir.display()),
            USAGE,
        );
    }
    if cli.status || cli.clear_cache {
        return cache(format, cli.cache_dir, cli.clear_cache, cli.quiet);
    }
    if cli.usage {
        return usage(format, cli.reset, cli.quiet);
    }
    crate::usage::command("classify");
    // The format asked for, though -q renders none.
    crate::usage::format(match (cli.ndjson, cli.json) {
        (true, _) => Format::Ndjson,
        (_, true) => Format::Json,
        _ => Format::Text,
    });
    if let Err(error) = git::check_version() {
        return fail(format, &format!("{error:#}"), ERROR);
    }
    let mut git = git::Git::new(cli.dir.clone());
    let dashdash = std::env::args().skip(1).any(|a| a == "--");
    let input = match input(&cli, dashdash, &git) {
        Ok(input) => input,
        Err((message, code)) => return fail(format, &message, code),
    };
    let stdin_message = cli.message.as_deref() == Some(std::path::Path::new("-"));
    if stdin_message && matches!(input, Input::Stdin) {
        return fail(
            format,
            "--message - needs a change named on the command line: stdin carries the diff without one",
            USAGE,
        );
    }
    let mut remote = None;
    let (form, local): (_, Option<(&str, &[String])>) = match &input {
        Input::Stdin => (report::Input::Stdin, None),
        Input::Local { form, range, paths } => (*form, Some((range.as_str(), paths.as_slice()))),
        Input::GitHub(target) => {
            let form = match target {
                github::Target::Pull { .. } => report::Input::GithubPr,
                github::Target::Commit { .. } => report::Input::GithubCommit,
            };
            (form, None)
        }
    };
    let paths = local.is_some_and(|(_, paths)| !paths.is_empty());
    crate::usage::input(form, paths);
    let text = match &input {
        Input::Local { form, range, paths } => {
            git.diff(range, *form == report::Input::Staged, paths)
        }
        Input::GitHub(target) => {
            let cache = match github::Cache::resolve(cli.cache_dir.clone()) {
                Ok(cache) => cache,
                Err(message) => return fail(format, &message, USAGE),
            };
            match github::fetch(target, &cache) {
                Err(error) => return fail(format, &format!("{error:#}"), ERROR),
                Ok(fetched) => {
                    let text = fetched.git.diff(&fetched.range, false, &[]);
                    remote = Some(fetched);
                    text
                }
            }
        }
        Input::Stdin if std::io::stdin().is_terminal() => return fail(format, NO_INPUT, USAGE),
        Input::Stdin => {
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
    let stdin = matches!(input, Input::Stdin);
    if stdin && files.is_empty() {
        // An empty pipe is usually an upstream failure (`git diff nope | shapr`).
        if text.trim().is_empty() {
            return fail(format, NO_INPUT, USAGE);
        }
        return fail(format, "stdin is not a diff: no file headers", ERROR);
    }
    let source = remote.as_ref().map(|r| r.source.clone());
    let messages = match messages(&cli, &git, local, stdin_message, remote.as_mut()) {
        Ok(messages) => messages,
        Err(error) => return fail(format, &format!("{error:#}"), ERROR),
    };
    // The range git diffed: a GitHub change's own, in its clone.
    let range = remote
        .as_ref()
        .map(|r| r.range.clone())
        .or(local.map(|(range, _)| range.to_string()));
    let mut claims = Claims::read(&messages);
    if let Some(remote) = remote {
        claims.resolve(&remote.reference_kinds(&claims.references()));
        git = remote.git;
    }
    // Where uses outside the change are searched: a diff on stdin names
    // its own after side only by blob ids.
    let states = match (stdin, git.after_revision(range.as_deref())) {
        (_, Some(rev)) => vec![git::State::Revision(rev)],
        (false, None) if form == report::Input::Staged => vec![git::State::Index],
        (false, None) => vec![git::State::Worktree],
        (true, None) => git.stdin_after(&files),
    };
    if stdin && states.is_empty() {
        crate::verbose!(
            "the diff doesn't end in the index or the working tree: uses outside it aren't searched"
        );
    }
    // `git diff` never shows untracked files: say so rather than look clean.
    let untracked = match (local, states.as_slice()) {
        (Some((_, paths)), [git::State::Worktree]) => git.untracked_in(paths).ok(),
        _ => None,
    };
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
    let mut meta = report::Meta::new("classify", source).input(form, paths);
    meta.subject = subject(&input, cli.merge_base.as_deref());
    meta.untracked = untracked.as_ref().map(Vec::len);
    if let Some(note) = untracked
        .as_deref()
        .and_then(|files| untracked_note(files, cli.dir.as_deref()))
    {
        report::note(&note);
    }
    let mut report = report::build(&change, &labels, claims, meta);
    let styles = crate::norms::styles(&change, snapshot, outside, cli.norm_threshold);
    report.summary.config_trajectory = crate::norms::trajectory(&change, snapshot);
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
    crate::usage::counts(&report.summary, &["files", "hunks", "kinds", "verdict"]);
    if !cli.quiet {
        let written = report::to_stdout(|out| report::render(out, format, &report));
        // A closed pipe (`shapr … | head`) is the reader's choice, not an error.
        if let Err(error) = written.filter_broken_pipe() {
            report::note(&format!("writing output: {error}"));
            return exit(ERROR);
        }
    }
    ExitCode::SUCCESS
}

/// What the command line names.
enum Input {
    Stdin,
    /// What `git diff RANGE -- PATHS` shows; a lone revision in RANGE
    /// is against the working tree, as `git diff REV` is.
    Local {
        form: report::Input,
        range: String,
        paths: Vec<String>,
    },
    GitHub(github::Target),
}

/// The change the arguments name, read as git reads them; an error's
/// message and exit code.
fn input(cli: &Cli, dashdash: bool, git: &git::Git) -> Result<Input, (String, u8)> {
    let usage = |message: String| (message, USAGE);
    let error = |error: anyhow::Error| (format!("{error:#}"), ERROR);
    // git would read it as an option (`--output=FILE`); no revision starts so.
    if let Some(rev) = cli
        .merge_base
        .iter()
        .chain(&cli.args)
        .find(|a| a.starts_with('-'))
    {
        return Err(usage(format!(
            "`{rev}` starts with `-`: a revision never does (paths go after `--`)"
        )));
    }
    if let [arg] = cli.args.as_slice()
        && !dashdash
        && cli.merge_base.is_none()
        && !cli.staged
        && !cli
            .dir
            .as_deref()
            .unwrap_or(Path::new("."))
            .join(arg)
            .exists()
    {
        if github_file(arg) {
            return Err(usage(
                "a GitHub file isn't a change: give a pull request or commit, \
                 or run shapr norms on a local checkout"
                    .to_string(),
            ));
        }
        if let Some(target) = github::resolve(arg, git).map_err(usage)? {
            return Ok(Input::GitHub(target));
        }
    }
    let (revisions, mut paths) = match dashdash || cli.args.is_empty() {
        true => (cli.args.clone(), Vec::new()),
        false => git.split(&cli.args).map_err(error)?.map_err(usage)?,
    };
    paths.extend(cli.paths.iter().cloned());
    let shape = match cli.staged {
        true => staged(&revisions).map_err(usage)?,
        false => shape(&revisions, cli.merge_base.as_deref(), !paths.is_empty()).map_err(usage)?,
    };
    let (form, range) = match shape {
        None => return Ok(Input::Stdin),
        Some(Shape::Commit(rev)) => {
            let Some(name) = git.commit_name(&rev) else {
                return Err(usage(format!(
                    "`{rev}` doesn't name one commit: shapr REV takes a commit, not a tree, a file or several commits"
                )));
            };
            (
                report::Input::Commit,
                git.commit_range(&name).map_err(error)?,
            )
        }
        Some(Shape::Range(range)) => (report::Input::Range, range),
        Some(Shape::MergeBaseRange(range)) => (report::Input::MergeBaseRange, range),
        Some(Shape::MergeBaseWorktree(base)) => (
            report::Input::MergeBaseWorktree,
            git.merge_base(&base, "HEAD").map_err(error)?,
        ),
        Some(Shape::Worktree) => (report::Input::Worktree, "HEAD".to_string()),
        Some(Shape::Staged(rev)) => (report::Input::Staged, rev),
    };
    Ok(Input::Local { form, range, paths })
}

/// The untracked FILES a working-tree diff left out, and the command that
/// adds them to it; `None` when there are none. DIR is `-C`'s.
fn untracked_note(files: &[String], dir: Option<&Path>) -> Option<String> {
    let git = match dir {
        Some(dir) => format!("git -C {} add -N", shell_word(&dir.to_string_lossy())),
        None => "git add -N".to_string(),
    };
    let named: Vec<std::borrow::Cow<str>> = files
        .iter()
        .take(3)
        .map(|f| report::quote_path(f))
        .collect();
    let words: Vec<String> = files.iter().take(3).map(|f| shell_word(f)).collect();
    Some(match files.len() {
        0 => return None,
        1 => format!(
            "1 untracked file not shown: {} ({git} {} to include it)",
            named[0], words[0]
        ),
        n @ 2..=3 => format!(
            "{n} untracked files not shown: {} ({git} {} to include them)",
            named.join(", "),
            words.join(" ")
        ),
        n => format!(
            "{n} untracked files not shown: {}, ... ({git} <path> to include them)",
            named.join(", ")
        ),
    })
}

/// WORD as one shell word: quoted when it holds anything but plain path
/// characters.
fn shell_word(word: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_-./@+:,=".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// What INPUT compares, in the words of the text summary, when shapr chose it
/// (the working tree, the index, a merge base, stdin); `None` when the caller
/// spelled it out as revisions, or for a GitHub change, which names its source.
fn subject(input: &Input, base: Option<&str>) -> Option<String> {
    let Input::Local { form, range, paths } = input else {
        return matches!(input, Input::Stdin).then(|| "stdin".to_string());
    };
    let compared = match form {
        // The caller typed these; echoing them back says nothing.
        report::Input::Commit | report::Input::Range | report::Input::MergeBaseRange => {
            return None;
        }
        report::Input::MergeBaseWorktree => format!("{}...working tree", base.unwrap_or(range)),
        report::Input::Worktree => "uncommitted changes".to_string(),
        report::Input::Staged if range == "HEAD" => "staged changes".to_string(),
        report::Input::Staged => format!("staged changes against {range}"),
        report::Input::Stdin | report::Input::GithubPr | report::Input::GithubCommit => {
            return None;
        }
    };
    Some(match paths.as_slice() {
        [] => compared,
        paths => format!("{compared} in {}", paths.join(" ")),
    })
}

/// The form revisions take, before git is asked anything.
#[derive(Debug, PartialEq)]
enum Shape {
    /// `git show REV`.
    Commit(String),
    /// What `git diff` takes: `A..B`, `A...B`, `R^!`.
    Range(String),
    MergeBaseRange(String),
    /// The merge base of REF and HEAD, against the working tree.
    MergeBaseWorktree(String),
    /// HEAD against the working tree.
    Worktree,
    /// REV against the index: `git diff --cached REV`.
    Staged(String),
}

/// The shape of REVISIONS under `--staged`: one revision at most, as
/// `git diff --cached` takes.
fn staged(revisions: &[String]) -> Result<Option<Shape>, String> {
    match revisions {
        [] => Ok(Some(Shape::Staged("HEAD".to_string()))),
        [rev] if !is_range(rev) => Ok(Some(Shape::Staged(rev.clone()))),
        _ => Err("--staged takes one revision at most, not a range: \
             shapr --staged [REV] [-- PATH...]"
            .to_string()),
    }
}

/// The shape of REVISIONS under `--merge-base BASE`, with or without PATHS;
/// `None` for a diff on stdin, a usage message for a shape git has no
/// command for.
fn shape(revisions: &[String], base: Option<&str>, paths: bool) -> Result<Option<Shape>, String> {
    if let Some(base) = base {
        if revisions.len() > 1 {
            return Err("--merge-base takes one revision after REF at most: \
                 shapr --merge-base REF [REV] [-- PATH...]"
                .to_string());
        }
        if let Some(range) = std::iter::once(base)
            .chain(revisions.iter().map(String::as_str))
            .find(|r| is_range(r))
        {
            return Err(format!(
                "--merge-base takes revisions, not a range like `{range}`: \
                 shapr --merge-base main feature, or shapr main...feature"
            ));
        }
        return Ok(Some(match revisions {
            [rev] => Shape::MergeBaseRange(format!("{base}...{rev}")),
            _ => Shape::MergeBaseWorktree(base.to_string()),
        }));
    }
    Ok(match revisions {
        [] if paths => Some(Shape::Worktree),
        [] => None,
        [rev] if is_range(rev) => Some(Shape::Range(rev.clone())),
        [rev] => Some(Shape::Commit(rev.clone())),
        [a, b] if is_range(a) || is_range(b) => {
            return Err(format!(
                "two revisions, not ranges: shapr A B, or shapr A..B (got `{a}` and `{b}`)"
            ));
        }
        [a, b] => Some(Shape::Range(format!("{a}..{b}"))),
        _ => {
            return Err("at most two revisions, as git diff takes them; \
                 paths go after them, or after `--`: shapr A B -- PATH..."
                .to_string());
        }
    })
}

/// REV is git's notation for two endpoints: `A..B`, `A...B`, `R^!`, `R^-N`.
fn is_range(rev: &str) -> bool {
    rev.contains("..")
        || rev.ends_with("^!")
        || rev
            .rsplit_once("^-")
            .is_some_and(|(_, n)| n.bytes().all(|b| b.is_ascii_digit()))
}

/// The range's commit messages, then the description: `--message`, else a
/// pull request's own.
fn messages(
    cli: &Cli,
    git: &git::Git,
    local: Option<(&str, &[String])>,
    stdin: bool,
    remote: Option<&mut github::Remote>,
) -> anyhow::Result<Vec<Message>> {
    let mut messages = Vec::new();
    let mut description = None;
    if let Some(remote) = remote {
        messages.append(&mut remote.commits);
        description = remote.description.take();
    } else if let Some((range, paths)) = local {
        for (id, text) in git.messages(range, paths)? {
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

/// A GitHub page for a file or directory (`/blob/`, `/tree/`), which names
/// no change.
fn github_file(arg: &str) -> bool {
    let lower = arg.to_ascii_lowercase();
    let Some((_, path)) = lower.split_once("github.com/") else {
        return false;
    };
    matches!(path.split('/').nth(2), Some("blob" | "tree"))
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
    let log = crate::usage::summary();
    let mut record = serde_json::to_value(&status).unwrap_or_default();
    record["usage"] = serde_json::json!({
        "path": log.path,
        "bytes": log.bytes,
        "enabled": log.enabled,
    });
    let rendered = match format {
        Format::Json => serde_json::to_string_pretty(&record).map_err(std::io::Error::from),
        Format::Ndjson => serde_json::to_string(&record).map_err(std::io::Error::from),
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
            text.push_str(&format!("\nusage log: {}", usage_line(&log)));
            Ok(text)
        }
    };
    let written = rendered.and_then(|r| report::to_stdout(|out| print(out, format, &r)));
    if let Err(error) = written.filter_broken_pipe() {
        report::note(&format!("writing output: {error}"));
        return exit(ERROR);
    }
    ExitCode::SUCCESS
}

/// The usage log's path and size, and whether it is off.
fn usage_line(log: &crate::usage::Summary) -> String {
    let path = log.path.as_deref().unwrap_or("no home directory");
    let off = if log.enabled { "" } else { " (off)" };
    format!("{path}, {}{off}", human_bytes(log.bytes))
}

/// `--usage`, after `--reset` empties the log: what it holds, counted.
fn usage(format: Format, reset: bool, quiet: bool) -> ExitCode {
    if reset && let Err(error) = crate::usage::reset() {
        return fail(format, &format!("emptying the usage log: {error}"), ERROR);
    }
    if quiet {
        return ExitCode::SUCCESS;
    }
    let log = crate::usage::summary();
    let rendered = match format {
        Format::Json => serde_json::to_string_pretty(&log).map_err(std::io::Error::from),
        Format::Ndjson => serde_json::to_string(&log).map_err(std::io::Error::from),
        Format::Text if log.runs == 0 => Ok(match &log.path {
            Some(path) if log.enabled => format!("no runs logged yet at {path}"),
            Some(path) => format!("no runs logged yet at {path} (the log is off)"),
            None => "no runs logged: no home directory for the log".to_string(),
        }),
        Format::Text => {
            let span = match (&log.first, &log.last) {
                (Some(first), Some(last)) => format!(", {first} to {last}"),
                _ => String::new(),
            };
            let mut text = format!("{}: {} runs{span}", usage_line(&log), log.runs);
            let row = |label: &str, counts: &std::collections::BTreeMap<String, u64>| {
                let mut counts: Vec<(&String, &u64)> =
                    counts.iter().filter(|(_, n)| **n > 0).collect();
                // Most first; ties by name.
                counts.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                let list: Vec<String> = counts.iter().map(|(k, n)| format!("{k} {n}")).collect();
                format!("\n  {label:<9} {}", list.join(", "))
            };
            text.push_str(&row("commands", &log.commands));
            text.push_str(&row("inputs", &log.inputs));
            text.push_str(&row("flags", &log.flags));
            text.push_str(&row("kinds", &log.kinds));
            text.push_str(&row("verdicts", &log.verdicts));
            Ok(text)
        }
    };
    let written = rendered.and_then(|r| report::to_stdout(|out| print(out, format, &r)));
    if let Err(error) = written.filter_broken_pipe() {
        report::note(&format!("writing output: {error}"));
        return exit(ERROR);
    }
    ExitCode::SUCCESS
}

/// RENDERED to OUT: text escaped line by line, JSON as serde wrote it.
fn print(out: &mut impl Write, format: Format, rendered: &str) -> std::io::Result<()> {
    match format {
        Format::Text => report::emit_text(out, rendered),
        Format::Json | Format::Ndjson => writeln!(out, "{rendered}"),
    }
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
    report::note(message);
    emit_error(format, message, code);
    exit(code)
}

/// CODE as the process's exit code, recorded for the usage log.
pub(crate) fn exit(code: u8) -> ExitCode {
    crate::usage::exit(code);
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
        let _ = report::to_stdout(|out| print(out, format, &rendered));
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

    /// The example lines of --help and of the README's Usage block.
    fn examples() -> Vec<String> {
        let help = AFTER_HELP
            .lines()
            .map(str::trim_start)
            .filter_map(|l| l.split("  ").next());
        let readme = include_str!("../README.md");
        let usage = readme
            .split("## Usage")
            .nth(1)
            .and_then(|u| u.split("```sh\n").nth(1))
            .and_then(|u| u.split("```").next())
            .expect("README has a Usage block");
        let readme = usage.lines().filter_map(|l| l.split(" # ").next());
        help.chain(readme)
            .filter(|l| l.starts_with("shapr ") || l.contains("| shapr"))
            .filter_map(|l| l.find("shapr").map(|at| l[at..].trim().to_string()))
            .collect()
    }

    #[test]
    fn help_examples() {
        let examples = examples();
        assert!(examples.len() >= 20, "{examples:?}");
        for example in examples {
            let args: Vec<&str> = example.split_whitespace().collect();
            if args.get(1) == Some(&"norms") {
                let parsed = crate::norms::command::NormsCli::try_parse_from(&args[1..]);
                assert!(parsed.is_ok(), "{example}");
                continue;
            }
            let cli = Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{example}: {e}"));
            // What git would decide, from the examples' own names: a path has
            // a slash or is `.`, unless it is a GitHub form.
            let github = |a: &str| a.contains('#') || a.contains('@') || a.contains("://");
            if cli.args.iter().any(|a| github(a)) {
                continue;
            }
            let path = |a: &&String| *a == "." || a.contains('/');
            let revisions: Vec<String> = cli.args.iter().filter(|a| !path(a)).cloned().collect();
            let paths = cli.args.iter().any(|a| path(&a)) || !cli.paths.is_empty();
            let shape = shape(&revisions, cli.merge_base.as_deref(), paths);
            assert!(shape.is_ok(), "{example}: {shape:?}");
        }
    }

    #[test]
    fn shapes_follow_git() {
        let s = |revisions: &[&str], base: Option<&str>, paths: bool| {
            let revisions: Vec<String> = revisions.iter().map(|r| r.to_string()).collect();
            shape(&revisions, base, paths)
        };
        let range = |r: &str| Ok(Some(Shape::Range(r.to_string())));
        assert_eq!(s(&[], None, false), Ok(None));
        assert_eq!(s(&[], None, true), Ok(Some(Shape::Worktree)));
        assert_eq!(
            s(&["main"], None, false),
            Ok(Some(Shape::Commit("main".into())))
        );
        for r in ["a..b", "a...", "HEAD^!", "HEAD^-2", "HEAD^-"] {
            assert_eq!(s(&[r], None, false), range(r));
        }
        assert_eq!(s(&["a", "b"], None, true), range("a..b"));
        assert!(s(&["a..b", "c"], None, false).is_err());
        assert!(s(&["a", "b", "c"], None, false).is_err());
        assert_eq!(
            s(&[], Some("main"), false),
            Ok(Some(Shape::MergeBaseWorktree("main".into())))
        );
        assert_eq!(
            s(&["f"], Some("main"), false),
            Ok(Some(Shape::MergeBaseRange("main...f".into())))
        );
        assert!(s(&["f", "g"], Some("main"), false).is_err());
        assert!(s(&[], Some("a..b"), false).is_err());
    }

    #[test]
    fn a_panic_is_an_error_not_a_crash() {
        assert_eq!(guarded(|| ExitCode::SUCCESS), ExitCode::SUCCESS);
        let code = guarded(|| panic!("boom"));
        assert_eq!(code, ExitCode::from(ERROR));
        assert_eq!(
            internal_error(&"boom", Some("src/x.rs:1:2")),
            "internal error: boom at src/x.rs:1:2, please report it"
        );
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
