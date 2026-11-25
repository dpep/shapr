//! Every closed vocabulary the summary counts is complete: each count map
//! holds every value docs/schema.md lists, zero or not. The maps are filled
//! from hand-kept `ALL` lists, so a value missing from one would drop its key.

use serde_json::Value;
use std::collections::BTreeSet;

mod common;
use common::*;

const SCHEMA: &str = include_str!("../docs/schema.md");

/// The backticked values in the first column of the first table under
/// HEADING.
fn table(heading: &str) -> BTreeSet<String> {
    let mut lines = SCHEMA.lines().skip_while(|l| *l != heading).skip(1);
    let rows = lines
        .by_ref()
        .skip_while(|l| !l.starts_with('|'))
        .skip(2)
        .take_while(|l| l.starts_with('|'));
    rows.flat_map(|row| ticked(row.split('|').nth(1).unwrap()))
        .collect()
}

fn ticked(text: &str) -> Vec<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

fn keys(map: &Value) -> BTreeSet<String> {
    map.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn every_documented_value_is_a_summary_key() {
    let report = ruby("def a\n  1\nend\n", "def a\n  2\nend\n");
    let summary = &report["summary"];
    for (field, heading) in [
        ("kinds", "## Parts"),
        ("verdicts", "## Verdicts"),
        ("categories", "## Categories"),
        ("assumptions", "## Proof status"),
        ("facets", "## Facets"),
        ("move_contexts", "## Moves"),
    ] {
        let documented = table(heading);
        assert!(!documented.is_empty(), "no table under {heading}");
        assert_eq!(keys(&summary[field]), documented, "summary.{field}");
    }
    let row = (SCHEMA.lines())
        .find(|l| l.starts_with("| `categories` | object | Messages carrying"))
        .unwrap();
    let documented: BTreeSet<String> = ticked(row.split("present:").nth(1).unwrap())
        .into_iter()
        .collect();
    assert_eq!(
        keys(&summary["claims"]["experimental"]["categories"]),
        documented,
        "summary.claims.experimental.categories"
    );
}
