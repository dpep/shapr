//! End to end: proof status. A part that depends on code outside the change
//! is verified against the repository at the after revision; it is structure
//! only when nothing it assumes is left unchecked.

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

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (path, content) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

/// A repository holding BASE and BEFORE, then a commit making AFTER.
fn repo(
    base: &[(&str, &str)],
    before: &[(&str, &str)],
    after: &[(&str, &str)],
) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    write(dir.path(), base);
    for state in [before, after] {
        write(dir.path(), state);
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "state"]);
    }
    dir
}

fn shapr(dir: &Path, args: &[&str]) -> Value {
    let out = command(env!("CARGO_BIN_EXE_shapr"), dir)
        .args(args)
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

/// `shapr -j` on the last commit of a repository holding BASE unchanged.
fn classify(base: &[(&str, &str)], before: &[(&str, &str)], after: &[(&str, &str)]) -> Value {
    let dir = repo(base, before, after);
    shapr(dir.path(), &["-j", "HEAD~1..HEAD"])
}

fn parts(report: &Value) -> Vec<&Value> {
    report["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|h| h["parts"].as_array().unwrap())
        .collect()
}

fn part<'r>(report: &'r Value, kind: &str) -> &'r Value {
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
        &[("lib/widgets/meter.rb", &meter("read", "sample"))],
        &[("lib/widgets/meter.rb", &meter("read", "probe"))],
    );
    let rename = part(&report, "rename");
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
        &[("meter.rb", &meter("read", "sample"))],
        &[("meter.rb", &meter("read", "probe"))],
    );
    assert!(
        parts(&report).iter().all(|p| p["kind"] == "logic"),
        "{report:#}"
    );
    assert_eq!(report["summary"]["verdict"], "behavior");
    let reason = part(&report, "logic")["reason"].as_str().unwrap();
    assert!(reason.contains("report.rb"), "{reason}");
}

#[test]
fn a_public_gem_method_renamed_is_behavior_as_public_api() {
    let report = classify(
        &[GEMSPEC],
        &[("lib/widgets/meter.rb", &meter("read", "sample"))],
        &[("lib/widgets/meter.rb", &meter("value", "sample"))],
    );
    let rename = part(&report, "rename");
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
        &[("lib/widgets/meter.rb", &meter("read", "sample"))],
        &[("lib/widgets/meter.rb", &meter("value", "sample"))],
    );
    assert_eq!(part(&report, "rename")["structure"], true, "{report:#}");
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
    let rename = part(&report, "rename");
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
    let rename = part(&report, "rename");
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
    let moved = part(&report, "move");
    assert_eq!(moved["evidence"]["candidates"], 2, "{report:#}");
    assert_eq!(assumptions(moved), ["ambiguous_match"]);
    assert_eq!(moved["structure"], false);
    assert!(moved["review_lines"].as_u64().unwrap() > 0);
    assert_ne!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_rename_against_the_working_tree_searches_the_working_tree() {
    let dir = repo(
        &[],
        &[("meter.rb", &meter("read", "sample"))],
        &[("README.md", "Meters.\n")],
    );
    write(dir.path(), &[("meter.rb", &meter("read", "probe"))]);
    let contained = shapr(dir.path(), &["-j", "HEAD"]);
    assert_eq!(
        part(&contained, "rename")["structure"],
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
    let diff = command("git", dir.path())
        .args(["diff", "HEAD", "--", "meter.rb"])
        .output()
        .unwrap()
        .stdout;
    let mut child = command(env!("CARGO_BIN_EXE_shapr"), dir.path())
        .arg("-j")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(&diff).unwrap();
    let out = child.wait_with_output().unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let reason = part(&report, "logic")["reason"].as_str().unwrap();
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
    let inline = part(&report, "inline");
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
        part(&contained, "extract")["structure"],
        true,
        "{contained:#}"
    );
    let report = classify(&[sub], &[("meter.rb", before)], &[("meter.rb", after)]);
    assert!(
        parts(&report).iter().all(|p| p["kind"] == "logic"),
        "{report:#}"
    );
    let reason = part(&report, "logic")["reason"].as_str().unwrap();
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
        &[("meter.rb", &meter("read", "sample"))],
        &[("meter.rb", &meter("read", "probe"))],
    );
    assert_eq!(part(&report, "rename")["structure"], true, "{report:#}");
    // A file shapr can't parse holding the word counts as a use.
    let report = classify(
        &[notes, template],
        &[("meter.rb", &meter("read", "sample"))],
        &[("meter.rb", &meter("read", "probe"))],
    );
    let reason = part(&report, "logic")["reason"].as_str().unwrap();
    assert!(
        reason.contains("show.erb") && !reason.contains("notes.rb"),
        "{reason}"
    );
}
