//! End to end: changes that look structural but can change behavior. Each
//! must stay out of the structural kinds.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn command(program: &str, dir: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd
}

fn git(dir: &Path, args: &[&str]) {
    let out = command("git", dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `shapr -j` over one commit writing each (path, content) pair.
fn classify(before: &[(&str, &str)], after: &[(&str, &str)]) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "Test"]);
    for files in [before, after] {
        for (name, content) in files {
            std::fs::write(path.join(name), content).unwrap();
        }
        git(path, &["add", "-A"]);
        git(path, &["commit", "-q", "--allow-empty", "-m", "state"]);
    }
    let out = command(env!("CARGO_BIN_EXE_shapr"), path)
        .args(["-j", "HEAD~1..HEAD"])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

fn ruby(before: &str, after: &str) -> Value {
    classify(&[("a.rb", before)], &[("a.rb", after)])
}

fn rust(before: &str, after: &str) -> Value {
    classify(&[("lib.rs", before)], &[("lib.rs", after)])
}

fn hunks(report: &Value) -> &Vec<Value> {
    report["hunks"].as_array().unwrap()
}

fn kinds(report: &Value) -> Vec<&str> {
    hunks(report)
        .iter()
        .map(|h| h["kind"].as_str().unwrap())
        .collect()
}

/// Every hunk is `logic`, and some hunk was a near miss of `near` for `risk`.
fn assert_near(report: &Value, near: &str, risk: Option<&str>) {
    assert!(kinds(report).iter().all(|k| *k == "logic"), "{report:#}");
    assert!(
        hunks(report)
            .iter()
            .any(|h| h["detail"]["near"] == near && risk.is_none_or(|r| h["detail"]["risk"] == r)),
        "{report:#}"
    );
}

// The line method: a file with a syntax error can't prove structure.

#[test]
fn a_whitespace_edit_in_a_file_that_doesnt_parse_is_logic() {
    let tail = "def b\n  1\nend\ndef c(\n";
    let report = ruby(
        &format!("def a(x)\n  puts -x\nend\n{tail}"),
        &format!("def a(x)\n  puts - x\nend\n{tail}"),
    );
    assert_near(&report, "format_only", None);
    let hunk = &hunks(&report)[0];
    assert_eq!(hunk["evidence"]["method"], "line");
    let reason = hunk["reason"].as_str().unwrap();
    assert!(reason.contains("compared line by line"), "{reason}");
}

#[test]
fn a_rust_token_split_that_breaks_the_parse_is_logic() {
    let report = rust(
        "fn f(a: u32) -> u32 {\n    a >> 1\n}\n",
        "fn f(a: u32) -> u32 {\n    a > > 1\n}\n",
    );
    assert_near(&report, "format_only", None);
}

#[test]
fn a_comment_edit_in_a_file_that_doesnt_parse_is_logic() {
    let tail = "def c(\n";
    let report = ruby(
        &format!("def a\n  1 # one\nend\n{tail}"),
        &format!("def a\n  1 # uno\nend\n{tail}"),
    );
    assert_near(&report, "comment_only", None);
}
