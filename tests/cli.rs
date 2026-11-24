//! End to end: the built binary against throwaway git repos.

use serde_json::Value;
use std::io::Write;
use std::process::Stdio;

mod common;
use common::*;

const METHOD: &str = "class Greeter\n  def greet(name)\n    \"hi #{name}\"\n  end\nend\n";

#[test]
fn reformatting_is_format_only() {
    let after = "class Greeter\n  def greet( name )\n      \"hi #{name}\"\n  end\nend\n";
    let report = classify(&[("a.rb", METHOD)], &[("a.rb", after)]);
    assert_eq!(kinds(&report), ["format_only"]);
    let hunk = &report["hunks"][0];
    assert_eq!(lead(hunk)["confidence"], 1.0);
    assert_eq!(hunk["parsed"], true);
    assert_eq!(lead(hunk)["evidence"]["comparison"], "ast");
    assert_eq!(lead(hunk)["evidence"].get("method"), None);
    assert_eq!(hunk["review_lines"], 0);
    assert_eq!(report["summary"]["verdict"], "structure");
    assert_eq!(report["summary"]["structure_share"], 1.0);
}

#[test]
fn rewording_a_comment_is_comment_only_and_counted() {
    let before = "# Says hello.\ndef greet\n  1 # the answer\nend\n";
    let after = "# Says hello politely.\n# Twice.\ndef greet\n  1 # the answer\nend\n";
    let report = classify(&[("a.rb", before)], &[("a.rb", after)]);
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
}

#[test]
fn a_magic_comment_is_code() {
    let body = "def greet\n  \"hi\"\nend\n";
    let before = format!("# frozen_string_literal: true\n{body}");
    let after = format!("# frozen_string_literal: false\n{body}");
    let report = classify(&[("a.rb", &before)], &[("a.rb", &after)]);
    assert_eq!(kinds(&report), ["logic"]);
    assert_eq!(report["hunks"][0]["code_added"], 1);
}

#[test]
fn changing_code_is_logic_with_its_evidence() {
    let after = METHOD.replace("hi", "hello");
    let report = classify(&[("a.rb", METHOD)], &[("a.rb", &after)]);
    assert_eq!(kinds(&report), ["logic"]);
    let hunk = &report["hunks"][0];
    assert_eq!(hunk["review_lines"], 2);
    assert!(lead(hunk)["evidence"]["code_similarity"].as_f64().unwrap() < 1.0);
    assert_eq!(report["summary"]["verdict"], "behavior");
    assert_eq!(report["summary"]["behavior_lines"], 2);
}

#[test]
fn whitespace_inside_a_string_is_logic() {
    let after = METHOD.replace("hi #", "hi  #");
    let report = classify(&[("a.rb", METHOD)], &[("a.rb", &after)]);
    assert_eq!(kinds(&report), ["logic"]);
}

#[test]
fn format_and_logic_in_one_change_is_mixed() {
    let filler = "X = 1\n".repeat(10);
    let before = format!("def a\n  1+1\nend\n{filler}def b\n  2\nend\n");
    let after = format!("def a\n  1 + 1\nend\n{filler}def b\n  3\nend\n");
    let report = classify(&[("lib/a.rb", &before)], &[("lib/a.rb", &after)]);
    assert_eq!(kinds(&report), ["format_only", "logic"]);
    let summary = &report["summary"];
    assert_eq!(summary["verdict"], "mixed");
    assert_eq!(summary["kinds"]["format_only"], 1);
    assert_eq!(summary["review_lines"], 2);
    assert_eq!(summary["structure_share"], 0.5);
    assert_eq!(report["files"][0]["lines_before"], 16);
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
    let report = classify(&[("a.rb", before)], &[("a.rb", after)]);
    assert!(kinds(&report).iter().all(|k| *k == "extract"), "{report:#}");
    // Every changed code line: git's diff keeps the moved statements as
    // context and changes the lines around them.
    assert_eq!(report["summary"]["review_lines"], 5);
    assert_eq!(report["summary"]["behavior_lines"], 0);
    assert_eq!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_moved_method_with_an_edit_reviews_only_the_changed_lines() {
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
    let report = classify(&[("a.rb", &before)], &[("a.rb", &after)]);
    // Moved and edited: a move part and the edited line as logic.
    assert_eq!(kinds(&report), ["logic", "logic"], "{report:#}");
    // `log(y)` out of the removed body, `warn(y)` into the added one.
    assert_eq!(review_lines(&report), [1, 1]);
    assert_eq!(report["summary"]["behavior_lines"], 2);
}

#[test]
fn a_lockfile_is_generated_and_not_counted_in_size() {
    let reformatted = METHOD.replace("greet(name)", "greet( name )");
    let report = classify(
        &[
            ("a.rb", METHOD),
            ("Gemfile.lock", "rack (2.0)\nrake (13.0)\n"),
        ],
        &[
            ("a.rb", &reformatted),
            ("Gemfile.lock", "rack (3.0)\nrake (13.1)\n"),
        ],
    );
    assert_eq!(kinds(&report), ["logic", "format_only"]);
    let lock = &report["hunks"][0];
    assert_eq!(lock["category"], "generated");
    assert_eq!(lock["added"], 2);
    assert_eq!(lock["review_lines"], 0);
    assert_eq!(report["files"][0]["category"], "generated");
    assert_eq!(report["hunks"][1]["category"], "code");
    let summary = &report["summary"];
    assert_eq!(summary["categories"]["generated"], 1);
    assert_eq!(summary["categories"]["code"], 1);
    assert_eq!(
        (summary["added"].as_u64(), summary["review_lines"].as_u64()),
        (Some(1), Some(0))
    );
    assert_eq!(summary["behavior_lines"], 0);
    assert_eq!(summary["structure_share"], 1.0);
    // Generated files are outside the verdict.
    assert_eq!(lock["verdict"], "unknown");
    assert_eq!(summary["verdict"], "structure");
}

/// Each file's path and category.
fn categories(report: &Value) -> Vec<(&str, &str)> {
    report["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["file"].as_str().unwrap(), f["category"].as_str().unwrap()))
        .collect()
}

#[test]
fn every_file_has_one_category() {
    let attributes = "gen/** linguist-generated\nyarn.lock -linguist-generated\nthird/** linguist-vendored\nguide.html linguist-documentation\n";
    let files = |n: &str| -> Vec<(&'static str, String)> {
        vec![
            (".gitattributes", attributes.to_string()),
            ("lib/a.rb", format!("X = {n}\n")),
            ("spec/a_spec.rb", format!("X = {n}\n")),
            ("src/widget.test.ts", format!("x({n})\n")),
            ("README.md", format!("v{n}\n")),
            ("docs/setup.html", format!("<p>{n}</p>\n")),
            ("guide.html", format!("<p>{n}</p>\n")),
            ("config/app.yml", format!("x: {n}\n")),
            (".env.test", format!("X={n}\n")),
            ("Gemfile", format!("gem \"a\", \"{n}\"\n")),
            ("package.json", format!("{{\"v\": {n}}}\n")),
            ("Gemfile.lock", format!("a ({n})\n")),
            ("gen/api.rb", format!("X = {n}\n")),
            ("yarn.lock", format!("a@{n}\n")),
            ("third/lib.js", format!("x({n})\n")),
            ("logo.png", format!("\0PNG{n}")),
            ("run.sh", format!("echo {n}\n")),
        ]
    };
    let (before, after) = (files("1"), files("2"));
    let borrow = |v: &'static [(&'static str, String)]| -> Vec<(&'static str, &'static str)> {
        v.iter().map(|(p, c)| (*p, c.as_str())).collect()
    };
    let (before, after) = (borrow(before.leak()), borrow(after.leak()));
    let report = classify(&before, &after);
    let mut got = categories(&report);
    got.sort();
    assert_eq!(
        got,
        [
            (".env.test", "config"),
            ("Gemfile", "manifest"),
            ("Gemfile.lock", "generated"),
            ("README.md", "docs"),
            ("config/app.yml", "config"),
            ("docs/setup.html", "docs"),
            ("gen/api.rb", "generated"),
            ("guide.html", "docs"),
            ("lib/a.rb", "code"),
            ("logo.png", "binary"),
            ("package.json", "manifest"),
            ("run.sh", "unknown"),
            ("spec/a_spec.rb", "test"),
            ("src/widget.test.ts", "test"),
            ("third/lib.js", "vendored"),
            ("yarn.lock", "unknown"),
        ]
    );
    // Category is orthogonal to parsing: a test file and a Gemfile are parsed.
    let file = |path: &str| {
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["file"] == path)
            .unwrap()
            .clone()
    };
    assert_eq!(file("spec/a_spec.rb")["language"], "ruby");
    assert_eq!(file("spec/a_spec.rb")["parsed"], true);
    assert_eq!(file("Gemfile")["language"], "ruby");
    assert_eq!(file("run.sh")["language"], Value::Null);
}

#[test]
fn a_syntax_error_falls_back_to_lines_and_proves_no_structure() {
    let before = "def a\n  1+1\nend\ndef broken(\n";
    let after = "def a\n  1 + 1\nend\ndef broken(\n";
    let report = classify(&[("a.rb", before)], &[("a.rb", after)]);
    let hunk = &report["hunks"][0];
    assert_eq!(hunk["kind"], "logic");
    assert_eq!(lead(hunk)["detail"]["near"], "format_only");
    assert_eq!(hunk["parsed"], false);
    assert_eq!(lead(hunk)["evidence"]["comparison"], "line");
    assert_eq!(lead(hunk)["confidence"], 0.0);
    assert_eq!(report["summary"]["unparsed_hunks"], 1);
}

#[test]
fn an_unparsed_language_is_logic_outside_the_verdict() {
    let report = classify(&[("run.sh", "echo a\n")], &[("run.sh", "echo b\n")]);
    let hunk = &report["hunks"][0];
    assert_eq!(hunk["kind"], "logic");
    assert_eq!(hunk["verdict"], "unknown");
    assert_eq!(lead(hunk)["confidence"], 0.0);
    assert_eq!(hunk["language"], Value::Null);
    assert_eq!(lead(hunk)["evidence"]["comparison"], "none");
    assert_eq!(report["summary"]["verdict"], "unknown");
}

#[test]
fn a_readme_only_change_has_no_verdict() {
    let report = classify(&[("README.md", "# A\n")], &[("README.md", "# B\n")]);
    let hunk = &report["hunks"][0];
    assert_eq!(hunk["category"], "docs");
    assert_eq!(hunk["verdict"], "none");
    assert_eq!(hunk["review_lines"], 0);
    let summary = &report["summary"];
    assert_eq!(summary["verdict"], "none");
    assert_eq!(summary["verdicts"]["none"], 1);
    assert_eq!(summary["behavior_lines"], 0);
}

#[test]
fn a_docs_edit_doesnt_flip_a_structure_verdict() {
    let reformatted = METHOD.replace("greet(name)", "greet( name )");
    let report = classify(
        &[("a.rb", METHOD), ("README.md", "# A\n")],
        &[("a.rb", &reformatted), ("README.md", "# B\n")],
    );
    assert_eq!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_config_change_is_behavior() {
    let report = classify(
        &[("config/app.yml", "port: 1\n")],
        &[("config/app.yml", "port: 2\n")],
    );
    assert_eq!(report["hunks"][0]["category"], "config");
    assert_eq!(report["hunks"][0]["verdict"], "behavior");
    assert_eq!(report["summary"]["verdict"], "behavior");
    assert_eq!(report["summary"]["review_lines"], 2);
}

#[test]
fn trailing_whitespace_and_blank_lines_in_an_unparsed_file_are_neutral() {
    let report = classify(
        &[("config/app.yml", "a: 1\nb: 2\n")],
        &[("config/app.yml", "a: 1  \n\nb: 2\n")],
    );
    assert_eq!(report["hunks"][0]["verdict"], "none");
    assert_eq!(report["summary"]["verdict"], "none");
    // Indentation is not neutral: YAML, Python and Makefiles read it.
    let report = classify(
        &[("config/app.yml", "a:\n  b: 1\n")],
        &[("config/app.yml", "a:\nb: 1\n")],
    );
    assert_eq!(report["summary"]["verdict"], "behavior");
}

#[test]
fn a_lockfile_alone_is_unknown() {
    let report = classify(
        &[("Gemfile.lock", "a (1)\n")],
        &[("Gemfile.lock", "a (2)\n")],
    );
    assert_eq!(report["hunks"][0]["verdict"], "unknown");
    assert_eq!(report["summary"]["verdict"], "unknown");
    assert_eq!(report["summary"]["verdicts"]["unknown"], 1);
}

#[test]
fn test_review_lines_are_split_out() {
    let spec = "RSpec.describe Greeter do\n  it { expect(1).to eq(1) }\nend\n";
    let report = classify(
        &[("lib/a.rb", METHOD), ("spec/a_spec.rb", spec)],
        &[
            ("lib/a.rb", &METHOD.replace("hi", "yo")),
            ("spec/a_spec.rb", &spec.replace("eq(1)", "eq(2)")),
        ],
    );
    let summary = &report["summary"];
    assert_eq!(summary["review_lines"], 4);
    assert_eq!(summary["test_review_lines"], 2);
    assert_eq!(summary["verdict"], "behavior");
}

#[test]
fn a_piped_diff_matches_the_range() {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    // Uncommitted: the after blob is only in the working tree.
    write(dir.path(), &[("a.rb", &METHOD.replace("  end", "    end"))]);
    let diff = git(dir.path(), &["diff"]);
    let piped = shapr_stdin(dir.path(), &["-J"], &diff);
    let ranged = shapr(dir.path(), &["-J", "HEAD"]);
    assert_eq!(
        String::from_utf8_lossy(&piped.stdout),
        String::from_utf8_lossy(&ranged.stdout)
    );
    let lines = records(&piped);
    let records: Vec<&str> = lines
        .iter()
        .map(|l| l["record"].as_str().unwrap())
        .collect();
    assert_eq!(records, ["meta", "file", "hunk", "summary"]);
    assert_eq!(lines[2]["kind"], "format_only");
    assert_eq!(piped.status.code(), Some(0));
}

#[test]
fn a_meta_record_leads_with_the_schema_and_tool_versions() {
    let dir = repo(&[
        &[("a.rb", METHOD)],
        &[("a.rb", &METHOD.replace("hi", "yo"))],
    ]);
    let out = shapr(dir.path(), &["-J", "HEAD~1..HEAD"]);
    let meta = records(&out).remove(0);
    assert_eq!(meta["record"], "meta");
    assert_eq!(meta["schema"], 1);
    assert_eq!(meta["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(meta["source"], Value::Null);

    let out = shapr(dir.path(), &["-j", "HEAD~1..HEAD"]);
    let report = report(&out);
    assert_eq!(report["meta"], meta);
    assert_eq!(report["summary"].get("schema"), None);
}

#[test]
fn dash_c_reads_that_repository_whatever_git_dir_says() {
    let repo = repo(&[&[("a.rb", METHOD)]]);
    write(
        repo.path(),
        &[("a.rb", &METHOD.replace("  end", "    end"))],
    );
    let diff = git(repo.path(), &["diff"]);
    let elsewhere = tempfile::tempdir().unwrap();
    let dir = repo.path().to_str().unwrap();
    let run = |args: &[&str], stdin: &str| {
        let mut child = command(env!("CARGO_BIN_EXE_shapr"), elsewhere.path())
            .env("GIT_DIR", elsewhere.path().join("missing"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let report = report(&out);
        (report, out.status.code())
    };
    // A range, and a piped diff whose working-tree blob is read from disk.
    for (args, stdin) in [
        (vec!["-C", dir, "-j", "HEAD"], ""),
        (vec!["-C", dir, "-j"], diff.as_str()),
    ] {
        let (report, code) = run(&args, stdin);
        assert_eq!(kinds(&report), ["format_only"], "{args:?}");
        assert_eq!(code, Some(0));
    }

    let (error, code) = run(&["-C", "missing", "-J", "HEAD"], "");
    assert_eq!(error["record"], "error");
    assert_eq!(code, Some(64));
}

#[test]
fn errors_have_their_own_exit_codes() {
    let dir = repo(&[&[("a.rb", "1\n")]]);
    let bad_range = shapr(dir.path(), &["-J", "nope..HEAD"]);
    assert_eq!(bad_range.status.code(), Some(2));
    let error = report(&bad_range);
    assert_eq!(
        (error["record"].as_str(), error["code"].as_u64()),
        (Some("error"), Some(2))
    );
    assert_eq!(error["schema"], 1);

    let usage = shapr(dir.path(), &["-J", "--bogus"]);
    assert_eq!(usage.status.code(), Some(64));
    let error = report(&usage);
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
        let out = shapr(dir.path(), &["--completions", shell]);
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
    let report = classify(&[("a.rb", METHOD)], &[("a.rb", &after)]);
    let records = [
        &report["meta"],
        &report["files"][0],
        &report["hunks"][0],
        &report["hunks"][0]["parts"][0],
        &report["hunks"][0]["parts"][0]["evidence"],
        &report["hunks"][0]["parts"][0]["lines"],
        &report["hunks"][0]["before"],
        &report["summary"],
        &report["summary"]["kinds"],
        &report["summary"]["facets"],
        &report["summary"]["categories"],
        &report["summary"]["verdicts"],
    ];
    let quoted = "def a\n  x = 'a'\n  y = 'b'\n  z = 'c'\nend\n";
    let styled = classify(
        &[("b.rb", quoted)],
        &[("b.rb", &quoted.replace("end", "  w = \"d\"\nend"))],
    );
    let style = &styled["hunks"][0]["style"];
    let config = ".rubocop.yml";
    let rule = "Style/StringLiterals:\n  EnforcedStyle: single_quotes\n";
    let trailing = "def a\n  x = 'a'  \n  y = 'b'\n  z = \"c\"\nend\n";
    let dir = repo(&[&[(config, rule), ("b.rb", quoted)], &[("b.rb", trailing)]]);
    let changed = common::report(&shapr(dir.path(), &["-j", "HEAD~1..HEAD"]));
    let norms = common::report(&shapr(dir.path(), &["norms", "-j"]));
    let quotes = (norms["norms"].as_array().unwrap().iter())
        .find(|n| n["norm"] == "Style/StringLiterals")
        .unwrap();
    let records = records.into_iter().chain([
        style,
        &style["norms"][0],
        &style["norms"][0]["deviations"][0],
        &changed["summary"]["config_trajectory"][0],
        &changed["summary"]["config_trajectory"][0]["before"],
        &norms["files"][0],
        quotes,
        &quotes["config"],
        &quotes["deviations"][0],
        &norms["hotspots"][0],
        &norms["hotspots"][0]["lines"],
        &norms["steps"][0],
        &norms["steps"][0]["edits"][0],
        &norms["summary"],
        &norms["summary"]["skipped"],
    ]);
    for record in records {
        for key in record.as_object().unwrap().keys() {
            assert!(
                schema.contains(&format!("`{key}`")),
                "docs/schema.md doesn't name `{key}`"
            );
        }
    }
}

#[test]
fn git_range_syntax_passes_through() {
    let dir = repo(&[
        &[("a.rb", METHOD)],
        &[("a.rb", &METHOD.replace("  end", "    end"))],
    ]);
    let caret = shapr(dir.path(), &["-j", "HEAD^!"]);
    assert_eq!(caret.status.code(), Some(0), "{caret:?}");
    let ranged = shapr(dir.path(), &["-j", "HEAD~1..HEAD"]);
    assert_eq!(report(&caret)["hunks"], report(&ranged)["hunks"]);
    assert_eq!(report(&caret)["summary"], report(&ranged)["summary"]);
}

#[test]
fn global_format_flags_may_precede_norms() {
    let dir = repo(&[&[("a.rb", "A = 'a'\n")]]);
    let out = shapr(dir.path(), &["-J", "norms", "a.rb"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(records(&out).iter().any(|r| r["record"] == "summary"));
}

#[test]
fn help_and_usage_name_every_kind_and_both_commands() {
    let dir = tempfile::tempdir().unwrap();
    let help = shapr(dir.path(), &["--help"]);
    let first = String::from_utf8_lossy(&help.stdout)
        .lines()
        .next()
        .unwrap()
        .to_string();
    for kind in [
        "format", "comment", "restyle", "rename", "move", "extract", "inline", "logic",
    ] {
        assert!(first.contains(kind), "{first}");
    }
    let usage = shapr(dir.path(), &["-J", "--bogus"]);
    let stderr = String::from_utf8_lossy(&usage.stderr);
    assert!(!stderr.contains("shapr --ndjson"), "{stderr}");
    assert!(stderr.contains("shapr norms"), "{stderr}");
}

#[test]
fn a_merge_against_its_second_parent_starts_from_that_parent() {
    let code = "class A\n  def total\n    1\n  end\nend\n";
    let dir = repo(&[&[("lib/a.rb", code), ("lib/u.rb", "x = 1\n")]]);
    git(dir.path(), &["checkout", "-q", "-b", "feat"]);
    commit(dir.path(), &[("lib/u.rb", "x = 2\n")], "feat");
    git(dir.path(), &["checkout", "-q", "main"]);
    // The first parent marks the file generated while editing it.
    let edited = code.replace("1\n", "2\n");
    let marked = [
        (".gitattributes", "lib/a.rb linguist-generated\n"),
        ("lib/a.rb", edited.as_str()),
    ];
    commit(dir.path(), &marked, "main");
    git(dir.path(), &["merge", "-q", "feat", "--no-edit"]);
    let report = report(&shapr(dir.path(), &["-j", "HEAD^-2"]));
    let hunk = hunks(&report)
        .iter()
        .find(|h| h["file"] == "lib/a.rb")
        .unwrap();
    assert_eq!(hunk["category"], "code", "{report:#}");
    assert_eq!(report["summary"]["verdict"], "behavior");
}

#[test]
fn a_root_commit_against_its_parents_diffs_against_the_working_tree() {
    let dir = repo(&[&[("a.rb", METHOD)], &[("b.rb", "x = 1\n")]]);
    // git diffs a parentless R^! as R against the working tree.
    write(dir.path(), &[("a.rb", &METHOD.replace("greet", "welcome"))]);
    let caret = report(&shapr(dir.path(), &["-j", "HEAD~1^!"]));
    let plain = report(&shapr(dir.path(), &["-j", "HEAD~1"]));
    assert_eq!(caret["hunks"], plain["hunks"]);
}

#[test]
fn every_meta_record_names_its_command() {
    let dir = repo(&[
        &[("a.rb", METHOD)],
        &[("a.rb", &METHOD.replace("hi", "yo"))],
    ]);
    let classify = records(&shapr(dir.path(), &["-J", "HEAD~1..HEAD"])).remove(0);
    assert_eq!(classify["command"], "classify");
    let norms = records(&shapr(dir.path(), &["-J", "norms", "a.rb"])).remove(0);
    assert_eq!(norms["command"], "norms");
    assert_eq!(
        report(&shapr(dir.path(), &["-j", "norms", "a.rb"]))["meta"],
        norms
    );
}

#[test]
fn a_github_file_url_is_not_a_change() {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    for url in [
        "https://github.com/acme/widgets/blob/main/lib/a.rb",
        "https://github.com/acme/widgets/tree/main/lib",
        "github.com/acme/widgets/blob/1a2b3c4/a.rb",
    ] {
        let out = shapr(dir.path(), &["-J", url]);
        assert_eq!(out.status.code(), Some(64), "{url}: {out:?}");
        assert_eq!(
            report(&out)["error"],
            "a GitHub file isn't a change: give a pull request or commit, \
             or run shapr norms on a local checkout",
            "{url}"
        );
    }
}

#[test]
fn help_names_the_last_commit_form() {
    let dir = repo(&[]);
    let help = String::from_utf8(shapr(dir.path(), &["--help"]).stdout).unwrap();
    assert!(help.contains("shapr HEAD^!"), "{help}");
}

#[test]
fn dash_c_may_come_before_or_after_norms() {
    let dir = repo(&[&[("a.rb", METHOD)]]);
    let root = dir.path().to_str().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    for args in [
        &["-C", root, "-j", "norms", "a.rb"][..],
        &["-j", "norms", "-C", root, "a.rb"],
    ] {
        let out = shapr(elsewhere.path(), args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {out:?}");
        assert_eq!(report(&out)["summary"]["files"], 1, "{args:?}");
    }
}
