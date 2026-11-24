//! End to end: `facets` on logic hunks, what kind of logic change each is.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// The part a hunk's kind stands for: the last of that kind (the residual,
/// for `logic`), else the first that isn't structure.
fn lead(hunk: &Value) -> &Value {
    let parts = hunk["parts"].as_array().unwrap();
    parts
        .iter()
        .rev()
        .find(|p| p["kind"] == hunk["kind"])
        .or_else(|| parts.iter().find(|p| p["structure"] == false))
        .unwrap_or_else(|| panic!("no parts: {hunk:#}"))
}

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

fn git(dir: &Path, args: &[&str]) {
    let out = command("git", dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `shapr -j` on a commit taking `before` to `after`.
fn classify(before: &[(&str, &str)], after: &[(&str, &str)]) -> Value {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    for state in [before, after] {
        for (path, content) in state {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-q", "-m", "state"]);
    }
    let out = command(env!("CARGO_BIN_EXE_shapr"), dir.path())
        .args(["-j", "HEAD~1..HEAD"])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

/// Each hunk's facets, for one file changed from `before` to `after`.
fn facets(path: &str, before: &str, after: &str) -> Vec<Vec<String>> {
    let report = classify(&[(path, before)], &[(path, after)]);
    report["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            let facets = lead(h)["facets"]
                .as_array()
                .unwrap_or_else(|| panic!("{h:#}"));
            facets
                .iter()
                .map(|f| f.as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

fn one(path: &str, before: &str, after: &str) -> Vec<String> {
    let mut all = facets(path, before, after);
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

const GATE: &str = "class Gate\n  def open?(n)\n    if n > 1\n      :open\n    else\n      :shut\n    end\n  end\nend\n";

#[test]
fn a_flipped_comparison_changes_a_condition() {
    let after = GATE.replace("n > 1", "n >= 1");
    assert_eq!(one("gate.rb", GATE, &after), ["condition"]);
}

#[test]
fn a_new_guard_is_a_condition() {
    let after = GATE.replace(
        "    if n > 1\n",
        "    return :shut if n.nil?\n    if n > 1\n",
    );
    assert_eq!(one("gate.rb", GATE, &after), ["condition"]);
}

#[test]
fn a_new_method_adds_a_definition() {
    let after = GATE.replace(
        "  end\nend\n",
        "  end\n\n  def shut?(n)\n    !open?(n)\n  end\nend\n",
    );
    assert_eq!(one("gate.rb", GATE, &after), ["adds_definition"]);
}

#[test]
fn a_deleted_method_removes_a_definition() {
    let before = GATE.replace(
        "  end\nend\n",
        "  end\n\n  def shut?(n)\n    !open?(n)\n  end\nend\n",
    );
    assert_eq!(one("gate.rb", &before, GATE), ["removes_definition"]);
}

#[test]
fn a_changed_threshold_is_literal_only_and_a_condition() {
    let after = GATE.replace("n > 1", "n > 2");
    assert_eq!(one("gate.rb", GATE, &after), ["condition", "literal_only"]);
}

#[test]
fn changed_settings_are_literal_only() {
    let before = "module Config\n  TIMEOUT = 5\n  NAME = \"svc\"\n  MODE = :fast\nend\n";
    let after = "module Config\n  TIMEOUT = 10\n  NAME = \"service\"\n  MODE = :safe\nend\n";
    assert_eq!(one("config.rb", before, after), ["literal_only"]);
}

#[test]
fn a_new_argument_is_not_literal_only() {
    let before = "def go\n  run(1)\nend\n";
    let after = "def go\n  run(1, 2)\nend\n";
    assert_eq!(one("go.rb", before, after), Vec::<String>::new());
}

#[test]
fn an_added_rescue_is_error_handling() {
    let before = "def fetch(url)\n  get(url)\nend\n";
    let after = "def fetch(url)\n  get(url)\nrescue Timeout::Error\n  nil\nend\n";
    assert_eq!(one("fetch.rb", before, after), ["error_handling"]);
}

#[test]
fn rescuing_another_class_is_error_handling() {
    let before = "def fetch(url)\n  get(url)\nrescue Timeout::Error\n  nil\nend\n";
    let after = before.replace("Timeout::Error", "IOError");
    assert_eq!(one("fetch.rb", before, &after), ["error_handling"]);
}

#[test]
fn a_raising_guard_is_a_condition_and_error_handling() {
    let before = "def fetch(url)\n  get(url)\nend\n";
    let after = "def fetch(url)\n  raise ArgumentError if url.nil?\n  get(url)\nend\n";
    assert_eq!(
        one("fetch.rb", before, after),
        ["condition", "error_handling"]
    );
}

#[test]
fn a_new_parameter_changes_the_signature() {
    let before = "def greet(name)\n  \"hi #{name}\"\nend\n";
    let after = "def greet(name, greeting)\n  \"#{greeting} #{name}\"\nend\n";
    assert_eq!(one("greet.rb", before, after), ["signature"]);
}

#[test]
fn a_spec_change_is_test() {
    let before = "RSpec.describe Gate do\n  it { expect(Gate.new.open?(2)).to eq(:open) }\nend\n";
    let after = before.replace("open?(2)", "open?(3)");
    assert_eq!(
        one("spec/gate_spec.rb", before, &after),
        ["test", "literal_only"]
    );
}

#[test]
fn a_gem_bump_is_a_manifest_change() {
    let before = "source \"https://rubygems.org\"\ngem \"rake\", \"~> 12.0\"\n";
    let after = before.replace("12.0", "13.0");
    assert_eq!(one("Gemfile", before, &after), ["manifest", "literal_only"]);
}

#[test]
fn structure_has_no_facets() {
    let after = GATE.replace("    if n > 1", "    if n >   1");
    let report = classify(&[("gate.rb", GATE)], &[("gate.rb", &after)]);
    assert_eq!(report["hunks"][0]["kind"], "format_only");
    assert_eq!(lead(&report["hunks"][0])["facets"], serde_json::json!([]));
}

#[test]
fn the_summary_counts_facets() {
    let after = GATE.replace("n > 1", "n > 2");
    let report = classify(&[("gate.rb", GATE)], &[("gate.rb", &after)]);
    let counts = &report["summary"]["facets"];
    assert_eq!(counts["condition"], 1);
    assert_eq!(counts["literal_only"], 1);
    assert_eq!(counts["test"], 0, "every facet is present: {counts}");
}

// Rust.

const LIB: &str = "pub fn open(n: u32) -> bool {\n    if n > 1 {\n        true\n    } else {\n        false\n    }\n}\n";

#[test]
fn rust_a_flipped_comparison_changes_a_condition() {
    let after = LIB.replace("n > 1", "n >= 1");
    assert_eq!(one("lib.rs", LIB, &after), ["condition"]);
}

#[test]
fn rust_a_new_fn_adds_a_definition() {
    let after = format!("{LIB}\npub fn shut(n: u32) -> bool {{\n    !open(n)\n}}\n");
    assert_eq!(one("lib.rs", LIB, &after), ["adds_definition"]);
}

#[test]
fn rust_a_changed_parameter_type_changes_the_signature() {
    let after = LIB.replace("n: u32", "n: u64");
    assert_eq!(one("lib.rs", LIB, &after), ["signature"]);
}

#[test]
fn rust_a_changed_constant_is_literal_only() {
    let before = "pub const LIMIT: u32 = 5;\npub const NAME: &str = \"svc\";\n";
    let after = "pub const LIMIT: u32 = 10;\npub const NAME: &str = \"service\";\n";
    assert_eq!(one("lib.rs", before, after), ["literal_only"]);
}

#[test]
fn rust_unwrap_to_question_mark_is_error_handling() {
    let before = "fn port(s: &str) -> u16 {\n    let n = s.parse().unwrap();\n    n\n}\n";
    let after = before.replace(".unwrap()", ".expect(\"a port\")");
    assert_eq!(one("lib.rs", before, &after), ["error_handling"]);
}

#[test]
fn rust_a_test_item_change_is_test() {
    let before = format!(
        "{LIB}\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn opens() {{\n        assert!(super::open(2));\n    }}\n}}\n"
    );
    let after = before.replace("open(2)", "open(3)");
    assert_eq!(one("lib.rs", &before, &after), ["test", "literal_only"]);
}

#[test]
fn rust_a_dependency_bump_is_a_manifest_change() {
    let before = "[package]\nname = \"demo\"\n\n[dependencies]\nserde = \"1.0.100\"\n";
    let after = before.replace("1.0.100", "1.0.200");
    assert_eq!(one("Cargo.toml", before, &after), ["manifest"]);
}

#[test]
fn a_line_added_inside_a_branch_body_is_not_a_condition() {
    let after = GATE.replace("      :open\n", "      log(n)\n      :open\n");
    assert_eq!(one("gate.rb", GATE, &after), Vec::<String>::new());
}

#[test]
fn rust_an_argument_under_question_mark_is_not_error_handling() {
    let before = "fn port(s: &str) -> Result<u16, E> {\n    let n = parse(s)?;\n    Ok(n)\n}\n";
    let after = before.replace("parse(s)", "parse(s.trim())");
    assert_eq!(one("lib.rs", before, &after), Vec::<String>::new());
}
