//! Errors and input edges: an error looks like an error, in one line.

use std::process::Output;

mod common;
use common::*;

/// The JSON error record, after checking stderr is the same one line.
fn error(out: &Output, code: i32) -> String {
    assert_eq!(out.status.code(), Some(code), "{out:?}");
    let record = report(out);
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
        let message = error(&shapr_stdin(dir.path(), &["-J"], stdin), 64);
        assert!(message.starts_with("no input"), "{message}");
    }
}

#[test]
fn stdin_that_holds_no_diff_is_an_error() {
    let dir = repo(&[]);
    let message = error(&shapr_stdin(dir.path(), &["-J"], "hello\n"), 2);
    assert!(message.contains("not a diff"), "{message}");
}

#[test]
fn a_rename_without_hunks_on_stdin_is_a_diff() {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    git(dir.path(), &["mv", "a.rb", "b.rb"]);
    let diff = git(dir.path(), &["diff", "--staged", "-M"]);
    let out = shapr_stdin(dir.path(), &["-j"], &diff);
    assert_eq!(out.status.code(), Some(0));
    let report = report(&out);
    assert_eq!(report["summary"]["verdict"], "none");
}

#[test]
fn an_empty_range_has_no_verdict() {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    let out = shapr(dir.path(), &["-J", "main..main"]);
    let records = records(&out);
    assert_eq!(records.len(), 2, "a meta and a summary record");
    let summary = &records[1];
    assert_eq!(summary["verdict"], "none");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn outside_a_repository_is_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let message = error(&shapr(dir.path(), &["-J", "main..HEAD"]), 2);
    assert!(message.starts_with("not a git repository: "), "{message}");

    let elsewhere = repo(&[]);
    let target = dir.path().to_str().unwrap();
    let message = error(
        &shapr(elsewhere.path(), &["-J", "-C", target, "main..HEAD"]),
        2,
    );
    assert_eq!(message, format!("not a git repository: {target}"));
}

#[test]
fn a_range_that_isnt_a_revision_is_one_line() {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    // `..` names the parent directory: git would read it as a path.
    for (range, rev) in [("..", ".."), ("nope..HEAD", "nope..HEAD")] {
        let message = error(&shapr(dir.path(), &["-J", range]), 2);
        assert!(
            message.contains(&format!("bad revision '{rev}'")),
            "{message}"
        );
    }
}

#[test]
fn a_usage_error_message_has_no_error_prefix() {
    let dir = repo(&[]);
    let out = shapr(dir.path(), &["-J", "--bogus"]);
    assert_eq!(out.status.code(), Some(64));
    let record = report(&out);
    let message = record["error"].as_str().unwrap();
    assert!(message.starts_with("unexpected argument"), "{message}");
}

#[test]
fn quiet_writes_no_error_record() {
    let dir = repo(&[]);
    for args in [&["-q", "-J"][..], &["-qJ", "--bogus"]] {
        let out = shapr(dir.path(), args);
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
    let out = shapr(dir.path(), &["-j", "HEAD~1..HEAD"]);
    let report = report(&out);
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
    let out = shapr(dir.path(), &["HEAD~1..HEAD"]);
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
    let out = shapr(dir.path(), &["-q", "-v", "HEAD~1..HEAD"]);
    let log = String::from_utf8_lossy(&out.stderr);
    assert!(log.contains("`guard` not an extract"), "{log}");
}

#[test]
fn deeply_nested_code_is_classified_or_not_compared() {
    let sum = |n: usize| vec!["1"; n].join(" + ");
    let blocks = |n: usize| format!("fn f() {{ {}{} }}\n", "{ ".repeat(n), "}".repeat(n));
    // (path, before, comment, nested past the limit)
    let cases = [
        ("a.rb", format!("x = {}\n", sum(50_000)), "# c\n", true),
        (
            "lib.rs",
            format!("fn f() -> i32 {{ {} }}\n", sum(10_000)),
            "// c\n",
            true,
        ),
        (
            "lib.rs",
            format!("fn f() -> i32 {{ {} }}\n", sum(5_000)),
            "// c\n",
            false,
        ),
        ("lib.rs", blocks(5_000), "// c\n", false),
    ];
    for (path, before, comment, too_deep) in cases {
        let after = format!("{before}{comment}");
        let report = classify(&[(path, &before)], &[(path, &after)]);
        let hunk = &hunks(&report)[0];
        assert_eq!(hunk["parsed"], !too_deep, "{path} {}", lead(hunk)["reason"]);
        if too_deep {
            assert_eq!(hunk["verdict"], "unknown", "{path}");
            let reason = lead(hunk)["reason"].as_str().unwrap();
            assert!(reason.contains("nested deeper than"), "{reason}");
        }
    }
}
