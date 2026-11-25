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

type Files = Vec<(&'static str, String)>;

/// Four reformatted lines in `lib/a.rb`, one changed line in `lib/b.rb`.
fn mostly_reformat() -> (Files, Files) {
    let a = |sep: &str| (1..=4).map(|n| format!("X{n} = {n}{sep}1\n")).collect();
    let b = |n: u8| format!("def f\n  {n}\nend\n");
    (
        vec![("lib/a.rb", a("+")), ("lib/b.rb", b(1))],
        vec![("lib/a.rb", a(" + ")), ("lib/b.rb", b(2))],
    )
}

fn pairs<'a>(files: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    files.iter().map(|(p, c)| (*p, c.as_str())).collect()
}

#[test]
fn a_mostly_refactor_change_names_the_logic_inside_it() {
    let (before, after) = mostly_reformat();
    let dir = repo(&[&pairs(&before), &pairs(&after)]);
    let summary = report(&shapr(dir.path(), &["-j", "HEAD~1..HEAD"]))["summary"].clone();
    assert_eq!(summary["refactor_lines"], 8);
    assert_eq!(summary["refactor_share"], 0.8);
    let text = String::from_utf8(shapr(dir.path(), &["HEAD"]).stdout).unwrap();
    assert_eq!(
        text.lines().nth(1),
        Some("refactor: 80% of changed lines; 2 logic lines inside (lib/b.rb:2)"),
        "{text}"
    );
    // Two thirds is enough; a hunk is named by its first line.
    let (mut before, mut after) = mostly_reformat();
    before[1].1 = "X = 1\nY = 1\n".into();
    after[1].1 = "X = 2\nY = 2\n".into();
    let dir = repo(&[&pairs(&before), &pairs(&after)]);
    let text = String::from_utf8(shapr(dir.path(), &["HEAD"]).stdout).unwrap();
    assert!(
        text.contains("refactor: 67% of changed lines; 4 logic lines inside (lib/b.rb:1)"),
        "{text}"
    );
}

#[test]
fn refactor_counts_parts_shapr_couldnt_prove_structure() {
    // Without a lockfile naming sorbet-runtime, a sig may be checked.
    let sig = |s: &str| {
        format!(
            "class A\n  extend T::Sig\n\n{s}  def a(x)\n    x\n  end\n\n{s}  def b(x)\n    x\n  end\nend\n"
        )
    };
    let (mut before, mut after) = mostly_reformat();
    before[0] = ("lib/a.rb", sig(""));
    after[0] = (
        "lib/a.rb",
        sig("  sig { params(x: Integer).returns(Integer) }\n"),
    );
    let report = classify(&pairs(&before), &pairs(&after));
    let summary = &report["summary"];
    assert_eq!(summary["structure_share"], 0.0, "{summary:#}");
    assert!(
        summary["refactor_share"].as_f64().unwrap() >= 0.5,
        "{summary:#}"
    );
}

#[test]
fn the_refactor_line_stays_out_when_it_says_nothing() {
    let text = |before: &[(&str, &str)], after: &[(&str, &str)]| {
        let dir = repo(&[before, after]);
        String::from_utf8(shapr(dir.path(), &["HEAD"]).stdout).unwrap()
    };
    // All refactor: nothing inside to point at.
    let (before, after) = mostly_reformat();
    let pure = text(&pairs(&before[..1]), &pairs(&after[..1]));
    assert!(!pure.contains("refactor:"), "{pure}");
    // Mostly logic: not a refactor with something inside it.
    let logic = text(&pairs(&before[1..]), &pairs(&after[1..]));
    assert!(!logic.contains("refactor:"), "{logic}");
    // Under two thirds refactor (8 changed lines of 14).
    let (mut before, mut after) = mostly_reformat();
    before[1].1 = format!("{}X = 1\nY = 1\n", before[1].1);
    after[1].1 = format!("{}X = 2\nY = 2\n", after[1].1);
    let under = text(&pairs(&before), &pairs(&after));
    assert!(!under.contains("refactor:"), "{under}");
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
    let ranged = shapr(dir.path(), &["-J", "."]);
    // All but meta, which names the input form.
    assert_eq!(records(&piped)[1..], records(&ranged)[1..]);
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
        (vec!["-C", dir, "-j", "."], ""),
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

/// `summary.kinds` counts the parts the verdict rolls up, as the text
/// summary does: a docs or unparsed file's residual isn't a logic change.
#[test]
fn kinds_count_parts_inside_the_verdict() {
    let report = classify(
        &[("README.md", "a\n"), ("Makefile", "a:\n"), ("a.rb", METHOD)],
        &[
            ("README.md", "b\n"),
            ("Makefile", "b:\n"),
            ("a.rb", &METHOD.replace("hi", "yo")),
        ],
    );
    assert_eq!(
        report["summary"]["kinds"]["logic"], 1,
        "{:#}",
        report["summary"]
    );
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
    for args in [&["-J", "norms", "a.rb"][..], &["-vJ", "norms", "a.rb"]] {
        let out = shapr(dir.path(), args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {out:?}");
        assert!(records(&out).iter().any(|r| r["record"] == "summary"));
    }
    let out = shapr(dir.path(), &["-qJ", "norms", "a.rb"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
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

/// A path limit that leaves out the `.gitattributes` a change edits still
/// reads each file's category under the attributes it started from.
#[test]
fn a_path_limit_keeps_the_before_attributes() {
    let code = "class A\n  def total\n    1\n  end\nend\n";
    let dir = repo(&[
        &[("lib/a.rb", code)],
        &[
            (".gitattributes", "*.rb linguist-generated\n"),
            ("lib/a.rb", &code.replace("1\n", "2\n")),
        ],
    ]);
    let report = report(&shapr(dir.path(), &["-j", "HEAD~1..HEAD", "--", "lib"]));
    assert_eq!(hunks(&report)[0]["category"], "code", "{report:#}");
    assert_eq!(report["summary"]["verdict"], "behavior");
}

#[test]
fn a_root_commit_against_its_parents_diffs_against_the_working_tree() {
    let dir = repo(&[&[("a.rb", METHOD)], &[("b.rb", "x = 1\n")]]);
    // git diffs a parentless R^! as R against the working tree.
    write(dir.path(), &[("a.rb", &METHOD.replace("greet", "welcome"))]);
    let caret = report(&shapr(dir.path(), &["-j", "HEAD~1^!"]));
    let plain = report(&shapr(dir.path(), &["-j", "--merge-base", "HEAD~1"]));
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
    assert!(help.contains("shapr HEAD  "), "{help}");
}

// The input grammar: each form is a git command (README, Usage).

/// `a.rb` and `lib/b.rb` on main, a branch `feat` off it with one commit
/// changing both, and an uncommitted edit to each.
fn branched() -> tempfile::TempDir {
    let dir = repo(&[&[("a.rb", METHOD), ("lib/b.rb", "B = 1\n")]]);
    git(dir.path(), &["checkout", "-q", "-b", "feat"]);
    commit(
        dir.path(),
        &[
            ("a.rb", &METHOD.replace("hi", "yo")),
            ("lib/b.rb", "B = 2\n"),
        ],
        "feat: both",
    );
    write(
        dir.path(),
        &[
            ("a.rb", &METHOD.replace("hi", "hey")),
            ("lib/b.rb", "B = 3\n"),
        ],
    );
    dir
}

/// The files a run's hunks are in, and its meta record.
fn ran(dir: &std::path::Path, args: &[&str]) -> (Vec<String>, Value) {
    let mut all = vec!["-j"];
    all.extend(args);
    let out = shapr(dir, &all);
    assert_eq!(out.status.code(), Some(0), "{args:?}: {out:?}");
    let report = report(&out);
    let files = hunks(&report)
        .iter()
        .map(|h| h["file"].as_str().unwrap().to_string())
        .collect();
    (files, report["meta"].clone())
}

/// The error record's message and code.
fn refused(dir: &std::path::Path, args: &[&str]) -> (String, i64) {
    let mut all = vec!["-J"];
    all.extend(args);
    let out = shapr(dir, &all);
    let record = report(&out);
    assert_eq!(record["record"], "error", "{args:?}");
    assert_eq!(out.status.code().map(i64::from), record["code"].as_i64());
    (
        record["error"].as_str().unwrap().to_string(),
        record["code"].as_i64().unwrap(),
    )
}

#[test]
fn one_revision_is_that_commit_against_its_parent() {
    let dir = branched();
    let one = report(&shapr(dir.path(), &["-j", "feat"]));
    let range = report(&shapr(dir.path(), &["-j", "feat^..feat"]));
    assert_eq!(one["hunks"], range["hunks"]);
    assert_eq!(one["summary"]["claims"]["messages"], 1);
    assert_eq!(one["meta"]["input"], "commit");
    assert_eq!(one["meta"]["paths"], false);
}

#[test]
fn a_root_commit_alone_is_everything_it_added() {
    let dir = repo(&[&[("a.rb", METHOD)], &[("b.rb", "x = 1\n")]]);
    write(dir.path(), &[("a.rb", "x = 2\n")]);
    let report = report(&shapr(dir.path(), &["-j", "HEAD~1"]));
    let hunk = &hunks(&report)[0];
    assert_eq!(hunks(&report).len(), 1, "{report:#}");
    assert_eq!(hunk["file"], "a.rb");
    assert_eq!(hunk["deleted"], 0, "{hunk:#}");
    assert_eq!(report["summary"]["claims"]["messages"], 1);
}

#[test]
fn two_revisions_are_the_range_between_them() {
    let dir = branched();
    let two = report(&shapr(dir.path(), &["-j", "main", "feat"]));
    let range = report(&shapr(dir.path(), &["-j", "main..feat"]));
    assert_eq!(two["hunks"], range["hunks"]);
    assert_eq!(two["meta"]["input"], "range");
    let (message, code) = refused(dir.path(), &["main", "feat", "HEAD"]);
    assert_eq!(code, 64);
    assert!(message.contains("--"), "{message}");
}

#[test]
fn merge_base_is_the_branch_against_where_it_left_main() {
    let dir = branched();
    // No second revision: committed and uncommitted work on the branch.
    let (files, meta) = ran(dir.path(), &["--merge-base", "main"]);
    assert_eq!(files, ["a.rb", "lib/b.rb"]);
    assert_eq!(meta["input"], "merge_base_worktree");
    let worktree = report(&shapr(dir.path(), &["-j", "--merge-base", "main"]));
    let piped = shapr_stdin(dir.path(), &["-j"], &git(dir.path(), &["diff", "main"]));
    assert_eq!(worktree["hunks"], report(&piped)["hunks"]);
    assert_eq!(worktree["summary"]["claims"]["messages"], 1);
    // With one: `main...feat`.
    let both = report(&shapr(dir.path(), &["-j", "--merge-base", "main", "feat"]));
    let dots = report(&shapr(dir.path(), &["-j", "main...feat"]));
    assert_eq!(both["hunks"], dots["hunks"]);
    assert_eq!(both["meta"]["input"], "merge_base_range");
    assert_eq!(both["summary"]["claims"]["messages"], 1);

    let out = shapr(dir.path(), &["-J", "--merge-base"]);
    assert_eq!(out.status.code(), Some(64), "{out:?}");
    let (_, code) = refused(dir.path(), &["--merge-base", "main", "feat", "HEAD"]);
    assert_eq!(code, 64);
}

#[test]
fn paths_alone_are_the_uncommitted_changes_to_them() {
    let dir = branched();
    let (files, meta) = ran(dir.path(), &["a.rb"]);
    assert_eq!(files, ["a.rb"]);
    assert_eq!(
        (meta["input"].as_str(), meta["paths"].as_bool()),
        (Some("worktree"), Some(true))
    );
    let (files, _) = ran(dir.path(), &["."]);
    assert_eq!(files, ["a.rb", "lib/b.rb"]);
    let staged = git(dir.path(), &["diff", "HEAD"]);
    let piped = shapr_stdin(dir.path(), &["-j"], &staged);
    let dotted = report(&shapr(dir.path(), &["-j", "."]));
    assert_eq!(report(&piped)["hunks"], dotted["hunks"]);
    // From a subdirectory (-C), paths are relative to it, as git reads them.
    let (files, _) = ran(dir.path(), &["-C", "lib", "b.rb"]);
    assert_eq!(files, ["lib/b.rb"]);
}

#[test]
fn every_form_takes_paths_after_its_revisions() {
    let dir = branched();
    for args in [
        &["feat", "--", "lib"][..],
        &["feat", "lib"],
        &["main..feat", "--", "lib/b.rb"],
        &["main", "feat", "lib"],
        &["--merge-base", "main", "lib"],
        &["--merge-base", "main", "feat", "--", "lib"],
        &["--", "lib"],
    ] {
        let (files, meta) = ran(dir.path(), args);
        assert_eq!(files, ["lib/b.rb"], "{args:?}");
        assert_eq!(meta["paths"], true, "{args:?}");
    }
}

#[test]
fn a_name_that_is_a_revision_and_a_path_needs_dashes() {
    let dir = branched();
    git(dir.path(), &["branch", "lib", "main"]);
    let (message, code) = refused(dir.path(), &["lib"]);
    assert_eq!(code, 64);
    // Both readings, each ready to run.
    assert_eq!(
        message,
        "`lib` is both a revision and a path: `shapr lib --` for the revision, `shapr -- lib` for the path"
    );
    let (message, _) = refused(dir.path(), &["main", "lib"]);
    assert!(
        message.contains("`shapr main lib --` for the revision, `shapr main -- lib` for the path"),
        "{message}"
    );
    // `--` decides: after it a path, before it a revision.
    let (files, meta) = ran(dir.path(), &["--", "lib"]);
    assert_eq!(
        (files, meta["input"].as_str()),
        (vec!["lib/b.rb".to_string()], Some("worktree"))
    );
    let (_, meta) = ran(dir.path(), &["lib", "--"]);
    assert_eq!(meta["input"], "commit");
}

#[test]
fn a_name_that_is_neither_says_how_to_name_a_path() {
    let dir = branched();
    // Not ambiguous, and `--` wouldn't help.
    let (message, code) = refused(dir.path(), &["nope"]);
    assert_eq!(
        (message.as_str(), code),
        ("nope: no such revision or path", 2)
    );
    let (message, code) = refused(dir.path(), &["main", "nope"]);
    assert_eq!(
        (message.as_str(), code),
        ("nope: no such revision or path", 2)
    );
    let (message, code) = refused(dir.path(), &["--merge-base", "nope"]);
    assert_eq!((message.as_str(), code), ("nope: no such revision", 2));
    // A revision after a path.
    let (message, code) = refused(dir.path(), &["a.rb", "main"]);
    assert_eq!(code, 64);
    assert!(message.contains("main"), "{message}");
}

#[test]
fn meta_names_the_input_form() {
    let dir = branched();
    let piped = shapr_stdin(dir.path(), &["-j"], &git(dir.path(), &["diff"]));
    let meta = &report(&piped)["meta"];
    assert_eq!(
        (meta["input"].as_str(), meta["paths"].as_bool()),
        (Some("stdin"), Some(false))
    );
    assert_eq!(ran(dir.path(), &["main...feat"]).1["input"], "range");
    assert_eq!(ran(dir.path(), &["HEAD^!"]).1["input"], "range");
    let norms = report(&shapr(dir.path(), &["-j", "norms", "a.rb"]));
    assert_eq!(norms["meta"]["input"], Value::Null);
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

// The usage log.

/// shapr in DIR with ENV, stdin empty.
fn shapr_env(dir: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = command(env!("CARGO_BIN_EXE_shapr"), dir);
    cmd.args(args).stdin(Stdio::null());
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output().unwrap()
}

#[test]
fn each_run_appends_names_and_counts_never_paths_or_revisions() {
    let dir = branched();
    let state = tempfile::tempdir().unwrap();
    let log = state.path().join("usage.jsonl");
    let env = [("SHAPR_USAGE", log.to_str().unwrap())];
    let message = state.path().join("secret-description.md");
    std::fs::write(&message, "fix: the secret thing\n").unwrap();
    let root = dir.path().to_str().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let args = [
        "-j",
        "-C",
        root,
        "--message",
        message.to_str().unwrap(),
        "main..feat",
        "--",
        "lib",
    ];
    assert_eq!(
        shapr_env(elsewhere.path(), &args, &env).status.code(),
        Some(0)
    );
    assert_eq!(
        shapr_env(dir.path(), &["-qJ", "norms", "a.rb"], &env)
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        shapr_env(dir.path(), &["nope"], &env).status.code(),
        Some(2)
    );
    // Neither --usage nor --status is a run worth logging.
    shapr_env(dir.path(), &["--usage"], &env);

    let text = std::fs::read_to_string(&log).unwrap();
    for secret in [root, "feat", "lib", "secret", "nope", "a.rb"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3, "{text}");
    let classify = &lines[0];
    assert_eq!(classify["command"], "classify");
    assert_eq!(classify["input"], "range");
    assert_eq!(classify["paths"], true);
    assert_eq!(
        classify["flags"],
        serde_json::json!(["dir", "json", "message"])
    );
    assert_eq!(classify["format"], "json");
    assert_eq!(classify["exit"], 0);
    assert_eq!(classify["counts"]["files"], 1);
    assert_eq!(classify["counts"]["verdict"], "behavior");
    assert_eq!(classify["counts"]["kinds"]["logic"], 1);
    assert!(classify["ts"].as_str().unwrap().ends_with('Z'));
    assert!(classify["duration_ms"].is_u64());
    assert_eq!(classify["version"], env!("CARGO_PKG_VERSION"));
    let norms = &lines[1];
    assert_eq!(
        (norms["command"].as_str(), norms["format"].as_str()),
        (Some("norms"), Some("ndjson"))
    );
    assert_eq!(norms["flags"], serde_json::json!(["ndjson", "quiet"]));
    assert_eq!(norms["counts"]["files"], 1);
    assert!(norms["counts"]["steps"].is_u64());
    assert_eq!(
        (lines[2]["exit"].as_u64(), lines[2]["input"].clone()),
        (Some(2), Value::Null)
    );
}

#[test]
fn usage_summarizes_the_log_and_reset_empties_it() {
    let dir = branched();
    let state = tempfile::tempdir().unwrap();
    let log = state.path().join("usage.jsonl");
    let env = [("SHAPR_USAGE", log.to_str().unwrap())];
    for args in [
        &["-j", "feat"][..],
        &["-q", "."],
        &["-q", "--merge-base", "main"],
    ] {
        assert_eq!(
            shapr_env(dir.path(), args, &env).status.code(),
            Some(0),
            "{args:?}"
        );
    }
    let summary = report(&shapr_env(dir.path(), &["-j", "--usage"], &env));
    assert_eq!(summary["record"], "usage");
    assert_eq!(summary["runs"], 3);
    assert_eq!(summary["commands"]["classify"], 3);
    assert_eq!(
        summary["inputs"],
        serde_json::json!({"commit": 1, "merge_base_worktree": 1, "worktree": 1})
    );
    assert_eq!(
        summary["flags"],
        serde_json::json!({"json": 1, "merge-base": 1, "quiet": 2})
    );
    assert_eq!(summary["verdicts"]["behavior"], 3);
    assert_eq!(summary["kinds"]["logic"], 6);
    assert!(summary["first"].as_str() <= summary["last"].as_str());
    let text = String::from_utf8(shapr_env(dir.path(), &["--usage"], &env).stdout).unwrap();
    assert!(
        text.contains("3 runs") && text.contains("worktree 1"),
        "{text}"
    );

    let reset = records(&shapr_env(dir.path(), &["-J", "--usage", "--reset"], &env));
    assert_eq!(reset[0]["runs"], 0);
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
    // An empty log is one line, not a table of blank rows.
    let text = String::from_utf8(shapr_env(dir.path(), &["--usage"], &env).stdout).unwrap();
    assert_eq!(text, format!("no runs logged yet at {}\n", log.display()));
    assert_eq!(
        shapr_env(dir.path(), &["--reset"], &env).status.code(),
        Some(64)
    );
}

/// A log path that isn't a regular file (a FIFO would block the open) is
/// skipped, and the run finishes.
#[test]
fn a_log_path_that_isnt_a_file_is_skipped() {
    let dir = branched();
    let state = tempfile::tempdir().unwrap();
    let log = state.path().join("usage.jsonl");
    let made = std::process::Command::new("mkfifo")
        .arg(&log)
        .status()
        .unwrap();
    assert!(made.success());
    for args in [&["-q", "feat"][..], &["-j", "--usage"]] {
        let mut child = command(env!("CARGO_BIN_EXE_shapr"), dir.path())
            .args(args)
            .env("SHAPR_USAGE", &log)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if started.elapsed().as_secs() > 20 {
                child.kill().unwrap();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        assert_eq!(
            status.and_then(|s| s.code()),
            Some(0),
            "{args:?} hung or failed"
        );
    }
}

#[test]
fn the_log_defaults_to_xdg_state_and_status_shows_it() {
    let dir = branched();
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().to_str().unwrap();
    let env = [
        ("SHAPR_USAGE", "1"),
        ("HOME", home_path),
        ("XDG_STATE_HOME", ""),
    ];
    assert_eq!(
        shapr_env(dir.path(), &["-q", "feat"], &env).status.code(),
        Some(0)
    );
    let log = home.path().join(".local/state/shapr/usage.jsonl");
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    let status = report(&shapr_env(dir.path(), &["-j", "--status"], &env));
    assert_eq!(status["usage"]["path"], "~/.local/state/shapr/usage.jsonl");
    assert_eq!(status["usage"]["enabled"], true);
    assert!(status["usage"]["bytes"].as_u64().unwrap() > 0);
    let text = String::from_utf8(shapr_env(dir.path(), &["--status"], &env).stdout).unwrap();
    assert!(
        text.contains("usage log: ~/.local/state/shapr/usage.jsonl"),
        "{text}"
    );

    // Off: written nowhere, and said so.
    let off = [("SHAPR_USAGE", "0"), ("HOME", home_path)];
    shapr_env(dir.path(), &["-q", "feat"], &off);
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    let status = report(&shapr_env(dir.path(), &["-j", "--status"], &off));
    assert_eq!(status["usage"]["enabled"], false);
}

#[test]
fn shapr_usage_reads_the_usual_words_for_off_and_on() {
    let dir = branched();
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().to_str().unwrap();
    for (value, enabled) in [
        ("0", false),
        ("false", false),
        ("off", false),
        ("No", false),
        ("1", true),
        ("true", true),
        ("on", true),
        ("YES", true),
    ] {
        // Set explicitly, it wins over CI=true.
        let env = [("SHAPR_USAGE", value), ("HOME", home_path), ("CI", "true")];
        let status = report(&shapr_env(dir.path(), &["-j", "--status"], &env));
        assert_eq!(status["usage"]["enabled"], enabled, "SHAPR_USAGE={value}");
    }
}

#[test]
fn the_log_follows_no_symlink_and_is_private() {
    let dir = branched();
    let state = tempfile::tempdir().unwrap();
    let victim = state.path().join("victim");
    std::fs::write(&victim, "keep\n").unwrap();
    let link = state.path().join("usage.jsonl");
    std::os::unix::fs::symlink(&victim, &link).unwrap();
    let env = [("SHAPR_USAGE", link.to_str().unwrap())];
    let out = shapr_env(dir.path(), &["-q", "feat"], &env);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep\n");

    let log = state.path().join("new.jsonl");
    let env = [("SHAPR_USAGE", log.to_str().unwrap())];
    shapr_env(dir.path(), &["-q", "feat"], &env);
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&log).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn runs_trimming_the_log_at_once_keep_its_newest_half() {
    let dir = branched();
    let state = tempfile::tempdir().unwrap();
    let log = state.path().join("usage.jsonl");
    let line = "{\"command\":\"classify\",\"ts\":\"2026-01-01T00:00:00Z\"}\n";
    let full = line.repeat(5_100_000 / line.len());
    std::fs::write(&log, &full).unwrap();
    let env = [("SHAPR_USAGE", log.to_str().unwrap())];
    let runs: Vec<_> = (0..30)
        .map(|_| {
            let mut cmd = command(env!("CARGO_BIN_EXE_shapr"), dir.path());
            cmd.args(["-q", "feat"]).envs(env).stdin(Stdio::null());
            cmd.spawn().unwrap()
        })
        .collect();
    for mut run in runs {
        assert!(run.wait().unwrap().success());
    }
    let kept = std::fs::read_to_string(&log).unwrap();
    let half = full.len() / 2;
    assert!(
        kept.len() > half - line.len() && kept.len() < half + 30 * 1000,
        "{} of {}",
        kept.len(),
        full.len()
    );
    assert!(
        kept.lines()
            .all(|l| serde_json::from_str::<Value>(l).is_ok())
    );
    let names: Vec<_> = std::fs::read_dir(state.path()).unwrap().collect();
    assert_eq!(names.len(), 1, "{names:?}");
}

#[test]
fn a_log_that_cant_be_written_never_fails_the_run() {
    let dir = branched();
    let out = shapr_env(
        dir.path(),
        &["-j", "feat"],
        &[("SHAPR_USAGE", "/dev/null/usage.jsonl")],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
}

#[test]
fn the_text_summary_counts_kinds_and_leaves_out_what_says_nothing() {
    let dir = repo(&[
        &[("a.rb", "def f\n  1\nend\n")],
        &[("a.rb", "def f\n  2\nend\n")],
    ]);
    let out = shapr(dir.path(), &["HEAD"]);
    let text = String::from_utf8_lossy(&out.stdout);
    let last = text.lines().next().unwrap_or_default();
    assert!(last.starts_with("behavior: 1 hunk (1 logic)"), "{text}");
    assert!(!last.contains("comments"), "{text}");
    assert!(!last.contains("structure share"), "{text}");
    assert!(
        !text.contains("claims:"),
        "nothing claimed, no claims line: {text}"
    );
}

#[test]
fn the_text_kinds_count_only_hunks_inside_the_verdict() {
    let dir = repo(&[
        &[("a.rb", "def f\n  1\nend\n"), ("README.md", "hi\n")],
        &[("a.rb", "def f\n  2\nend\n"), ("README.md", "hello\n")],
    ]);
    let out = shapr(dir.path(), &["HEAD"]);
    let text = String::from_utf8_lossy(&out.stdout);
    let last = text.lines().next().unwrap_or_default();
    assert!(
        last.contains("behavior: 2 hunks (1 logic); outside the verdict: 1 none;"),
        "{text}"
    );
}

/// A class whose private method PRIVATE is called once, inside it.
fn meter(private: &str) -> String {
    format!(
        "class Meter\n  def level\n    {private} + 1\n  end\n\n  private\n\n  def {private}\n    41\n  end\nend\n"
    )
}

#[test]
fn staged_is_the_index_against_head_or_a_revision() {
    let dir = repo(&[
        &[("meter.rb", &meter("sample")), ("lib/b.rb", "B = 1\n")],
        &[("lib/b.rb", "B = 2\n")],
    ]);
    write(
        dir.path(),
        &[("meter.rb", &meter("probe")), ("lib/b.rb", "B = 3\n")],
    );
    git(dir.path(), &["add", "meter.rb", "lib/b.rb"]);
    // Unstaged edits aren't shown: the index is the after side.
    write(dir.path(), &[("lib/b.rb", "B = 4\n")]);
    let same = |args: &[&str], diff: &[&str]| {
        let mut all = vec!["-j"];
        all.extend(args);
        let out = shapr(dir.path(), &all);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {out:?}");
        let ours = report(&out);
        let piped = report(&shapr_stdin(dir.path(), &["-j"], &git(dir.path(), diff)));
        // The same lines; a diff on stdin from REV can't search the index for uses.
        let lines = |r: &Value| -> Vec<Value> {
            hunks(r)
                .iter()
                .map(|h| serde_json::json!([h["file"], h["before"], h["after"]]))
                .collect()
        };
        assert_eq!(lines(&ours), lines(&piped), "{args:?}");
        assert_eq!(ours["meta"]["input"], "staged", "{args:?}");
        ours
    };
    same(&["--staged"], &["diff", "--cached"]);
    same(&["--cached"], &["diff", "--cached"]);
    same(&["--staged", "HEAD~1"], &["diff", "--cached", "HEAD~1"]);
    let limited = same(
        &["--staged", "--", "lib"],
        &["diff", "--cached", "--", "lib"],
    );
    assert_eq!(limited["meta"]["paths"], true);
    let limited = same(&["--staged", "lib"], &["diff", "--cached", "--", "lib"]);
    assert_eq!(limited["meta"]["paths"], true);

    // Uses are searched in the index: an unstaged call doesn't count.
    write(
        dir.path(),
        &[(
            "report.rb",
            "class Report\n  def show\n    Meter.new.sample\n  end\nend\n",
        )],
    );
    let out = report(&shapr(dir.path(), &["-j", "--staged", "--", "meter.rb"]));
    let parts: Vec<&Value> = hunks(&out)
        .iter()
        .flat_map(|h| h["parts"].as_array().unwrap())
        .collect();
    assert!(
        parts
            .iter()
            .any(|p| p["kind"] == "rename" && p["structure"] == true),
        "{out:#}"
    );

    for args in [
        &["--staged", "HEAD~1..HEAD"][..],
        &["--staged", "HEAD~1", "HEAD"],
        &["--staged", "--merge-base", "main"],
    ] {
        let (_, code) = refused(dir.path(), args);
        assert_eq!(code, 64, "{args:?}");
    }
}

#[test]
fn the_text_summary_names_what_shapr_chose_to_compare() {
    let dir = branched();
    git(dir.path(), &["add", "lib/b.rb"]);
    let summary = |args: &[&str]| {
        let out = shapr(dir.path(), args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {out:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        let last = text.lines().next().unwrap_or_default().to_string();
        last.split(": ").next().unwrap().to_string()
    };
    // Revisions the caller typed aren't echoed back: the line starts with the verdict.
    for args in [
        &["feat"][..],
        &["main...feat"],
        &["main..."],
        &["main", "feat"],
        &["main..feat", "--", "lib"],
    ] {
        assert_eq!(summary(args), "behavior", "{args:?}");
    }
    assert_eq!(summary(&["a.rb"]), "uncommitted changes in a.rb");
    assert_eq!(summary(&["--merge-base", "main"]), "main...working tree");
    assert_eq!(summary(&["--staged"]), "staged changes");
    assert_eq!(
        summary(&["--staged", "main"]),
        "staged changes against main"
    );
    let piped = shapr_stdin(dir.path(), &[], &git(dir.path(), &["diff"]));
    let text = String::from_utf8(piped.stdout).unwrap();
    assert!(
        text.lines().next().unwrap().starts_with("stdin: behavior"),
        "{text}"
    );
}

#[test]
fn untracked_files_a_working_tree_form_leaves_out_are_counted_and_named() {
    let dir = branched();
    write(
        dir.path(),
        &[
            ("lib/new.rb", "N = 1\n"),
            ("c.rb", "C = 1\n"),
            ("skip.log", "x\n"),
            (".gitignore", "*.log\n"),
        ],
    );
    let run = |args: &[&str]| {
        let mut all = vec!["-j"];
        all.extend(args);
        let out = shapr(dir.path(), &all);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {out:?}");
        let stderr = String::from_utf8(out.stderr.clone()).unwrap();
        (report(&out)["meta"]["untracked"].clone(), stderr)
    };
    let (count, stderr) = run(&["."]);
    assert_eq!(count, 3);
    assert_eq!(
        stderr,
        "shapr: 3 untracked files not shown: .gitignore, c.rb, lib/new.rb \
         (git add -N .gitignore c.rb lib/new.rb to include them)\n"
    );
    let (count, stderr) = run(&["--", "lib"]);
    assert_eq!(count, 1);
    assert_eq!(
        stderr,
        "shapr: 1 untracked file not shown: lib/new.rb (git add -N lib/new.rb to include it)\n"
    );
    assert_eq!(run(&["--merge-base", "main"]).0, 3);
    // More than three: the first three named.
    write(dir.path(), &[("d.rb", "D = 1\n")]);
    let (count, stderr) = run(&["."]);
    assert_eq!(count, 4);
    assert!(
        stderr.contains("4 untracked files not shown: .gitignore, c.rb, d.rb, ... (git add -N <path> to include them)"),
        "{stderr}"
    );
    // Forms that don't end in the working tree have none to leave out.
    for args in [&["feat"][..], &["main..feat"], &["--staged"]] {
        assert_eq!(run(args), (Value::Null, String::new()), "{args:?}");
    }
    // Added with -N, a new file is diffed and no longer untracked.
    git(dir.path(), &["add", "-N", "lib/new.rb"]);
    let (files, meta) = ran(dir.path(), &["--", "lib"]);
    assert_eq!(files, ["lib/b.rb", "lib/new.rb"]);
    assert_eq!(meta["untracked"], 0);
}

#[test]
fn text_names_the_category_of_a_file_shapr_doesnt_parse() {
    let dir = repo(&[
        &[
            ("README.md", "hi\n"),
            ("Makefile", "x\n"),
            ("config/x.yml", "a: 1\n"),
            ("config/y.yml", "b: 1  \n"),
            ("spec/fixtures/f.rb", "X = 1\n"),
        ],
        &[
            ("README.md", "hello\n"),
            ("Makefile", "y\n"),
            ("config/x.yml", "a: 2\n"),
            ("config/y.yml", "b: 1\n"),
            ("spec/fixtures/f.rb", "X = 2\n"),
        ],
    ]);
    let text = String::from_utf8(shapr(dir.path(), &["HEAD"]).stdout).unwrap();
    let lines: Vec<String> = hunk_lines(&text)
        .take(5)
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    assert_eq!(
        lines,
        [
            "Makefile:1 unknown file (outside the verdict)",
            "README.md:1 docs change (outside the verdict)",
            "config/x.yml:1 config change",
            "config/y.yml:1 config change, whitespace only (outside the verdict)",
            "spec/fixtures/f.rb:1 fixture change (outside the verdict)",
        ],
        "{text}"
    );
}

#[test]
fn text_drops_the_generic_logic_reason_and_keeps_specific_ones() {
    let dir = repo(&[
        &[("a.rb", "def f\n  1\nend\n"), ("b.rb", "def g(\n")],
        &[("a.rb", "def f\n  2\nend\n"), ("b.rb", "def h(\n")],
    ]);
    let text = String::from_utf8(shapr(dir.path(), &["HEAD"]).stdout).unwrap();
    let lines: Vec<&str> = hunk_lines(&text).take(2).collect();
    assert_eq!(
        lines,
        [
            "a.rb:2  logic             1.00  [literal_only]",
            "b.rb:1  logic             0.00  syntax error on both sides; compared line by line",
        ],
        "{text}"
    );
}

/// The text output's hunk lines: after the summary block and its blank line.
fn hunk_lines(text: &str) -> std::str::Lines<'_> {
    text.split_once("\n\n")
        .map_or("", |(_, hunks)| hunks)
        .lines()
}

/// A NUL makes git call a file binary, but Ruby runs the code before it and
/// Rust reads it as content: a file a frontend parses is code, judged.
#[test]
fn a_code_file_holding_a_nul_counts_toward_the_verdict() {
    let dir = repo(&[
        &[
            ("lib/a.rb", "def f\n  1\nend\n\0junk\n"),
            ("src/b.rs", "fn g() -> &'static str {\n    \"a\0b\"\n}\n"),
            ("lib/c.rb", "def h\n  1\nend\n"),
        ],
        &[
            ("lib/a.rb", "def f\n  system(\"x\")\nend\n\0junk\n"),
            ("src/b.rs", "fn g() -> &'static str {\n    \"c\0d\"\n}\n"),
            ("lib/c.rb", "def h\n    1\nend\n"),
        ],
    ]);
    let report = report(&shapr(dir.path(), &["-j", "HEAD"]));
    let mut got = categories(&report);
    got.sort();
    assert_eq!(
        got,
        [
            ("lib/a.rb", "code"),
            ("lib/c.rb", "code"),
            ("src/b.rs", "code")
        ]
    );
    for file in ["lib/a.rb", "src/b.rs"] {
        let hunk = hunks(&report).iter().find(|h| h["file"] == file).unwrap();
        assert_eq!(hunk["verdict"], "behavior", "{hunk:#}");
    }
    assert_eq!(report["summary"]["verdict"], "mixed");
}

#[test]
fn a_diff_on_stdin_that_git_called_binary_for_an_attribute_is_code() {
    let dir = repo(&[
        &[("lib/a.rb", "def f\n  1\nend\n"), ("logo.png", "\0PNG1")],
        &[
            (".gitattributes", "*.rb -diff\n"),
            ("lib/a.rb", "def f\n  2\nend\n"),
            ("logo.png", "\0PNG2"),
        ],
    ]);
    let diff = git(dir.path(), &["diff", "HEAD~1..HEAD"]);
    assert!(diff.contains("Binary files a/lib/a.rb"), "{diff}");
    let report = report(&shapr_stdin(dir.path(), &["-j"], &diff));
    let mut got = categories(&report);
    got.sort();
    assert_eq!(
        got,
        [
            (".gitattributes", "unknown"),
            ("lib/a.rb", "code"),
            ("logo.png", "binary")
        ]
    );
    let hunk = hunks(&report)
        .iter()
        .find(|h| h["file"] == "lib/a.rb")
        .unwrap();
    assert_eq!(hunk["verdict"], "behavior");
    assert!(
        lead(hunk)["reason"].as_str().unwrap().contains("--text"),
        "{hunk:#}"
    );
}

#[test]
fn a_file_too_big_to_compare_is_logic_and_says_why() {
    let side = |name: &str| -> String { (0..4001).map(|i| format!("{name}{i} = 1\n")).collect() };
    let (before, after) = (side("x"), side("y"));
    let report = classify(&[("lib/a.rb", &before)], &[("lib/a.rb", &after)]);
    let file = &report["files"][0];
    assert_eq!(file["parsed"], false, "{file:#}");
    let hunk = &hunks(&report)[0];
    assert_eq!(hunk["verdict"], "behavior");
    let reason = lead(hunk)["reason"].as_str().unwrap();
    assert!(reason.contains("not compared: too big"), "{reason}");
    assert_eq!(report["summary"]["verdict"], "behavior");
}

/// The change's budget is spent smallest file first, so big files named to
/// sort first can't push a small edit out of the verdict.
#[test]
fn big_files_cant_push_a_small_edit_past_the_change_budget() {
    let big = "#\n".repeat(8000);
    let names = ["lib/a.rb", "lib/b.rb", "lib/c.rb", "lib/d.rb"];
    let mut before: Vec<(&str, &str)> = names.iter().map(|n| (*n, big.as_str())).collect();
    before.push(("lib/z.rb", "def z\n  1\nend\n"));
    let dir = repo(&[&before]);
    for name in names {
        git(dir.path(), &["rm", "-q", name]);
    }
    commit(dir.path(), &[("lib/z.rb", "def z\n  2\nend\n")], "delete");
    let report = report(&shapr(dir.path(), &["-j", "HEAD"]));
    let hunk = hunks(&report)
        .iter()
        .find(|h| h["file"] == "lib/z.rb")
        .unwrap();
    assert_eq!(hunk["verdict"], "behavior", "{hunk:#}");
}

/// A file only added has no removed-by-added pairs, but its declarations
/// are compared with each other: its lines count against the same cap.
#[test]
fn a_file_added_too_big_to_compare_is_logic() {
    let big: String = (0..8001).map(|i| format!("def m{i}; end\n")).collect();
    let report = classify(
        &[("lib/a.rb", "x = 1\n")],
        &[("lib/a.rb", "x = 1\n"), ("lib/b.rb", &big)],
    );
    let hunk = hunks(&report)
        .iter()
        .find(|h| h["file"] == "lib/b.rb")
        .unwrap();
    let reason = lead(hunk)["reason"].as_str().unwrap();
    assert!(reason.contains("not compared: too big"), "{reason}");
    assert_eq!(hunk["verdict"], "behavior");
    assert_eq!(report["summary"]["verdict"], "behavior");
}

/// Code shapr didn't compare never leaves the verdict on its own say: over
/// a cap, nested too deep, unreadable or diffed as binary on stdin, a code
/// or test hunk is logic inside the verdict, its reason naming why.
#[test]
fn a_counted_file_shapr_didnt_compare_stays_in_the_verdict() {
    let renamed = ("lib/a.rb", "def f\n  x = 1\n  x\nend\n");
    let renamed_after = ("lib/a.rb", "def f\n  y = 1\n  y\nend\n");
    let big: String = (0..9000).map(|i| format!("def m{i}; end\n")).collect();
    let deep = format!("x = {}\n", vec!["1"; 50_000].join(" + "));
    let deep_after = format!("{deep}# c\n");
    let big_rs = big.replace("def m", "fn m").replace("; end", "() {}");
    type Files<'a> = Vec<(&'a str, &'a str)>;
    let cases: [(&str, Files, Files, &str); 4] = [
        (
            "added",
            vec![renamed],
            vec![renamed_after, ("lib/evil.rb", &big)],
            "too big",
        ),
        (
            "deleted",
            vec![renamed, ("spec/evil_spec.rb", &big)],
            vec![renamed_after],
            "too big",
        ),
        (
            "deep",
            vec![renamed, ("lib/deep.rb", &deep)],
            vec![renamed_after, ("lib/deep.rb", &deep_after)],
            "nested deeper",
        ),
        (
            "rust",
            vec![renamed, ("src/lib.rs", "fn f() {}\n")],
            vec![renamed_after, ("src/lib.rs", &big_rs)],
            "too big",
        ),
    ];
    for (name, before, after, why) in cases {
        let dir = repo(&[&before]);
        for (path, _) in &before {
            if !after.iter().any(|(p, _)| p == path) {
                git(dir.path(), &["rm", "-q", path]);
            }
        }
        commit(dir.path(), &after, name);
        let report = report(&shapr(dir.path(), &["-j", "HEAD"]));
        assert_eq!(report["summary"]["verdicts"]["unknown"], 0, "{name}");
        assert_eq!(report["summary"]["verdict"], "mixed", "{name}");
        let hunk = hunks(&report)
            .iter()
            .find(|h| h["parsed"] == false)
            .unwrap();
        assert_eq!(hunk["verdict"], "behavior", "{name}");
        let reason = lead(hunk)["reason"].as_str().unwrap();
        assert!(reason.starts_with("not compared: "), "{name}: {reason}");
        assert!(reason.contains(why), "{name}: {reason}");
        let text = String::from_utf8(shapr(dir.path(), &["HEAD"]).stdout).unwrap();
        assert!(!text.contains("outside the verdict"), "{name}: {text}");
        assert!(!text.contains("too big to compare"), "{name}: {text}");
    }
    // On stdin: a diff with no blob ids can't be read.
    let dir = repo(&[&[renamed]]);
    let diff = "diff --git a/lib/a.rb b/lib/a.rb\n--- a/lib/a.rb\n+++ b/lib/a.rb\n@@ -2 +2 @@\n-  x = 1\n+  x = 2\n";
    let report = report(&shapr_stdin(dir.path(), &["-j"], diff));
    let hunk = &hunks(&report)[0];
    assert_eq!(hunk["parsed"], false, "{hunk:#}");
    assert_eq!(hunk["verdict"], "behavior", "{hunk:#}");
}

/// The caps see more than line counts: code packed onto one long line, a
/// body too wide to align, and a file too large to parse are each logic
/// inside the verdict, not a hang.
#[test]
fn long_lines_wide_bodies_and_large_files_are_not_compared() {
    let methods =
        |n: usize| -> Vec<String> { (0..n).map(|i| format!("def m{i}(x) = x + {i}")).collect() };
    let mut shuffled = methods(4000);
    shuffled.reverse();
    let one_line = |ms: &[String]| format!("class K; {}; end\n", ms.join("; "));
    let wide: String = methods(5000).iter().map(|m| format!("{m}\n")).collect();
    let large = format!("x = \"{}\"\n", "a".repeat(5 << 20));
    let cases = [
        (
            "one line",
            one_line(&methods(4000)),
            one_line(&shuffled),
            "changed bytes",
        ),
        ("wide", wide.clone(), format!("{wide}x = 1\n"), "children"),
        ("large", large.clone(), format!("{large}x = 1\n"), "bytes"),
    ];
    for (name, before, after, why) in cases {
        let start = std::time::Instant::now();
        let report = classify(&[("lib/a.rb", &before)], &[("lib/a.rb", &after)]);
        let hunk = &hunks(&report)[0];
        assert_eq!(hunk["parsed"], false, "{name}");
        assert_eq!(hunk["verdict"], "behavior", "{name}");
        let reason = lead(hunk)["reason"].as_str().unwrap();
        assert!(reason.contains(why), "{name}: {reason}");
        assert!(
            start.elapsed().as_secs() < 10,
            "{name}: {:?}",
            start.elapsed()
        );
    }
}

#[test]
fn help_keeps_its_line_breaks() {
    let dir = tempfile::tempdir().unwrap();
    for args in [&["--help"][..], &["norms", "--help"], &["-h"]] {
        let out = shapr(dir.path(), args);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            !text.contains("\\n"),
            "{args:?}: literal \\n in help: {text}"
        );
        assert!(text.lines().count() > 10, "{args:?}: {text}");
    }
}

#[test]
fn a_malformed_hunk_header_is_an_error_not_an_empty_change() {
    let dir = tempfile::tempdir().unwrap();
    for header in [
        "@@ -99999999999999999999999 +1 @@",
        "@@ -1,x +1 @@",
        "@@ garbage",
    ] {
        let diff = format!("--- a/a.rb\n+++ b/a.rb\n{header}\n-a\n+b\n");
        let out = shapr_stdin(dir.path(), &[], &diff);
        assert_eq!(out.status.code(), Some(2), "{header}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("not a diff: malformed hunk header at line 3"),
            "{stderr}"
        );
    }
}

#[test]
fn a_hunk_header_outside_a_file_section_is_message_text() {
    let dir = tempfile::tempdir().unwrap();
    let mail = "From abc Mon Sep 17 00:00:00 2001\nSubject: x\n\n@@ not a header in the message\n---\ndiff --git a/x.rb b/x.rb\nindex 1111111..2222222 100644\n--- a/x.rb\n+++ b/x.rb\n@@ -1 +1 @@\n-x = 1\n+x = 2\n";
    let out = shapr_stdin(dir.path(), &["-j"], mail);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(report(&out)["summary"]["hunks"], 1);
}
