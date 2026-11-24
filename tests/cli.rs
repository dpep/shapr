//! End to end: the built binary against throwaway git repos.

use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// git and shapr with the caller's repository context removed, so a run
/// under `git rebase --exec` can't write into the real repository.
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

fn git(dir: &Path, args: &[&str]) -> String {
    let out = command("git", dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// A repo with one commit per state; each state maps paths to contents.
fn repo(states: &[&[(&str, &str)]]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    for state in states {
        write(dir.path(), state);
        git(dir.path(), &["add", "-A"]);
        git(
            dir.path(),
            &["commit", "-q", "--allow-empty", "-m", "state"],
        );
    }
    dir
}

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, content) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

fn shapr(dir: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = command(env!("CARGO_BIN_EXE_shapr"), dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(stdin.unwrap_or_default().as_bytes())
        .unwrap();
    drop(pipe);
    child.wait_with_output().unwrap()
}

/// `shapr -j` on the last commit: the report and the exit code.
fn classify(before: &[(&str, &str)], after: &[(&str, &str)]) -> (Value, i32) {
    let dir = repo(&[before, after]);
    let out = shapr(dir.path(), &["-j", "HEAD~1..HEAD"], None);
    let report = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    (report, out.status.code().unwrap())
}

fn kinds(report: &Value) -> Vec<&str> {
    report["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["kind"].as_str().unwrap())
        .collect()
}

const METHOD: &str = "class Greeter\n  def greet(name)\n    \"hi #{name}\"\n  end\nend\n";

#[test]
fn reformatting_is_format_only() {
    let after = "class Greeter\n  def greet( name )\n      \"hi #{name}\"\n  end\nend\n";
    let (report, code) = classify(&[("a.rb", METHOD)], &[("a.rb", after)]);
    assert_eq!(kinds(&report), ["format_only"]);
    let hunk = &report["hunks"][0];
    assert_eq!(hunk["confidence"], 1.0);
    assert_eq!(hunk["parsed"], true);
    assert_eq!(hunk["evidence"]["method"], "ast");
    assert_eq!(hunk["review_lines"], 0);
    assert_eq!(report["summary"]["verdict"], "structure");
    assert_eq!(report["summary"]["structure_share"], 1.0);
    assert_eq!(code, 0);
}

#[test]
fn rewording_a_comment_is_comment_only_and_counted() {
    let before = "# Says hello.\ndef greet\n  1 # the answer\nend\n";
    let after = "# Says hello politely.\n# Twice.\ndef greet\n  1 # the answer\nend\n";
    let (report, code) = classify(&[("a.rb", before)], &[("a.rb", after)]);
    assert_eq!(kinds(&report), ["comment_only"]);
    let hunk = &report["hunks"][0];
    assert_eq!(
        (
            hunk["comment_added"].as_u64(),
            hunk["comment_deleted"].as_u64()
        ),
        (Some(2), Some(1))
    );
    assert_eq!(hunk["code_added"], 0);
    assert_eq!(code, 0);
}

#[test]
fn a_magic_comment_is_code() {
    let body = "def greet\n  \"hi\"\nend\n";
    let before = format!("# frozen_string_literal: true\n{body}");
    let after = format!("# frozen_string_literal: false\n{body}");
    let (report, code) = classify(&[("a.rb", &before)], &[("a.rb", &after)]);
    assert_eq!(kinds(&report), ["logic"]);
    assert_eq!(report["hunks"][0]["code_added"], 1);
    assert_eq!(code, 1);
}

#[test]
fn changing_code_is_logic_with_its_evidence() {
    let after = METHOD.replace("hi", "hello");
    let (report, code) = classify(&[("a.rb", METHOD)], &[("a.rb", &after)]);
    assert_eq!(kinds(&report), ["logic"]);
    let hunk = &report["hunks"][0];
    assert_eq!(hunk["review_lines"], 2);
    assert!(hunk["evidence"]["similarity"].as_f64().unwrap() < 1.0);
    assert_eq!(report["summary"]["verdict"], "behavior");
    assert_eq!(report["summary"]["behavior_lines"], 2);
    assert_eq!(code, 1);
}

#[test]
fn whitespace_inside_a_string_is_logic() {
    let after = METHOD.replace("hi #", "hi  #");
    let (report, _) = classify(&[("a.rb", METHOD)], &[("a.rb", &after)]);
    assert_eq!(kinds(&report), ["logic"]);
}

#[test]
fn format_and_logic_in_one_change_is_mixed() {
    let filler = "X = 1\n".repeat(10);
    let before = format!("def a\n  1+1\nend\n{filler}def b\n  2\nend\n");
    let after = format!("def a\n  1 + 1\nend\n{filler}def b\n  3\nend\n");
    let (report, code) = classify(&[("lib/a.rb", &before)], &[("lib/a.rb", &after)]);
    assert_eq!(kinds(&report), ["format_only", "logic"]);
    let summary = &report["summary"];
    assert_eq!(summary["verdict"], "mixed");
    assert_eq!(summary["kinds"]["format_only"], 1);
    assert_eq!(summary["review_lines"], 2);
    assert_eq!(summary["structure_share"], 0.5);
    assert_eq!(report["files"][0]["lines_before"], 16);
    assert_eq!(code, 3);
}

fn review_lines(report: &Value) -> Vec<u64> {
    report["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["review_lines"].as_u64().unwrap())
        .collect()
}

#[test]
fn an_extract_is_structure_but_still_read() {
    let before = "class Report\n  def render(title)\n    puts title\n    puts \"-\" * 10\n    puts \"=\" * 10\n    puts \"end\"\n  end\nend\n";
    let after = "class Report\n  def render(title)\n    puts title\n    rule\n    puts \"end\"\n  end\n\n  def rule\n    puts \"-\" * 10\n    puts \"=\" * 10\n  end\nend\n";
    let (report, code) = classify(&[("a.rb", before)], &[("a.rb", after)]);
    assert!(kinds(&report).iter().all(|k| *k == "extract"), "{report:#}");
    // Every changed code line: git's diff keeps the moved statements as
    // context and changes the lines around them.
    assert_eq!(report["summary"]["review_lines"], 5);
    assert_eq!(report["summary"]["behavior_lines"], 0);
    assert_eq!(report["summary"]["verdict"], "structure");
    assert_eq!(code, 0);
}

#[test]
fn a_move_with_changes_reviews_only_the_changed_lines() {
    let method = |name: &str, call: &str| {
        format!("  def {name}(x)\n    y = x * 2\n    {call}(y)\n    y + 1\n  end\n")
    };
    let fillers: String = (0..3).map(|i| method(&format!("m{i}"), "log")).collect();
    let before = format!(
        "class Shape\n{}{}{fillers}end\n",
        method("moved", "log"),
        method("first", "log")
    );
    let after = format!(
        "class Shape\n{}{fillers}{}end\n",
        method("first", "log"),
        method("moved", "warn")
    );
    let (report, _) = classify(&[("a.rb", &before)], &[("a.rb", &after)]);
    assert_eq!(
        kinds(&report),
        ["move_with_changes", "move_with_changes"],
        "{report:#}"
    );
    // `log(y)` out of the removed body, `warn(y)` into the added one.
    assert_eq!(review_lines(&report), [1, 1]);
    assert_eq!(report["summary"]["behavior_lines"], 2);
}

#[test]
fn a_syntax_error_falls_back_to_lines_with_capped_confidence() {
    let before = "def a\n  1+1\nend\ndef broken(\n";
    let after = "def a\n  1 + 1\nend\ndef broken(\n";
    let (report, _) = classify(&[("a.rb", before)], &[("a.rb", after)]);
    let hunk = &report["hunks"][0];
    assert_eq!(hunk["kind"], "format_only");
    assert_eq!(hunk["parsed"], false);
    assert_eq!(hunk["evidence"]["method"], "line");
    assert!(hunk["confidence"].as_f64().unwrap() <= 0.5);
    assert_eq!(report["summary"]["unparsed_hunks"], 1);
}

#[test]
fn other_languages_are_unparsed_logic() {
    let (report, code) = classify(&[("notes.md", "a\n")], &[("notes.md", "b\n")]);
    let hunk = &report["hunks"][0];
    assert_eq!(hunk["kind"], "logic");
    assert_eq!(hunk["confidence"], 0.0);
    assert_eq!(hunk["language"], Value::Null);
    assert_eq!(hunk["evidence"]["method"], "none");
    assert_eq!(code, 1);
}

#[test]
fn a_piped_diff_matches_the_range() {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    // Uncommitted: the after blob is only in the working tree.
    write(dir.path(), &[("a.rb", &METHOD.replace("  end", "    end"))]);
    let diff = git(dir.path(), &["diff"]);
    let piped = shapr(dir.path(), &["-J"], Some(&diff));
    let ranged = shapr(dir.path(), &["-J", "HEAD"], None);
    assert_eq!(
        String::from_utf8_lossy(&piped.stdout),
        String::from_utf8_lossy(&ranged.stdout)
    );
    let lines: Vec<Value> = piped
        .stdout
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect();
    let records: Vec<&str> = lines
        .iter()
        .map(|l| l["record"].as_str().unwrap())
        .collect();
    assert_eq!(records, ["file", "hunk", "summary"]);
    assert_eq!(lines[1]["kind"], "format_only");
    assert_eq!(piped.status.code(), Some(0));
}

#[test]
fn empty_input_is_an_empty_verdict() {
    let dir = repo(&[]);
    let out = shapr(dir.path(), &["-J"], Some(""));
    let summary: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["verdict"], "empty");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn errors_have_their_own_exit_codes() {
    let dir = repo(&[&[("a.rb", "1\n")]]);
    let bad_range = shapr(dir.path(), &["-J", "nope..HEAD"], None);
    assert_eq!(bad_range.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&bad_range.stdout).unwrap();
    assert_eq!(
        (error["record"].as_str(), error["code"].as_u64()),
        (Some("error"), Some(2))
    );

    let usage = shapr(dir.path(), &["-J", "--bogus"], None);
    assert_eq!(usage.status.code(), Some(64));
    let error: Value = serde_json::from_slice(&usage.stdout).unwrap();
    assert_eq!(error["code"], 64);
}

#[test]
fn completions_carry_each_shells_marker() {
    let dir = tempfile::tempdir().unwrap();
    for (shell, marker) in [
        ("bash", "complete -F"),
        ("zsh", "#compdef"),
        ("fish", "complete -c"),
    ] {
        let out = shapr(dir.path(), &["--completions", shell], None);
        assert!(out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(marker),
            "{shell}"
        );
    }
    let unknown = command(env!("CARGO_BIN_EXE_shapr"), dir.path())
        .args(["--completions"])
        .env("SHELL", "/bin/unknown")
        .output()
        .unwrap();
    assert_eq!(unknown.status.code(), Some(64));
}

#[test]
fn every_output_field_is_documented() {
    let schema =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/schema.md")).unwrap();
    let after = METHOD.replace("hi", "hello");
    let (report, _) = classify(&[("a.rb", METHOD)], &[("a.rb", &after)]);
    let records = [
        &report["files"][0],
        &report["hunks"][0],
        &report["hunks"][0]["evidence"],
        &report["hunks"][0]["before"],
        &report["summary"],
        &report["summary"]["kinds"],
    ];
    for record in records {
        for key in record.as_object().unwrap().keys() {
            assert!(
                schema.contains(&format!("`{key}`")),
                "docs/schema.md doesn't name `{key}`"
            );
        }
    }
}
