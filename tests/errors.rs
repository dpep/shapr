//! Errors and input edges: an error looks like an error, in one line.

use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// The part a hunk's kind stands for: the last of that kind (the residual,
/// for `logic`), else the first that isn't structure.
fn lead(hunk: &Value) -> &Value {
    let parts = hunk["parts"].as_array().unwrap();
    parts
        .iter()
        .rev()
        .find(|p| p["kind"] == hunk["kind"])
        .or_else(|| parts.iter().find(|p| p["structure"] == false))
        .unwrap_or_else(|| panic!("no parts: {hunk:#}"))
}

fn command(program: &str, dir: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CEILING_DIRECTORIES", dir.parent().unwrap_or(dir));
    cmd
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = command("git", dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// A repo with one commit per state; each state maps paths to contents.
fn repo(states: &[&[(&str, &str)]]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    for state in states {
        for (path, content) in *state {
            std::fs::write(dir.path().join(path), content).unwrap();
        }
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "s"]);
    }
    dir
}

fn shapr(dir: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = command(env!("CARGO_BIN_EXE_shapr"), dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// The JSON error record, after checking stderr is the same one line.
fn error(out: &Output, code: i32) -> String {
    assert_eq!(out.status.code(), Some(code), "{out:?}");
    let record: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(record["record"], "error");
    assert_eq!(record["code"], code);
    assert_eq!(record["schema"], 1);
    let message = record["error"].as_str().unwrap().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.trim_end(), format!("shapr: {message}"));
    message
}

const METHOD: &str = "def greet(name)\n  \"hi #{name}\"\nend\n";

#[test]
fn no_input_on_stdin_is_a_usage_error() {
    let dir = repo(&[]);
    for stdin in ["", "\n  \n"] {
        let message = error(&shapr(dir.path(), &["-J"], stdin), 64);
        assert!(message.starts_with("no input"), "{message}");
    }
}

#[test]
fn stdin_that_holds_no_diff_is_an_error() {
    let dir = repo(&[]);
    let message = error(&shapr(dir.path(), &["-J"], "hello\n"), 2);
    assert!(message.contains("not a diff"), "{message}");
}

#[test]
fn a_rename_without_hunks_on_stdin_is_a_diff() {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    git(dir.path(), &["mv", "a.rb", "b.rb"]);
    let diff = git(dir.path(), &["diff", "--staged", "-M"]);
    let out = shapr(dir.path(), &["-j"], &diff);
    assert_eq!(out.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["summary"]["verdict"], "none");
}

#[test]
fn an_empty_range_has_no_verdict() {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    let out = shapr(dir.path(), &["-J", "main..main"], "");
    let records: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(records.len(), 2, "a meta and a summary record");
    let summary = &records[1];
    assert_eq!(summary["verdict"], "none");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn outside_a_repository_is_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let message = error(&shapr(dir.path(), &["-J", "main..HEAD"], ""), 2);
    assert!(message.starts_with("not a git repository: "), "{message}");

    let elsewhere = repo(&[]);
    let target = dir.path().to_str().unwrap();
    let message = error(
        &shapr(elsewhere.path(), &["-J", "-C", target, "main..HEAD"], ""),
        2,
    );
    assert_eq!(message, format!("not a git repository: {target}"));
}

#[test]
fn a_range_that_isnt_a_revision_is_one_line() {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    // `..` names the parent directory: git would read it as a path.
    for (range, rev) in [("..", ".."), ("nope..HEAD", "nope..HEAD")] {
        let message = error(&shapr(dir.path(), &["-J", range], ""), 2);
        assert!(
            message.contains(&format!("bad revision '{rev}'")),
            "{message}"
        );
    }
}

#[test]
fn a_usage_error_message_has_no_error_prefix() {
    let dir = repo(&[]);
    let out = shapr(dir.path(), &["-J", "--bogus"], "");
    assert_eq!(out.status.code(), Some(64));
    let record: Value = serde_json::from_slice(&out.stdout).unwrap();
    let message = record["error"].as_str().unwrap();
    assert!(message.starts_with("unexpected argument"), "{message}");
}

#[test]
fn quiet_writes_no_error_record() {
    let dir = repo(&[]);
    for args in [&["-q", "-J"][..], &["-qJ", "--bogus"]] {
        let out = shapr(dir.path(), args, "");
        assert_eq!(out.status.code(), Some(64), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert!(!out.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn a_syntax_error_is_named_in_the_reason() {
    let before = "def a\n  1\nend\n";
    let after = "def a\n  2\nend\ndef broken(\n";
    let dir = repo(&[&[("a.rb", before)], &[("a.rb", after)]]);
    let out = shapr(dir.path(), &["-j", "HEAD~1..HEAD"], "");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let hunk = &report["hunks"][0];
    assert_eq!(hunk["kind"], "logic");
    assert_eq!(lead(hunk)["evidence"]["comparison"], "line");
    let reason = lead(hunk)["reason"].as_str().unwrap();
    assert!(
        reason.contains("syntax error on the after side; compared line by line"),
        "{reason}"
    );
}

#[test]
fn one_review_line_is_singular() {
    let after = METHOD.replace("end", "  1\nend");
    let dir = repo(&[&[("a.rb", METHOD)], &[("a.rb", &after)]]);
    let out = shapr(dir.path(), &["HEAD~1..HEAD"], "");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("review 1 line;"), "{text}");
}

#[test]
fn verbose_names_a_near_extract_with_an() {
    let before =
        "def check(order)\n  validate(order)\n  return :empty if order.empty?\n  :ok\nend\n";
    // `guard`'s body isn't the removed statement, so it is no extract at all.
    let after = "def check(order)\n  validate(order)\n  guard(order)\n  :ok\nend\n\ndef guard(order)\n  order.clear\nend\n";
    let dir = repo(&[&[("a.rb", before)], &[("a.rb", after)]]);
    let out = shapr(dir.path(), &["-q", "-v", "HEAD~1..HEAD"], "");
    let log = String::from_utf8_lossy(&out.stderr);
    assert!(log.contains("`guard` not an extract"), "{log}");
}
