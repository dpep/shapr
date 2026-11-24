//! Shared helpers for the end-to-end tests: throwaway git repositories, the
//! built binary, and reads of its JSON. Each test file is its own crate and
//! uses a subset, so unused helpers are expected.
#![allow(dead_code)]

use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// git and shapr with the caller's repository context removed, so a run
/// under `git rebase --exec` can't write into the real repository, with
/// no system or user git config, and with shapr's usage log off.
pub fn command(program: &str, dir: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("SHAPR_USAGE", "0");
    cmd
}

/// git's stdout, after checking it succeeded.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = command("git", dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// An empty repository on `main` with a committer.
pub fn init(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
}

/// Writes each (path, content) pair under `dir`, making directories.
pub fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, content) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

/// Writes `files` and commits everything; the new commit's sha.
pub fn commit(dir: &Path, files: &[(&str, &str)], message: &str) -> String {
    write(dir, files);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
    git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

/// A repository with one commit per state; each state writes (path,
/// content) pairs over the last.
pub fn repo(states: &[&[(&str, &str)]]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    for state in states {
        commit(dir.path(), state, "state");
    }
    dir
}

/// The built shapr in `dir`, stdin empty.
pub fn shapr(dir: &Path, args: &[&str]) -> Output {
    command(env!("CARGO_BIN_EXE_shapr"), dir)
        .args(args)
        .output()
        .unwrap()
}

/// The built shapr in `dir`, reading `stdin`.
pub fn shapr_stdin(dir: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = command(env!("CARGO_BIN_EXE_shapr"), dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(stdin.as_bytes()).unwrap();
    drop(pipe);
    child.wait_with_output().unwrap()
}

/// stdout as one JSON value (`-j`).
pub fn report(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: stdout {} stderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// stdout as one JSON value per line (`-J`).
pub fn records(out: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
}

/// `shapr -j` over one commit taking `before` to `after`, each (path,
/// content) pairs; the run must succeed.
pub fn classify(before: &[(&str, &str)], after: &[(&str, &str)]) -> Value {
    let dir = repo(&[before, after]);
    let out = shapr(dir.path(), &["-j", "HEAD~1..HEAD"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    report(&out)
}

/// One Ruby file, `a.rb`, before and after.
pub fn ruby(before: &str, after: &str) -> Value {
    classify(&[("a.rb", before)], &[("a.rb", after)])
}

/// One Rust file, `lib.rs`, before and after.
pub fn rust(before: &str, after: &str) -> Value {
    classify(&[("lib.rs", before)], &[("lib.rs", after)])
}

pub fn hunks(report: &Value) -> &Vec<Value> {
    report["hunks"].as_array().unwrap()
}

/// Each hunk's `kind`.
pub fn kinds(report: &Value) -> Vec<&str> {
    hunks(report)
        .iter()
        .map(|h| h["kind"].as_str().unwrap())
        .collect()
}

/// The part a hunk's kind stands for: the last of that kind (the residual,
/// for `logic`), else the first that isn't structure.
pub fn lead(hunk: &Value) -> &Value {
    let parts = hunk["parts"].as_array().unwrap();
    parts
        .iter()
        .rev()
        .find(|p| p["kind"] == hunk["kind"])
        .or_else(|| parts.iter().find(|p| p["structure"] == false))
        .unwrap_or_else(|| panic!("no parts: {hunk:#}"))
}

/// Each of a hunk's parts' `kind`.
pub fn part_kinds(hunk: &Value) -> Vec<&str> {
    hunk["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect()
}

/// A hunk's first part of `kind`.
pub fn part<'h>(hunk: &'h Value, kind: &str) -> &'h Value {
    hunk["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} part: {hunk:#}"))
}

/// A part's lines as (before, after) line numbers.
pub fn lines(part: &Value) -> (Vec<u64>, Vec<u64>) {
    let side = |key: &str| {
        part["lines"][key]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|run| {
                let start = run["start"].as_u64().unwrap();
                start..start + run["count"].as_u64().unwrap()
            })
            .collect()
    };
    (side("before"), side("after"))
}
