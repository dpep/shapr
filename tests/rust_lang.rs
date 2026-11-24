//! End to end: Rust files, through the tree-sitter frontend.

use serde_json::Value;

mod common;
use common::*;

fn assert_kinds(report: &Value, want: &[&str]) {
    assert_eq!(kinds(report), want, "{report:#}");
}

/// Every hunk is an unchanged body moved under a new `context`: a `move`
/// part that isn't structure, so the hunk is `logic`, reviewing only the
/// definitions' first lines.
fn assert_context_move(report: &Value, context: &str) {
    assert!(kinds(report).iter().all(|k| *k == "logic"), "{report:#}");
    for hunk in hunks(report) {
        assert_eq!(
            lead(hunk)["detail"]["context"],
            serde_json::json!([context])
        );
        assert_eq!(lead(hunk)["evidence"]["min_similarity"], 1.0);
        // One line per end of a move in this hunk: the definition's first.
        let ends: usize = lead(hunk)["detail"]["moves"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                usize::from(m["from_hunk"] == hunk["id"]) + usize::from(m["to_hunk"] == hunk["id"])
            })
            .sum();
        assert_eq!(hunk["review_lines"], ends, "{hunk:#}");
    }
    assert_eq!(
        report["summary"]["move_contexts"][context],
        hunks(report).len()
    );
}

#[test]
fn rust_files_are_parsed() {
    let report = rust("fn a() {}\n", "fn a() { b(); }\n");
    let hunk = &hunks(&report)[0];
    assert_eq!(hunk["language"], "rust");
    assert_eq!(hunk["parsed"], true);
    assert_eq!(hunk["kind"], "logic");
}

#[test]
fn a_rustfmt_reflow_is_format_only() {
    let report = rust(
        "fn total(items: &[u32]) -> u32 { items.iter().map(|x| x * 2).filter(|x| *x > 3).sum() }\n",
        "fn total(items: &[u32]) -> u32 {\n    items\n        .iter()\n        .map(|x| x * 2)\n        .filter(|x| *x > 3)\n        .sum()\n}\n",
    );
    assert_kinds(&report, &["format_only"]);
}

#[test]
fn whitespace_inside_a_string_is_logic() {
    let report = rust(
        "fn a() -> &'static str { \"a b\" }\n",
        "fn a() -> &'static str { \"a  b\" }\n",
    );
    assert_kinds(&report, &["logic"]);
}

#[test]
fn a_doc_comment_edit_is_comment_only() {
    let report = rust(
        "/// Adds one.\npub fn inc(x: u32) -> u32 {\n    x + 1 // plain\n}\n",
        "/// Adds one to `x`.\n///\n/// Wraps on overflow? No: panics.\npub fn inc(x: u32) -> u32 {\n    x + 1 /* block */\n}\n",
    );
    assert_kinds(&report, &["comment_only"]);
}

#[test]
fn an_attribute_change_is_logic() {
    for (before, after) in [
        ("#[inline]\nfn a() {}\n", "#[inline(never)]\nfn a() {}\n"),
        (
            "#[cfg(test)]\nfn a() {}\n",
            "#[cfg(not(test))]\nfn a() {}\n",
        ),
        ("#[doc = \"x\"]\nfn a() {}\n", "#[doc = \"y\"]\nfn a() {}\n"),
        (
            "#![allow(dead_code)]\nfn a() {}\n",
            "#![deny(dead_code)]\nfn a() {}\n",
        ),
    ] {
        assert_kinds(&rust(before, after), &["logic"]);
    }
}

#[test]
fn a_visibility_change_is_logic() {
    assert_kinds(&rust("fn a() {}\n", "pub fn a() {}\n"), &["logic"]);
}

#[test]
fn a_local_rename_is_a_rename() {
    let report = rust(
        "fn area(w: u32, h: u32) -> u32 {\n    let n = w * h;\n    n + 1\n}\n",
        "fn area(w: u32, h: u32) -> u32 {\n    let size = w * h;\n    size + 1\n}\n",
    );
    assert_kinds(&report, &["rename"]);
    let rename = &lead(&hunks(&report)[0])["detail"]["renames"][0];
    assert_eq!(rename["from"], "n");
    assert_eq!(rename["to"], "size");
    assert_eq!(rename["scope"], "local");
}

#[test]
fn a_local_renamed_inside_a_format_macro_is_a_rename() {
    let report = rust(
        "fn show(w: u32) {\n    let n = w * 2;\n    println!(\"{} {n}\", n);\n}\n",
        "fn show(w: u32) {\n    let size = w * 2;\n    println!(\"{} {size}\", size);\n}\n",
    );
    assert_kinds(&report, &["rename"]);
}

#[test]
fn a_rename_whose_old_name_survives_in_a_macro_is_logic() {
    // The macro's tokens still say `n`: the parser can't tell what it means.
    for line in ["println!(\"{}\", n);", "println!(\"{n}\");"] {
        let report = rust(
            &format!("fn show(w: u32) {{\n    let n = w * 2;\n    {line}\n}}\n"),
            &format!("fn show(w: u32) {{\n    let size = w * 2;\n    {line}\n}}\n"),
        );
        assert_kinds(&report, &["logic"]);
    }
}

#[test]
fn a_function_rename_with_its_call_sites_is_a_rename() {
    let before = "pub fn compute(x: u32) -> u32 {\n    x + 1\n}\n\npub fn run() -> u32 {\n    compute(2) + compute(3)\n}\n";
    let after = "pub fn evaluate(x: u32) -> u32 {\n    x + 1\n}\n\npub fn run() -> u32 {\n    evaluate(2) + evaluate(3)\n}\n";
    let report = rust(before, after);
    assert!(kinds(&report).iter().all(|k| *k == "rename"), "{report:#}");
    let rename = &lead(&hunks(&report)[0])["detail"]["renames"][0];
    assert_eq!(rename["scope"], "method");
    assert_eq!(rename["occurrences"], 3);
}

#[test]
fn a_function_rename_missing_a_use_is_logic() {
    // `compute` still passed by name to `map`.
    let before = "fn compute(x: u32) -> u32 {\n    x + 1\n}\n\nfn run(v: Vec<u32>) -> Vec<u32> {\n    let a = compute(2);\n    v.into_iter().map(compute).collect()\n}\n";
    let after = "fn evaluate(x: u32) -> u32 {\n    x + 1\n}\n\nfn run(v: Vec<u32>) -> Vec<u32> {\n    let a = evaluate(2);\n    v.into_iter().map(compute).collect()\n}\n";
    let report = rust(before, after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn a_field_rename_reaches_every_file() {
    let before = [
        ("a.rs", "pub struct Point {\n    pub x: i32,\n}\n"),
        ("b.rs", "fn get(p: &Point) -> i32 {\n    p.x\n}\n"),
    ];
    let renamed = [
        ("a.rs", "pub struct Point {\n    pub left: i32,\n}\n"),
        ("b.rs", "fn get(p: &Point) -> i32 {\n    p.left\n}\n"),
    ];
    assert_kinds(&classify(&before, &renamed), &["rename", "rename"]);
    // A use left behind in another file.
    let partial = [
        ("a.rs", "pub struct Point {\n    pub left: i32,\n}\n"),
        ("b.rs", "fn get(p: &Point) -> i32 {\n    p.x + 0\n}\n"),
    ];
    assert_kinds(&classify(&before, &partial), &["logic", "logic"]);
}

#[test]
fn a_field_named_by_a_derive_is_not_renamed() {
    // Debug prints field names; serde reads and writes them.
    let report = rust(
        "#[derive(Debug)]\npub struct Point {\n    pub x: i32,\n}\n",
        "#[derive(Debug)]\npub struct Point {\n    pub left: i32,\n}\n",
    );
    assert_kinds(&report, &["logic"]);
}

const SWAP_BEFORE: &str = "pub struct Counter {\n    n: u32,\n}\n\nimpl Counter {\n    pub fn add(&mut self, by: u32) {\n        self.n += by;\n        self.log();\n    }\n\n    pub fn reset(&mut self) {\n        self.n = 0;\n        self.log();\n    }\n}\n";
const SWAP_AFTER: &str = "pub struct Counter {\n    n: u32,\n}\n\nimpl Counter {\n    pub fn reset(&mut self) {\n        self.n = 0;\n        self.log();\n    }\n\n    pub fn add(&mut self, by: u32) {\n        self.n += by;\n        self.log();\n    }\n}\n";

#[test]
fn swapping_methods_in_an_impl_is_a_move() {
    let report = rust(SWAP_BEFORE, SWAP_AFTER);
    assert!(kinds(&report).iter().all(|k| *k == "move"), "{report:#}");
}

#[test]
fn a_method_moved_to_another_impl_is_a_move_to_a_new_owner() {
    let before =
        "impl A {\n    fn go(&self) -> u32 {\n        self.x + self.y\n    }\n}\n\nimpl B {\n}\n";
    let after =
        "impl A {\n}\n\nimpl B {\n    fn go(&self) -> u32 {\n        self.x + self.y\n    }\n}\n";
    let report = rust(before, after);
    assert_context_move(&report, "owner");
}

#[test]
fn an_attribute_left_behind_by_a_move_is_logic() {
    // Swapping the functions under a fixed attribute line moves the attribute
    // onto the other function.
    let f = |name: &str| format!("fn {name}() -> u32 {{\n    let v = 1;\n    v + 2\n}}\n");
    let before = format!("#[cfg(test)]\n{}{}", f("a"), f("b"));
    let after = format!("#[cfg(test)]\n{}{}", f("b"), f("a"));
    let report = rust(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn a_function_moved_to_another_module_file_is_a_move_to_a_new_module() {
    // Each file is a module: the function's path and the names it resolves
    // change with its file.
    let f = "pub fn helper(x: u32) -> u32 {\n    let y = x * 2;\n    y + 1\n}\n";
    let report = classify(
        &[("a.rs", f), ("b.rs", "pub fn other() {}\n")],
        &[
            ("a.rs", ""),
            ("b.rs", &format!("pub fn other() {{}}\n\n{f}")),
        ],
    );
    assert_context_move(&report, "module");
}

#[test]
fn reordering_struct_fields_or_enum_variants_is_logic() {
    for (before, after) in [
        (
            "struct P {\n    x: u32,\n    y: u32,\n}\n",
            "struct P {\n    y: u32,\n    x: u32,\n}\n",
        ),
        (
            "enum E {\n    A,\n    B,\n}\n",
            "enum E {\n    B,\n    A,\n}\n",
        ),
    ] {
        assert!(
            kinds(&rust(before, after)).iter().all(|k| *k == "logic"),
            "{before}"
        );
    }
}

const CALLER: &str = "impl Report {\n    pub fn render(&self, rows: &[u32]) -> String {\n        let header = self.title.clone();\n";

#[test]
fn extracting_a_helper_is_an_extract() {
    let before = format!(
        "{CALLER}        let total: u32 = rows.iter().sum();\n        let scaled = total * self.scale;\n        format!(\"{{}}: {{}}\", header, scaled)\n    }}\n}}\n"
    );
    let after = format!(
        "{CALLER}        self.summary(rows, header)\n    }}\n\n    fn summary(&self, rows: &[u32], header: String) -> String {{\n        let total: u32 = rows.iter().sum();\n        let scaled = total * self.scale;\n        format!(\"{{}}: {{}}\", header, scaled)\n    }}\n}}\n"
    );
    let report = rust(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "extract"), "{report:#}");
    let detail = &lead(&hunks(&report)[0])["detail"];
    assert_eq!(detail["method"], "summary");
    assert_eq!(detail["caller"], "render");
}

#[test]
fn extracting_statements_holding_a_question_mark_is_logic() {
    let caller = "impl Loader {\n    pub fn load(&self, path: &str) -> Result<u32, Error> {\n        let base = self.root.len();\n";
    let before = format!(
        "{caller}        let text = std::fs::read_to_string(path)?;\n        let n = text.trim().parse::<u32>()?;\n        let size = n + base as u32;\n        Ok(size)\n    }}\n}}\n"
    );
    let after = format!(
        "{caller}        let size = self.read(path, base)?;\n        Ok(size)\n    }}\n\n    fn read(&self, path: &str, base: usize) -> Result<u32, Error> {{\n        let text = std::fs::read_to_string(path)?;\n        let n = text.trim().parse::<u32>()?;\n        Ok(n + base as u32)\n    }}\n}}\n"
    );
    let report = rust(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn extracting_statements_followed_by_more_code_that_bind_locals_is_logic() {
    // `guard` would now drop when `lock_and_count` returns, before `notify`.
    let caller = "impl Pool {\n    pub fn run(&self, jobs: &[u32]) -> usize {\n        let start = self.clock.now();\n";
    let before = format!(
        "{caller}        let guard = self.lock.lock();\n        let count = jobs.len() + guard.len();\n        self.log(count);\n        self.notify(start);\n        count\n    }}\n}}\n"
    );
    let after = format!(
        "{caller}        let count = self.lock_and_count(jobs);\n        self.notify(start);\n        count\n    }}\n\n    fn lock_and_count(&self, jobs: &[u32]) -> usize {{\n        let guard = self.lock.lock();\n        let count = jobs.len() + guard.len();\n        self.log(count);\n        count\n    }}\n}}\n"
    );
    let report = rust(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
    assert_eq!(
        lead(&hunks(&report)[0])["detail"]["risk"],
        "drop",
        "{report:#}"
    );
}

#[test]
fn extracting_statements_holding_an_unknown_macro_is_logic() {
    // `ensure!` may expand to a `return`.
    let caller = "impl Check {\n    pub fn run(&self, v: u32) -> Result<u32, Error> {\n        let limit = self.limit;\n";
    let before = format!(
        "{caller}        ensure!(v < limit, \"too big\");\n        let doubled = v * 2 + limit;\n        Ok(doubled)\n    }}\n}}\n"
    );
    let after = format!(
        "{caller}        let doubled = self.double(v, limit);\n        Ok(doubled)\n    }}\n\n    fn double(&self, v: u32, limit: u32) -> u32 {{\n        ensure!(v < limit, \"too big\");\n        v * 2 + limit\n    }}\n}}\n"
    );
    let report = rust(&before, &after);
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}

#[test]
fn editing_a_type_in_place_is_not_a_move() {
    let report = rust(
        "enum Key {\n    Name(u64),\n    Code(u64),\n    Path(Vec<String>),\n    Pair(u32, u32),\n}\n",
        "enum Key<'t> {\n    Name(u64),\n    Code(u64),\n    Path(Vec<String>),\n    Pair(u32, u32),\n    Declaration(&'t str),\n}\n",
    );
    assert_kinds(&report, &["logic"]);
}

#[test]
fn a_function_moved_between_same_named_cfg_modules_is_a_move_to_a_new_owner() {
    let f = "    pub fn open(x: u32) -> u32 {\n        let y = x * 2;\n        y + 1\n    }\n";
    let before = format!("#[cfg(unix)]\nmod imp {{\n{f}}}\n\n#[cfg(windows)]\nmod imp {{\n}}\n");
    let after = format!("#[cfg(unix)]\nmod imp {{\n}}\n\n#[cfg(windows)]\nmod imp {{\n{f}}}\n");
    let report = rust(&before, &after);
    assert_context_move(&report, "owner");
}

#[test]
fn a_reflow_adding_trailing_commas_is_format_only() {
    let report = rust(
        "fn a() {\n    call(first_argument, second_argument, third_argument);\n    let v = [1, 2];\n    match v.len() { 0 => 1, _ => 2 }\n}\n",
        "fn a() {\n    call(\n        first_argument,\n        second_argument,\n        third_argument,\n    );\n    let v = [1, 2,];\n    match v.len() {\n        0 => 1,\n        _ => 2,\n    }\n}\n",
    );
    assert_kinds(&report, &["format_only"]);
}

#[test]
fn a_comma_making_a_one_element_tuple_is_logic() {
    assert_kinds(
        &rust("fn a() -> u8 { (1) }\n", "fn a() -> (u8,) { (1,) }\n"),
        &["logic"],
    );
    assert_kinds(
        &rust(
            "fn a(t: (u8,)) { let (x,) = t; }\n",
            "fn a(t: (u8,)) { let (x) = t; }\n",
        ),
        &["logic"],
    );
}

#[test]
fn editing_a_type_s_attributes_in_place_is_not_a_move() {
    let fields = "    pub verbose: bool,\n    pub quiet: bool,\n    pub path: String,\n    pub depth: u32,\n";
    let report = rust(
        &format!("#[derive(Parser)]\n#[command(about = \"old\")]\npub struct Cli {{\n{fields}}}\n"),
        &format!(
            "#[derive(Parser)]\n#[command(about = \"new\")]\npub struct Cli {{\n{fields}    pub color: bool,\n}}\n"
        ),
    );
    assert!(kinds(&report).iter().all(|k| *k == "logic"), "{report:#}");
}
