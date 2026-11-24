//! End to end: a hunk holding several runs of changed lines (edits git's
//! context merged into one hunk) is classified run by run, after the whole
//! hunk.

use serde_json::Value;

mod common;
use common::*;

/// The report's only hunk.
fn hunk(report: &Value) -> &Value {
    match report["hunks"].as_array().unwrap().as_slice() {
        [only] => only,
        _ => panic!("expected one hunk: {report:#}"),
    }
}

/// Each part as (kind, before lines, after lines).
fn parts(hunk: &Value) -> Vec<(String, Vec<u64>, Vec<u64>)> {
    let parts = hunk["parts"].as_array().unwrap().iter();
    parts
        .map(|p| {
            let (before, after) = lines(p);
            (p["kind"].as_str().unwrap().to_string(), before, after)
        })
        .collect()
}

fn expected(kind: &str, before: &[u64], after: &[u64]) -> (String, Vec<u64>, Vec<u64>) {
    (kind.to_string(), before.to_vec(), after.to_vec())
}

#[test]
fn a_reformat_near_an_added_statement_is_a_format_part_and_a_residual() {
    let before = "def a\n  x = foo( 1 )\n  y = 2\n  z = 3\n  w\nend\n";
    let after = "def a\n  x = foo(1)\n  y = 2\n  z = 3\n  log(z)\n  w\nend\n";
    let report = ruby(before, after);
    let hunk = hunk(&report);
    assert_eq!(
        parts(hunk),
        [
            expected("format_only", &[2], &[2]),
            expected("logic", &[], &[5])
        ],
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
    let report = ruby(before, &after);
    let hunk = hunk(&report);
    assert_eq!(
        parts(hunk),
        [
            expected("comment_only", &[2], &[2]),
            expected("logic", &[4], &[4])
        ],
        "{report:#}"
    );
}

#[test]
fn a_reflow_git_splits_into_runs_stays_one_format_part() {
    let before = "module M\nclass A\n  def a\n    b do\n      c\n    end\n  end\nend\nend\n";
    let after =
        "module M\n  class A\n    def a\n      b do\n        c\n      end\n    end\n  end\nend\n";
    let report = ruby(before, after);
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
    let report = ruby(before, after);
    let hunk = hunk(&report);
    let kinds: Vec<String> = parts(hunk).into_iter().map(|p| p.0).collect();
    assert_eq!(kinds, ["logic"], "{report:#}");
    assert_eq!(hunk["verdict"], "behavior");
}

#[test]
fn a_rename_near_an_added_statement_is_a_rename_part_and_a_residual() {
    let before = "def total(items)\n  sum = items.sum\n  log(sum)\n  tax = 2\n  tax\nend\n";
    let after = "def total(items)\n  amount = items.sum\n  log(amount)\n  tax = 2\n  log(tax)\n  tax\nend\n";
    let report = ruby(before, after);
    let hunk = hunk(&report);
    assert_eq!(
        parts(hunk),
        [
            expected("rename", &[2, 3], &[2, 3]),
            expected("logic", &[], &[5])
        ],
        "{report:#}"
    );
}
