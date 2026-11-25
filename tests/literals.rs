//! Whitespace inside a literal's content is part of its value: every kind of
//! string, command, regexp, heredoc and quoted symbol, with the whitespace
//! beside an interpolation, on a continued line, or setting a `<<~`
//! heredoc's dedent. No such edit may read as structure.

mod common;
use common::*;

/// Templates holding `{W}` where the whitespace is edited.
const RUBY: &[&str] = &[
    "\"a{W}b\"",
    "'a{W}b'",
    "\"a\n{W}b\"",
    "\"#{x}{W}b\"",
    "\"a{W}#{x}\"",
    "\"a\n{W}#{x}\"",
    "\"a\n{W}#{x} b\"",
    "`echo{W}a`",
    "`echo{W}#{x}`",
    "`echo\n{W}#{x}`",
    "/a{W}b/",
    "/a{W}#{x}/",
    "/a\n{W}#{x}/",
    "%r{a{W}#{x}}",
    "%q(a{W}b)",
    "%Q(a{W}#{x})",
    "%Q(a\n{W}#{x})",
    "%x(echo{W}#{x})",
    ":\"a{W}b\"",
    ":\"a{W}#{x}\"",
    "%s(a{W}b)",
    "<<EOS\n{W}a\nEOS",
    "<<-EOS\n{W}a\n  EOS",
    "<<-EOS\n{W}#{x} a\n  EOS",
    "<<~EOS\n    a\n{W}b\n  EOS",
    "<<~EOS\n    a #{x}\n{W}#{x} b\n  EOS",
    "<<~EOS\n    a\n{W}#{x}\n  EOS",
    "<<~EOS\n{W}#{x}\n    b\n  EOS",
    "<<~`EOS`\n    echo\n{W}#{x}\n  EOS",
    "<<~'EOS'\n    a\n{W}b\n  EOS",
];

const RUST: &[&str] = &[
    "\"a{W}b\"",
    "\"a\n{W}b\"",
    "r#\"a\n{W}b\"#",
    "b\"a\n{W}b\"",
    "br\"a\n{W}b\"",
    "format!(\"a\n{W}{x}\")",
    "format!(\"{x}{W}b\")",
];

/// Each edit of the whitespace: (before, after).
const EDITS: &[(&str, &str)] = &[("  ", "    "), (" ", ""), ("\t", " ")];

/// Runs every template under every edit, one file each, and names the
/// files whose hunks read as structure.
fn structural(templates: &[&str], ext: &str, wrap: fn(&str) -> String) -> Vec<String> {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for (t, template) in templates.iter().enumerate() {
        for (e, (b, a)) in EDITS.iter().enumerate() {
            let path = format!("lib/c{t}_{e}.{ext}");
            before.push((path.clone(), wrap(&template.replace("{W}", b))));
            after.push((path, wrap(&template.replace("{W}", a))));
        }
    }
    fn borrow(v: &[(String, String)]) -> Vec<(&str, &str)> {
        v.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect()
    }
    let report = classify(&borrow(&before), &borrow(&after));
    let mut found: Vec<String> = Vec::new();
    for hunk in hunks(&report) {
        assert_eq!(hunk["parsed"], true, "{hunk}");
        let file = hunk["file"].as_str().unwrap();
        let (t, e) = file
            .trim_start_matches("lib/c")
            .split_once('.')
            .unwrap()
            .0
            .split_once('_')
            .unwrap();
        let (t, e): (usize, usize) = (t.parse().unwrap(), e.parse().unwrap());
        if hunk["verdict"] == "structure" || hunk["kind"] != "logic" {
            found.push(format!(
                "{:?} {:?}: {}",
                templates[t], EDITS[e], hunk["kind"]
            ));
        }
    }
    // Every file changed, so every one has a hunk.
    assert_eq!(hunks(&report).len(), before.len());
    found
}

#[test]
fn whitespace_inside_ruby_literals_is_content() {
    let wrap = |literal: &str| format!("def f(x)\n  y = {literal}\n  y\nend\n");
    let found = structural(RUBY, "rb", wrap);
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn whitespace_inside_rust_literals_is_content() {
    let wrap = |literal: &str| {
        format!("fn f(x: u8) -> String {{\n    let y = {literal};\n    y.into()\n}}\n")
    };
    let found = structural(RUST, "rs", wrap);
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_squiggly_heredoc_reindented_whole_keeps_its_value() {
    let before = "def f(x)\n  <<~EOS\n    a #{x}\n      b\n  EOS\nend\n";
    let after = "def f(x)\n    <<~EOS\n      a #{x}\n        b\n    EOS\nend\n";
    let report = ruby(before, after);
    assert_eq!(kinds(&report), ["restyle"], "{report}");
}
