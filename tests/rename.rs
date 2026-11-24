//! End to end: rename detection against throwaway git repos.

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

/// `shapr -j` on a commit that changes `before` into `after`.
fn classify(before: &[(&str, &str)], after: &[(&str, &str)]) -> Value {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    for state in [before, after] {
        for (path, content) in state {
            std::fs::write(dir.path().join(path), content).unwrap();
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

fn one(before: &str, after: &str) -> Value {
    classify(&[("a.rb", before)], &[("a.rb", after)])
}

fn kinds(report: &Value) -> Vec<&str> {
    report["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["kind"].as_str().unwrap())
        .collect()
}

/// Every hunk is logic, and the rename pass says why: proof the guard under
/// test fired, not some earlier mismatch.
fn logic_because(report: &Value, why: &str) {
    assert!(kinds(report).iter().all(|k| *k == "logic"), "{report}");
    let reasons: Vec<&str> = report["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| lead(h)["reason"].as_str().unwrap())
        .collect();
    assert!(
        reasons.iter().any(|r| r.contains(why)),
        "{why}: {reasons:?}"
    );
}

fn confidence(hunk: &Value) -> f64 {
    lead(hunk)["confidence"].as_f64().unwrap()
}

/// Lines that separate two edits into two hunks.
fn filler() -> String {
    "  touch\n".repeat(8)
}

const TOTAL: &str = "class Cart\n  def total(items)\n    sum = 0\n    items.each { |item| sum += item.price }\n    sum\n  end\nend\n";

#[test]
fn a_local_variable_renamed_in_a_def_is_a_rename() {
    let report = one(TOTAL, &TOTAL.replace("sum", "subtotal"));
    assert_eq!(kinds(&report), ["rename"]);
    let hunk = &report["hunks"][0];
    assert_eq!(confidence(hunk), 1.0);
    assert_eq!(
        lead(hunk)["detail"]["renames"],
        serde_json::json!([{"from": "sum", "to": "subtotal", "scope": "local", "occurrences": 3}])
    );
    assert_eq!(lead(hunk)["evidence"]["renamed"], 3);
    assert_eq!(lead(hunk)["evidence"]["uses"], 3);
    assert_eq!(lead(hunk)["evidence"]["unseen_uses"], false);
    assert_eq!(hunk["review_lines"], 0);
    assert_eq!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_rename_that_also_rewords_a_comment_is_a_rename() {
    let before = "# Adds one.\ndef bump(n)\n  n + 1\nend\n";
    let after = "# Adds one to a count.\n# Pure.\ndef bump(count)\n  count + 1\nend\n";
    assert_eq!(kinds(&one(before, after)), ["rename"]);
}

#[test]
fn a_local_use_left_behind_is_logic() {
    // The leftover `sum` is no longer a local: Ruby now calls a method.
    let after = TOTAL
        .replace("sum = 0", "subtotal = 0")
        .replace("sum +=", "subtotal +=");
    logic_because(&one(TOTAL, &after), "sum is still used");
}

#[test]
fn a_rename_spread_over_hunks_is_checked_across_them() {
    let before = format!("def run\n  value = 1\n{}  value\nend\n", filler());
    let both = before.replace("value", "amount");
    assert_eq!(kinds(&one(&before, &both)), ["rename", "rename"]);
    let first_only = before.replacen("value", "amount", 1);
    logic_because(&one(&before, &first_only), "value is still used");
}

#[test]
fn renaming_onto_an_existing_name_is_logic() {
    let before = "def run\n  a = 1\n  b = 2\n  a + b\nend\n";
    let after = "def run\n  b = 1\n  b = 2\n  b + b\nend\n";
    logic_because(&one(before, after), "b already exists");
}

#[test]
fn renaming_a_local_onto_a_method_called_in_scope_is_logic() {
    // The untouched `size` was a method call; after the rename it reads the local.
    let before = format!("def run\n  n = 1\n  puts n\n{}  size\nend\n", filler());
    let after = before.replacen("n = 1\n  puts n", "size = 1\n  puts size", 1);
    logic_because(&one(&before, &after), "size already exists");
}

#[test]
fn a_parameter_rename_is_a_rename_but_adding_one_is_logic() {
    let renamed = TOTAL.replace("items", "lines");
    assert_eq!(kinds(&one(TOTAL, &renamed)), ["rename"]);
    let added = TOTAL.replace("total(items)", "total(items, tax)");
    assert_eq!(kinds(&one(TOTAL, &added)), ["logic"]);
    let defaulted = TOTAL.replace("total(items)", "total(items = [])");
    assert_eq!(kinds(&one(TOTAL, &defaulted)), ["logic"]);
}

#[test]
fn swapping_parameters_is_logic() {
    let before = "def pair(a, b)\n  [a, b]\nend\n";
    let after = "def pair(b, a)\n  [b, a]\nend\n";
    logic_because(&one(before, after), "is still used");
}

#[test]
fn a_keyword_parameter_rename_is_logic() {
    // Callers pass `rate:` by name, so the method's interface changed.
    let before = "def charge(rate:)\n  rate * 2\nend\n";
    let after = "def charge(fee:)\n  fee * 2\nend\n";
    logic_because(&one(before, after), "keyword parameter");
}

#[test]
fn a_rename_mixed_with_a_logic_edit_is_logic() {
    let after = TOTAL.replace("sum", "subtotal").replace("= 0", "= 1");
    assert_eq!(kinds(&one(TOTAL, &after)), ["logic"]);
}

const GREETER: &str = "class Greeter\n  def greet(name)\n    \"hi #{name}\"\n  end\n\n  def welcome\n    greet(\"you\") + self.greet(\"all\")\n  end\nend\n";

#[test]
fn a_method_rename_with_every_call_site_is_a_graded_rename() {
    let report = one(GREETER, &GREETER.replace("greet", "salute"));
    assert_eq!(kinds(&report), ["rename"]);
    let hunk = &report["hunks"][0];
    assert_eq!(
        lead(hunk)["detail"]["renames"],
        serde_json::json!([{"from": "greet", "to": "salute", "scope": "method", "occurrences": 3}])
    );
    assert_eq!(lead(hunk)["evidence"]["unseen_uses"], true);
    // Callers outside the diff can't be seen: (3 + 1) / (3 + 2).
    assert_eq!(confidence(hunk), 0.8);
}

#[test]
fn a_method_rename_across_files_checks_every_file() {
    let caller = "class Door\n  def open(g)\n    g.greet(\"x\")\n  end\nend\n";
    let renamed = GREETER.replace("greet", "salute");
    let report = classify(
        &[("a.rb", GREETER), ("b.rb", caller)],
        &[
            ("a.rb", &renamed),
            ("b.rb", &caller.replace("greet", "salute")),
        ],
    );
    assert_eq!(kinds(&report), ["rename", "rename"]);
    assert_eq!(
        lead(&report["hunks"][0])["detail"]["renames"][0]["occurrences"],
        4
    );

    let stale = "class Door\n  def open(g)\n    g.greet(\"x\")\n    g.close\n  end\nend\n";
    let report = classify(
        &[("a.rb", GREETER), ("b.rb", caller)],
        &[("a.rb", &renamed), ("b.rb", stale)],
    );
    logic_because(&report, "greet is still used");
}

#[test]
fn a_call_renamed_without_its_definition_is_logic() {
    // Calls a different method.
    let before = "def run\n  start\nend\n";
    let after = "def run\n  begin_now\nend\n";
    logic_because(&one(before, after), "definition isn't renamed");
}

#[test]
fn a_method_rename_leaving_send_of_the_old_name_is_logic() {
    // `send(:greet)` would now raise NoMethodError.
    let before = format!("{GREETER}Greeter.new.send(:greet, \"x\")\n");
    let after = GREETER.replace("greet", "salute") + "Greeter.new.send(:greet, \"x\")\n";
    logic_because(&one(&before, &after), "greet is still used");
}

#[test]
fn a_method_rename_that_updates_send_is_a_rename() {
    let before = format!("{GREETER}Greeter.new.send(:greet, \"x\")\n");
    let after = before.replace("greet", "salute");
    let report = one(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "rename"), "{report}");
}

#[test]
fn a_method_rename_leaving_a_symbol_of_the_old_name_is_logic() {
    // A DSL (here a callback) names the method by symbol.
    let before = format!(
        "class Page\n  before_save :tidy\n{}  def tidy\n    1\n  end\nend\n",
        filler()
    );
    let after = before.replace("def tidy", "def clean");
    logic_because(&one(&before, &after), "tidy is still used");
}

#[test]
fn renaming_a_method_onto_an_existing_one_is_logic() {
    let before = "def a\n  1\nend\n\ndef b\n  2\nend\n\na\n";
    let after = "def b\n  1\nend\n\ndef b\n  2\nend\n\nb\n";
    logic_because(&one(before, after), "b already exists");
}

#[test]
fn renaming_a_method_that_calls_super_is_logic() {
    // `super` now reaches the parent's method of the new name.
    let before = "class Kid < Base\n  def go\n    super\n  end\nend\n";
    let after = before.replace("go", "run");
    logic_because(&one(before, &after), "super");
}

#[test]
fn a_class_rename_counts_its_name_once() {
    let before = "class Box\n  def self.build\n    Box.new\n  end\nend\n";
    let report = one(before, &before.replace("Box", "Crate"));
    assert_eq!(kinds(&report), ["rename"]);
    let rename = &lead(&report["hunks"][0])["detail"]["renames"][0];
    assert_eq!(
        (rename["scope"].as_str(), rename["occurrences"].as_u64()),
        (Some("constant"), Some(2))
    );
}

#[test]
fn an_instance_variable_must_be_renamed_in_every_method() {
    let before = format!(
        "class Pot\n  def initialize\n    @heat = 1\n  end\n{}  def hot?\n    @heat > 5\n  end\nend\n",
        filler()
    );
    let report = one(&before, &before.replace("@heat", "@temp"));
    assert_eq!(kinds(&report), ["rename", "rename"]);
    assert_eq!(lead(&report["hunks"][0])["evidence"]["unseen_uses"], true);
    logic_because(
        &one(&before, &before.replacen("@heat", "@temp", 1)),
        "@heat is still used",
    );
}

#[test]
fn renaming_an_instance_variable_behind_an_attr_reader_is_logic() {
    let before = "class Pot\n  attr_reader :heat\n  def initialize\n    @heat = 1\n  end\nend\n";
    let after = before.replace("@heat", "@temp");
    logic_because(&one(before, &after), "@heat is still used");
}

#[test]
fn rename_fields_are_documented() {
    let schema =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/schema.md")).unwrap();
    let report = one(TOTAL, &TOTAL.replace("sum", "subtotal"));
    let hunk = &report["hunks"][0];
    let records = [
        &lead(hunk)["evidence"],
        &lead(hunk)["detail"],
        &lead(hunk)["detail"]["renames"][0],
    ];
    for record in records {
        for key in record.as_object().unwrap().keys() {
            assert!(
                schema.contains(&format!("`{key}`")),
                "docs/schema.md doesn't name `{key}`"
            );
        }
    }
}
