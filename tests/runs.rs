//! End to end: a hunk holding several runs of changed lines (edits git's
//! context merged into one hunk) is classified run by run, after the whole
//! hunk.

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

/// `shapr -j` over one commit changing `a.rb` from `before` to `after`.
fn one(before: &str, after: &str) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "Test"]);
    for content in [before, after] {
        std::fs::write(path.join("a.rb"), content).unwrap();
        git(path, &["add", "-A"]);
        git(path, &["commit", "-q", "-m", "state"]);
    }
    let out = command(env!("CARGO_BIN_EXE_shapr"), path)
        .args(["-j", "HEAD~1..HEAD"])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

/// The report's only hunk.
fn hunk(report: &Value) -> &Value {
    match report["hunks"].as_array().unwrap().as_slice() {
        [only] => only,
        _ => panic!("expected one hunk: {report:#}"),
    }
}

/// Each part as (kind, before lines, after lines).
fn parts(hunk: &Value) -> Vec<(String, Vec<u64>, Vec<u64>)> {
    let side = |part: &Value, key: &str| -> Vec<u64> {
        let runs = part["lines"][key].as_array().unwrap().iter();
        runs.flat_map(|run| {
            let start = run["start"].as_u64().unwrap();
            start..start + run["count"].as_u64().unwrap()
        })
        .collect()
    };
    let parts = hunk["parts"].as_array().unwrap().iter();
    parts
        .map(|p| {
            let kind = p["kind"].as_str().unwrap().to_string();
            (kind, side(p, "before"), side(p, "after"))
        })
        .collect()
}

fn part(kind: &str, before: &[u64], after: &[u64]) -> (String, Vec<u64>, Vec<u64>) {
    (kind.to_string(), before.to_vec(), after.to_vec())
}

#[test]
fn a_reformat_near_an_added_statement_is_a_format_part_and_a_residual() {
    let before = "def a\n  x = foo( 1 )\n  y = 2\n  z = 3\n  w\nend\n";
    let after = "def a\n  x = foo(1)\n  y = 2\n  z = 3\n  log(z)\n  w\nend\n";
    let report = one(before, after);
    let hunk = hunk(&report);
    assert_eq!(
        parts(hunk),
        [part("format_only", &[2], &[2]), part("logic", &[], &[5])],
        "{report:#}"
    );
    assert_eq!(hunk["verdict"], "mixed");
}

#[test]
fn a_comment_fix_near_an_edit_is_a_comment_part_and_a_residual() {
    let before = "class A\n  # Sums teh items.\n  def total(items)\n    items.sum\n  end\nend\n";
    let after = before
        .replace("teh", "the")
        .replace("items.sum\n", "items.sum(&:price)\n");
    let report = one(before, &after);
    let hunk = hunk(&report);
    assert_eq!(
        parts(hunk),
        [part("comment_only", &[2], &[2]), part("logic", &[4], &[4])],
        "{report:#}"
    );
}

#[test]
fn a_reflow_git_splits_into_runs_stays_one_format_part() {
    let before = "module M\nclass A\n  def a\n    b do\n      c\n    end\n  end\nend\nend\n";
    let after =
        "module M\n  class A\n    def a\n      b do\n        c\n      end\n    end\n  end\nend\n";
    let report = one(before, after);
    let hunk = hunk(&report);
    let parts = parts(hunk);
    assert_eq!(parts.len(), 1, "{report:#}");
    assert_eq!(parts[0].0, "format_only");
    assert_eq!(hunk["verdict"], "structure");
}

#[test]
fn code_reindented_under_a_new_wrapper_stays_logic() {
    let before = "def a\n  foo\n\n  bar\n\n  baz\nend\n";
    let after = "def a\n  if ok\n    foo\n\n    bar\n\n    baz\n  end\nend\n";
    let report = one(before, after);
    let hunk = hunk(&report);
    let kinds: Vec<String> = parts(hunk).into_iter().map(|p| p.0).collect();
    assert_eq!(kinds, ["logic"], "{report:#}");
    assert_eq!(hunk["verdict"], "behavior");
}

#[test]
fn a_rename_near_an_added_statement_is_a_rename_part_and_a_residual() {
    let before = "def total(items)\n  sum = items.sum\n  log(sum)\n  tax = 2\n  tax\nend\n";
    let after = "def total(items)\n  amount = items.sum\n  log(amount)\n  tax = 2\n  log(tax)\n  tax\nend\n";
    let report = one(before, after);
    let hunk = hunk(&report);
    assert_eq!(
        parts(hunk),
        [part("rename", &[2, 3], &[2, 3]), part("logic", &[], &[5])],
        "{report:#}"
    );
}
