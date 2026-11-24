//! Style norms end to end where a fixture can't reach: the cases as data
//! are under tests/fixtures/norms (docs/fixtures.md).

use serde_json::Value;

mod common;
use common::*;

/// A `.editorconfig` above the repository, as any user's home can hold, is
/// never read: config stops at the repository's top.
#[test]
fn config_above_the_repository_is_not_read() {
    let outer = tempfile::tempdir().unwrap();
    std::fs::write(
        outer.path().join(".editorconfig"),
        "[*]\ntrim_trailing_whitespace = true\n",
    )
    .unwrap();
    std::fs::write(
        outer.path().join(".rubocop.yml"),
        "Style/StringLiterals:\n  EnforcedStyle: double_quotes\n",
    )
    .unwrap();
    let root = outer.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    init(&root);
    for (side, content) in [
        ("before", "def a\n  go\nend\n"),
        ("after", "def a\n  go('x')  \nend\n"),
    ] {
        commit(&root, &[("a.rb", content)], side);
    }
    for args in [
        vec!["-j", "HEAD~1..HEAD"],
        vec!["-j", "HEAD~1"],
        vec!["norms", "-j", "a.rb"],
    ] {
        let report = report(&shapr(&root, &args));
        let norms = match args[0] {
            "norms" => report["norms"].clone(),
            _ => report["hunks"][0]["style"]["norms"].clone(),
        };
        let norms = norms.as_array().unwrap();
        assert!(!norms.is_empty(), "{args:?}");
        for norm in norms {
            assert_eq!(norm["configured"], Value::Null, "{args:?}: {norm}");
        }
    }
}
