//! End to end: changes that look structural but can change behavior. Each
//! must stay out of the structural kinds.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

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

/// `shapr -j` over one commit writing each (path, content) pair.
fn classify(before: &[(&str, &str)], after: &[(&str, &str)]) -> Value {
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
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

fn ruby(before: &str, after: &str) -> Value {
    classify(&[("a.rb", before)], &[("a.rb", after)])
}

fn rust(before: &str, after: &str) -> Value {
    classify(&[("lib.rs", before)], &[("lib.rs", after)])
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

/// Every hunk is `logic`, and some hunk was a near miss of `near` for `risk`.
fn assert_near(report: &Value, near: &str, risk: Option<&str>) {
    assert!(kinds(report).iter().all(|k| *k == "logic"), "{report:#}");
    assert!(
        hunks(report)
            .iter()
            .any(|h| h["detail"]["near"] == near && risk.is_none_or(|r| h["detail"]["risk"] == r)),
        "{report:#}"
    );
}

// The line method: a file with a syntax error can't prove structure.

#[test]
fn a_whitespace_edit_in_a_file_that_doesnt_parse_is_logic() {
    let tail = "def b\n  1\nend\ndef c(\n";
    let report = ruby(
        &format!("def a(x)\n  puts -x\nend\n{tail}"),
        &format!("def a(x)\n  puts - x\nend\n{tail}"),
    );
    assert_near(&report, "format_only", None);
    let hunk = &hunks(&report)[0];
    assert_eq!(hunk["evidence"]["method"], "line");
    let reason = hunk["reason"].as_str().unwrap();
    assert!(reason.contains("compared line by line"), "{reason}");
}

#[test]
fn a_rust_token_split_that_breaks_the_parse_is_logic() {
    let report = rust(
        "fn f(a: u32) -> u32 {\n    a >> 1\n}\n",
        "fn f(a: u32) -> u32 {\n    a > > 1\n}\n",
    );
    assert_near(&report, "format_only", None);
}

#[test]
fn a_comment_edit_in_a_file_that_doesnt_parse_is_logic() {
    let tail = "def c(\n";
    let report = ruby(
        &format!("def a\n  1 # one\nend\n{tail}"),
        &format!("def a\n  1 # uno\nend\n{tail}"),
    );
    assert_near(&report, "comment_only", None);
}

// Comments the language reads as code.

#[test]
fn a_shebang_is_code() {
    let report = ruby(
        "#!/usr/bin/env ruby\nputs 1\n",
        "#!/usr/bin/env ruby -w\nputs 1\n",
    );
    assert_eq!(kinds(&report), ["logic"]);
    let report = rust(
        "#!/usr/bin/env run-cargo-script\nfn main() {}\n",
        "#!/usr/bin/env rust-script\nfn main() {}\n",
    );
    assert_eq!(kinds(&report), ["logic"]);
}

// Rust doc comments: fenced code is a doctest, and docs belong to an item.

#[test]
fn editing_a_doctest_is_logic() {
    let doc =
        |body: &str| format!("/// Adds.\n///\n/// ```\n/// {body}\n/// ```\npub fn f() {{}}\n");
    let report = rust(&doc("assert_eq!(1, 1);"), &doc("assert_eq!(1, 2);"));
    assert_eq!(kinds(&report), ["logic"]);
    let report = rust(
        "/// Does a thing.\npub fn f() {}\n",
        "/// Does a thing.\n///\n/// ```\n/// assert_eq!(1, 2);\n/// ```\npub fn f() {}\n",
    );
    assert_eq!(kinds(&report), ["logic"]);
}

#[test]
fn a_fence_info_string_and_a_block_doctest_are_code() {
    let report = rust(
        "//! ```\n//! f();\n//! ```\npub fn f() {}\n",
        "//! ```ignore\n//! f();\n//! ```\npub fn f() {}\n",
    );
    assert_eq!(kinds(&report), ["logic"]);
    let report = rust(
        "/** ```\nassert!(true);\n``` */\npub fn f() {}\n",
        "/** ```\nassert!(false);\n``` */\npub fn f() {}\n",
    );
    assert_eq!(kinds(&report), ["logic"]);
}

#[test]
fn doc_prose_beside_a_doctest_is_a_comment() {
    let doc = |prose: &str| {
        format!("/// {prose}\n///\n/// ```\n/// assert!(true);\n/// ```\npub fn f() {{}}\n")
    };
    let report = rust(&doc("Adds."), &doc("Adds two numbers."));
    assert_eq!(kinds(&report), ["comment_only"]);
}

#[test]
fn a_doc_moved_to_another_item_is_not_format_only() {
    let report = rust(
        "/// a\nfn f() {}\n\nfn g() {}\n",
        "fn f() {}\n\n/// a\nfn g() {}\n",
    );
    assert_eq!(kinds(&report), ["comment_only"]);
    let report = ruby(
        "def a\n  # note\n  1\nend\n\ndef b\n  2\nend\n",
        "def a\n  1\nend\n\ndef b\n  # note\n  2\nend\n",
    );
    assert_eq!(kinds(&report), ["comment_only"]);
}
/// No hunk is structural.
fn assert_not_structure(report: &Value) {
    assert!(
        kinds(report)
            .iter()
            .all(|k| matches!(*k, "logic" | "move_with_changes")),
        "{report:#}"
    );
}

// Moves: a definition's place can matter.

#[test]
fn constants_whose_values_run_code_keep_their_order() {
    let report = ruby(
        "class A\n  X = Counter.next\n  Y = Counter.next\nend\n",
        "class A\n  Y = Counter.next\n  X = Counter.next\nend\n",
    );
    assert_not_structure(&report);
    // Literal values run nothing: reordering them is a move.
    let report = ruby(
        "class A\n  X = [1]\n  Y = [2, 3, 4]\nend\n",
        "class A\n  Y = [2, 3, 4]\n  X = [1]\nend\n",
    );
    assert_eq!(kinds(&report), ["move"]);
}

// Extract and inline: code bound to its method's frame or its position.

/// `go`'s `chunk` (lines indented four spaces) extracted into `show(a)`.
fn extract(chunk: &str) -> Value {
    ruby(
        &format!("class A\n  def go(a)\n{chunk}    1\n  end\nend\n"),
        &format!(
            "class A\n  def go(a)\n    show(a)\n    1\n  end\n\n  def show(a)\n{chunk}  end\nend\n"
        ),
    )
}

#[test]
fn a_chunk_reading_its_frame_is_not_extracted() {
    for chunk in [
        "    t = a * 2 + 1\n    puts binding.local_variables.to_s + t.to_s\n",
        "    t = eval('a * 2')\n    puts t.to_s + a.to_s + 'z'\n",
        "    n = __method__\n    puts n.to_s + a.to_s + 'x'\n",
        "    puts caller.first.to_s + a.to_s\n    puts a.to_s + 'y' + 'z'\n",
        "    puts caller_locations.first.to_s + a.to_s\n    puts a.to_s + 'y'\n",
        "    t = a * 2 + 1\n    puts t.to_s + 'zz' + __LINE__.to_s\n",
        "    t = Proc.new.call + a * 2\n    puts t.to_s + 'zz' + a.to_s\n",
        "    puts $1.to_s + a.to_s + 'zz'\n    puts a.to_s * 2\n",
        "    puts $~.to_s + a.to_s + 'zz'\n    puts a.to_s * 2\n",
    ] {
        assert_near(&extract(chunk), "extract", Some("jump"));
    }
    // The file is the same file: `__FILE__` reads the same in the new method.
    let report = extract("    t = a * 2 + 1\n    puts t.to_s + 'zz' + __FILE__\n");
    assert_eq!(kinds(&report), ["extract"]);
}

#[test]
fn a_rust_chunk_reading_its_line_is_not_extracted() {
    let report = rust(
        "fn go(a: u32) -> u32 {\n    let t = a * 2 + 1;\n    println!(\"{} {}\", t, line!());\n    1\n}\n",
        "fn go(a: u32) -> u32 {\n    show(a);\n    1\n}\n\nfn show(a: u32) {\n    let t = a * 2 + 1;\n    println!(\"{} {}\", t, line!());\n}\n",
    );
    assert_near(&report, "extract", Some("jump"));
}

fn contexts(report: &Value) -> Vec<&str> {
    hunks(report)
        .iter()
        .flat_map(|h| h["detail"]["context"].as_array().into_iter().flatten())
        .map(|c| c.as_str().unwrap())
        .collect()
}

#[test]
fn a_moved_definition_reading_its_line_changes_position() {
    let report = ruby(
        "class A\n  def x\n    __LINE__\n  end\n\n  def y\n    1\n  end\nend\n",
        "class A\n  def y\n    1\n  end\n\n  def x\n    __LINE__\n  end\nend\n",
    );
    assert_not_structure(&report);
    assert!(contexts(&report).contains(&"position"), "{report:#}");
    // In the same file, `__FILE__` reads the same.
    let report = ruby(
        "class A\n  def x\n    __FILE__\n  end\n\n  def y\n    1\n  end\nend\n",
        "class A\n  def y\n    1\n  end\n\n  def x\n    __FILE__\n  end\nend\n",
    );
    assert!(kinds(&report).iter().all(|k| *k == "move"), "{report:#}");
}

#[test]
fn a_definition_reading_its_file_changes_position_across_files() {
    let report = classify(
        &[
            (
                "a.rb",
                "def here\n  File.dirname(__FILE__)\nend\n\ndef keep\n  1\nend\n",
            ),
            ("b.rb", "def other\n  2\nend\n"),
        ],
        &[
            ("a.rb", "def keep\n  1\nend\n"),
            (
                "b.rb",
                "def other\n  2\nend\n\ndef here\n  File.dirname(__FILE__)\nend\n",
            ),
        ],
    );
    assert_not_structure(&report);
    assert!(contexts(&report).contains(&"position"), "{report:#}");
}

#[test]
fn a_function_moved_past_a_macro_redefinition_it_invokes_is_not_a_move() {
    let report = rust(
        "macro_rules! a { () => { 1 } }\nfn f() -> i32 { a!() }\nmacro_rules! a { () => { 2 } }\nfn g() {}\n",
        "macro_rules! a { () => { 1 } }\nmacro_rules! a { () => { 2 } }\nfn f() -> i32 { a!() }\nfn g() {}\n",
    );
    assert_not_structure(&report);
    // One that doesn't invoke it moves freely.
    let report = rust(
        "macro_rules! a { () => { 1 } }\nfn f() -> i32 { a!() }\nfn g() {}\n",
        "fn g() {}\nmacro_rules! a { () => { 1 } }\nfn f() -> i32 { a!() }\n",
    );
    assert_eq!(kinds(&report), ["move"]);
}

// Names the language or its runtime calls without a visible call site.

#[test]
fn a_chunk_extracted_into_a_protocol_or_builtin_name_collides() {
    let chunk = "    t = a * 2 + 1\n    u = t + a * 3\n    v = u - t\n";
    for name in [
        "inspect",
        "hash",
        "initialize",
        "method_missing",
        "each",
        "call",
        "freeze",
        "require",
        "display",
        "respond_to?",
        "to_str",
    ] {
        let report = ruby(
            &format!("class A\n  def go(a)\n{chunk}    1\n  end\nend\n"),
            &format!(
                "class A\n  def go(a)\n    {name}(a)\n    1\n  end\n\n  def {name}(a)\n{chunk}  end\nend\n"
            ),
        );
        assert_near(&report, "extract", Some("collision"));
    }
    let report = rust(
        "struct A;\nimpl A {\n    fn go(&self, a: u32) -> u32 {\n        let t = a * 2 + 1;\n        let u = t + a * 3;\n        u - t\n    }\n}\n",
        "struct A;\nimpl A {\n    fn go(&self, a: u32) -> u32 {\n        self.clone(a)\n    }\n\n    fn clone(&self, a: u32) -> u32 {\n        let t = a * 2 + 1;\n        let u = t + a * 3;\n        u - t\n    }\n}\n",
    );
    assert_near(&report, "extract", Some("collision"));
}

#[test]
fn a_method_renamed_to_or_from_a_protocol_name_is_logic() {
    for (from, to) in [
        ("fmt", "to_s"),
        ("setup", "initialize"),
        ("walk", "each"),
        ("call", "run"),
        ("fallback", "method_missing"),
    ] {
        let report = ruby(
            &format!("class A\n  def {from}(x)\n    x\n  end\nend\n"),
            &format!("class A\n  def {to}(x)\n    x\n  end\nend\n"),
        );
        assert_eq!(kinds(&report), ["logic"], "{from} -> {to}");
    }
    for (before, after) in [
        ("fn start() {}\n", "fn main() {}\n"),
        ("fn main() {}\n", "fn start() {}\n"),
        (
            "struct A;\nimpl A { fn show(&self) -> i32 { 1 } }\n",
            "struct A;\nimpl A { fn clone(&self) -> i32 { 1 } }\n",
        ),
    ] {
        assert_eq!(kinds(&rust(before, after)), ["logic"], "{after}");
    }
}
