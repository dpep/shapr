//! The git commands shapr runs: the diff for a range, and file contents by
//! blob id.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// Where git runs: the current directory, or the one `-C` names.
pub struct Git {
    dir: Option<PathBuf>,
}

impl Git {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Git { dir }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new("git");
        // Paths verbatim, whatever the user's quoting config.
        cmd.args(["-c", "core.quotePath=false"]);
        if let Some(dir) = &self.dir {
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
            None if range.contains("..") => range.to_string(),
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

    fn toplevel(&self) -> Option<PathBuf> {
        let out = run(self.command().args(["rev-parse", "--show-toplevel"])).ok()?;
        Some(PathBuf::from(String::from_utf8_lossy(&out).trim()))
    }

    /// Each path's `linguist-generated` attribute, where .gitattributes sets
    /// or unsets it. Empty outside a repository.
    pub fn linguist_generated(&self, paths: &[&str]) -> HashMap<String, bool> {
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        let Some(toplevel) = self.toplevel() else {
            return HashMap::new();
        };
        // check-attr reads paths against the cwd; the diff's are from the top.
        let mut cmd = self.command();
        cmd.current_dir(toplevel)
            .args(["check-attr", "-z", "--stdin", "linguist-generated"]);
        let Ok(out) = run_with_input(&mut cmd, &input) else {
            return HashMap::new();
        };
        let out = String::from_utf8_lossy(&out);
        let fields: Vec<&str> = out.split('\0').collect();
        let (triples, _) = fields.as_chunks::<3>();
        triples
            .iter()
            .filter_map(|[path, _, value]| {
                let value = match *value {
                    "set" | "true" => true,
                    "unset" | "false" => false,
                    _ => return None,
                };
                Some((path.to_string(), value))
            })
            .collect()
    }
}

fn run(cmd: &mut Command) -> Result<Vec<u8>> {
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
