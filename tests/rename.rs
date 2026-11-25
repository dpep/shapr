//! End to end: rename detection against throwaway git repos.

use serde_json::Value;

mod common;
use common::*;

/// Every hunk is logic, and the rename pass says why: proof the guard under
/// test fired, not some earlier mismatch.
fn logic_because(report: &Value, why: &str) {
    assert!(kinds(report).iter().all(|k| *k == "logic"), "{report}");
    let reasons: Vec<&str> = report["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| lead(h)["reason"].as_str().unwrap())
        .collect();
    assert!(
        reasons.iter().any(|r| r.contains(why)),
        "{why}: {reasons:?}"
    );
}

fn confidence(hunk: &Value) -> f64 {
    lead(hunk)["confidence"].as_f64().unwrap()
}

/// Lines that separate two edits into two hunks.
fn filler() -> String {
    "  touch\n".repeat(8)
}

const TOTAL: &str = "class Cart\n  def total(items)\n    sum = 0\n    items.each { |item| sum += item.price }\n    sum\n  end\nend\n";

#[test]
fn a_local_variable_renamed_in_a_def_is_a_rename() {
    let report = ruby(TOTAL, &TOTAL.replace("sum", "subtotal"));
    assert_eq!(kinds(&report), ["rename"]);
    let hunk = &report["hunks"][0];
    assert_eq!(confidence(hunk), 1.0);
    assert_eq!(
        lead(hunk)["detail"]["renames"],
        serde_json::json!([{"from": "sum", "to": "subtotal", "scope": "local", "occurrences": 3}])
    );
    assert_eq!(lead(hunk)["evidence"]["renamed"], 3);
    assert_eq!(lead(hunk)["evidence"]["uses"], 3);
    // A local can't be used outside its scope: nothing to search.
    assert_eq!(lead(hunk)["evidence"]["searched"], serde_json::Value::Null);
    assert_eq!(hunk["review_lines"], 0);
    assert_eq!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_rename_that_also_rewords_a_comment_is_a_rename() {
    let before = "# Adds one.\ndef bump(n)\n  n + 1\nend\n";
    let after = "# Adds one to a count.\n# Pure.\ndef bump(count)\n  count + 1\nend\n";
    assert_eq!(kinds(&ruby(before, after)), ["rename"]);
}

#[test]
fn a_local_use_left_behind_is_logic() {
    // The leftover `sum` is no longer a local: Ruby now calls a method.
    let after = TOTAL
        .replace("sum = 0", "subtotal = 0")
        .replace("sum +=", "subtotal +=");
    logic_because(&ruby(TOTAL, &after), "sum is still used");
}

#[test]
fn a_rename_spread_over_hunks_is_checked_across_them() {
    let before = format!("def run\n  value = 1\n{}  value\nend\n", filler());
    let both = before.replace("value", "amount");
    assert_eq!(kinds(&ruby(&before, &both)), ["rename", "rename"]);
    let first_only = before.replacen("value", "amount", 1);
    logic_because(&ruby(&before, &first_only), "value is still used");
}

#[test]
fn renaming_onto_an_existing_name_is_logic() {
    let before = "def run\n  a = 1\n  b = 2\n  a + b\nend\n";
    let after = "def run\n  b = 1\n  b = 2\n  b + b\nend\n";
    logic_because(&ruby(before, after), "b already exists");
}

#[test]
fn renaming_a_local_onto_a_method_called_in_scope_is_logic() {
    // The untouched `size` was a method call; after the rename it reads the local.
    let before = format!("def run\n  n = 1\n  puts n\n{}  size\nend\n", filler());
    let after = before.replacen("n = 1\n  puts n", "size = 1\n  puts size", 1);
    logic_because(&ruby(&before, &after), "size already exists");
}

#[test]
fn a_parameter_rename_is_a_rename_but_adding_one_is_logic() {
    let renamed = TOTAL.replace("items", "lines");
    assert_eq!(kinds(&ruby(TOTAL, &renamed)), ["rename"]);
    let added = TOTAL.replace("total(items)", "total(items, tax)");
    assert_eq!(kinds(&ruby(TOTAL, &added)), ["logic"]);
    let defaulted = TOTAL.replace("total(items)", "total(items = [])");
    assert_eq!(kinds(&ruby(TOTAL, &defaulted)), ["logic"]);
}

#[test]
fn swapping_parameters_is_logic() {
    let before = "def pair(a, b)\n  [a, b]\nend\n";
    let after = "def pair(b, a)\n  [b, a]\nend\n";
    // A swap, not a rename: no rename story.
    logic_because(&ruby(before, after), "code differs");
}

#[test]
fn a_keyword_parameter_rename_is_logic() {
    // Callers pass `rate:` by name, so the method's interface changed.
    let before = "def charge(rate:)\n  rate * 2\nend\n";
    let after = "def charge(fee:)\n  fee * 2\nend\n";
    logic_because(&ruby(before, after), "keyword parameter");
}

#[test]
fn a_rename_mixed_with_a_logic_edit_is_logic() {
    let after = TOTAL.replace("sum", "subtotal").replace("= 0", "= 1");
    assert_eq!(kinds(&ruby(TOTAL, &after)), ["logic"]);
}

const GREETER: &str = "class Greeter\n  def greet(name)\n    \"hi #{name}\"\n  end\n\n  def welcome\n    greet(\"you\") + self.greet(\"all\")\n  end\nend\n";

#[test]
fn a_method_rename_with_every_call_site_is_a_graded_rename() {
    let report = ruby(GREETER, &GREETER.replace("greet", "salute"));
    assert_eq!(kinds(&report), ["rename"]);
    let hunk = &report["hunks"][0];
    assert_eq!(
        lead(hunk)["detail"]["renames"],
        serde_json::json!([{"from": "greet", "to": "salute", "scope": "method", "occurrences": 3}])
    );
    // The repository was searched for callers outside the diff: the file itself.
    assert_eq!(lead(hunk)["evidence"]["searched"], 1);
    assert_eq!(lead(hunk)["assumptions"], serde_json::json!([]));
    // Callers grep can't see (a computed name) still may exist: (3 + 1) / (3 + 2).
    assert_eq!(confidence(hunk), 0.8);
}

#[test]
fn a_method_rename_across_files_checks_every_file() {
    let caller = "class Door\n  def open(g)\n    g.greet(\"x\")\n  end\nend\n";
    let renamed = GREETER.replace("greet", "salute");
    let report = classify(
        &[("a.rb", GREETER), ("b.rb", caller)],
        &[
            ("a.rb", &renamed),
            ("b.rb", &caller.replace("greet", "salute")),
        ],
    );
    assert_eq!(kinds(&report), ["rename", "rename"]);
    // Each file's part counts its own occurrences.
    let occurrences = |h: usize| &lead(&report["hunks"][h])["detail"]["renames"][0]["occurrences"];
    assert_eq!((occurrences(0), occurrences(1)), (&3.into(), &1.into()));

    let stale = "class Door\n  def open(g)\n    g.greet(\"x\")\n    g.close\n  end\nend\n";
    let report = classify(
        &[("a.rb", GREETER), ("b.rb", caller)],
        &[("a.rb", &renamed), ("b.rb", stale)],
    );
    logic_because(&report, "greet is still used");
}

/// The hunk's reason, for a change to one Ruby file.
fn reason(before: &str, after: &str) -> String {
    let report = ruby(before, after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report}");
    lead(&hunks(&report)[0])["reason"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_call_renamed_without_its_definition_is_logic() {
    // Calls a method the change doesn't define.
    assert_eq!(
        reason("def run\n  start\nend\n", "def run\n  begin_now\nend\n"),
        "renames start to begin_now, but start isn't defined in the change"
    );
    // The old method is kept beside the new call.
    assert_eq!(
        reason(
            "def start; end\n\ndef run\n  start\nend\n",
            "def start; end\n\ndef run\n  begin_now\nend\n"
        ),
        "renames start to begin_now, but start's definition is kept, not renamed"
    );
}

/// polyid#107: a helper deleted and its calls replaced reads as a
/// replacement, not a rename left dangling.
#[test]
fn a_removed_definition_whose_calls_become_another_name_is_a_replacement() {
    let pad = format!("def pad\n{}end\n\n", filler());
    let report = ruby(
        &format!("def start; end\n\n{pad}def run\n  start\nend\n"),
        &format!("{pad}def run\n  begin_now\nend\n"),
    );
    logic_because(
        &report,
        "start is removed and its uses become begin_now: a replacement, not a rename",
    );
}

#[test]
fn a_call_switched_to_another_existing_method_says_so() {
    let methods = "def start; end\ndef begin_now; end\n";
    let before = format!("{methods}def run\n  start\nend\n");
    let after = format!("{methods}def run\n  begin_now\nend\n");
    logic_because(&ruby(&before, &after), "calls begin_now instead of start");
    // Defined in a file outside the change.
    let report = classify(
        &[("a.rb", "def run\n  start\nend\n"), ("b.rb", methods)],
        &[("a.rb", "def run\n  begin_now\nend\n"), ("b.rb", methods)],
    );
    logic_because(&report, "calls begin_now instead of start");
}

#[test]
fn a_method_rename_leaving_send_of_the_old_name_is_logic() {
    // `send(:greet)` would now raise NoMethodError.
    let before = format!("{GREETER}Greeter.new.send(:greet, \"x\")\n");
    let after = GREETER.replace("greet", "salute") + "Greeter.new.send(:greet, \"x\")\n";
    logic_because(&ruby(&before, &after), "greet is still used");
}

#[test]
fn a_method_rename_that_updates_send_is_a_rename() {
    let before = format!("{GREETER}Greeter.new.send(:greet, \"x\")\n");
    let after = before.replace("greet", "salute");
    let report = ruby(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "rename"), "{report}");
}

#[test]
fn a_method_rename_leaving_a_symbol_of_the_old_name_is_logic() {
    // A DSL (here a callback) names the method by symbol.
    let before = format!(
        "class Page\n  before_save :tidy\n{}  def tidy\n    1\n  end\nend\n",
        filler()
    );
    let after = before.replace("def tidy", "def clean");
    logic_because(&ruby(&before, &after), "tidy is still used");
}

#[test]
fn renaming_a_method_onto_an_existing_one_is_logic() {
    let before = "def a\n  1\nend\n\ndef b\n  2\nend\n\na\n";
    let after = "def b\n  1\nend\n\ndef b\n  2\nend\n\nb\n";
    logic_because(&ruby(before, after), "b already exists");
}

#[test]
fn renaming_a_method_that_calls_super_is_logic() {
    // `super` now reaches the parent's method of the new name.
    let before = "class Kid < Base\n  def go\n    super\n  end\nend\n";
    let after = before.replace("go", "run");
    logic_because(&ruby(before, &after), "super");
}

#[test]
fn an_internal_class_rename_is_a_rename() {
    let before = "class Box\n  def self.build\n    Box.new\n  end\nend\n";
    let report = ruby(before, &before.replace("Box", "Crate"));
    assert_eq!(kinds(&report), ["rename"]);
    assert_eq!(report["summary"]["verdict"], "structure");
}

#[test]
fn a_constant_rename_counts_its_name_once() {
    let before = "module M\n  LIMIT = 3\n\n  def self.limit\n    LIMIT\n  end\nend\n";
    let report = ruby(before, &before.replace("LIMIT", "CAP"));
    assert_eq!(kinds(&report), ["rename"]);
    let rename = &lead(&report["hunks"][0])["detail"]["renames"][0];
    assert_eq!(
        (rename["scope"].as_str(), rename["occurrences"].as_u64()),
        (Some("constant"), Some(2))
    );
}

#[test]
fn an_instance_variable_must_be_renamed_in_every_method() {
    let before = format!(
        "class Pot\n  def initialize\n    @heat = 1\n  end\n{}  def hot?\n    @heat > 5\n  end\nend\n",
        filler()
    );
    let report = ruby(&before, &before.replace("@heat", "@temp"));
    assert_eq!(kinds(&report), ["rename", "rename"]);
    assert_eq!(lead(&report["hunks"][0])["evidence"]["searched"], 1);
    logic_because(
        &ruby(&before, &before.replacen("@heat", "@temp", 1)),
        "@heat is still used",
    );
}

#[test]
fn renaming_an_instance_variable_behind_an_attr_reader_is_logic() {
    let before = "class Pot\n  attr_reader :heat\n  def initialize\n    @heat = 1\n  end\nend\n";
    let after = before.replace("@heat", "@temp");
    logic_because(&ruby(before, &after), "@heat is still used");
}

#[test]
fn rename_fields_are_documented() {
    let schema =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/schema.md")).unwrap();
    let report = ruby(TOTAL, &TOTAL.replace("sum", "subtotal"));
    let hunk = &report["hunks"][0];
    let records = [
        &lead(hunk)["evidence"],
        &lead(hunk)["detail"],
        &lead(hunk)["detail"]["renames"][0],
    ];
    for record in records {
        for key in record.as_object().unwrap().keys() {
            assert!(
                schema.contains(&format!("`{key}`")),
                "docs/schema.md doesn't name `{key}`"
            );
        }
    }
}

/// A pair the alignment reads as a rename, whose old name is still there
/// after the change and new name already was before it, is a swap or a
/// replaced call: its reason tells no rename story.
#[test]
fn a_swap_or_replaced_call_tells_no_rename_story() {
    let reports = [
        rust(
            "extern \"C\" {\n    fn a();\n    fn b();\n}\n",
            "extern \"C\" {\n    fn b();\n    fn a();\n}\n",
        ),
        rust(
            "fn a() -> u8 {\n    1\n}\n\nfn b() -> u8 {\n    2\n}\n\nfn t() -> u8 {\n    a() + b()\n}\n",
            "fn b() -> u8 {\n    2\n}\n\npub(super) fn a() -> u8 {\n    1\n}\n\nfn t() -> u8 {\n    a() + b()\n}\n",
        ),
        rust(
            "fn f(x: u8) -> u8 {\n    strip_uuid(x)\n}\n\nfn strip_uuid(x: u8) -> u8 {\n    x\n}\n\nfn g() -> u8 {\n    create(1)\n}\n",
            "fn f(x: u8) -> u8 {\n    create(x)\n}\n\nfn strip_uuid(x: u8) -> u8 {\n    x\n}\n\nfn g() -> u8 {\n    strip_uuid(1)\n}\n",
        ),
        rust(
            "fn f(v: &[u8]) -> usize {\n    v.iter().count()\n}\n\nfn g(v: &[u8]) {\n    v.zip();\n}\n",
            "fn f(v: &[u8]) -> usize {\n    v.zip().count()\n}\n\nfn g(v: &[u8]) {\n    v.iter();\n}\n",
        ),
    ];
    let stories: Vec<&str> = (reports.iter())
        .flat_map(|r| hunks(r).iter())
        .flat_map(|h| h["parts"].as_array().unwrap())
        .filter_map(|p| p["reason"].as_str())
        .filter(|r| r.starts_with("renames "))
        .collect();
    assert!(stories.is_empty(), "{stories:?}");
}
