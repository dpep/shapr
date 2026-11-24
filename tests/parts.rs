//! End to end: a hunk's parts (the structural changes proven in it) and its
//! residual (the lines no part explains).

use serde_json::{Value, json};
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

/// `shapr -j` over one commit writing each (path, content) pair.
fn classify(before: &[(&str, &str)], after: &[(&str, &str)]) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "Test"]);
    for files in [before, after] {
        for (name, content) in files {
            std::fs::write(path.join(name), content).unwrap();
        }
        git(path, &["add", "-A"]);
        git(path, &["commit", "-q", "--allow-empty", "-m", "state"]);
    }
    let out = command(env!("CARGO_BIN_EXE_shapr"), path)
        .args(["-j", "HEAD~1..HEAD"])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

fn one(before: &str, after: &str) -> Value {
    classify(&[("a.rb", before)], &[("a.rb", after)])
}

fn hunks(report: &Value) -> &Vec<Value> {
    report["hunks"].as_array().unwrap()
}

fn part_kinds(hunk: &Value) -> Vec<&str> {
    hunk["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect()
}

fn part<'h>(hunk: &'h Value, kind: &str) -> &'h Value {
    hunk["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} part: {hunk:#}"))
}

/// A part's lines as (before, after) line numbers.
fn lines(part: &Value) -> (Vec<u64>, Vec<u64>) {
    let side = |key: &str| {
        part["lines"][key]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|run| {
                let start = run["start"].as_u64().unwrap();
                start..start + run["count"].as_u64().unwrap()
            })
            .collect()
    };
    (side("before"), side("after"))
}

const CART: &str = "class Cart
  def subtotal(items)
    total = items.sum(&:price)
    total * 0.1
  end
end
";

const CHECKOUT: &str = "class Checkout
  def charge(cart, items)
    cart.subtotal(items)
  end
end
";

#[test]
fn a_rename_and_an_edit_in_one_hunk_are_a_rename_part_and_a_residual() {
    let after = CART
        .replace("subtotal", "items_total")
        .replace("0.1", "0.2");
    let report = classify(
        &[("cart.rb", CART), ("checkout.rb", CHECKOUT)],
        &[
            ("cart.rb", &after),
            ("checkout.rb", &CHECKOUT.replace("subtotal", "items_total")),
        ],
    );
    let [cart, checkout] = &hunks(&report)[..] else {
        panic!("{report:#}")
    };
    assert_eq!(part_kinds(cart), ["rename", "logic"], "{report:#}");
    assert_eq!(cart["kind"], "logic");
    assert_eq!(cart["verdict"], "mixed");
    assert_eq!(lines(part(cart, "rename")), (vec![2], vec![2]));
    let residual = part(cart, "logic");
    assert_eq!(lines(residual), (vec![4], vec![4]));
    assert_eq!(residual["facets"], json!(["literal_only"]));
    assert_eq!(cart["review_lines"], 2);
    assert_eq!(
        part(cart, "rename")["detail"]["renames"],
        json!([{"from": "subtotal", "to": "items_total", "scope": "method", "occurrences": 2}])
    );
    // The call site's rename is backed by the definition's, inside the edit.
    assert_eq!(checkout["kind"], "rename", "{report:#}");
    assert_eq!(checkout["verdict"], "structure");
    for part in [cart, checkout]
        .iter()
        .flat_map(|h| h["parts"].as_array().unwrap())
    {
        let reason = part["reason"].as_str().unwrap();
        assert!(!reason.contains("isn't renamed"), "{reason}");
    }
    assert_eq!(report["summary"]["verdict"], "mixed");
    assert_eq!(report["summary"]["kinds"]["rename"], 2);
    assert_eq!(report["summary"]["kinds"]["logic"], 1);
}

#[test]
fn a_rename_and_a_reformat_are_two_structure_parts() {
    let before = "def area(n)\n  m  =  1\n  n + m\nend\n";
    let after = "def area(count)\n  m = 1\n  count + m\nend\n";
    let report = one(before, after);
    let hunk = &hunks(&report)[0];
    assert_eq!(part_kinds(hunk), ["format_only", "rename"], "{report:#}");
    assert_eq!(lines(part(hunk, "format_only")), (vec![2], vec![2]));
    assert_eq!(lines(part(hunk, "rename")), (vec![1, 3], vec![1, 3]));
    assert_eq!(hunk["kind"], "rename");
    assert_eq!(hunk["verdict"], "structure");
    assert_eq!(part(hunk, "rename")["confidence"], 1.0);
    assert_eq!(hunk["review_lines"], 0);
    assert_eq!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_reformat_beside_an_edit_is_a_format_part_and_a_residual() {
    let before = "def area\n  x  =  1\n  y = 2\nend\n";
    let after = "def area\n  x = 1\n  y = 3\nend\n";
    let report = one(before, after);
    let hunk = &hunks(&report)[0];
    assert_eq!(part_kinds(hunk), ["format_only", "logic"], "{report:#}");
    assert_eq!(lines(part(hunk, "format_only")), (vec![2], vec![2]));
    assert_eq!(lines(part(hunk, "logic")), (vec![3], vec![3]));
    assert_eq!(hunk["kind"], "logic");
    assert_eq!(part(hunk, "logic")["confidence"], 1.0);
    assert_eq!(hunk["review_lines"], 2);
    assert_eq!(report["summary"]["verdict"], "mixed");
}

#[test]
fn a_rename_whose_new_name_new_code_also_defines_is_logic() {
    let before =
        "class Meter\n  def compute\n    1\n  end\n\n  def run\n    compute + 2\n  end\nend\n";
    let after = before.replace("compute", "total");
    let gauge = "class Gauge\n  def total\n    3\n  end\nend\n";
    let renamed_alone = classify(&[("meter.rb", before)], &[("meter.rb", &after)]);
    assert_eq!(
        hunks(&renamed_alone)[0]["kind"],
        "rename",
        "{renamed_alone:#}"
    );
    // The same rename, while new code in another file defines the new name.
    let report = classify(
        &[("meter.rb", before)],
        &[("meter.rb", &after), ("gauge.rb", gauge)],
    );
    for hunk in hunks(&report) {
        assert_eq!(part_kinds(hunk), ["logic"], "{report:#}");
    }
    let reasons: Vec<&str> = hunks(&report)
        .iter()
        .flat_map(|h| h["parts"].as_array().unwrap())
        .map(|p| p["reason"].as_str().unwrap())
        .collect();
    assert!(
        reasons.iter().any(|r| r.contains("total is also defined")),
        "{reasons:?}"
    );
    assert_eq!(report["summary"]["verdict"], "behavior");
}

/// A method with a few statements, so it is well above the size floor.
fn method(name: &str, value: &str) -> String {
    format!("  def {name}(x)\n    y = x * {value}\n    log(y)\n    y + 1\n  end\n")
}

fn class(name: &str, body: &[&str]) -> String {
    format!("class {name}\n{}end\n", body.join("\n"))
}

#[test]
fn a_move_with_an_edit_is_a_move_part_and_a_residual() {
    let a = method("first", "2");
    let filler: Vec<String> = (0..3).map(|i| method(&format!("m{i}"), "5")).collect();
    let b = method("second", "3");
    let changed = b.replace("log(y)", "warn(y)");
    let mut before: Vec<&str> = vec![&b, &a];
    before.extend(filler.iter().map(String::as_str));
    let mut after: Vec<&str> = vec![&a];
    after.extend(filler.iter().map(String::as_str));
    after.push(&changed);
    let report = one(&class("Shape", &before), &class("Shape", &after));
    let [from, to] = &hunks(&report)[..] else {
        panic!("{report:#}")
    };
    // Each half carries its side of the edited line.
    for (hunk, side) in [(from, 0), (to, 1)] {
        assert_eq!(part_kinds(hunk), ["move", "logic"], "{report:#}");
        assert_eq!(hunk["kind"], "logic");
        assert_eq!(hunk["review_lines"], 1);
        let residual = lines(part(hunk, "logic"));
        let edited = [residual.0, residual.1][side].clone();
        assert!(!edited.is_empty(), "{report:#}");
    }
    assert_eq!(part(to, "move")["structure"], true);
    assert_eq!(report["summary"]["verdict"], "mixed");
}

#[test]
fn a_move_into_another_class_is_a_move_part_that_is_not_structure() {
    let a = method("first", "2");
    let b = method("second", "3");
    let source = format!(
        "{}\n{}",
        class("Left", &[&a, &b]),
        class("Right", &[&method("z", "1")])
    );
    let target = format!(
        "{}\n{}",
        class("Left", &[&a]),
        class("Right", &[&method("z", "1"), &b])
    );
    let report = one(&source, &target);
    for hunk in hunks(&report) {
        let moved = part(hunk, "move");
        assert_eq!(moved["structure"], false, "{report:#}");
        assert_eq!(moved["detail"]["context"], json!(["owner"]));
        assert_eq!(hunk["kind"], "logic");
        assert_eq!(hunk["verdict"], "behavior");
        assert_eq!(hunk["review_lines"], 1);
    }
    assert_eq!(report["summary"]["verdict"], "behavior");
    assert_eq!(report["summary"]["move_contexts"]["owner"], 2);
}

const RENDER: &str = "class Report
  def render(title)
    puts title
    puts \"-\" * 10
    puts \"=\" * 10
    puts \"end\"
  end

  def footer
    puts 1
  end
end
";

const RENDER_EXTRACTED: &str = "class Report
  def render(title)
    puts title
    rule
    puts \"end\"
  end

  def rule
    puts \"-\" * 10
    puts \"=\" * 10
  end

  def footer
    puts 2
  end
end
";

#[test]
fn an_extract_beside_an_edit_is_an_extract_part_and_a_residual() {
    let report = one(RENDER, RENDER_EXTRACTED);
    let hunk = &hunks(&report)[0];
    assert!(part_kinds(hunk).contains(&"extract"), "{report:#}");
    assert_eq!(hunk["kind"], "logic");
    // Only the edited sibling is left over.
    assert_eq!(lines(part(hunk, "logic")), (vec![10], vec![14]));
    assert_eq!(report["summary"]["verdict"], "mixed");
}
