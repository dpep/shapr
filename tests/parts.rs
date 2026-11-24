//! End to end: a hunk's parts (the structural changes proven in it) and its
//! residual (the lines no part explains), where the case checks `reason`;
//! the rest are fixtures under tests/fixtures/parts.

use serde_json::json;

mod common;
use common::*;

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
