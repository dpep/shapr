//! End to end: extract-method and inline-method detection.

use serde_json::Value;

mod common;
use common::*;

/// Methods that keep an edit's hunks apart.
fn filler() -> String {
    (1..=4)
        .map(|i| format!("  def filler{i}\n    {i}\n  end\n\n"))
        .collect()
}

/// A structural near miss: still `logic`, with the risk that kept it there.
fn assert_near(report: &Value, near: &str, risk: &str) {
    assert!(kinds(report).iter().all(|k| *k == "logic"), "{report:#}");
    assert!(
        hunks(report)
            .iter()
            .any(|h| lead(h)["detail"]["near"] == near && lead(h)["detail"]["risk"] == risk),
        "{report:#}"
    );
}

const RENDER: &str = "class Report
  def render(title)
    puts title
    puts \"-\" * 10
    puts \"=\" * 10
    puts \"end\"
  end
end
";

const RENDER_EXTRACTED: &str = "class Report
  def render(title)
    puts title
    rule
    puts \"end\"
  end

  def rule
    puts \"-\" * 10
    puts \"=\" * 10
  end
end
";

#[test]
fn a_clean_extract_without_parameters() {
    let report = ruby(RENDER, RENDER_EXTRACTED);
    assert!(kinds(&report).iter().all(|k| *k == "extract"), "{report:#}");
    let hunk = &hunks(&report)[0];
    assert_eq!(lead(hunk)["detail"]["method"], "rule");
    assert_eq!(lead(hunk)["detail"]["caller"], "render");
    assert_eq!(lead(hunk)["evidence"]["statements"], 2);
    assert_eq!(lead(hunk)["evidence"]["nodes"], 12);
    assert_eq!(lead(hunk)["evidence"]["params"], 0);
    assert_eq!(lead(hunk)["evidence"]["verbatim"], 1.0);
    assert_eq!(lead(hunk)["evidence"]["returns"], false);
    // Twelve nodes, under the sixteen that saturate the size factor.
    assert_eq!(lead(hunk)["confidence"], 0.75);
    assert_eq!(report["summary"]["verdict"], "structure");
}

fn invoice(body: &str, extracted: Option<&str>) -> String {
    format!(
        "class Invoice\n  def total(items, rate)\n{body}  end\n\n{}{}end\n",
        filler(),
        extracted.unwrap_or_default()
    )
}

const TOTAL: &str = "    subtotal = items.sum
    taxed = subtotal * rate
    rounded = taxed.round(2)
    puts rounded
    rounded
";

const TOTAL_EXTRACTED: &str = "    subtotal = items.sum
    rounded = apply_tax(subtotal, rate)
    puts rounded
    rounded
";

const APPLY_TAX: &str = "  def apply_tax(amount, rate)
    taxed = amount * rate
    taxed.round(2)
  end
";

#[test]
fn an_extract_with_parameters_and_a_return_value_cross_references_its_hunks() {
    let before = invoice(TOTAL, None);
    let after = invoice(TOTAL_EXTRACTED, Some(APPLY_TAX));
    let report = ruby(&before, &after);
    assert_eq!(kinds(&report), ["extract", "extract"], "{report:#}");
    for hunk in hunks(&report) {
        assert_eq!(lead(hunk)["detail"]["method"], "apply_tax");
        assert_eq!(lead(hunk)["detail"]["caller"], "total");
        assert_eq!(lead(hunk)["detail"]["hunks"], serde_json::json!([0, 1]));
    }
    let evidence = &lead(&hunks(&report)[0])["evidence"];
    assert_eq!(evidence["params"], 2);
    assert_eq!(evidence["returns"], true);
    assert_eq!(evidence["nodes"], 10);
    // `subtotal` reaches the body as `amount`: one renamed node in ten.
    assert_eq!(evidence["verbatim"], 0.9);
    assert_eq!(lead(&hunks(&report)[0])["confidence"], 0.59);
}

#[test]
fn a_clean_inline_is_the_inverse() {
    let before = invoice(TOTAL_EXTRACTED, Some(APPLY_TAX));
    let after = invoice(TOTAL, None);
    let report = ruby(&before, &after);
    assert_eq!(kinds(&report), ["inline", "inline"], "{report:#}");
    let hunk = &hunks(&report)[0];
    assert_eq!(lead(hunk)["detail"]["method"], "apply_tax");
    assert_eq!(lead(hunk)["detail"]["caller"], "total");
    assert_eq!(lead(hunk)["detail"]["hunks"], serde_json::json!([0, 1]));
}

#[test]
fn an_extract_mixed_with_a_logic_edit_in_the_same_method_is_logic() {
    let before = invoice(TOTAL, None);
    let edited = TOTAL_EXTRACTED.replace("puts rounded", "puts rounded.to_s");
    let after = invoice(&edited, Some(APPLY_TAX));
    let report = ruby(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
    assert_eq!(report["summary"]["verdict"], "behavior");
}

#[test]
fn wrong_argument_wiring_is_logic() {
    let before = "def area(width, height)
  scaled = width * 2 + height
  puts scaled.inspect
end
";
    let after = "def area(width, height)
  scaled = scale(height, width)
  puts scaled.inspect
end

def scale(a, b)
  a * 2 + b
end
";
    let report = ruby(before, after);
    assert_near(&report, "extract", "wiring");
    assert_eq!(report["summary"]["verdict"], "behavior");
}

#[test]
fn a_chunk_containing_return_is_logic() {
    let before = "def check(order)
  validate(order)
  return :empty if order.items.empty?
  log(order, :checked)
  :ok
end
";
    let after = "def check(order)
  validate(order)
  guard(order)
  :ok
end

def guard(order)
  return :empty if order.items.empty?
  log(order, :checked)
end
";
    let report = ruby(before, after);
    assert_near(&report, "extract", "jump");
}

#[test]
fn a_chunk_using_yield_is_logic() {
    let before = "def each_row(rows)
  rows.each do |row|
    cells = row.split(\",\")
    yield cells.map(&:strip)
  end
end
";
    let after = "def each_row(rows)
  rows.each do |row|
    emit(row)
  end
end

def emit(row)
  cells = row.split(\",\")
  yield cells.map(&:strip)
end
";
    let report = ruby(before, after);
    assert_near(&report, "extract", "jump");
}

#[test]
fn a_chunk_using_the_methods_block_parameter_is_logic() {
    let before = "def each_row(rows, &block)
  rows.each do |row|
    cells = row.split(\",\")
    block.call(cells.map(&:strip))
  end
end
";
    let after = "def each_row(rows, &block)
  rows.each do |row|
    emit(row, block)
  end
end

def emit(row, block)
  cells = row.split(\",\")
  block.call(cells.map(&:strip))
end
";
    let report = ruby(before, after);
    assert_near(&report, "extract", "block_parameter");
}

#[test]
fn a_tiny_extract_below_the_floor_is_logic() {
    let before = "def greet(name)
  puts name.upcase
  name
end
";
    let after = "def greet(name)
  shout(name)
  name
end

def shout(name)
  puts name.upcase
end
";
    let report = ruby(before, after);
    assert_near(&report, "extract", "tiny");
}

#[test]
fn a_new_method_whose_name_is_already_in_use_is_logic() {
    let other = "class Other\n  def rule\n    :other\n  end\nend\n";
    let before = format!("{RENDER}{other}");
    let after = format!("{RENDER_EXTRACTED}{other}");
    let report = ruby(&before, &after);
    assert_near(&report, "extract", "collision");
}

#[test]
fn a_chunk_updating_a_local_of_the_caller_is_logic() {
    // `total += …` reads the caller's `total` with no reference node to see.
    let before = "def tally(items)
  total = 10
  total += items.size
  puts total.inspect
end
";
    let after = "def tally(items)
  total = 10
  report(items)
end

def report(items)
  total += items.size
  puts total.inspect
end
";
    let report = ruby(before, after);
    assert_near(&report, "extract", "wiring");
    assert_eq!(report["summary"]["verdict"], "behavior");
}

#[test]
fn a_class_method_calling_a_new_instance_method_is_logic() {
    for receiver in ["self.", "self::"] {
        let before = SHELF.replace("self.", receiver);
        let after = SHELF_SHOWN.replace("self.", receiver);
        let report = ruby(&before, &after);
        assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
        assert_eq!(report["summary"]["verdict"], "behavior");
    }
}

const SHELF: &str = "class Shelf
  def self.label(book)
    title = book.title.strip
    puts title.upcase.center(20)
  end
end
";

const SHELF_SHOWN: &str = "class Shelf
  def self.label(book)
    show(book)
  end

  def show(book)
    title = book.title.strip
    puts title.upcase.center(20)
  end
end
";

#[test]
fn inlining_a_method_still_called_elsewhere_is_logic() {
    let caller = "\ndef again(rate)\n  apply_tax(1, rate)\nend\n";
    let before = format!("{}{caller}", invoice(TOTAL_EXTRACTED, Some(APPLY_TAX)));
    let after = format!("{}{caller}", invoice(TOTAL, None));
    let report = ruby(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn an_extract_returning_through_a_trailing_read() {
    let before = invoice(TOTAL, None);
    let body = "  def apply_tax(amount, rate)
    taxed = amount * rate
    rounded = taxed.round(2)
    rounded
  end
";
    let after = invoice(TOTAL_EXTRACTED, Some(body));
    let report = ruby(&before, &after);
    assert_eq!(kinds(&report), ["extract", "extract"], "{report:#}");
}

#[test]
fn an_extract_whose_new_method_drops_the_returned_value_is_logic() {
    let before = invoice(TOTAL, None);
    let body = "  def apply_tax(amount, rate)
    taxed = amount * rate
    rounded = taxed.round(2)
    log(rounded)
  end
";
    let after = invoice(TOTAL_EXTRACTED, Some(body));
    let report = ruby(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn a_reformatted_sibling_keeps_its_label_beside_an_extract() {
    let before = invoice(TOTAL, None);
    let after =
        invoice(TOTAL_EXTRACTED, Some(APPLY_TAX)).replace("def filler2\n", "def  filler2\n");
    let report = ruby(&before, &after);
    assert_eq!(
        kinds(&report),
        ["extract", "format_only", "extract"],
        "{report:#}"
    );
    assert_eq!(
        lead(&hunks(&report)[0])["detail"]["hunks"],
        serde_json::json!([0, 2])
    );
}

#[test]
fn a_new_use_of_the_new_method_elsewhere_is_not_a_collision() {
    let spec = "describe Report do\n  it { Report.new.rule }\nend\n";
    let report = classify(
        &[("a.rb", RENDER)],
        &[("a.rb", RENDER_EXTRACTED), ("a_spec.rb", spec)],
    );
    let by_file: Vec<(&str, &str)> = hunks(&report)
        .iter()
        .map(|h| (h["file"].as_str().unwrap(), h["kind"].as_str().unwrap()))
        .collect();
    assert!(by_file.contains(&("a.rb", "extract")), "{report:#}");
    assert!(by_file.contains(&("a_spec.rb", "logic")), "{report:#}");
}

#[test]
fn a_new_method_overriding_one_defined_elsewhere_in_the_change_is_logic() {
    let base = "class Base\n  def rule\n    :base\n  end\nend\n";
    let report = classify(
        &[("a.rb", RENDER), ("base.rb", "class Base\nend\n")],
        &[("a.rb", RENDER_EXTRACTED), ("base.rb", base)],
    );
    assert!(
        hunks(&report)
            .iter()
            .any(|h| lead(h)["detail"]["risk"] == "collision"),
        "{report:#}"
    );
    assert!(!kinds(&report).contains(&"extract"), "{report:#}");
}

#[test]
fn an_extract_beside_an_edit_to_another_method_is_mixed() {
    let before = invoice(TOTAL, None);
    let after =
        invoice(TOTAL_EXTRACTED, Some(APPLY_TAX)).replace("filler2\n    2\n", "filler2\n    20\n");
    let report = ruby(&before, &after);
    assert_eq!(
        kinds(&report),
        ["extract", "logic", "extract"],
        "{report:#}"
    );
    assert_eq!(
        lead(&hunks(&report)[0])["detail"]["hunks"],
        serde_json::json!([0, 2])
    );
    assert_eq!(report["summary"]["verdict"], "mixed");
}

#[test]
fn an_extract_sharing_a_hunk_with_another_edit_is_logic() {
    let before = format!("{RENDER}X = 1\n");
    let after = format!("{RENDER_EXTRACTED}X = 2\n");
    let report = ruby(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn an_extract_sharing_a_hunk_with_an_edit_to_a_sibling_leaves_the_edit_as_logic() {
    let before = invoice(TOTAL, None);
    let after = invoice(TOTAL_EXTRACTED, Some(APPLY_TAX)).replace("    4\n", "    40\n");
    let report = ruby(&before, &after);
    assert_eq!(kinds(&report), ["extract", "logic"], "{report:#}");
    let parts: Vec<&str> = hunks(&report)[1]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect();
    assert_eq!(parts, ["extract", "logic"], "{report:#}");
    // The sibling's edited line, `4` becoming `40`, is the residual's only code.
    let line = |text: &str| {
        text.lines()
            .position(|l| l == "    40" || l == "    4")
            .unwrap()
            + 1
    };
    let residual = &hunks(&report)[1]["parts"][1]["lines"];
    assert_eq!(residual["before"][0]["start"], line(&before));
    assert_eq!(residual["after"][0]["start"], line(&after));
    assert_eq!(report["summary"]["verdict"], "mixed");
}

#[test]
fn a_chunk_using_instance_variables_needs_no_parameter() {
    let before = "class Meter
  def tick(step)
    @count += step
    @total = @count * 2 + step
    @total
  end
end
";
    let after = "class Meter
  def tick(step)
    advance(step)
    @total
  end

  def advance(step)
    @count += step
    @total = @count * 2 + step
  end
end
";
    let report = ruby(before, after);
    assert!(kinds(&report).iter().all(|k| *k == "extract"), "{report:#}");
}

#[test]
fn a_new_method_with_keyword_parameters_is_not_an_extract() {
    let before = "def show(item)
  label = item.name.strip
  puts label.center(20)
end
";
    let after = "def show(item)
  render(item: item)
end

def render(item:)
  label = item.name.strip
  puts label.center(20)
end
";
    let report = ruby(before, after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

/// The refusal's `reason` text, from the first hunk's residual.
fn reason(report: &Value) -> &str {
    lead(&hunks(report)[0])["reason"].as_str().unwrap()
}

#[test]
fn a_local_the_statements_reassign_is_named_in_the_reason() {
    let before = "class C
  def run(items)
    total = 0
    items.each { |i| total += i }
    total
  end
end
";
    let after = "class C
  def run(items)
    total = 0
    total = sum_items(items, total)
    total
  end

  def sum_items(items, total)
    items.each { |i| total += i }
    total
  end
end
";
    let report = ruby(before, after);
    assert_near(&report, "extract", "wiring");
    assert_eq!(
        reason(&report),
        "not an extract of `sum_items`: the statements assign total, which run also assigns"
    );
}

#[test]
fn an_inline_reassigning_a_parameter_names_it_in_the_reason() {
    let before = "class C
  def run(a)
    puts \"start\"
    helper(a)
    puts a
  end

  def helper(a)
    a += 1
    puts a
    puts a * 2
  end
end
";
    let after = "class C
  def run(a)
    puts \"start\"
    a += 1
    puts a
    puts a * 2
    puts a
  end
end
";
    let report = ruby(before, after);
    assert_near(&report, "inline", "wiring");
    assert_eq!(
        reason(&report),
        "not an inline of `helper`: the statements assign a, which run also assigns"
    );
}

#[test]
fn a_local_read_after_the_statements_but_not_returned_is_named_in_the_reason() {
    let before = "fn run(x: i32) -> i32 {
    let a = x + 1;
    let b = a * 2;
    a + b
}
";
    let after = "fn run(x: i32) -> i32 {
    let b = calc(x);
    a + b
}

fn calc(x: i32) -> i32 {
    let a = x + 1;
    a * 2
}
";
    let report = rust(before, after);
    assert_near(&report, "extract", "wiring");
    assert_eq!(
        reason(&report),
        "not an extract of `calc`: run reads a after the statements, but the call doesn't return it"
    );
}
