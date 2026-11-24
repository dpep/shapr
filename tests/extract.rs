//! End to end: extract-method and inline-method detection.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// The part a hunk's kind stands for: the last of that kind (the residual,
/// for `logic`), else the first that isn't structure.
fn lead(hunk: &Value) -> &Value {
    let parts = hunk["parts"].as_array().unwrap();
    parts
        .iter()
        .rev()
        .find(|p| p["kind"] == hunk["kind"])
        .or_else(|| parts.iter().find(|p| p["structure"] == false))
        .unwrap_or_else(|| panic!("no parts: {hunk:#}"))
}

fn command(program: &str, dir: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd
}

fn git(dir: &Path, args: &[&str]) {
    let out = command("git", dir).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `shapr -j` over one commit turning `before` into `after` in `a.rb`.
fn classify(before: &str, after: &str) -> (Value, i32) {
    classify_files(&[("a.rb", before)], &[("a.rb", after)])
}

/// `shapr -j` over one commit writing each (path, content) pair.
fn classify_files(before: &[(&str, &str)], after: &[(&str, &str)]) -> (Value, i32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    git(path, &["init", "-q", "-b", "main"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "Test"]);
    for files in [before, after] {
        for (name, content) in files {
            std::fs::write(path.join(name), content).unwrap();
        }
        git(path, &["add", "-A"]);
        git(path, &["commit", "-q", "--allow-empty", "-m", "state"]);
    }
    let out = command(env!("CARGO_BIN_EXE_shapr"), path)
        .args(["-j", "HEAD~1..HEAD"])
        .output()
        .unwrap();
    let report = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    (report, out.status.code().unwrap())
}

fn hunks(report: &Value) -> &Vec<Value> {
    report["hunks"].as_array().unwrap()
}

fn kinds(report: &Value) -> Vec<&str> {
    hunks(report)
        .iter()
        .map(|h| h["kind"].as_str().unwrap())
        .collect()
}

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
    let (report, code) = classify(RENDER, RENDER_EXTRACTED);
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
    assert_eq!(code, 0);
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
    let (report, code) = classify(&before, &after);
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
    assert_eq!(code, 0);
}

#[test]
fn a_clean_inline_is_the_inverse() {
    let before = invoice(TOTAL_EXTRACTED, Some(APPLY_TAX));
    let after = invoice(TOTAL, None);
    let (report, code) = classify(&before, &after);
    assert_eq!(kinds(&report), ["inline", "inline"], "{report:#}");
    let hunk = &hunks(&report)[0];
    assert_eq!(lead(hunk)["detail"]["method"], "apply_tax");
    assert_eq!(lead(hunk)["detail"]["caller"], "total");
    assert_eq!(lead(hunk)["detail"]["hunks"], serde_json::json!([0, 1]));
    assert_eq!(code, 0);
}

#[test]
fn an_extract_mixed_with_a_logic_edit_in_the_same_method_is_logic() {
    let before = invoice(TOTAL, None);
    let edited = TOTAL_EXTRACTED.replace("puts rounded", "puts rounded.to_s");
    let after = invoice(&edited, Some(APPLY_TAX));
    let (report, _) = classify(&before, &after);
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
    let (report, _) = classify(before, after);
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
    let (report, _) = classify(before, after);
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
    let (report, _) = classify(before, after);
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
    let (report, _) = classify(before, after);
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
    let (report, _) = classify(before, after);
    assert_near(&report, "extract", "tiny");
}

#[test]
fn a_new_method_whose_name_is_already_in_use_is_logic() {
    let other = "class Other\n  def rule\n    :other\n  end\nend\n";
    let before = format!("{RENDER}{other}");
    let after = format!("{RENDER_EXTRACTED}{other}");
    let (report, _) = classify(&before, &after);
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
    let (report, _) = classify(before, after);
    assert_near(&report, "extract", "wiring");
    assert_eq!(report["summary"]["verdict"], "behavior");
}

#[test]
fn a_class_method_calling_a_new_instance_method_is_logic() {
    for receiver in ["self.", "self::"] {
        let before = SHELF.replace("self.", receiver);
        let after = SHELF_SHOWN.replace("self.", receiver);
        let (report, _) = classify(&before, &after);
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
    let (report, _) = classify(&before, &after);
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
    let (report, _) = classify(&before, &after);
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
    let (report, _) = classify(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn a_reformatted_sibling_keeps_its_label_beside_an_extract() {
    let before = invoice(TOTAL, None);
    let after =
        invoice(TOTAL_EXTRACTED, Some(APPLY_TAX)).replace("def filler2\n", "def  filler2\n");
    let (report, code) = classify(&before, &after);
    assert_eq!(
        kinds(&report),
        ["extract", "format_only", "extract"],
        "{report:#}"
    );
    assert_eq!(
        lead(&hunks(&report)[0])["detail"]["hunks"],
        serde_json::json!([0, 2])
    );
    assert_eq!(code, 0);
}

#[test]
fn a_new_use_of_the_new_method_elsewhere_is_not_a_collision() {
    let spec = "describe Report do\n  it { Report.new.rule }\nend\n";
    let (report, _) = classify_files(
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
    let (report, _) = classify_files(
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
    let (report, _) = classify(&before, &after);
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
    let (report, _) = classify(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn an_extract_sharing_a_hunk_with_an_edit_to_a_sibling_leaves_the_edit_as_logic() {
    let before = invoice(TOTAL, None);
    let after = invoice(TOTAL_EXTRACTED, Some(APPLY_TAX)).replace("    4\n", "    40\n");
    let (report, _) = classify(&before, &after);
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
    let (report, _) = classify(before, after);
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
    let (report, _) = classify(before, after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}
