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

/// The text line names each scope's evidence, so a reader sees why.
#[test]
fn text_shows_the_evidence_per_scope() {
    let neighbors = "A = 'a'\nB = 'b'\nC = 'c'\n";
    let before = "class G\n  def one\n    'a'\n  end\n\n  def two\n    x = 1\n  end\nend\n";
    let after =
        "class G\n  def one\n    'a'\n  end\n\n  def two\n    x = 1\n    \"b\"\n  end\nend\n";
    let dir = repo(&[
        &[("lib/n.rb", neighbors), ("lib/a.rb", before)],
        &[("lib/a.rb", after)],
    ]);
    let out = String::from_utf8(shapr(dir.path(), &["HEAD~1..HEAD"]).stdout).unwrap();
    assert!(
        out.contains(
            "style Style/StringLiterals: diverging at line 8; \
             class expects single_quotes (0.84: class 1 of 1, directory 3 of 3)"
        ),
        "{out}"
    );
}

/// A directory isn't read yet: a usage error that says so, not an I/O error.
#[test]
fn a_directory_is_a_usage_error_for_now() {
    let dir = repo(&[&[("lib/a.rb", "A = 'a'\n")]]);
    let out = shapr(dir.path(), &["norms", "-j", "lib"]);
    assert_eq!(out.status.code(), Some(64), "{out:?}");
    let error = report(&out);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("directories aren't supported yet"),
        "{error}"
    );
}
