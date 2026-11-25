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
