//! End to end: `summary.claims`, hints read from commit messages and a
//! description, kept apart from what the code shows.

use serde_json::{Value, json};

mod common;
use common::*;

/// A repo whose first commit holds `before` and whose second, with
/// `message`, holds `after`.
fn gate_repo(before: &str, after: &str, message: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    commit(dir.path(), &[("gate.rb", before)], "Start");
    commit(dir.path(), &[("gate.rb", after)], message);
    dir
}

/// The last commit's report, its claims read from its message.
fn classify(before: &str, after: &str, message: &str) -> (Value, String) {
    let dir = gate_repo(before, after, message);
    let sha = git(dir.path(), &["rev-parse", "HEAD"]).trim()[..12].to_string();
    (report(&shapr(dir.path(), &["-j", "HEAD~1..HEAD"])), sha)
}

const GATE: &str = "class Gate\n  def open?(n)\n    if n > 1\n      :open\n    else\n      :shut\n    end\n  end\nend\n";

fn flipped() -> String {
    GATE.replace("n > 1", "n >= 1")
}

#[test]
fn a_fix_commit_is_a_claim_beside_the_code_facts() {
    let (report, sha) = classify(GATE, &flipped(), "fix: open the gate at one");
    let claims = &report["summary"]["claims"];
    assert_eq!(claims["messages"], 1);
    assert_eq!(
        claims["hints"],
        json!([{"hint": "type", "value": "fix", "category": "fix", "source": "commit", "commit": sha, "field": "subject"}])
    );
    assert_eq!(claims["experimental"]["categories"]["fix"], 1);
    assert_eq!(
        claims["experimental"]["categories"]["feature"], 0,
        "{claims}"
    );
    assert_eq!(claims["experimental"]["conflicts"], json!([]));
    // The claim never touches the code's labels.
    assert_eq!(report["hunks"][0]["kind"], "logic");
    assert_eq!(lead(&report["hunks"][0])["facets"], json!(["condition"]));
}

#[test]
fn a_refactor_claim_with_behavior_hunks_is_a_conflict() {
    let (report, _) = classify(GATE, &flipped(), "Refactor the gate check");
    let claims = &report["summary"]["claims"];
    assert_eq!(claims["hints"][0]["hint"], "keyword");
    assert_eq!(claims["hints"][0]["value"], "refactor");
    assert_eq!(claims["experimental"]["categories"]["refactor"], 1);
    assert_eq!(
        claims["experimental"]["conflicts"],
        json!([{"claim": "refactor", "finding": "behavior_hunks", "count": 1}])
    );
}

#[test]
fn a_no_behavior_change_phrase_is_a_refactor_claim() {
    let (report, _) = classify(
        GATE,
        &flipped(),
        "Tidy the gate\n\nNo functional change intended.",
    );
    let claims = &report["summary"]["claims"];
    let hints = claims["hints"].as_array().unwrap();
    assert!(
        hints.iter().any(|h| h["hint"] == "no_behavior_change"
            && h["value"] == "no functional change"
            && h["field"] == "body"),
        "{claims}"
    );
    assert_eq!(claims["experimental"]["conflicts"][0]["claim"], "refactor");
}

#[test]
fn a_fix_claim_with_only_structure_is_a_conflict() {
    let after = GATE.replace("    if n > 1", "    if n >   1");
    let (report, _) = classify(GATE, &after, "Fix the gate");
    assert_eq!(report["summary"]["verdict"], "structure");
    assert_eq!(
        report["summary"]["claims"]["experimental"]["conflicts"],
        json!([{"claim": "fix", "finding": "no_behavior_hunks", "count": 0}])
    );
}

#[test]
fn a_revert_names_the_commit_and_claims_nothing_it_quotes() {
    let message = "Revert \"fix: open the gate at one\"\n\nThis reverts commit 0123456789abcdef0123456789abcdef01234567.";
    let (report, _) = classify(&flipped(), GATE, message);
    let claims = &report["summary"]["claims"];
    let hints: Vec<(&str, &str)> = claims["hints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| (h["hint"].as_str().unwrap(), h["value"].as_str().unwrap()))
        .collect();
    assert_eq!(
        hints,
        [
            ("revert", "fix: open the gate at one"),
            ("revert", "0123456789abcdef0123456789abcdef01234567")
        ]
    );
    assert_eq!(claims["experimental"]["categories"]["revert"], 1);
    assert_eq!(claims["experimental"]["categories"]["fix"], 0, "{claims}");
}

#[test]
fn a_breaking_feature_is_typed_and_flagged() {
    let (report, _) = classify(GATE, &flipped(), "feat(gate)!: open at one");
    let hints = &report["summary"]["claims"]["hints"];
    assert_eq!(hints[0]["value"], "feat");
    assert_eq!(hints[0]["category"], "feature");
    assert_eq!(hints[1]["hint"], "breaking");
}

#[test]
fn a_description_file_adds_issue_references() {
    let dir = gate_repo(GATE, &flipped(), "Start over");
    std::fs::write(
        dir.path().join("pr.md"),
        "Handle small gates\n\nFixes #12, see PROJ-34 and UTF-8 docs.\n",
    )
    .unwrap();
    let out = shapr(dir.path(), &["-j", "--message", "pr.md", "HEAD~1..HEAD"]);
    let claims = &report(&out)["summary"]["claims"];
    assert_eq!(claims["messages"], 2);
    let from_file: Vec<&Value> = claims["hints"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["source"] == "message")
        .collect();
    assert_eq!(
        from_file,
        [
            &json!({"hint": "issue", "value": "#12", "category": "fix", "source": "message", "commit": null, "field": "body"}),
            &json!({"hint": "issue", "value": "PROJ-34", "category": null, "source": "message", "commit": null, "field": "body"}),
        ]
    );
}

#[test]
fn a_description_can_come_from_stdin_with_a_range() {
    let dir = gate_repo(GATE, &flipped(), "Start over");
    let out = shapr_stdin(
        dir.path(),
        &["-j", "--message", "-", "HEAD~1..HEAD"],
        "fix: small gates\n",
    );
    let claims = &report(&out)["summary"]["claims"];
    assert_eq!(claims["experimental"]["categories"]["fix"], 1, "{claims}");
}

#[test]
fn a_description_from_stdin_needs_a_range() {
    let dir = gate_repo(GATE, &flipped(), "Start");
    let diff = git(dir.path(), &["diff", "HEAD~1"]);
    let out = shapr_stdin(dir.path(), &["-j", "--message", "-"], &diff);
    assert_eq!(out.status.code(), Some(64));
}

#[test]
fn a_piped_diff_has_no_claims() {
    let dir = gate_repo(GATE, &flipped(), "fix: open at one");
    let diff = git(dir.path(), &["diff", "HEAD~1"]);
    let claims = &report(&shapr_stdin(dir.path(), &["-j"], &diff))["summary"]["claims"];
    assert_eq!(claims["messages"], 0);
    assert_eq!(claims["hints"], json!([]));
    assert_eq!(claims["experimental"]["conflicts"], json!([]));
}

#[test]
fn claims_show_in_text_output() {
    let dir = gate_repo(GATE, &flipped(), "Refactor the gate check");
    let out = shapr(dir.path(), &["HEAD~1..HEAD"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("claims: refactor 1 (1 message)"), "{text}");
    assert!(
        text.contains("conflict: claims refactor; 1 behavior hunk"),
        "{text}"
    );
}

#[test]
fn every_claims_field_is_documented() {
    let schema =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/schema.md")).unwrap();
    let (report, _) = classify(GATE, &flipped(), "refactor: tidy the gate\n\nSee #1.");
    let claims = &report["summary"]["claims"];
    for record in [
        claims,
        &claims["experimental"],
        &claims["hints"][0],
        &claims["experimental"]["categories"],
        &claims["experimental"]["conflicts"][0],
    ] {
        for key in record.as_object().unwrap().keys() {
            assert!(
                schema.contains(&format!("`{key}`")),
                "docs/schema.md doesn't name `{key}`"
            );
        }
    }
}
