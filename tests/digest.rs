//! `--digest`: the reviewer's view of the report.

mod common;
use common::*;

const BEFORE: &str = "\
def area(x)
  x * 2
end






def name
  'a'
end
";

const AFTER: &str = "\
def area( x )
  x * 2
end






def name
  if ready?
    'b'
  end
end
";

#[test]
fn digest_lists_logic_and_counts_structure_to_skip() {
    let dir = repo(&[
        &[("a.rb", BEFORE), ("README.md", "a\n")],
        &[("a.rb", AFTER), ("README.md", "b\n")],
    ]);
    let digest = report(&shapr(dir.path(), &["-j", "--digest", "HEAD~1..HEAD"]));
    assert_eq!(digest["record"], "digest");
    assert_eq!(digest["verdict"], "mixed");
    let read = digest["read_first"].as_array().unwrap();
    assert_eq!(read.len(), 1, "{digest}");
    assert_eq!(read[0]["at"], serde_json::json!(["a.rb:11"]));
    assert!(
        read[0]["facets"]
            .as_array()
            .unwrap()
            .contains(&"condition".into())
    );
    assert_eq!(digest["skip"]["kinds"]["format_only"], 1);
    assert_eq!(digest["skip"]["hunks"], 1);
    assert_eq!(
        digest["outside"]["docs"],
        serde_json::json!(["README.md:1"])
    );

    let text = String::from_utf8(shapr(dir.path(), &["--digest", "HEAD~1..HEAD"]).stdout).unwrap();
    assert!(text.contains("read first:\n  a.rb:11 "), "{text}");
    assert!(
        text.contains("skip, proven structure: 2 changed lines (1 format_only)"),
        "{text}"
    );
}

#[test]
fn digest_groups_hunks_sharing_a_note_and_is_smaller_than_the_report() {
    // The same unproven default dropped twice reads as one item.
    let before = "class F\n  def fetch(key, retries: 3)\n    key\n  end\n\n  def go(x)\n    x.fetch(x, retries: 3)\n  end\n\n\n\n\n\n\n  def stop(x)\n    x.fetch(x, retries: 3)\n  end\nend\n";
    let after = before
        .replace(", retries: 3)\n  end\n\n\n", ")\n  end\n\n\n")
        .replace("x, retries: 3)\n  end\nend", "x)\n  end\nend");
    let dir = repo(&[&[("a.rb", before)], &[("a.rb", &after)]]);
    let full = shapr(dir.path(), &["-j", "HEAD~1..HEAD"]).stdout;
    let out = shapr(dir.path(), &["-j", "--digest", "HEAD~1..HEAD"]).stdout;
    let digest: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let read = digest["read_first"].as_array().unwrap();
    assert_eq!(read.len(), 1, "{digest}");
    assert_eq!(read[0]["at"], serde_json::json!(["a.rb:7", "a.rb:16"]));
    assert!(
        out.len() * 4 < full.len(),
        "{} vs {}",
        out.len(),
        full.len()
    );
}

#[test]
fn digest_refuses_ndjson() {
    let dir = repo(&[&[("a.rb", BEFORE)], &[("a.rb", AFTER)]]);
    let out = shapr(dir.path(), &["-J", "--digest", "HEAD~1..HEAD"]);
    assert_eq!(out.status.code(), Some(64));
}

#[test]
fn every_digest_field_is_documented() {
    let schema =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/schema.md")).unwrap();
    let dir = repo(&[&[("a.rb", BEFORE)], &[("a.rb", AFTER)]]);
    let digest = report(&shapr(dir.path(), &["-j", "--digest", "HEAD~1..HEAD"]));
    for record in [
        &digest,
        &digest["read_first"][0],
        &digest["skip"],
        &digest["more"],
    ] {
        for key in record.as_object().unwrap().keys() {
            assert!(
                schema.contains(&format!("`{key}`")),
                "docs/schema.md doesn't name `{key}`"
            );
        }
    }
}

const INVOICE: &str = "\
class Invoice
  def total(items, rate)
    subtotal = items.sum
    taxed = subtotal * rate
    rounded = taxed.round(2)
    puts rounded
    rounded
  end

  def filler1
    1
  end

  def filler2
    2
  end

  def filler3
    3
  end

  def filler4
    4
  end

end
";

const INVOICE_EXTRACTED: &str = "\
class Invoice
  def total(items, rate)
    subtotal = items.sum
    rounded = apply_tax(subtotal, rate)
    puts rounded
    rounded
  end

  def filler1
    1
  end

  def filler2
    2
  end

  def filler3
    3
  end

  def filler4
    4
  end

  def apply_tax(amount, rate)
    taxed = amount * rate
    taxed.round(2)
  end
end
";

/// The digest's review lines, read_first through outside, against the
/// summary's: every line to read is placed somewhere.
fn assert_reconciles(digest: &serde_json::Value) {
    let sum = |items: &serde_json::Value| -> u64 {
        (items.as_array().unwrap().iter())
            .map(|i| i["review_lines"].as_u64().unwrap())
            .sum()
    };
    let outside: u64 = (digest["outside"].as_object().unwrap().values())
        .map(|g| g["review_lines"].as_u64().unwrap())
        .sum();
    let placed = sum(&digest["read_first"])
        + digest["more"]["review_lines"].as_u64().unwrap()
        + sum(&digest["unproven_structure"])
        + outside;
    assert_eq!(placed, digest["review_lines"].as_u64().unwrap(), "{digest}");
}

#[test]
fn structure_with_lines_to_read_is_listed_not_skipped() {
    let dir = repo(&[&[("a.rb", INVOICE)], &[("a.rb", INVOICE_EXTRACTED)]]);
    let digest = report(&shapr(dir.path(), &["-j", "--digest", "HEAD~1..HEAD"]));
    let read = digest["read_first"].as_array().unwrap();
    assert!(!read.is_empty(), "{digest}");
    assert!(read.iter().all(|i| i["kind"] == "extract"), "{digest}");
    assert_eq!(digest["skip"]["changed_lines"], 0, "{digest}");
    assert_eq!(digest["skip"]["hunks"], 0, "{digest}");
    assert_reconciles(&digest);

    let text = String::from_utf8(shapr(dir.path(), &["--digest", "HEAD~1..HEAD"]).stdout).unwrap();
    assert!(text.contains("proven extract, still read"), "{text}");
    assert!(!text.contains("skip,"), "{text}");
}

#[test]
fn a_group_aggregates_its_members_and_keeps_tests_apart() {
    // The same unproven default dropped twice, the second hunk beside a
    // reindented `end`; and a rename shared by code and its spec.
    let before = "class F\n  def fetch(key, retries: 3)\n    key\n  end\n\n  def go(x)\n    x.fetch(x, retries: 3)\n  end\n\n\n\n\n\n\n  def stop(x)\n    x.fetch(x, retries: 3)\n  end\nend\n";
    let after = before
        .replace("x, retries: 3)\n  end\n\n", "x)\n  end\n\n")
        .replace("x, retries: 3)\n  end\nend", "x)\n      end\nend");
    let spec = "describe 'a' do\n  it 'x' do\n    foo(1)\n  end\nend\n";
    let code = "def go\n  foo(1)\nend\n";
    let dir = repo(&[
        &[
            ("a.rb", before),
            ("spec/b_spec.rb", spec),
            ("src/b.rb", code),
        ],
        &[
            ("a.rb", &after),
            ("spec/b_spec.rb", &spec.replace("foo", "bar")),
            ("src/b.rb", &code.replace("foo", "bar")),
        ],
    ]);
    let digest = report(&shapr(dir.path(), &["-j", "--digest", "HEAD~1..HEAD"]));
    let read = digest["read_first"].as_array().unwrap();
    let at = |place: &str| {
        (read.iter())
            .find(|i| i["at"].as_array().unwrap().contains(&place.into()))
            .unwrap_or_else(|| panic!("{place} unplaced: {digest}"))
    };
    let fetch = at("a.rb:7");
    assert_eq!(fetch["at"], serde_json::json!(["a.rb:7", "a.rb:16"]));
    assert_eq!(fetch["with"], serde_json::json!({"format_only": 2}));
    assert_eq!(at("src/b.rb:2")["test"], false, "{digest}");
    assert_eq!(at("spec/b_spec.rb:3")["test"], true, "{digest}");
}
