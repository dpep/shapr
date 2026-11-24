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
    let message = error(&shapr(dir.path(), &["-J", "nope..HEAD"]), 2);
    assert_eq!(message, "nope..HEAD: no such revision");
    // `..` names the parent directory, a path outside the repository.
    let message = error(&shapr(dir.path(), &["-J", ".."]), 2);
    assert!(message.contains("outside repository"), "{message}");
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
    assert!(
        text.contains("review 1 line") && !text.contains("review 1 lines"),
        "{text}"
    );
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

/// A merge commit whose result differs from both parents (an evil merge),
/// and one that differs from neither in any line git's combined diff shows.
fn merges() -> tempfile::TempDir {
    let dir = repo(&[&[("a.rb", "x = 1\n"), ("b.rb", "y = 1\n")]]);
    git(dir.path(), &["checkout", "-q", "-b", "feat"]);
    commit(dir.path(), &[("a.rb", "x = 2\n")], "feat");
    git(dir.path(), &["checkout", "-q", "main"]);
    commit(dir.path(), &[("a.rb", "x = 3\n")], "main");
    git(
        dir.path(),
        &["merge", "-q", "feat", "-s", "ours", "--no-edit"],
    );
    write(dir.path(), &[("a.rb", "x = 4\n")]);
    git(dir.path(), &["commit", "-q", "-a", "--amend", "--no-edit"]);
    git(dir.path(), &["checkout", "-q", "-b", "side", "HEAD~1"]);
    commit(dir.path(), &[("b.rb", "y = 2\n")], "side");
    git(dir.path(), &["checkout", "-q", "main"]);
    git(dir.path(), &["merge", "-q", "side", "--no-edit"]);
    dir
}

#[test]
fn a_merge_against_its_parents_is_an_error_naming_one_parent() {
    let dir = merges();
    for (arg, rev) in [
        ("HEAD", "HEAD"),
        ("HEAD~1", "HEAD~1"),
        ("HEAD^!", "HEAD"),
        ("HEAD~1^!", "HEAD~1"),
    ] {
        let message = error(&shapr(dir.path(), &["-J", arg]), 2);
        assert_eq!(
            message,
            format!(
                "{rev} is a merge: diff it against one parent, {rev}^1..{rev} or {rev}^2..{rev}"
            ),
            "{arg}"
        );
    }
    // A first-parent diff of the same merge is an ordinary diff.
    assert_eq!(shapr(dir.path(), &["-J", "HEAD^-1"]).status.code(), Some(0));
}

#[test]
fn a_merge_as_a_combined_diff_on_stdin_is_an_error() {
    let dir = merges();
    let diff = git(dir.path(), &["diff", "HEAD~1^!"]);
    assert!(diff.starts_with("diff --cc "), "{diff}");
    let message = error(&shapr_stdin(dir.path(), &["-J"], &diff), 2);
    assert!(message.contains("merge"), "{message}");
}

#[test]
fn a_usage_error_keeps_the_whole_message() {
    let dir = repo(&[]);
    // clap prints its own usage to stderr; the record carries its message.
    let out = shapr(dir.path(), &["-J", "norms", "--top"]);
    assert_eq!(out.status.code(), Some(64));
    let message = report(&out)["error"].as_str().unwrap().to_string();
    assert!(message.contains("--top <N>"), "{message}");
}

// Untrusted repositories: content and config a pull request controls.

/// A repository whose config names programs git would run, and the marker
/// file they'd leave.
fn armed() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    write(
        dir.path(),
        &[("a.rb", "def greet(other)\n  \"hi #{other}\"\nend\n")],
    );
    let marker = dir.path().join(".git/ran");
    let script = dir.path().join(".git/run.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let script = script.to_str().unwrap();
    git(dir.path(), &["config", "core.fsmonitor", script]);
    let hooks = dir.path().join(".git/hooks");
    for hook in ["reference-transaction", "post-index-change"] {
        std::fs::copy(script, hooks.join(hook)).unwrap();
    }
    (dir, marker)
}

#[test]
fn a_repositorys_config_runs_no_program() {
    let (dir, marker) = armed();
    for args in [&["-J", "."][..], &["-J", "HEAD"], &["-J", "--staged"]] {
        let out = shapr(dir.path(), args);
        assert_eq!(out.status.code(), Some(0), "{args:?} {out:?}");
        assert!(!marker.exists(), "{args:?} ran the repository's program");
    }
}

#[test]
fn a_revision_that_looks_like_an_option_is_refused() {
    let dir = repo(&[&[("a.rb", METHOD)], &[("a.rb", "def greet\nend\n")]]);
    let written = dir.path().join("written");
    let flag = format!("--merge-base=--output={}", written.display());
    for args in [&["-J", &flag][..], &["-J", &flag, "HEAD"]] {
        let message = error(&shapr(dir.path(), args), 64);
        assert!(message.contains("starts with `-`"), "{message}");
    }
    assert!(!dir.path().join("written").exists());
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("written")
    }));
}
