//! The git commands shapr runs: the diff for a range, and file contents by
//! blob id.

use crate::diff::FileDiff;
use anyhow::{Context, Result, bail};
use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};

/// Where git runs: the current directory, the one `-C` names, or a bare
/// repository with no worktree. Answers that don't change within a run are
/// asked once, and objects are read through one long-lived `cat-file --batch`.
pub struct Git {
    dir: Option<PathBuf>,
    bare: Option<Bare>,
    batch: RefCell<Option<Batch>>,
    toplevel: OnceCell<Option<PathBuf>>,
    parents: RefCell<HashMap<String, usize>>,
    promisor: OnceCell<Option<String>>,
    /// Linguist attributes by source revision and path.
    linguist: RefCell<HashMap<(Option<String>, String), Linguist>>,
}

/// The files a change ends in: a revision's, the index's (a staged diff on
/// stdin), or the working tree's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Revision(String),
    Index,
    Worktree,
}

/// A bare repository, and the commit whose `.gitattributes` apply.
struct Bare {
    git_dir: PathBuf,
    attributes_from: String,
}

impl Git {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Git {
            dir,
            bare: None,
            batch: RefCell::default(),
            toplevel: OnceCell::new(),
            parents: RefCell::default(),
            promisor: OnceCell::new(),
            linguist: RefCell::default(),
        }
    }

    /// Runs in the bare repository GIT_DIR, reading attributes from the
    /// commit ATTRIBUTES_FROM.
    pub fn bare(git_dir: PathBuf, attributes_from: String) -> Self {
        Git {
            bare: Some(Bare {
                git_dir,
                attributes_from,
            }),
            ..Git::new(None)
        }
    }

    fn command(&self) -> Command {
        let mut cmd = command();
        // Paths verbatim, whatever the user's quoting config.
        cmd.args(["-c", "core.quotePath=false"]);
        if let Some(bare) = &self.bare {
            // A partial clone fetches blobs on demand: fail, never prompt.
            cmd.arg("--git-dir")
                .arg(&bare.git_dir)
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .env("GIT_TERMINAL_PROMPT", "0");
        } else if let Some(dir) = &self.dir {
            // `-C` names the repository; an inherited GIT_DIR would override it.
            cmd.current_dir(dir)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE");
        }
        cmd
    }

    /// `git diff [--cached] RANGE -- PATHS`, in a fixed format: prefixes,
    /// rename detection and three lines of context whatever the user's
    /// config says.
    pub fn diff(&self, range: &str, cached: bool, paths: &[String]) -> Result<String> {
        self.verify_range(range)?;
        // Against all its parents a merge diffs as a combined diff, or nothing.
        if let Some(rev) = range.strip_suffix("^!")
            && self.parents(rev)? > 1
        {
            bail!("{}", merge(rev));
        }
        let out = run(self
            .command()
            .args([
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                "-M",
                "-U3",
            ])
            .args(cached.then_some("--cached"))
            .args([END_OF_OPTIONS, range, "--"])
            .args(paths))?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// What `git show REV` diffs, as a range: `REV^!`, or from the empty
    /// tree for a root commit. A merge is refused in `diff`.
    pub fn commit_range(&self, rev: &str) -> Result<String> {
        Ok(match self.parents(rev)? {
            0 => format!("{EMPTY_TREE}..{rev}"),
            _ => format!("{rev}^!"),
        })
    }

    /// The best common ancestor of A and B.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        let out = self
            .command()
            .args(["merge-base", END_OF_OPTIONS, a, b])
            .output()
            .context("could not run git")?;
        if !out.status.success() {
            self.outside_repository(&String::from_utf8_lossy(&out.stderr))?;
            if let Some(missing) = [a, b].into_iter().find(|r| !self.resolves(r)) {
                bail!("{missing}: no such revision");
            }
        }
        Ok(String::from_utf8_lossy(&check(out)?).trim().to_string())
    }

    /// ARGS, given with no `--`, as git reads them: the leading revisions,
    /// then paths. `Ok(Err(message))` when git won't tell them apart
    /// (a name that is both, a revision after a path): a usage error.
    pub fn split(&self, args: &[String]) -> Result<Result<Split, String>> {
        // `git log` disambiguates as `git diff` does; `rev-parse` doesn't.
        let out = self
            .command()
            .args(["log", "-n0", "--format=", END_OF_OPTIONS])
            .args(args)
            .output()
            .context("could not run git")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            self.outside_repository(&stderr)?;
            let first = stderr.lines().next().unwrap_or_default();
            let first = first.strip_prefix("fatal: ").unwrap_or(first);
            let first = first.trim_end_matches('.');
            let usage = "shapr [REV...] -- [PATH...]";
            // git names the argument it stopped at: `ambiguous argument 'NAME': ...`.
            let name = first.split('\'').nth(1).unwrap_or_default();
            if first.contains("both revision and filename")
                && let Some(at) = args.iter().position(|a| a == name)
            {
                let command = |split: usize| {
                    let (before, after) = args.split_at(split);
                    let words: Vec<&str> = std::iter::once("shapr")
                        .chain(before.iter().map(String::as_str))
                        .chain(["--"])
                        .chain(after.iter().map(String::as_str))
                        .collect();
                    words.join(" ")
                };
                return Ok(Err(format!(
                    "`{name}` is both a revision and a path: `{}` for the revision, `{}` for the path",
                    command(at + 1),
                    command(at)
                )));
            }
            if first.contains("no such path in the working tree") {
                return Ok(Err(format!(
                    "{first}. Revisions go before paths, and a path that doesn't exist after '--': {usage}"
                )));
            }
            if first.contains("unknown revision or path") {
                let what = if name.contains("..") {
                    "revision"
                } else {
                    "revision or path"
                };
                bail!("{name}: no such {what}");
            }
            bail!("git: {first}");
        }
        let revisions = args.iter().take_while(|a| self.resolves(a)).count();
        Ok(Ok((args[..revisions].to_vec(), args[revisions..].to_vec())))
    }

    /// Each commit's id and message in the commits `git diff RANGE` spans
    /// that touch PATHS (all when empty), newest first: `A..B` and `A...B`
    /// are the commits in B and not A, a root commit from the empty tree is
    /// itself, and one revision `R` (against the working tree) is `R..HEAD`.
    pub fn messages(&self, range: &str, paths: &[String]) -> Result<Vec<(String, String)>> {
        let root = range
            .strip_prefix(EMPTY_TREE)
            .and_then(|r| r.strip_prefix(".."));
        let commits = match (root, range.split_once("...")) {
            (Some(root), _) => root.to_string(),
            (None, Some((from, to))) => format!("{from}..{to}"),
            (None, None) => match self.parent_diff(range) {
                Some((rev, None)) => format!("{rev}..HEAD"),
                Some(_) => range.to_string(),
                None if range.contains("..") => range.to_string(),
                None => format!("{range}..HEAD"),
            },
        };
        let out = run(self
            .command()
            .args(["log", "--no-color", "--format=%H%x00%B%x1e"])
            .args([END_OF_OPTIONS, &commits, "--"])
            .args(paths))?;
        Ok(String::from_utf8_lossy(&out)
            .split('\x1e')
            .filter_map(|entry| entry.trim_start().split_once('\0'))
            .map(|(id, message)| (id.to_string(), message.trim_end().to_string()))
            .collect())
    }

    /// What `R^!` or `R^-N`, git's shorthand for R against a parent, diffs:
    /// from `R^N` to R, or from R to the working tree for a commit with no
    /// parent, as git does.
    fn parent_diff<'r>(&self, range: &'r str) -> Option<(String, Option<&'r str>)> {
        let (rev, n) = match range.strip_suffix("^!") {
            Some(rev) if self.parents(rev).ok()? == 0 => return Some((rev.to_string(), None)),
            // A merge is refused in `diff`.
            Some(rev) => (rev, ""),
            None => {
                let (rev, n) = range.rsplit_once("^-")?;
                n.bytes().all(|b| b.is_ascii_digit()).then_some((rev, n))?
            }
        };
        let n = if n.is_empty() { "1" } else { n };
        Some((format!("{rev}^{n}"), Some(rev)))
    }

    /// How many parents commit REV has.
    fn parents(&self, rev: &str) -> Result<usize> {
        if let Some(&n) = self.parents.borrow().get(rev) {
            return Ok(n);
        }
        let out =
            run(self
                .command()
                .args(["rev-list", "--parents", "-n1", END_OF_OPTIONS, rev, "--"]))?;
        let n = String::from_utf8_lossy(&out)
            .split_whitespace()
            .count()
            .saturating_sub(1);
        self.parents.borrow_mut().insert(rev.to_string(), n);
        Ok(n)
    }

    /// Fails in one line unless RANGE resolves as revisions in a repository.
    /// Outside a repository, or given a path that exists outside it (`..`),
    /// `git diff` drops into `--no-index` mode and prints its whole usage.
    fn verify_range(&self, range: &str) -> Result<()> {
        let out = self
            .command()
            .args(["rev-parse", "--quiet", END_OF_OPTIONS, range, "--"])
            .output()
            .context("could not run git")?;
        self.outside_repository(&String::from_utf8_lossy(&out.stderr))?;
        check(out).map(drop)
    }

    /// Fails in one line when git's STDERR says it ran outside a repository.
    fn outside_repository(&self, stderr: &str) -> Result<()> {
        if stderr.contains("not a git repository") {
            let dir = match &self.dir {
                Some(dir) => dir.clone(),
                None => std::env::current_dir().unwrap_or_default(),
            };
            bail!("not a git repository: {}", dir.display());
        }
        Ok(())
    }

    /// ARG names revisions in this repository (read-only; false outside one).
    pub fn resolves(&self, arg: &str) -> bool {
        self.command()
            .args(["rev-parse", "--quiet", END_OF_OPTIONS, arg, "--"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Each remote's name and URL. Empty outside a repository.
    pub fn remote_urls(&self) -> Vec<(String, String)> {
        let Ok(out) = run(self
            .command()
            .args(["config", "--get-regexp", r"^remote\..*\.url$"]))
        else {
            return Vec::new();
        };
        String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|line| {
                let (key, url) = line.split_once(' ')?;
                let name = key.strip_prefix("remote.")?.strip_suffix(".url")?;
                Some((name.to_string(), url.to_string()))
            })
            .collect()
    }

    pub fn toplevel(&self) -> Option<PathBuf> {
        self.toplevel
            .get_or_init(|| {
                let out = run(self.command().args(["rev-parse", "--show-toplevel"])).ok()?;
                Some(PathBuf::from(String::from_utf8_lossy(&out).trim()))
            })
            .clone()
    }

    /// The revision whose files the change ends in: B of `A..B` or `A...B`
    /// (HEAD when B is empty), a fetched GitHub change's head; `None` when
    /// it ends in the working tree (one revision, a parentless `R^!`, or a
    /// diff on stdin).
    pub fn after_revision(&self, range: Option<&str>) -> Option<String> {
        if let Some(bare) = &self.bare {
            return Some(bare.attributes_from.clone());
        }
        let range = range?;
        if let Some((_, after)) = self.parent_diff(range) {
            return after.map(str::to_string);
        }
        let (_, to) = range.split_once("...").or_else(|| range.split_once(".."))?;
        Some(if to.is_empty() { "HEAD" } else { to }.to_string())
    }

    /// Where a diff read from stdin may end, from its blob ids: the index
    /// when each file goes from HEAD to the index (`git diff --staged`), the
    /// working tree when each goes from the index or HEAD to the working
    /// tree (`git diff`, `git diff HEAD`); both when the ids fit both, as
    /// the same text may be either. Empty for anything else, such as a diff
    /// between two commits: the files it doesn't name can't be read as of
    /// its after side.
    pub fn stdin_after(&self, diffs: &[FileDiff]) -> Vec<State> {
        let Ok(index) = self.files(None) else {
            return Vec::new();
        };
        let index: HashMap<String, String> = index.into_iter().collect();
        let head: HashMap<String, String> = self
            .files(Some("HEAD"))
            .map(|f| f.into_iter().collect())
            .unwrap_or_default();
        let Some(worktree) = self.hash_worktree(diffs.iter().filter_map(|d| d.new_path.as_deref()))
        else {
            return Vec::new();
        };
        let all = |before: &[&HashMap<String, String>], after: &HashMap<String, String>| {
            diffs.iter().all(|d| {
                blob_is(&d.new_blob, d.new_path.as_deref(), after)
                    && before
                        .iter()
                        .any(|b| blob_is(&d.old_blob, d.old_path.as_deref(), b))
            })
        };
        let mut states = Vec::new();
        if all(&[&head], &index) {
            states.push(State::Index);
        }
        if all(&[&index, &head], &worktree) {
            states.push(State::Worktree);
        }
        states
    }

    /// Each existing working-tree file of PATHS (from the top) with the blob
    /// id git would give it.
    fn hash_worktree<'p>(
        &self,
        paths: impl Iterator<Item = &'p str>,
    ) -> Option<HashMap<String, String>> {
        let toplevel = self.toplevel()?;
        let paths: Vec<&str> = paths.filter(|p| toplevel.join(p).is_file()).collect();
        if paths.is_empty() {
            return Some(HashMap::new());
        }
        let mut input = paths.join("\n").into_bytes();
        input.push(b'\n');
        let mut cmd = self.command();
        cmd.current_dir(&toplevel)
            .args(["hash-object", "--stdin-paths"]);
        let out = run_with_input(&mut cmd, &input).ok()?;
        let ids = String::from_utf8_lossy(&out);
        Some(
            paths
                .iter()
                .zip(ids.lines())
                .map(|(p, id)| (p.to_string(), id.to_string()))
                .collect(),
        )
    }

    /// The revision a change starts from: A of `A..B` (HEAD when A is
    /// empty), the merge base of `A...B`, R of a single revision, R's parent
    /// of `R^!` or `R^-N`;
    /// `None` for a diff on stdin.
    pub fn before_revision(&self, range: Option<&str>) -> Option<String> {
        let range = range?;
        if let Some((before, _)) = self.parent_diff(range) {
            return Some(before);
        }
        if let Some((from, to)) = range.split_once("...") {
            let side = |r: &str| if r.is_empty() { "HEAD" } else { r }.to_string();
            let out =
                run(self
                    .command()
                    .args(["merge-base", END_OF_OPTIONS, &side(from), &side(to)]))
                .ok()?;
            return Some(String::from_utf8_lossy(&out).trim().to_string());
        }
        Some(match range.split_once("..") {
            Some(("", _)) => "HEAD".to_string(),
            Some((from, _)) => from.to_string(),
            None => range.to_string(),
        })
    }

    /// PATH's contents at REV, or in the index when REV is `""`; `None`
    /// when it has no such file.
    pub fn show(&self, rev: &str, path: &str) -> Option<Vec<u8>> {
        self.blob(&format!("{rev}:{path}"))
    }

    /// The blob NAME (an id, `rev:path` or `:path`) names; `None` when it
    /// names none. In a partial clone a missing blob is fetched, as
    /// `cat-file blob` would.
    pub fn blob(&self, name: &str) -> Option<Vec<u8>> {
        // `--batch` reads one name per line.
        if !name.contains('\n') {
            let mut batch = self.batch.borrow_mut();
            if batch.is_none() {
                *batch = Batch::spawn(self.command()).ok();
            }
            if let Some(reader) = batch.as_mut() {
                match reader.read(name) {
                    Ok(found) => return found,
                    // A batch that broke is dropped; the next read starts another.
                    Err(error) => {
                        crate::verbose!("git cat-file --batch: {error:#}");
                        *batch = None;
                    }
                }
            }
        }
        run(self
            .command()
            .args(["cat-file", "blob", END_OF_OPTIONS, name]))
        .ok()
    }

    /// The files directly in DIR at REV (`""` for the top), as paths.
    pub fn ls(&self, rev: &str, dir: &str) -> Vec<String> {
        let tree = format!("{rev}:{dir}");
        // Without --full-tree, ls-tree filters by the cwd's path from the top.
        let Ok(out) =
            run(self
                .command()
                .args(["ls-tree", "-z", "--full-tree", END_OF_OPTIONS, &tree]))
        else {
            return Vec::new();
        };
        String::from_utf8_lossy(&out)
            .split('\0')
            .filter_map(|entry| entry.split_once('\t'))
            .filter(|(meta, _)| meta.split(' ').nth(1) == Some("blob"))
            .map(|(_, name)| crate::norms::join(dir, name))
            .collect()
    }

    /// The files directly in DIR in the index (`""` for the top), as paths.
    pub fn ls_index(&self, dir: &str) -> Vec<String> {
        let spec = format!(":(top,glob){}", crate::norms::join(dir, "*"));
        let Ok(out) = run(self
            .command()
            .args(["ls-files", "-z", "--full-name", "--", &spec]))
        else {
            return Vec::new();
        };
        String::from_utf8_lossy(&out)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Every file of REV with its blob id, or of the working tree's index
    /// when `None`; paths from the top.
    pub fn files(&self, rev: Option<&str>) -> Result<Vec<(String, String)>> {
        let out = match rev {
            Some(rev) => run(self.command().args([
                "ls-tree",
                "-r",
                "-z",
                "--full-tree",
                END_OF_OPTIONS,
                rev,
            ]))?,
            None => run(self
                .command()
                .args(["ls-files", "-z", "-s", "--full-name", "--", ":/"]))?,
        };
        // `<mode> <type> <id>\t<path>` or `<mode> <id> <stage>\t<path>`.
        Ok(String::from_utf8_lossy(&out)
            .split('\0')
            .filter_map(|entry| entry.split_once('\t'))
            .filter_map(|(meta, path)| {
                let mut fields = meta.split(' ');
                let id = match rev {
                    Some(_) => fields.nth(1).filter(|&t| t == "blob").and(fields.next()),
                    None => fields.nth(1),
                };
                Some((path.to_string(), id?.to_string()))
            })
            .collect())
    }

    /// Fetches the blobs IDS a partial clone lacks, in one request: `git
    /// grep` would otherwise fetch each one as it reaches it.
    pub fn prefetch(&self, ids: &[&str]) -> Result<()> {
        let Some(remote) = self.promisor() else {
            return Ok(());
        };
        let mut input = ids.join("\n").into_bytes();
        input.push(b'\n');
        let mut check = self.command();
        check
            .env("GIT_NO_LAZY_FETCH", "1")
            .args(["cat-file", "--batch-check"]);
        let out = run_with_input(&mut check, &input)?;
        let missing: Vec<&str> = std::str::from_utf8(&out)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_suffix(" missing"))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        crate::verbose!("fetching {} blob(s) to search", missing.len());
        let mut input = missing.join("\n").into_bytes();
        input.push(b'\n');
        let mut fetch = self.command();
        fetch.args([
            "-c",
            "fetch.negotiationAlgorithm=noop",
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--recurse-submodules=no",
            "--filter=blob:none",
            "--stdin",
            &remote,
        ]);
        run_with_input(&mut fetch, &input).map(drop)
    }

    /// The object ids NAMES (`rev:path`) resolve to, from trees alone, so a
    /// partial clone needn't hold the blobs.
    pub fn object_ids(&self, names: &[&str]) -> Result<Vec<String>> {
        // `--revs-only` keeps rev-parse from echoing `--end-of-options` back.
        let out = run(self
            .command()
            .args(["rev-parse", "--revs-only", END_OF_OPTIONS])
            .args(names)
            .arg("--"))?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .map(str::to_string)
            .collect())
    }

    /// The contents of the blobs IDS, in order, in one request; `None` for
    /// one this repository lacks. Never fetches.
    pub fn blobs(&self, ids: &[&str]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut input = ids.join("\n").into_bytes();
        input.push(b'\n');
        let mut cmd = self.command();
        cmd.env("GIT_NO_LAZY_FETCH", "1")
            .args(["cat-file", "--batch"]);
        let out = run_with_input(&mut cmd, &input)?;
        // Each `<id> blob <size>\n<contents>\n`, or `<id> missing\n`.
        let mut rest = out.as_slice();
        let mut found = Vec::with_capacity(ids.len());
        for _ in ids {
            let end = rest
                .iter()
                .position(|&b| b == b'\n')
                .context("git cat-file: short output")?;
            let header = String::from_utf8_lossy(&rest[..end]).into_owned();
            rest = &rest[end + 1..];
            match header
                .rsplit_once(' ')
                .and_then(|(_, n)| n.parse::<usize>().ok())
            {
                Some(size) if !header.ends_with(" missing") && rest.len() > size => {
                    found.push(Some(rest[..size].to_vec()));
                    rest = &rest[size + 1..];
                }
                _ => found.push(None),
            }
        }
        Ok(found)
    }

    /// Whether this is a partial clone, which may lack blobs.
    pub fn partial(&self) -> bool {
        self.promisor().is_some()
    }

    /// The remote a partial clone fetches missing objects from.
    fn promisor(&self) -> Option<String> {
        self.promisor.get_or_init(|| self.read_promisor()).clone()
    }

    fn read_promisor(&self) -> Option<String> {
        let out = run(self.command().args([
            "config",
            "--get-regexp",
            r"^(extensions\.partialclone|remote\..*\.promisor)$",
        ]))
        .ok()?;
        let out = String::from_utf8_lossy(&out);
        out.lines().find_map(|line| {
            let (key, value) = line.split_once(' ')?;
            match key.strip_prefix("remote.") {
                Some(rest) => {
                    (value == "true").then(|| rest.strip_suffix(".promisor").map(str::to_string))?
                }
                None => Some(value.to_string()),
            }
        })
    }

    /// The working tree's untracked files git doesn't ignore, as paths from
    /// the top.
    pub fn untracked(&self) -> Result<Vec<String>> {
        let out = run(self.command().args([
            "ls-files",
            "-z",
            "--others",
            "--exclude-standard",
            "--full-name",
            "--",
            ":/",
        ]))?;
        Ok(String::from_utf8_lossy(&out)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// The untracked files git doesn't ignore under PATHS (the whole
    /// repository when empty), as `git diff` reads paths, and named relative
    /// to where git runs, ready for `git add`.
    pub fn untracked_in(&self, paths: &[String]) -> Result<Vec<String>> {
        let mut cmd = self.command();
        cmd.args(["ls-files", "-z", "--others", "--exclude-standard", "--"]);
        match paths {
            [] => cmd.arg(":/"),
            paths => cmd.args(paths),
        };
        let out = run(&mut cmd)?;
        Ok(String::from_utf8_lossy(&out)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Files of STATE matching PATHSPECS that hold one of WORDS as a whole
    /// word, as paths from the top; the working tree's untracked files
    /// included, binary files only when BINARY. Never fetches: a blob a
    /// partial clone lacks is an error.
    pub fn grep(
        &self,
        state: &State,
        words: &[&str],
        pathspecs: &[String],
        binary: bool,
    ) -> Result<Vec<String>> {
        let mut cmd = self.command();
        cmd.env("GIT_NO_LAZY_FETCH", "1")
            .args(["grep", "--no-color", "-l", "-w", "-F", "--full-name"])
            .arg(if binary { "--text" } else { "-I" });
        for word in words {
            cmd.args(["-e", word]);
        }
        let rev = match state {
            State::Revision(rev) => Some(rev.as_str()),
            State::Index => {
                cmd.arg("--cached");
                None
            }
            State::Worktree => {
                cmd.arg("--untracked");
                None
            }
        };
        cmd.arg(END_OF_OPTIONS).args(rev).arg("--").args(pathspecs);
        let out = cmd.output().context("could not run git")?;
        // Exit 1 with nothing on stderr: no match.
        if out.status.code() == Some(1) && out.stderr.is_empty() {
            return Ok(Vec::new());
        }
        let out = check(out)?;
        let prefix = rev.map(|r| format!("{r}:"));
        let mut found: Vec<String> = String::from_utf8_lossy(&out)
            .lines()
            .map(|line| match &prefix {
                Some(p) => line.strip_prefix(p.as_str()).unwrap_or(line),
                None => line,
            })
            .map(str::to_string)
            .collect();
        found.sort();
        Ok(found)
    }

    /// Each path's linguist attributes, where .gitattributes sets or unsets
    /// them: as of revision AT, else the working tree's (a bare repository's
    /// head commit's). Empty outside a repository.
    pub fn linguist(&self, paths: &[&str], at: Option<&str>) -> HashMap<String, Linguist> {
        let source = at.or(self.bare.as_ref().map(|b| b.attributes_from.as_str()));
        let key = |path: &str| (source.map(str::to_string), path.to_string());
        let asked: Vec<&str> = {
            let known = self.linguist.borrow();
            paths
                .iter()
                .copied()
                .filter(|p| !known.contains_key(&key(p)))
                .collect()
        };
        if !asked.is_empty()
            && let Some(found) = self.check_attr(&asked, source)
        {
            let mut known = self.linguist.borrow_mut();
            for path in &asked {
                let attributes = found.get(*path).copied().unwrap_or_default();
                known.insert(key(path), attributes);
            }
        }
        let known = self.linguist.borrow();
        paths
            .iter()
            .filter_map(|p| Some((p.to_string(), *known.get(&key(p))?)))
            .filter(|(_, l)| *l != Linguist::default())
            .collect()
    }

    /// `git check-attr` for PATHS as of SOURCE; `None` when it can't run.
    fn check_attr(
        &self,
        paths: &[&str],
        source: Option<&str>,
    ) -> Option<HashMap<String, Linguist>> {
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        let mut cmd = self.command();
        if self.bare.is_none() {
            let toplevel = self.toplevel()?;
            // check-attr reads paths against the cwd; the diff's are from the top.
            cmd.current_dir(toplevel);
        }
        cmd.arg("check-attr");
        if let Some(source) = source {
            cmd.arg(format!("--source={source}"));
        }
        cmd.args(["-z", "--stdin"]).args(LINGUIST);
        let out = match run_with_input(&mut cmd, &input) {
            Ok(out) => out,
            Err(error) => {
                // `check-attr --source` needs git 2.40.
                if source.is_some() {
                    crate::verbose!(
                        "can't read .gitattributes from a commit ({error:#}): categories by name only"
                    );
                }
                return None;
            }
        };
        let out = String::from_utf8_lossy(&out);
        let fields: Vec<&str> = out.split('\0').collect();
        let (triples, _) = fields.as_chunks::<3>();
        let mut found: HashMap<String, Linguist> = HashMap::new();
        for [path, attribute, value] in triples {
            let value = match *value {
                "set" | "true" => true,
                "unset" | "false" => false,
                _ => continue,
            };
            let entry = found.entry(path.to_string()).or_default();
            match *attribute {
                "linguist-generated" => entry.generated = Some(value),
                "linguist-vendored" => entry.vendored = Some(value),
                "linguist-documentation" => entry.documentation = Some(value),
                _ => {}
            }
        }
        Some(found)
    }
}

/// `git`, kept from running any program a repository's config or hooks
/// name: a checkout under review is untrusted. Every git shapr runs starts
/// here. System and global config still apply (credential helpers).
pub fn command() -> Command {
    let mut cmd = Command::new("git");
    cmd.args([
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=/dev/null",
    ]);
    cmd
}

/// Why a merge's combined diff isn't read, and what to run instead.
pub const MERGE: &str = "a merge: diff it against one parent, R^1..R or R^2..R";

/// Why merge commit REV isn't diffed against all its parents, and what to run instead.
pub fn merge(rev: &str) -> String {
    format!("{rev} is a merge: diff it against one parent, {rev}^1..{rev} or {rev}^2..{rev}")
}

/// Ends git's options before a revision that comes from the command line or
/// a repository, so `--output=FILE` can't be read as one.
const END_OF_OPTIONS: &str = "--end-of-options";

/// Arguments as revisions, then paths.
pub type Split = (Vec<String>, Vec<String>);

/// What a root commit is compared against.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

const LINGUIST: [&str; 3] = [
    "linguist-generated",
    "linguist-vendored",
    "linguist-documentation",
];

/// Whether ID, a possibly abbreviated blob id from a diff (all zeros for no
/// file), is PATH's blob in FILES.
fn blob_is(id: &Option<String>, path: Option<&str>, files: &HashMap<String, String>) -> bool {
    let Some(id) = id else {
        return false;
    };
    let found = path.and_then(|p| files.get(p));
    match found {
        None => id.bytes().all(|b| b == b'0'),
        Some(full) => !id.is_empty() && full.starts_with(id.as_str()),
    }
}

/// The linguist attributes .gitattributes sets (`Some(true)`) or unsets
/// (`Some(false)`) on a path; `None` where it says nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Linguist {
    pub generated: Option<bool>,
    pub vendored: Option<bool>,
    pub documentation: Option<bool>,
}

pub(crate) fn run(cmd: &mut Command) -> Result<Vec<u8>> {
    check(cmd.output().context("could not run git")?)
}

fn run_with_input(cmd: &mut Command, input: &[u8]) -> Result<Vec<u8>> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run git")?;
    let mut stdin = child.stdin.take().context("git stdin")?;
    // Write from another thread: git replies as it reads, and a reply that
    // fills its pipe would block it while we block writing the rest.
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input));
        let output = child.wait_with_output();
        let written = writer.join().expect("git stdin writer panicked");
        let output = output?;
        // A git that failed early closes its stdin: report its error, not the broken pipe.
        if output.status.success() {
            written?;
        }
        check(output)
    })
}

fn check(output: Output) -> Result<Vec<u8>> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git: {}", stderr.trim());
    }
    Ok(output.stdout)
}

/// A running `git cat-file --batch`: each name written gets its object back.
struct Batch {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Batch {
    fn spawn(mut cmd: Command) -> Result<Batch> {
        let mut child = cmd
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("could not run git")?;
        let stdin = child.stdin.take().context("git stdin")?;
        let stdout = BufReader::new(child.stdout.take().context("git stdout")?);
        Ok(Batch {
            child,
            stdin: Some(stdin),
            stdout,
        })
    }

    /// NAME's contents when it names a blob; `Ok(None)` when it names
    /// nothing, or something else.
    fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        let stdin = self.stdin.as_mut().context("git stdin closed")?;
        writeln!(stdin, "{name}")?;
        stdin.flush()?;
        // `<id> <type> <size>`, else `<name> missing` or `<name> ambiguous`.
        let mut header = String::new();
        if self.stdout.read_line(&mut header)? == 0 {
            bail!("git cat-file exited");
        }
        let fields: Vec<&str> = header.trim_end().rsplitn(3, ' ').collect();
        let [size, kind, _] = fields[..] else {
            return Ok(None);
        };
        let Ok(size) = size.parse::<usize>() else {
            return Ok(None);
        };
        let mut contents = vec![0; size + 1];
        self.stdout.read_exact(&mut contents)?;
        contents.pop();
        Ok((kind == "blob").then_some(contents))
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        // Closing its input ends it.
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

/// Reads file contents by blob id. Blobs `git diff` names for a working-tree
/// file aren't in the object store, so those are read from disk when the
/// file there still hashes to the id.
pub struct Blobs<'a> {
    git: &'a Git,
    toplevel: Option<PathBuf>,
}

impl<'a> Blobs<'a> {
    pub fn new(git: &'a Git) -> Self {
        Blobs {
            git,
            toplevel: None,
        }
    }

    /// `None` when the id names no blob we can find.
    pub fn read(&mut self, id: &str, path: &str) -> Option<Vec<u8>> {
        if id.bytes().all(|b| b == b'0') {
            return Some(Vec::new());
        }
        self.git.blob(id).or_else(|| self.read_worktree(id, path))
    }

    fn read_worktree(&mut self, id: &str, path: &str) -> Option<Vec<u8>> {
        if self.toplevel.is_none() {
            self.toplevel = self.git.toplevel();
        }
        let file = self.toplevel.as_ref()?.join(path);
        let hashed = run(self.git.command().arg("hash-object").arg(&file)).ok()?;
        String::from_utf8_lossy(&hashed)
            .trim()
            .starts_with(id)
            .then(|| std::fs::read(&file).ok())
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, Git) {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(dir.path())
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
        };
        git(&["init", "-q"]);
        std::fs::create_dir(dir.path().join("lib")).unwrap();
        std::fs::write(dir.path().join("lib/a.rb"), "x = 1\n").unwrap();
        std::fs::write(dir.path().join("gen.rb"), "y\n").unwrap();
        std::fs::write(
            dir.path().join(".gitattributes"),
            "gen.rb linguist-generated\n",
        )
        .unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "one"]);
        std::fs::write(dir.path().join("lib/a.rb"), "x = 2\n").unwrap();
        git(&["add", "-A"]);
        let shell = Git::new(Some(dir.path().to_path_buf()));
        (dir, shell)
    }

    #[test]
    fn blobs_are_read_by_id_revision_path_and_index_path() {
        let (_dir, git) = repo();
        assert_eq!(git.show("HEAD", "lib/a.rb").unwrap(), b"x = 1\n");
        assert_eq!(git.show("", "lib/a.rb").unwrap(), b"x = 2\n");
        let id = String::from_utf8(run(git.command().args(["rev-parse", "HEAD:gen.rb"])).unwrap())
            .unwrap();
        assert_eq!(git.blob(id.trim()).unwrap(), b"y\n");
        // A tree, a missing path and a name `--batch` can't take are no blob,
        // and the reader still answers after them.
        assert_eq!(git.show("HEAD", "lib"), None);
        assert_eq!(git.show("HEAD", "nope.rb"), None);
        assert_eq!(git.show("HEAD", "a\nb"), None);
        assert_eq!(git.show("HEAD", "gen.rb").unwrap(), b"y\n");
    }

    /// A revision is never an option, whatever reaches git.
    #[test]
    fn a_revision_cant_be_an_option() {
        let (dir, git) = repo();
        let file = dir.path().join("written");
        let option = format!("--output={}", file.display());
        assert!(git.diff(&option, false, &[]).is_err());
        assert!(git.messages(&format!("{option}..HEAD"), &[]).is_err());
        assert!(git.merge_base(&option, "HEAD").is_err());
        assert!(git.object_ids(&[&option]).is_err());
        assert!(!git.resolves(&option));
        assert!(git.files(Some(&option)).is_err());
        assert!(!file.exists());
    }

    /// Every git shapr runs is built by `command()`. Test code may spawn its own.
    #[test]
    fn every_git_starts_from_command() {
        // The norms lane moves its `ls-files` onto `command()`.
        let pending = ["src/norms/codebase.rs"];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut dirs = vec![root.join("src")];
        let mut offenders = Vec::new();
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                let code = text.split("#[cfg(test)]").next().unwrap();
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let allowed = usize::from(name == "src/git.rs");
                if code.matches("Command::new(\"git\")").count() > allowed
                    && !pending.contains(&name.as_str())
                {
                    offenders.push(name);
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "spawn git through git::command(): {offenders:?}"
        );
    }

    #[test]
    fn linguist_attributes_are_asked_once_per_revision_and_path() {
        let (_dir, git) = repo();
        let generated = Linguist {
            generated: Some(true),
            ..Linguist::default()
        };
        let first = git.linguist(&["gen.rb"], Some("HEAD"));
        assert_eq!(first, HashMap::from([("gen.rb".to_string(), generated)]));
        // A path asked again comes from what's known; a new one is asked.
        let both = git.linguist(&["gen.rb", "lib/a.rb"], Some("HEAD"));
        assert_eq!(both, first);
        assert_eq!(git.linguist.borrow().len(), 2);
    }
}
