//! The git commands shapr runs: the diff for a range, and file contents by
//! blob id.

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
        let mut cmd = self.command();
        if let Some(bare) = &self.bare {
            cmd.args(["check-attr", "--source", &bare.attributes_from]);
        } else {
            let Some(toplevel) = self.toplevel() else {
                return HashMap::new();
            };
            // check-attr reads paths against the cwd; the diff's are from the top.
            cmd.current_dir(toplevel).arg("check-attr");
        }
        cmd.args(["-z", "--stdin", "linguist-generated"]);
        let out = match run_with_input(&mut cmd, &input) {
            Ok(out) => out,
            Err(error) => {
                // `check-attr --source` needs git 2.40.
                if self.bare.is_some() {
                    crate::verbose!(
                        "can't read .gitattributes from the head commit ({error:#}): generated files by name only"
                    );
                }
                return HashMap::new();
            }
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
