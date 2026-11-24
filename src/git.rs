//! The git commands shapr runs: the diff for a range, and file contents by
//! blob id.

use crate::diff::FileDiff;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// Where git runs: the current directory, the one `-C` names, or a bare
/// repository with no worktree.
pub struct Git {
    dir: Option<PathBuf>,
    bare: Option<Bare>,
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
        Git { dir, bare: None }
    }

    /// Runs in the bare repository GIT_DIR, reading attributes from the
    /// commit ATTRIBUTES_FROM.
    pub fn bare(git_dir: PathBuf, attributes_from: String) -> Self {
        Git {
            dir: None,
            bare: Some(Bare {
                git_dir,
                attributes_from,
            }),
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new("git");
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

    /// `git diff RANGE`, in a fixed format: prefixes, rename detection and
    /// three lines of context whatever the user's config says.
    pub fn diff(&self, range: &str) -> Result<String> {
        self.verify_range(range)?;
        let out = run(self.command().args([
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "-M",
            "-U3",
            range,
            "--",
        ]))?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Each commit's id and message in the commits `git diff RANGE` spans,
    /// newest first: `A..B` and `A...B` are the commits in B and not A, and
    /// one revision `R` (against the working tree) is `R..HEAD`.
    pub fn messages(&self, range: &str) -> Result<Vec<(String, String)>> {
        let commits = match range.split_once("...") {
            Some((from, to)) => format!("{from}..{to}"),
            None if range.contains("..") || parent_range(range).is_some() => range.to_string(),
            None => format!("{range}..HEAD"),
        };
        let out = run(self.command().args([
            "log",
            "--no-color",
            "--format=%H%x00%B%x1e",
            &commits,
            "--",
        ]))?;
        Ok(String::from_utf8_lossy(&out)
            .split('\x1e')
            .filter_map(|entry| entry.trim_start().split_once('\0'))
            .map(|(id, message)| (id.to_string(), message.trim_end().to_string()))
            .collect())
    }

    /// Fails in one line unless RANGE resolves as revisions in a repository.
    /// Outside a repository, or given a path that exists outside it (`..`),
    /// `git diff` drops into `--no-index` mode and prints its whole usage.
    fn verify_range(&self, range: &str) -> Result<()> {
        let out = self
            .command()
            .args(["rev-parse", "--quiet", range, "--"])
            .output()
            .context("could not run git")?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("not a git repository") {
            let dir = match &self.dir {
                Some(dir) => dir.clone(),
                None => std::env::current_dir().unwrap_or_default(),
            };
            bail!("not a git repository: {}", dir.display());
        }
        check(out).map(drop)
    }

    /// ARG names revisions in this repository (read-only; false outside one).
    pub fn resolves(&self, arg: &str) -> bool {
        self.command()
            .args(["rev-parse", "--quiet", arg, "--"])
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
        let out = run(self.command().args(["rev-parse", "--show-toplevel"])).ok()?;
        Some(PathBuf::from(String::from_utf8_lossy(&out).trim()))
    }

    /// The revision whose files the change ends in: B of `A..B` or `A...B`
    /// (HEAD when B is empty), a fetched GitHub change's head; `None` when
    /// it ends in the working tree (one revision, or a diff on stdin).
    pub fn after_revision(&self, range: Option<&str>) -> Option<String> {
        if let Some(bare) = &self.bare {
            return Some(bare.attributes_from.clone());
        }
        let range = range?;
        if let Some(rev) = parent_range(range) {
            return Some(rev.to_string());
        }
        let (_, to) = range.split_once("...").or_else(|| range.split_once(".."))?;
        Some(if to.is_empty() { "HEAD" } else { to }.to_string())
    }

    /// Where a diff read from stdin ends, from its blob ids: the index when
    /// each file goes from HEAD to the index (`git diff --staged`), the
    /// working tree when each goes from the index or HEAD to the working
    /// tree (`git diff`, `git diff HEAD`). `None` for anything else, such as
    /// a diff between two commits: the files it doesn't name can't be read
    /// as of its after side.
    pub fn stdin_after(&self, diffs: &[FileDiff]) -> Option<State> {
        let index: HashMap<String, String> = self.files(None).ok()?.into_iter().collect();
        let head: HashMap<String, String> = self
            .files(Some("HEAD"))
            .map(|f| f.into_iter().collect())
            .unwrap_or_default();
        let worktree = self.hash_worktree(diffs.iter().filter_map(|d| d.new_path.as_deref()))?;
        let all = |before: &[&HashMap<String, String>], after: &HashMap<String, String>| {
            diffs.iter().all(|d| {
                blob_is(&d.new_blob, d.new_path.as_deref(), after)
                    && before
                        .iter()
                        .any(|b| blob_is(&d.old_blob, d.old_path.as_deref(), b))
            })
        };
        if all(&[&head], &index) {
            Some(State::Index)
        } else if all(&[&index, &head], &worktree) {
            Some(State::Worktree)
        } else {
            None
        }
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
    /// empty), the merge base of `A...B`, R of a single revision or `R^!`;
    /// `None` for a diff on stdin.
    pub fn before_revision(&self, range: Option<&str>) -> Option<String> {
        let range = range?;
        if let Some(rev) = parent_range(range) {
            return Some(format!("{rev}^"));
        }
        if let Some((from, to)) = range.split_once("...") {
            let side = |r: &str| if r.is_empty() { "HEAD" } else { r }.to_string();
            let out = run(self.command().args(["merge-base", &side(from), &side(to)])).ok()?;
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
        run(self
            .command()
            .args(["cat-file", "blob", &format!("{rev}:{path}")]))
        .ok()
    }

    /// The files directly in DIR at REV (`""` for the top), as paths.
    pub fn ls(&self, rev: &str, dir: &str) -> Vec<String> {
        let tree = format!("{rev}:{dir}");
        // Without --full-tree, ls-tree filters by the cwd's path from the top.
        let Ok(out) = run(self.command().args(["ls-tree", "-z", "--full-tree", &tree])) else {
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
            Some(rev) => run(self
                .command()
                .args(["ls-tree", "-r", "-z", "--full-tree", rev]))?,
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

    /// The remote a partial clone fetches missing objects from.
    fn promisor(&self) -> Option<String> {
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

    /// Files of STATE matching PATHSPECS that hold WORD as a whole word, as
    /// paths from the top; the working tree's untracked files included.
    /// Never fetches: a blob a partial clone lacks is an error.
    pub fn grep(&self, state: &State, word: &str, pathspecs: &[String]) -> Result<Vec<String>> {
        let mut cmd = self.command();
        cmd.env("GIT_NO_LAZY_FETCH", "1").args([
            "grep",
            "--no-color",
            "-l",
            "-I",
            "-w",
            "-F",
            "--full-name",
            "-e",
            word,
        ]);
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
        cmd.args(rev).arg("--").args(pathspecs);
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
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        let mut cmd = self.command();
        if self.bare.is_none() {
            let Some(toplevel) = self.toplevel() else {
                return HashMap::new();
            };
            // check-attr reads paths against the cwd; the diff's are from the top.
            cmd.current_dir(toplevel);
        }
        cmd.arg("check-attr");
        let source = at.or(self.bare.as_ref().map(|b| b.attributes_from.as_str()));
        if let Some(source) = source {
            cmd.args(["--source", source]);
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
                return HashMap::new();
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
        found
    }
}

const LINGUIST: [&str; 3] = [
    "linguist-generated",
    "linguist-vendored",
    "linguist-documentation",
];

/// R of `R^!` or `R^-` (`R^-N`): git's shorthand for R against a parent.
fn parent_range(range: &str) -> Option<&str> {
    if let Some(rev) = range.strip_suffix("^!") {
        return Some(rev);
    }
    let (rev, n) = range.rsplit_once("^-")?;
    n.bytes().all(|b| b.is_ascii_digit()).then_some(rev)
}

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
    child.stdin.take().context("git stdin")?.write_all(input)?;
    check(child.wait_with_output()?)
}

fn check(output: Output) -> Result<Vec<u8>> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git: {}", stderr.trim());
    }
    Ok(output.stdout)
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
        run(self.git.command().args(["cat-file", "blob", id]))
            .ok()
            .or_else(|| self.read_worktree(id, path))
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
