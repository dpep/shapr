//! End to end: default arguments, for what fixtures can't say (`reason`).

mod common;
use common::*;

const FETCHER: &str = "class Fetcher\n  def fetch(key, retries: 3)\n    [key, retries]\n  end\n\n  def go(x)\n    CALL\n  end\nend\n";

fn reason(call_before: &str, call_after: &str) -> String {
    let report = ruby(
        &FETCHER.replace("CALL", call_before),
        &FETCHER.replace("CALL", call_after),
    );
    lead(&hunks(&report)[0])["reason"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn the_reason_names_the_argument_and_the_method() {
    assert_eq!(
        reason("fetch(x, retries: 3)", "fetch(x)"),
        "drops `retries: 3`, `fetch`'s default"
    );
    assert_eq!(
        reason("x.fetch(x)", "x.fetch(x, retries: 3)"),
        "adds `retries: 3`, not shown to restate a default: the call has a receiver, so which `fetch` it reaches is unknown"
    );
}

/// A call with no arguments may mean something else than its defaults
/// (WebMock's bare `to_return` is an empty response sequence).
#[test]
fn a_near_miss_that_empties_the_call_says_so() {
    assert_eq!(
        reason("x.fetch(retries: 3)", "x.fetch"),
        "drops `retries: 3`, leaving `fetch` with no arguments, not shown to restate a default: the call has a receiver, so which `fetch` it reaches is unknown"
    );
    assert_eq!(
        reason("x.fetch", "x.fetch(retries: 3)"),
        "adds `retries: 3` to `fetch`, which passed none, not shown to restate a default: the call has a receiver, so which `fetch` it reaches is unknown"
    );
    let kept = reason("x.fetch(x, retries: 3)", "x.fetch(x)");
    assert!(!kept.contains("no arguments"), "{kept}");
    assert_eq!(
        reason("fetch(retries: 3)", "fetch"),
        "drops `retries: 3`, `fetch`'s default"
    );
}

/// A hunk's reason for one class and file, as `reason` reads it.
fn reason_of(before: &[(&str, &str)], after: &[(&str, &str)]) -> String {
    let report = classify(before, after);
    lead(&hunks(&report)[0])["reason"]
        .as_str()
        .unwrap()
        .to_string()
}

const MANY: &str = "class Fetcher\n  def fetch(key, retries: 3)\n    key\n  end\n\n  def go(x)\n    [A, B, C]\n  end\nend\n";

#[test]
fn several_calls_are_each_named() {
    let both = |a: &str, b: &str, c: &str| MANY.replace('A', a).replace('B', b).replace('C', c);
    let before = both("fetch(x, retries: 3)", "fetch(x, retries: 3)", "fetch(x)");
    let after = both("fetch(x)", "fetch(x)", "fetch(x, retries: 3)");
    assert_eq!(
        reason_of(&[("a.rb", &before)], &[("a.rb", &after)]),
        "3 calls: drops `retries: 3`, drops `retries: 3`, adds `retries: 3`, `fetch`'s default"
    );
    let before = both("fetch(x, retries: 3)", "x.fetch(x, retries: 3)", "fetch(x)");
    let after = both("fetch(x)", "x.fetch(x)", "fetch(x)");
    let why = reason_of(&[("a.rb", &before)], &[("a.rb", &after)]);
    assert!(
        why.starts_with("1 of 2 calls: drops `retries: 3`, not shown")
            && why.contains("the call has a receiver"),
        "{why}"
    );
}

#[test]
fn near_misses_get_their_own_reason() {
    let class = "class M\n  def mix(a, b = 1, k: 2); end\n\n  def fetch(key, retries: 3); end\n\n  def go(a)\n    CALL\n  end\nend\n";
    let near = |b: &str, a: &str| {
        reason_of(
            &[("a.rb", &class.replace("CALL", b))],
            &[("a.rb", &class.replace("CALL", a))],
        )
    };
    let mixed = near("mix(1, 1, k: 2)", "mix(1, k: 2)");
    assert!(mixed.contains("beside keyword arguments"), "{mixed}");
    let splat = near("fetch(*a, retries: 3)", "fetch(*a)");
    assert!(
        splat.contains("can't each be tied to a parameter"),
        "{splat}"
    );
}

#[test]
fn what_else_names_the_method_says_where() {
    let class = "class S\n  def go(x)\n    CALL\n  end\n\n  def sent(a, n: 1); end\nend\n";
    let user = "class U\n  def run(s)\n    s.send(:sent, 1)\n  end\nend\n";
    let why = reason_of(
        &[
            ("a.rb", &class.replace("CALL", "sent(x, n: 1)")),
            ("u.rb", user),
        ],
        &[("a.rb", &class.replace("CALL", "sent(x)"))],
    );
    assert!(why.ends_with("in u.rb"), "{why}");
    let own = class.replace(
        "  def sent",
        "  def other\n    send(:sent, 1)\n  end\n\n  def sent",
    );
    let why = reason_of(
        &[("a.rb", &own.replace("CALL", "sent(x, n: 1)"))],
        &[("a.rb", &own.replace("CALL", "sent(x)"))],
    );
    assert!(why.ends_with("in a.rb"), "{why}");
    let sub = "class T < S\n  def sent(a, n: 2); end\nend\n";
    let why = reason_of(
        &[
            ("a.rb", &class.replace("CALL", "sent(x, n: 1)")),
            ("t.rb", sub),
        ],
        &[("a.rb", &class.replace("CALL", "sent(x)"))],
    );
    assert!(
        why.contains("also defined in t.rb, which may override it"),
        "{why}"
    );
}

#[test]
fn a_directive_naming_the_method_stops_it() {
    let file = |call: &str| {
        format!(
            "# rubocop:disable Naming/fetch\n{}",
            FETCHER.replace("CALL", call)
        )
    };
    assert_eq!(
        reason_of(
            &[("a.rb", &file("fetch(x, retries: 3)"))],
            &[("a.rb", &file("fetch(x)"))]
        ),
        "drops `retries: 3`, not shown to restate a default: a directive names it in a.rb"
    );
}

#[test]
fn a_language_without_default_arguments_gets_no_default_reason() {
    let file = |call: &str| {
        format!(
            "fn fetch(key: u32, retries: Option<u32>) -> u32 {{\n    key\n}}\n\nfn go(x: u32) -> u32 {{\n    {call}\n}}\n"
        )
    };
    let report = rust(&file("fetch(x)"), &file("fetch(x, None)"));
    let reason = lead(&hunks(&report)[0])["reason"]
        .as_str()
        .unwrap_or_default();
    assert!(!reason.contains("default"), "{reason}");
}
