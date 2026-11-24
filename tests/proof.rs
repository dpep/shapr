//! End to end: proof status. A part that depends on code outside the change
//! is verified against the repository at the after revision; it is structure
//! only when nothing it assumes is left unchecked.

use serde_json::Value;

mod common;
use common::*;

/// A repository holding BASE and BEFORE, then a commit making AFTER.
fn based_repo(
    base: &[(&str, &str)],
    before: &[(&str, &str)],
    after: &[(&str, &str)],
) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    write(dir.path(), base);
    commit(dir.path(), before, "state");
    commit(dir.path(), after, "state");
    dir
}

/// `shapr -j` on the last commit of a repository holding BASE unchanged.
fn classify(base: &[(&str, &str)], before: &[(&str, &str)], after: &[(&str, &str)]) -> Value {
    let dir = based_repo(base, before, after);
    report(&shapr(dir.path(), &["-j", "HEAD~1..HEAD"]))
}

fn parts(report: &Value) -> Vec<&Value> {
    report["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|h| h["parts"].as_array().unwrap())
        .collect()
}

fn any_part<'r>(report: &'r Value, kind: &str) -> &'r Value {
    parts(report)
        .into_iter()
        .find(|p| p["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} part: {report:#}"))
}

fn assumptions(part: &Value) -> Vec<&str> {
    part["assumptions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect()
}

const GEMSPEC: (&str, &str) = (
    "widgets.gemspec",
    "Gem::Specification.new { |s| s.name = \"widgets\" }\n",
);

fn meter(public: &str, private: &str) -> String {
    format!(
        "class Meter\n  def {public}\n    {private} + 1\n  end\n\n  private\n\n  def {private}\n    41\n  end\nend\n"
    )
}

#[test]
fn a_private_method_renamed_with_no_use_outside_the_change_is_structure() {
    let report = classify(
        &[GEMSPEC],
        &[("lib/widgets/meter.rb", &meter("level", "sample"))],
        &[("lib/widgets/meter.rb", &meter("level", "probe"))],
    );
    let rename = any_part(&report, "rename");
    assert_eq!(rename["structure"], true, "{report:#}");
    assert!(assumptions(rename).is_empty());
    assert_eq!(report["summary"]["verdict"], "structure");
    assert_eq!(report["summary"]["review_lines"], 0);
    assert!(rename["evidence"]["searched"].as_u64().unwrap() >= 1);
}

#[test]
fn a_rename_whose_old_name_an_unchanged_file_still_uses_is_logic_naming_it() {
    let caller = (
        "report.rb",
        "class Report\n  def show\n    Meter.new.sample\n  end\nend\n",
    );
    let report = classify(
        &[caller],
        &[("meter.rb", &meter("level", "sample"))],
        &[("meter.rb", &meter("level", "probe"))],
    );
    assert!(
        parts(&report).iter().all(|p| p["kind"] == "logic"),
        "{report:#}"
    );
    assert_eq!(report["summary"]["verdict"], "behavior");
    let reason = any_part(&report, "logic")["reason"].as_str().unwrap();
    assert!(reason.contains("report.rb"), "{reason}");
}

#[test]
fn a_public_gem_method_renamed_is_behavior_as_public_api() {
    let report = classify(
        &[GEMSPEC],
        &[("lib/widgets/meter.rb", &meter("level", "sample"))],
        &[("lib/widgets/meter.rb", &meter("value", "sample"))],
    );
    let rename = any_part(&report, "rename");
    assert_eq!(rename["structure"], false, "{report:#}");
    assert_eq!(assumptions(rename), ["public_api"]);
    assert!(rename["reason"].as_str().unwrap().contains("public API"));
    assert!(rename["review_lines"].as_u64().unwrap() > 0);
    assert_eq!(report["summary"]["verdict"], "behavior");
    assert_eq!(report["summary"]["assumptions"]["public_api"], 1);
}

#[test]
fn the_same_rename_outside_a_gem_is_structure() {
    let report = classify(
        &[],
        &[("lib/widgets/meter.rb", &meter("level", "sample"))],
        &[("lib/widgets/meter.rb", &meter("value", "sample"))],
    );
    assert_eq!(any_part(&report, "rename")["structure"], true, "{report:#}");
    assert_eq!(report["summary"]["verdict"], "structure");
}

const CARGO: (&str, &str) = (
    "Cargo.toml",
    "[package]\nname = \"widgets\"\nversion = \"0.1.0\"\n",
);

fn total(name: &str) -> String {
    format!(
        "pub fn {name}() -> u8 {{\n    1 + 2\n}}\n\npub fn twice() -> u8 {{\n    {name}() * 2\n}}\n"
    )
}

#[test]
fn a_pub_fn_renamed_in_a_library_crate_is_behavior() {
    let report = classify(
        &[CARGO],
        &[("src/lib.rs", &total("total"))],
        &[("src/lib.rs", &total("sum"))],
    );
    let rename = any_part(&report, "rename");
    assert_eq!(assumptions(rename), ["public_api"], "{report:#}");
    assert_eq!(report["summary"]["verdict"], "behavior");
}

#[test]
fn a_pub_fn_renamed_in_a_binary_only_crate_is_structure() {
    let main = (
        "src/main.rs",
        "mod util;\n\nfn main() {\n    let _ = util::twice();\n}\n",
    );
    let report = classify(
        &[CARGO, main],
        &[("src/util.rs", &total("total"))],
        &[("src/util.rs", &total("sum"))],
    );
    let rename = any_part(&report, "rename");
    assert!(assumptions(rename).is_empty(), "{report:#}");
    assert_eq!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_crate_marked_publish_false_has_no_public_api() {
    let cargo = (
        "Cargo.toml",
        "[package]\nname = \"widgets\"\npublish = false\n",
    );
    let report = classify(
        &[cargo],
        &[("src/lib.rs", &total("total"))],
        &[("src/lib.rs", &total("sum"))],
    );
    assert_eq!(report["summary"]["verdict"], "structure", "{report:#}");
}

fn method(name: &str, value: &str) -> String {
    format!("  def {name}\n    compute({value}) + offset\n  end\n")
}

fn class(name: &str, methods: &[&str]) -> String {
    format!("class {name}\n{}end\n", methods.join("\n"))
}

#[test]
fn a_move_with_several_equal_candidates_is_not_proven() {
    let a = method("first", "2");
    let b = method("second", "3");
    let other = class("Other", &[&method("z", "1")]);
    let twice = class("Other", &[&method("z", "1"), &a]);
    let report = classify(
        &[],
        &[
            ("shape.rb", &class("Shape", &[&a, &b])),
            ("other.rb", &other),
        ],
        &[
            ("shape.rb", &class("Shape", &[&b, &a])),
            ("other.rb", &twice),
        ],
    );
    let moved = any_part(&report, "move");
    assert_eq!(moved["evidence"]["candidates"], 2, "{report:#}");
    assert_eq!(assumptions(moved), ["ambiguous_match"]);
    assert_eq!(moved["structure"], false);
    assert!(moved["review_lines"].as_u64().unwrap() > 0);
    assert_ne!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_rename_against_the_working_tree_searches_the_working_tree() {
    let dir = based_repo(
        &[],
        &[("meter.rb", &meter("level", "sample"))],
        &[("README.md", "Meters.\n")],
    );
    write(dir.path(), &[("meter.rb", &meter("level", "probe"))]);
    let contained = report(&shapr(dir.path(), &["-j", "."]));
    assert_eq!(
        any_part(&contained, "rename")["structure"],
        true,
        "{contained:#}"
    );
    // A tracked file the commits never held: only the working tree has it.
    write(
        dir.path(),
        &[(
            "report.rb",
            "class Report\n  def show\n    Meter.new.sample\n  end\nend\n",
        )],
    );
    git(dir.path(), &["add", "-N", "report.rb"]);
    // A diff on stdin ends in the working tree too.
    let diff = git(dir.path(), &["diff", "HEAD", "--", "meter.rb"]);
    let report = report(&shapr_stdin(dir.path(), &["-j"], &diff));
    let reason = any_part(&report, "logic")["reason"].as_str().unwrap();
    assert!(reason.contains("report.rb"), "{report:#}");
}

#[test]
fn an_inlined_public_gem_method_is_behavior() {
    let before = "class Meter\n  def read\n    total = base + offset\n    scaled(total)\n  end\n\n  def scaled(total)\n    clamped = [total, limit].min\n    clamped * factor + bias\n  end\nend\n";
    let after = "class Meter\n  def read\n    total = base + offset\n    clamped = [total, limit].min\n    clamped * factor + bias\n  end\nend\n";
    let report = classify(
        &[GEMSPEC],
        &[("lib/meter.rb", before)],
        &[("lib/meter.rb", after)],
    );
    let inline = any_part(&report, "inline");
    assert_eq!(assumptions(inline), ["public_api"], "{report:#}");
    assert_eq!(report["summary"]["verdict"], "behavior");
}

#[test]
fn an_extract_whose_new_name_another_file_defines_is_logic() {
    let before = "class Meter\n  def read\n    total = base + offset\n    clamped = [total, limit].min\n    clamped * factor + bias\n  end\nend\n";
    let after = "class Meter\n  def read\n    total = base + offset\n    scaled(total)\n  end\n\n  def scaled(total)\n    clamped = [total, limit].min\n    clamped * factor + bias\n  end\nend\n";
    let sub = (
        "fine_meter.rb",
        "class FineMeter < Meter\n  def scaled(total)\n    total\n  end\nend\n",
    );
    let contained = classify(&[], &[("meter.rb", before)], &[("meter.rb", after)]);
    assert_eq!(
        any_part(&contained, "extract")["structure"],
        true,
        "{contained:#}"
    );
    let report = classify(&[sub], &[("meter.rb", before)], &[("meter.rb", after)]);
    assert!(
        parts(&report).iter().all(|p| p["kind"] == "logic"),
        "{report:#}"
    );
    let reason = any_part(&report, "logic")["reason"].as_str().unwrap();
    assert!(reason.contains("fine_meter.rb"), "{reason}");
}

#[test]
fn the_old_name_in_another_files_string_or_comment_is_not_a_use() {
    let notes = (
        "notes.rb",
        "# sample is how it used to be read\nNOTE = \"take a sample first\"\n",
    );
    let template = ("show.erb", "<%= meter.sample %>\n");
    let report = classify(
        &[notes],
        &[("meter.rb", &meter("level", "sample"))],
        &[("meter.rb", &meter("level", "probe"))],
    );
    assert_eq!(any_part(&report, "rename")["structure"], true, "{report:#}");
    // A file shapr can't parse holding the word counts as a use.
    let report = classify(
        &[notes, template],
        &[("meter.rb", &meter("level", "sample"))],
        &[("meter.rb", &meter("level", "probe"))],
    );
    let reason = any_part(&report, "logic")["reason"].as_str().unwrap();
    assert!(
        reason.contains("show.erb") && !reason.contains("notes.rb"),
        "{reason}"
    );
}

const CALLER: (&str, &str) = (
    "report.rb",
    "class Report\n  def show\n    Meter.new.sample\n  end\nend\n",
);
const CALLER_GONE: (&str, &str) = ("report.rb", "class Report\n  def show\n  end\nend\n");

#[test]
fn a_diff_on_stdin_whose_after_side_is_neither_the_working_tree_nor_the_index_is_unseen() {
    let dir = based_repo(
        &[CALLER],
        &[("meter.rb", &meter("level", "sample"))],
        &[("meter.rb", &meter("level", "probe"))],
    );
    commit(dir.path(), &[CALLER_GONE], "drop the call");
    // The working tree no longer calls `sample`, but HEAD~1, where the diff
    // ends, still does.
    let diff = git(dir.path(), &["diff", "HEAD~2", "HEAD~1"]);
    let report = report(&shapr_stdin(dir.path(), &["-j"], &diff));
    let rename = any_part(&report, "rename");
    assert_eq!(rename["structure"], false, "{report:#}");
    assert_eq!(assumptions(rename), ["unseen_uses"]);
    assert_ne!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_staged_diff_on_stdin_is_searched_in_the_index() {
    let dir = based_repo(
        &[CALLER],
        &[("meter.rb", &meter("level", "sample"))],
        &[("README.md", "Meters.\n")],
    );
    write(dir.path(), &[("meter.rb", &meter("level", "probe"))]);
    git(dir.path(), &["add", "meter.rb"]);
    // Unstaged: the working tree drops the call the index still holds.
    write(dir.path(), &[CALLER_GONE]);
    let diff = git(dir.path(), &["diff", "--staged"]);
    let staged = report(&shapr_stdin(dir.path(), &["-j"], &diff));
    let reason = any_part(&staged, "logic")["reason"].as_str().unwrap();
    assert!(reason.contains("report.rb"), "{staged:#}");
    // Staged as well, the index holds no use.
    git(dir.path(), &["add", "report.rb"]);
    let diff = git(dir.path(), &["diff", "--staged", "--", "meter.rb"]);
    let report = report(&shapr_stdin(dir.path(), &["-j"], &diff));
    assert_eq!(any_part(&report, "rename")["structure"], true, "{report:#}");
}

#[test]
fn a_search_over_thousands_of_files_finishes() {
    // Enough blob ids that git's reply fills its pipe before shapr has
    // written them all.
    let files: Vec<(String, String)> = (0..4000)
        .map(|i| {
            (
                format!("lib/f{i}.rb"),
                format!("class F{i}\n  def m\n    {i}\n  end\nend\n"),
            )
        })
        .collect();
    let base: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let dir = based_repo(
        &base,
        &[("meter.rb", &meter("level", "sample"))],
        &[("meter.rb", &meter("level", "probe"))],
    );
    let mut child = command(env!("CARGO_BIN_EXE_shapr"), dir.path())
        .args(["-j", "HEAD~1..HEAD"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > std::time::Duration::from_secs(60) {
            child.kill().unwrap();
            panic!("shapr hung");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn a_diff_on_stdin_that_could_end_in_the_index_or_the_working_tree_is_searched_in_both() {
    let dir = based_repo(
        &[CALLER_GONE],
        &[("meter.rb", &meter("level", "sample"))],
        &[("README.md", "Meters.\n")],
    );
    write(dir.path(), &[("meter.rb", &meter("level", "probe"))]);
    git(dir.path(), &["add", "meter.rb"]);
    // Unstaged, and outside the diff: only the working tree calls `sample`.
    write(dir.path(), &[CALLER]);
    // `git diff --staged` and `git diff HEAD -- meter.rb` print the same text.
    let diff = git(dir.path(), &["diff", "--staged"]);
    let report = report(&shapr_stdin(dir.path(), &["-j"], &diff));
    let reason = any_part(&report, "logic")["reason"].as_str().unwrap();
    assert!(reason.contains("report.rb"), "{report:#}");
}

#[test]
fn a_use_in_a_file_with_an_odd_name_is_found() {
    let def = |name: &str| format!("class Z\n  def {name}(x)\n    x\n  end\nend\n");
    let caller = "class Z\n  def go\n    alpha_meth(1)\n  end\nend\n";
    for odd in [
        "lib/a\tb.rb",
        "lib/a\nb.rb",
        "lib/a\"b.rb",
        "lib/a\\b.rb",
        "lib/a\rb.rb",
    ] {
        let report = classify(
            &[(odd, caller)],
            &[("lib/d.rb", &def("alpha_meth"))],
            &[("lib/d.rb", &def("beta_meth"))],
        );
        assert_eq!(
            report["summary"]["verdict"], "behavior",
            "{odd:?}: {report:#}"
        );
    }
    // The text names the file as git quotes it, on one line.
    let dir = based_repo(
        &[("lib/a\nb.rb", caller)],
        &[("lib/d.rb", &def("alpha_meth"))],
        &[("lib/d.rb", &def("beta_meth"))],
    );
    let text = String::from_utf8(shapr(dir.path(), &["HEAD~1..HEAD"]).stdout).unwrap();
    assert!(text.contains(r"lib/a\nb.rb"), "{text}");
    assert!(!text.lines().any(|l| l.starts_with("b.rb")), "{text}");
}
