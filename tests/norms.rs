//! Style norms, as data: each directory under tests/norms is a case.
//!
//! - `case.json`: `about`; `path` (default `a.rb`), where the file lives in
//!   the repository; `command`, `diff` (default: `shapr -j HEAD~1..HEAD`,
//!   matching `expect.hunks` against each hunk's `style`) or `norms`
//!   (`shapr norms -j PATH` on the after side, matching the whole output);
//!   `norm`, a norm id: only that norm's entries are matched (a hunk's
//!   rollup `direction` and `flagged` span every norm, so a case naming one
//!   leaves them out); and `expect`.
//! - `before.rb`, `after.rb`: the file's two states; a missing one is a file
//!   that doesn't exist on that side.
//! - `repo/`: other files, the same in both states (`.rubocop.yml`, neighbors).
//! - `worktree/`: files written over the working tree after both commits,
//!   left uncommitted.
//!
//! `expect` is a subset: an object matches when each of its keys matches,
//! an array when it has the same length and each element matches.

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
    assert!(out.status.success(), "git {args:?}");
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Mismatches between `actual` and the subset `expect`, by JSON path.
fn mismatches(at: &str, expect: &Value, actual: &Value, out: &mut Vec<String>) {
    match (expect, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (key, value) in e {
                let path = format!("{at}.{key}");
                match a.get(key) {
                    Some(actual) => mismatches(&path, value, actual, out),
                    None => out.push(format!("{path}: missing")),
                }
            }
        }
        (Value::Array(e), Value::Array(a)) if e.len() == a.len() => {
            for (i, (e, a)) in e.iter().zip(a).enumerate() {
                mismatches(&format!("{at}[{i}]"), e, a, out);
            }
        }
        _ if expect != actual => out.push(format!("{at}: expected {expect}, got {actual}")),
        _ => {}
    }
}

fn run_case(case: &Path) -> Vec<String> {
    let spec: Value =
        serde_json::from_str(&std::fs::read_to_string(case.join("case.json")).unwrap()).unwrap();
    let path = spec["path"].as_str().unwrap_or("a.rb");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "user.name", "Test"]);
    if case.join("repo").is_dir() {
        copy_tree(&case.join("repo"), root);
    }
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    for side in ["before.rb", "after.rb"] {
        match std::fs::read(case.join(side)) {
            Ok(content) => std::fs::write(&file, content).unwrap(),
            Err(_) => {
                let _ = std::fs::remove_file(&file);
            }
        }
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "--allow-empty", "-m", side]);
    }
    if case.join("worktree").is_dir() {
        copy_tree(&case.join("worktree"), root);
    }
    let norms = spec["command"] == "norms";
    let args: Vec<&str> = if norms {
        vec!["norms", "-j", path]
    } else {
        vec!["-j", "HEAD~1..HEAD"]
    };
    let out = command(env!("CARGO_BIN_EXE_shapr"), root)
        .args(&args)
        .output()
        .unwrap();
    let report: Value = match serde_json::from_slice(&out.stdout) {
        Ok(report) => report,
        Err(e) => return vec![format!("{e}: {}", String::from_utf8_lossy(&out.stderr))],
    };
    let only = |entries: &Value| match spec["norm"].as_str() {
        Some(norm) => Value::Array(
            (entries.as_array().into_iter().flatten())
                .filter(|e| e["norm"] == norm)
                .cloned()
                .collect(),
        ),
        None => entries.clone(),
    };
    let actual = if norms {
        let mut report = report;
        report["norms"] = only(&report["norms"]);
        report
    } else {
        let styles = report["hunks"].as_array().unwrap().iter();
        Value::Object(
            [(
                "hunks".to_string(),
                Value::Array(
                    styles
                        .map(|h| {
                            let mut style = h["style"].clone();
                            if style.is_object() {
                                style["norms"] = only(&style["norms"]);
                            }
                            style
                        })
                        .collect(),
                ),
            )]
            .into_iter()
            .collect(),
        )
    };
    let mut out = Vec::new();
    mismatches("", &spec["expect"], &actual, &mut out);
    if !out.is_empty() {
        out.push(format!("actual: {actual:#}"));
    }
    out
}

#[test]
fn norm_cases() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/norms");
    let mut cases: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("case.json").is_file())
        .collect();
    cases.sort();
    assert!(!cases.is_empty());
    let mut failures = Vec::new();
    for case in &cases {
        let problems = run_case(case);
        if !problems.is_empty() {
            let name = case.file_name().unwrap().to_string_lossy();
            failures.push(format!("{name}:\n  {}", problems.join("\n  ")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// A `.editorconfig` above the repository, as any user's home can hold, is
/// never read: config stops at the repository's top.
#[test]
fn config_above_the_repository_is_not_read() {
    let outer = tempfile::tempdir().unwrap();
    std::fs::write(
        outer.path().join(".editorconfig"),
        "[*]\ntrim_trailing_whitespace = true\n",
    )
    .unwrap();
    std::fs::write(
        outer.path().join(".rubocop.yml"),
        "Style/StringLiterals:\n  EnforcedStyle: double_quotes\n",
    )
    .unwrap();
    let root = outer.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    for (side, content) in [
        ("before", "def a\n  go\nend\n"),
        ("after", "def a\n  go('x')  \nend\n"),
    ] {
        std::fs::write(root.join("a.rb"), content).unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", side]);
    }
    for args in [
        vec!["-j", "HEAD~1..HEAD"],
        vec!["-j", "HEAD~1"],
        vec!["norms", "-j", "a.rb"],
    ] {
        let out = command(env!("CARGO_BIN_EXE_shapr"), &root)
            .args(&args)
            .output()
            .unwrap();
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        let norms = match args[0] {
            "norms" => report["norms"].clone(),
            _ => report["hunks"][0]["style"]["norms"].clone(),
        };
        let norms = norms.as_array().unwrap();
        assert!(!norms.is_empty(), "{args:?}");
        for norm in norms {
            assert_eq!(norm["configured"], Value::Null, "{args:?}: {norm}");
        }
    }
}
