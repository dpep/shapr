//! End to end: `typing` parts' reasons and evidence, which fixtures don't
//! assert.

mod common;
use common::*;

const LOCK: &str = "GEM\n  specs:\n    sorbet-runtime (0.5.1)\n";
const BEFORE: &str =
    "# typed: true\nclass A\n  extend T::Sig\n\n  def run(x)\n    x.go\n  end\nend\n";

fn typing(lock: bool, after: &str) -> serde_json::Value {
    let before: &[(&str, &str)] = &[("Gemfile.lock", LOCK), ("lib/a.rb", BEFORE)];
    let dir = repo(&[&before[usize::from(!lock)..], &[("lib/a.rb", after)]]);
    let report = common::report(&shapr(dir.path(), &["-j", "HEAD~1..HEAD"]));
    part(&hunks(&report)[0], "typing").clone()
}

#[test]
fn a_checked_annotation_says_the_runtime_checks_it() {
    let part = typing(true, &BEFORE.replace("x.go", "T.must(x).go"));
    assert_eq!(
        part["reason"],
        "types only; sorbet-runtime checks them at runtime"
    );
    assert_eq!(part["evidence"]["annotations"], 1);
    assert_eq!(part["evidence"]["unchecked"], 0);
}

#[test]
fn an_unchecked_annotation_says_nothing_checks_it() {
    let part = typing(true, &BEFORE.replace("x.go", "T.unsafe(x).go"));
    assert_eq!(part["reason"], "types only; nothing checks them at runtime");
    assert_eq!(part["evidence"]["unchecked"], 1);
    // Without a lockfile, what `T` is isn't known.
    let part = typing(false, &BEFORE.replace("x.go", "T.unsafe(x).go"));
    assert_eq!(part["structure"], false);
    assert_eq!(part["evidence"]["unchecked"], 1);
}
