//! The git commands shapr runs: the diff for a range, and file contents by
//! blob id.

use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::process::Command;

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
}

fn run(cmd: &mut Command) -> Result<Vec<u8>> {
    let output = cmd.output().context("could not run git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git: {}", stderr.trim());
    }
    Ok(output.stdout)
}

/// Reads file contents by blob id. Blobs `git diff` names for a working-tree
/// file aren't in the object store, so those are read from disk when the
/// file there still hashes to the id.
pub struct Blobs {
    git: Git,
    toplevel: Option<PathBuf>,
}

impl Blobs {
    pub fn new(git: Git) -> Self {
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
            let out = run(self.git.command().args(["rev-parse", "--show-toplevel"])).ok()?;
            self.toplevel = Some(PathBuf::from(String::from_utf8_lossy(&out).trim()));
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
