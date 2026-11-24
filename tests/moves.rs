//! End to end: move detection, alone and beside an edit or a new context.

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

fn hunks(report: &Value) -> &Vec<Value> {
    report["hunks"].as_array().unwrap()
}

fn kinds(report: &Value) -> Vec<&str> {
    hunks(report)
        .iter()
        .map(|h| h["kind"].as_str().unwrap())
        .collect()
}

/// A method with a few statements, so it is well above the size floor.
fn method(name: &str, value: &str) -> String {
    format!("  def {name}(x)\n    y = x * {value}\n    log(y)\n    y + 1\n  end\n")
}

fn class(name: &str, body: &[&str]) -> String {
    format!("class {name}\n{}end\n", body.join("\n"))
}

/// Every move entry's hunk ids point at hunks labeled with a move kind, and
/// each hunk is one of its own entry's ends.
fn assert_cross_referenced(report: &Value) {
    let hunks = hunks(report);
    for hunk in hunks {
        let moves = lead(hunk)["detail"]["moves"].as_array().unwrap();
        assert!(!moves.is_empty());
        for entry in moves {
            let (from, to) = (entry["from_hunk"].as_u64(), entry["to_hunk"].as_u64());
            assert!(from == hunk["id"].as_u64() || to == hunk["id"].as_u64());
            for end in [from, to] {
                let other = &hunks[end.unwrap() as usize];
                assert_eq!(other["kind"], hunk["kind"]);
            }
        }
    }
}

/// The hunk's `move` part.
fn move_part(hunk: &Value) -> &Value {
    hunk["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "move")
        .unwrap_or_else(|| panic!("no move part: {hunk:#}"))
}

/// Every hunk is a move into a new `context`: a `move` part that isn't
/// structure, so the hunk is `logic`, reviewing only its moved definitions'
/// first lines (where the context shows).
fn assert_context_move(report: &Value, context: &str) {
    assert!(kinds(report).iter().all(|k| *k == "logic"), "{report:#}");
    for hunk in hunks(report) {
        assert_eq!(move_part(hunk)["structure"], false, "{hunk:#}");
    }
    assert_cross_referenced(report);
    for hunk in hunks(report) {
        assert_eq!(
            lead(hunk)["detail"]["context"],
            serde_json::json!([context])
        );
        assert_eq!(lead(hunk)["evidence"]["similarity"], 1.0);
        // One line per end of a move in this hunk: the definition's first.
        let ends: usize = lead(hunk)["detail"]["moves"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                usize::from(m["from_hunk"] == hunk["id"]) + usize::from(m["to_hunk"] == hunk["id"])
            })
            .sum();
        assert_eq!(hunk["review_lines"], ends, "{hunk:#}");
    }
    let summary = &report["summary"];
    assert_eq!(summary["move_contexts"][context], hunks(report).len());
    assert_eq!(summary["verdict"], "behavior");
}

#[test]
fn swapping_two_methods_is_a_move() {
    let a = method("first", "2");
    let b = method("second", "3");
    let report = classify(
        &[("shape.rb", &class("Shape", &[&a, &b]))],
        &[("shape.rb", &class("Shape", &[&b, &a]))],
    );
    assert!(kinds(&report).iter().all(|k| *k == "move"), "{report:#}");
    assert_cross_referenced(&report);
    let hunk = &hunks(&report)[0];
    assert_eq!(lead(hunk)["evidence"]["similarity"], 1.0);
    assert_eq!(lead(hunk)["evidence"]["candidates"], 1);
    assert!(lead(hunk)["confidence"].as_f64().unwrap() > 0.8);
    assert_eq!(hunk["review_lines"], 0);
    assert_eq!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_method_moved_to_another_file_is_a_move() {
    let area = method("area", "2");
    let perimeter = method("perimeter", "4");
    let volume = method("volume", "8");
    let report = classify(
        &[
            ("a.rb", &class("Shape", &[&area, &perimeter])),
            ("b.rb", &class("Shape", &[&volume])),
        ],
        &[
            ("a.rb", &class("Shape", &[&area])),
            ("b.rb", &class("Shape", &[&volume, &perimeter])),
        ],
    );
    assert_eq!(kinds(&report), ["move", "move"], "{report:#}");
    assert_cross_referenced(&report);
    let entry = &lead(&hunks(&report)[0])["detail"]["moves"][0];
    assert_eq!(entry["name"], "perimeter");
    assert_eq!(
        (entry["from_hunk"].as_u64(), entry["to_hunk"].as_u64()),
        (Some(0), Some(1))
    );
}

#[test]
fn a_moved_method_with_a_changed_line_is_a_move_part_and_logic() {
    let a = method("first", "2");
    let filler: Vec<String> = (0..3).map(|i| method(&format!("m{i}"), "5")).collect();
    let b = method("second", "3");
    let changed = b.replace("log(y)", "warn(y)");
    let mut before: Vec<&str> = vec![&b, &a];
    before.extend(filler.iter().map(String::as_str));
    let mut after: Vec<&str> = vec![&a];
    after.extend(filler.iter().map(String::as_str));
    after.push(&changed);
    let after = class("Shape", &after);
    let report = classify(
        &[("shape.rb", &class("Shape", &before))],
        &[("shape.rb", &after)],
    );
    assert_eq!(kinds(&report), ["logic", "logic"], "{report:#}");
    let added = &hunks(&report)[1];
    assert_eq!(move_part(added)["structure"], true);
    let entry = &move_part(added)["detail"]["moves"][0];
    let similarity = entry["similarity"].as_f64().unwrap();
    assert!(similarity < 1.0 && similarity > 0.5);
    let line = after.lines().position(|l| l.contains("warn(y)")).unwrap() + 1;
    assert_eq!(entry["changed_after"], serde_json::json!([line]));
    // The edited line is the residual; the rest of the body moved.
    let residual = added["parts"].as_array().unwrap().last().unwrap();
    assert_eq!(residual["kind"], "logic");
    assert_eq!(added["review_lines"], 1);
    assert_eq!(report["summary"]["verdict"], "mixed");
}

#[test]
fn a_method_moved_below_private_is_a_move_in_a_new_visibility() {
    let a = method("first", "2");
    let b = method("second", "3");
    let c = method("third", "4");
    let report = classify(
        &[("shape.rb", &class("Shape", &[&a, &b, "  private\n", &c]))],
        &[("shape.rb", &class("Shape", &[&b, "  private\n", &c, &a]))],
    );
    assert_context_move(&report, "visibility");
}

#[test]
fn a_constant_moved_out_from_under_private_is_a_move() {
    // `private` applies to methods: a constant below it is public anyway.
    let a = method("first", "2");
    let limits = "  LIMITS = { low: 1, high: 9 }.freeze\n";
    let report = classify(
        &[("shape.rb", &class("Shape", &[&a, "  private\n", limits]))],
        &[("shape.rb", &class("Shape", &[limits, &a, "  private\n"]))],
    );
    assert!(kinds(&report).iter().all(|k| *k == "move"), "{report:#}");
}

#[test]
fn a_method_moved_into_another_class_is_a_move_to_a_new_owner() {
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
    let report = classify(&[("shape.rb", &source)], &[("shape.rb", &target)]);
    assert_context_move(&report, "owner");
}

#[test]
fn a_method_changed_on_its_way_to_another_class_reviews_both() {
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
        class(
            "Right",
            &[&method("z", "1"), &b.replace("log(y)", "warn(y)")]
        )
    );
    let report = classify(&[("shape.rb", &source)], &[("shape.rb", &target)]);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
    let added = &hunks(&report)[1];
    let moved = move_part(added);
    assert_eq!(moved["detail"]["context"], serde_json::json!(["owner"]));
    assert_eq!(moved["structure"], false);
    let entry = &moved["detail"]["moves"][0];
    let line = target.lines().position(|l| l.contains("warn(y)")).unwrap() + 1;
    assert_eq!(entry["changed_after"], serde_json::json!([line]));
    // The changed line, and the definition's first line in its new owner.
    assert_eq!(added["review_lines"], 2, "{added:#}");
}

#[test]
fn a_method_moved_past_a_statement_naming_it_is_a_move_in_a_new_load_order() {
    let a = method("first", "2");
    let b = method("second", "3");
    let report = classify(
        &[(
            "shape.rb",
            &class("Shape", &[&a, "  alias_method :primary, :first\n", &b]),
        )],
        &[(
            "shape.rb",
            &class("Shape", &["  alias_method :primary, :first\n", &b, &a]),
        )],
    );
    assert_context_move(&report, "load_order");
}

#[test]
fn a_method_moved_to_a_file_with_other_directives_is_a_move_under_new_directives() {
    let frozen = "# frozen_string_literal: true\n\n";
    let (area, perimeter) = (method("area", "2"), method("perimeter", "4"));
    let volume = method("volume", "8");
    let report = classify(
        &[
            ("a.rb", &class("Shape", &[&area, &perimeter])),
            ("b.rb", &format!("{frozen}{}", class("Shape", &[&volume]))),
        ],
        &[
            ("a.rb", &class("Shape", &[&area])),
            (
                "b.rb",
                &format!("{frozen}{}", class("Shape", &[&volume, &perimeter])),
            ),
        ],
    );
    assert_context_move(&report, "directives");
}

#[test]
fn swapping_two_dsl_calls_is_logic() {
    let before = "class Shape\n  validates :name, presence: true, length: { maximum: 9 }\n  \
                  before_save :normalize, if: :changed?\nend\n";
    let after = "class Shape\n  before_save :normalize, if: :changed?\n  \
                 validates :name, presence: true, length: { maximum: 9 }\nend\n";
    let report = classify(&[("shape.rb", before)], &[("shape.rb", after)]);
    assert_eq!(kinds(&report), ["logic"]);
}

#[test]
fn trivial_code_is_never_matched() {
    // A bare `nil` and a one-literal constant are below the size floor.
    let a = method("first", "2");
    let before = format!("LIMIT = 1\n{}nil\n", class("Shape", &[&a]));
    let after = format!("nil\n{}LIMIT = 1\n", class("Shape", &[&a]));
    let report = classify(&[("shape.rb", &before)], &[("shape.rb", &after)]);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn methods_edited_in_place_are_not_moves() {
    // `rescue` gives the body a node spanning the whole `def`, keywords and
    // all; such a `def` must still align by its own hash.
    let def = |name: &str, guard: bool| {
        let body = "    y = 1\n".repeat(8);
        let tail = if guard {
            "  rescue StandardError\n    reset\n"
        } else {
            ""
        };
        format!("  def {name}\n{body}{tail}  end\n")
    };
    let report = classify(
        &[(
            "shape.rb",
            &class(
                "Shape",
                &[
                    &def("a", false),
                    &def("b", false),
                    &def("c", false),
                    &def("d", true),
                    &def("e", false),
                ],
            ),
        )],
        &[(
            "shape.rb",
            &class(
                "Shape",
                &[
                    &def("a", false),
                    &def("b", true),
                    &def("c", true),
                    &def("d", true),
                    &def("e", false),
                ],
            ),
        )],
    );
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn an_edit_inside_a_namespace_is_not_a_move() {
    // The requires outnumber the one module, but the module is what aligns.
    let requires = "require \"a\"\nrequire \"b\"\nrequire \"c\"\n";
    let reordered = "require \"a\"\nrequire \"c\"\nrequire \"b\"\n";
    let second = method("second", "3");
    let rest = [method("third", "4"), method("fourth", "5")];
    let body = class(
        "Shape",
        &[&method("first", "2"), &second, &rest[0], &rest[1]],
    );
    let edited = body.replace(&second, &second.replace("log", "warn"));
    let report = classify(
        &[("shape.rb", &format!("{requires}\n{body}"))],
        &[("shape.rb", &format!("{edited}\n{reordered}"))],
    );
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn a_method_edited_where_neighbors_swap_around_it_stays_put() {
    // `middle` is edited in place; `first` and `last` trade places around it.
    let long = |name: &str, value: &str| {
        method(name, value).replace("    log(y)\n", &"    log(y)\n".repeat(6))
    };
    let (first, middle, last) = (
        method("first", "2"),
        long("middle", "3"),
        method("last", "4"),
    );
    let edited = middle.replacen("log(y)", "warn(y)", 1);
    let report = classify(
        &[("shape.rb", &class("Shape", &[&first, &middle, &last]))],
        &[("shape.rb", &class("Shape", &[&last, &edited, &first]))],
    );
    let names_middle = hunks(&report).iter().any(|h| {
        lead(h)["detail"]["moves"]
            .as_array()
            .is_some_and(|m| m.iter().any(|e| e["name"] == "middle"))
    });
    assert!(!names_middle, "{report:#}");
}

#[test]
fn a_constant_moved_past_a_require_is_a_move_in_a_new_load_order() {
    // The required file may read the constant when it loads.
    let root = "ROOT = File.expand_path(File.join(__dir__, \"..\"))\n";
    let requires = "require \"a\"\nrequire \"b\"\nrequire \"c\"\nrequire \"d\"\n";
    let report = classify(
        &[("helper.rb", &format!("{root}{requires}"))],
        &[("helper.rb", &format!("{requires}{root}"))],
    );
    assert_context_move(&report, "load_order");
}

#[test]
fn a_duplicate_candidate_lowers_confidence() {
    let a = method("first", "2");
    let b = method("second", "3");
    let unique = classify(
        &[("shape.rb", &class("Shape", &[&a, &b]))],
        &[("shape.rb", &class("Shape", &[&b, &a]))],
    );
    let other = class("Other", &[&method("z", "1")]);
    let twice = class("Other", &[&method("z", "1"), &a]);
    let ambiguous = classify(
        &[
            ("shape.rb", &class("Shape", &[&a, &b])),
            ("other.rb", &other),
        ],
        &[
            ("shape.rb", &class("Shape", &[&b, &a])),
            ("other.rb", &twice),
        ],
    );
    let confidence = |r: &Value, file: &str| -> f64 {
        hunks(r)
            .iter()
            .filter(|h| h["file"] == file && h["kind"] == "move")
            .map(|h| lead(h)["confidence"].as_f64().unwrap())
            .fold(f64::INFINITY, f64::min)
    };
    // The copy added to `Other` is new code, and the move it might have been
    // is split between two candidates.
    let added = hunks(&ambiguous)
        .iter()
        .find(|h| h["file"] == "other.rb")
        .unwrap();
    assert_eq!(added["kind"], "logic", "{ambiguous:#}");
    let moved = hunks(&ambiguous)
        .iter()
        .find(|h| h["file"] == "shape.rb" && h["kind"] == "move")
        .unwrap();
    assert_eq!(lead(moved)["evidence"]["candidates"], 2);
    assert!(confidence(&ambiguous, "shape.rb") < confidence(&unique, "shape.rb"));
}

#[test]
fn move_fields_are_documented() {
    let schema =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/schema.md")).unwrap();
    let a = method("first", "2");
    let b = method("second", "3");
    let changed = b.replace("log(y)", "warn(y)");
    let filler = method("m", "5");
    let report = classify(
        &[("shape.rb", &class("Shape", &[&b, &a, &filler]))],
        &[("shape.rb", &class("Shape", &[&a, &filler, &changed]))],
    );
    let moved = move_part(&hunks(&report)[0]);
    let records = [
        &moved["evidence"],
        &moved["detail"],
        &moved["detail"]["moves"][0],
        &report["summary"]["move_contexts"],
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
