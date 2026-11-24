//! Fixtures as data: each `tests/fixtures/<area>/<case>/` holding a
//! `case.json` is one case. docs/fixtures.md is the format.
//!
//! - `SHAPR_FIXTURE=<text>` runs only the cases whose `<area>/<case>`
//!   contains it.
//! - `SHAPR_BLESS=1` rewrites each run case's `expect` from current output,
//!   keeping the fields it already asserts; review the diff before
//!   committing. Refused when `CI` is set; open cases are never blessed.

mod common;
use common::*;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The trees a case may hold, in the order they reach the repository.
const TREES: [&str; 4] = ["repo", "before", "after", "worktree"];
/// Who can label a case: a person, a model, or a tool.
const LABELLERS: [&str; 3] = ["human", "llm", "tool"];
/// `case.json`'s keys, in the order a rewrite writes them.
const KEYS: [&str; 7] = [
    "description",
    "args",
    "norm",
    "open",
    "source",
    "label",
    "expect",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every file under `dir`, as (path relative to `dir` with `/`, full path),
/// sorted by relative path.
fn files(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), &format!("{name}/"), out);
            } else {
                out.push((name, entry.path()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out.sort();
    out
}

/// SHA-256 over the case's trees: for each file in path order, its path
/// from the case directory (`after/lib/a.rb`), a NUL, its length in decimal,
/// a NUL, and its bytes. case.json is left out: one change is one case,
/// however it is labelled.
fn content_hash(case: &Path) -> String {
    let mut all: Vec<(String, PathBuf)> = TREES
        .iter()
        .flat_map(|tree| {
            files(&case.join(tree))
                .into_iter()
                .map(move |(rel, full)| (format!("{tree}/{rel}"), full))
        })
        .collect();
    all.sort();
    let mut hasher = Sha256::new();
    for (rel, full) in all {
        let bytes = std::fs::read(full).unwrap();
        hasher.update(rel.as_bytes());
        hasher.update(b"\0");
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(&bytes);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn copy_tree(from: &Path, to: &Path) {
    for (rel, full) in files(from) {
        let target = to.join(rel);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(full, target).unwrap();
    }
}

/// The case's repository: `repo/` and `before/` committed, then `after/`
/// replacing `before/`, then `worktree/` written uncommitted. shapr's
/// report (or why there is none), narrowed to `norm` when the case names one.
fn run(case: &Path, spec: &Value) -> Result<Value, String> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init(root);
    copy_tree(&case.join("repo"), root);
    copy_tree(&case.join("before"), root);
    commit(root, &[], "before");
    for (rel, _) in files(&case.join("before")) {
        std::fs::remove_file(root.join(rel)).unwrap();
    }
    copy_tree(&case.join("after"), root);
    commit(root, &[], "after");
    copy_tree(&case.join("worktree"), root);
    let args: Vec<&str> = match &spec["args"] {
        Value::Null => vec!["-j", "HEAD~1..HEAD"],
        args => (args.as_array().into_iter().flatten())
            .map(|a| a.as_str().unwrap_or_default())
            .collect(),
    };
    let out = shapr(root, &args);
    if out.status.code() != Some(0) {
        return Err(format!(
            "shapr {args:?} exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let mut report: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    if let Some(norm) = spec["norm"].as_str() {
        let only = |entries: &mut Value| {
            if let Some(entries) = entries.as_array_mut() {
                entries.retain(|e| e["norm"] == norm);
            }
        };
        only(&mut report["norms"]);
        for hunk in report["hunks"].as_array_mut().into_iter().flatten() {
            if hunk["style"].is_object() {
                only(&mut hunk["style"]["norms"]);
            }
        }
    }
    Ok(report)
}

/// An operator node: an object whose only key starts with `$`.
fn operator(expect: &Value) -> Option<(&str, &Value)> {
    match expect.as_object() {
        Some(map) if map.len() == 1 => {
            let (key, value) = map.iter().next().unwrap();
            key.starts_with('$').then_some((key.as_str(), value))
        }
        _ => None,
    }
}

/// Where `actual` fails the pattern `expect`, by JSON path: an object
/// matches when each of its keys does (others are ignored), an array when
/// it has as many elements and each matches, a number by value, anything
/// else by equality; `{"$not": P}`, `{"$some": P}` and `{"$every": P}` as
/// docs/fixtures.md says.
fn mismatches(at: &str, expect: &Value, actual: &Value, out: &mut Vec<String>) {
    let matches = |p: &Value, a: &Value| {
        let mut found = Vec::new();
        mismatches(at, p, a, &mut found);
        found.is_empty()
    };
    if let Some((op, pattern)) = operator(expect) {
        let elements = actual.as_array();
        match op {
            "$not" if matches(pattern, actual) => {
                out.push(format!("{at}: expected not {pattern}, got {actual}"));
            }
            "$not" => {}
            "$some" | "$every" if elements.is_none() => {
                out.push(format!("{at}: expected an array, got {actual}"));
            }
            "$some" if !elements.unwrap().iter().any(|a| matches(pattern, a)) => {
                out.push(format!("{at}: no element matches {pattern}"));
            }
            "$some" => {}
            "$every" => {
                for (i, a) in elements.unwrap().iter().enumerate() {
                    mismatches(&format!("{at}[{i}]"), pattern, a, out);
                }
            }
            _ => out.push(format!("{at}: unknown operator {op}")),
        }
        return;
    }
    match (expect, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (key, value) in e {
                let path = format!("{at}.{key}");
                match a.get(key) {
                    Some(actual) => mismatches(&path, value, actual, out),
                    None => out.push(format!("{path}: missing")),
                }
            }
        }
        (Value::Array(e), Value::Array(a)) if e.len() == a.len() => {
            for (i, (e, a)) in e.iter().zip(a).enumerate() {
                mismatches(&format!("{at}[{i}]"), e, a, out);
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            out.push(format!(
                "{at}: expected {} elements, got {}",
                e.len(),
                a.len()
            ));
        }
        (Value::Number(e), Value::Number(a)) if e.as_f64() == a.as_f64() => {}
        _ if expect != actual => out.push(format!("{at}: expected {expect}, got {actual}")),
        _ => {}
    }
}

/// What a fresh case asserts: each hunk's file, kind, verdict and parts
/// (kind, structure, lines, assumptions, facets, and any near miss), and the
/// change's verdict.
fn default_expect(report: &Value) -> Value {
    let pick = |from: &Value, keys: &[&str]| -> Value {
        let map: Map<String, Value> = keys
            .iter()
            .filter_map(|k| from.get(*k).map(|v| (k.to_string(), v.clone())))
            .collect();
        Value::Object(map)
    };
    let hunks: Vec<Value> = (report["hunks"].as_array().into_iter().flatten())
        .map(|hunk| {
            let mut out = pick(hunk, &["file", "kind", "verdict"]);
            let parts: Vec<Value> = (hunk["parts"].as_array().into_iter().flatten())
                .map(|part| {
                    let mut out = pick(
                        part,
                        &["kind", "structure", "lines", "assumptions", "facets"],
                    );
                    let near = pick(&part["detail"], &["near", "risk"]);
                    if !near.as_object().unwrap().is_empty() {
                        out["detail"] = near;
                    }
                    out
                })
                .collect();
            out["parts"] = Value::Array(parts);
            out
        })
        .collect();
    json!({"hunks": hunks, "summary": {"verdict": report["summary"]["verdict"]}})
}

/// `expect` with each value it asserts replaced by the actual one: the
/// same fields, current values. Operators are kept as written; an array
/// takes the actual's length, each element shaped by the pattern at its
/// index (or the last one).
fn bless(expect: &Value, actual: &Value) -> Value {
    if operator(expect).is_some() {
        return expect.clone();
    }
    match (expect, actual) {
        (Value::Object(e), Value::Object(a)) => Value::Object(
            e.iter()
                .filter_map(|(k, v)| a.get(k).map(|a| (k.clone(), bless(v, a))))
                .collect(),
        ),
        (Value::Array(e), Value::Array(a)) if !e.is_empty() => Value::Array(
            a.iter()
                .enumerate()
                .map(|(i, a)| bless(&e[i.min(e.len() - 1)], a))
                .collect(),
        ),
        _ => actual.clone(),
    }
}

/// A line diff of two texts: `-` lines only in `old`, `+` only in `new`.
fn diff(old: &str, new: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, String::new());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            out += &format!("   {}\n", a[i]);
            (i, j) = (i + 1, j + 1);
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out += &format!(" - {}\n", a[i]);
            i += 1;
        } else {
            out += &format!(" + {}\n", b[j]);
            j += 1;
        }
    }
    out
}

/// case.json as written: known keys in [`KEYS`] order, 2-space indent.
fn render(spec: &Value) -> String {
    let map = spec.as_object().unwrap();
    let fields: Vec<String> = KEYS
        .iter()
        .filter_map(|k| map.get(*k).map(|v| (k, v)))
        .map(|(k, v)| {
            let value = serde_json::to_string_pretty(v)
                .unwrap()
                .replace('\n', "\n  ");
            format!("  \"{k}\": {value}")
        })
        .collect();
    format!("{{\n{}\n}}\n", fields.join(",\n"))
}

/// What case.json must hold beyond valid JSON: known keys, a description,
/// a boolean `open`, and a `label.labeller` from the closed list.
fn validate(spec: &Value) -> Result<(), String> {
    let map = spec.as_object().ok_or("not an object")?;
    if let Some(key) = map.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return Err(format!("unknown key {key:?}"));
    }
    if !map.get("description").is_some_and(Value::is_string) {
        return Err("no description".into());
    }
    if map.get("open").is_some_and(|o| !o.is_boolean()) {
        return Err("`open` is true or false".into());
    }
    if let Some(label) = map.get("label") {
        let labeller = label["labeller"].as_str().unwrap_or_default();
        if !LABELLERS.contains(&labeller) {
            return Err(format!("label.labeller is one of {LABELLERS:?}"));
        }
    }
    Ok(())
}

enum Outcome {
    Pass,
    /// An open case still failing, as it should.
    Open,
    Fail(String),
}

fn check(case: &Path, bless_it: bool) -> Outcome {
    let text = std::fs::read_to_string(case.join("case.json")).unwrap();
    let mut spec: Value = match serde_json::from_str(&text) {
        Ok(spec) => spec,
        Err(e) => return Outcome::Fail(format!("case.json: {e}")),
    };
    if let Err(e) = validate(&spec) {
        return Outcome::Fail(format!("case.json: {e}"));
    }
    let open = spec["open"] == true;
    let report = match run(case, &spec) {
        Ok(report) => report,
        Err(e) => return Outcome::Fail(e),
    };
    if bless_it && !open {
        spec["expect"] = match &spec["expect"] {
            Value::Null => default_expect(&report),
            expect => bless(expect, &report),
        };
        std::fs::write(case.join("case.json"), render(&spec)).unwrap();
    }
    let expect = &spec["expect"];
    if expect.as_object().is_none_or(Map::is_empty) {
        return Outcome::Fail("case.json: no expect (SHAPR_BLESS=1 writes one)".into());
    }
    let mut found = Vec::new();
    mismatches("", expect, &report, &mut found);
    match (found.is_empty(), open) {
        (true, false) => Outcome::Pass,
        (false, true) => Outcome::Open,
        (true, true) => {
            Outcome::Fail("an open case now passes: drop `open` so it guards the fix".into())
        }
        (false, false) => {
            let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap();
            Outcome::Fail(format!(
                "{}\n  expected (-) vs actual (+):\n{}",
                found.join("\n  "),
                diff(&pretty(expect), &pretty(&bless(expect, &report)))
            ))
        }
    }
}

/// Each case directory, `<area>/<case>`, sorted; a case.json anywhere else
/// under tests/ is an error, so a case can't sit where no runner looks.
fn discover() -> Result<Vec<(String, PathBuf)>, String> {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let root = root();
    let mut cases = Vec::new();
    for (rel, full) in files(&tests) {
        if !rel.ends_with("case.json") {
            continue;
        }
        let dir = full.parent().unwrap();
        let name = dir.strip_prefix(&root).ok().map(Path::to_string_lossy);
        match name {
            Some(name) if name.split('/').count() == 2 => {
                cases.push((name.into_owned(), dir.to_path_buf()));
            }
            _ => {
                return Err(format!(
                    "tests/{rel}: cases live at tests/fixtures/<area>/<case>/"
                ));
            }
        }
    }
    Ok(cases)
}

#[test]
fn fixtures() {
    let filter = std::env::var("SHAPR_FIXTURE").unwrap_or_default();
    let bless_them = std::env::var("SHAPR_BLESS").is_ok_and(|v| v == "1");
    assert!(
        !(bless_them && std::env::var_os("CI").is_some()),
        "SHAPR_BLESS rewrites expectations; never under CI"
    );
    let cases = discover().unwrap_or_else(|e| panic!("{e}"));
    assert!(!cases.is_empty());

    let mut failures = Vec::new();
    let mut seen: std::collections::BTreeMap<String, &str> = Default::default();
    for (name, dir) in &cases {
        let hash = content_hash(dir);
        let leaf = name.rsplit('/').next().unwrap();
        if leaf.len() == 12
            && leaf.bytes().all(|b| b.is_ascii_hexdigit())
            && !hash.starts_with(leaf)
        {
            failures.push(format!(
                "{name}: named by hash, but its content hashes to {hash}"
            ));
        }
        if let Some(other) = seen.insert(hash, name) {
            failures.push(format!("{name}: the same change as {other}"));
        }
    }

    let chosen: Vec<&(String, PathBuf)> =
        cases.iter().filter(|(n, _)| n.contains(&filter)).collect();
    assert!(!chosen.is_empty(), "no case matches SHAPR_FIXTURE={filter}");
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                while let Some((name, dir)) = chosen.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let outcome = check(dir, bless_them);
                    results.lock().unwrap().push((name.clone(), outcome));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    let mut open = Vec::new();
    for (name, outcome) in results {
        match outcome {
            Outcome::Pass => {}
            Outcome::Open => open.push(name),
            Outcome::Fail(why) => failures.push(format!("{name}:\n  {why}")),
        }
    }
    eprintln!(
        "fixtures: {} run, {} open (shapr's to-do list){}",
        chosen.len(),
        open.len(),
        open.iter().map(|n| format!("\n  {n}")).collect::<String>()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n\n"));
}

fn failures(expect: Value, actual: Value) -> Vec<String> {
    let mut out = Vec::new();
    mismatches("", &expect, &actual, &mut out);
    out
}

#[test]
fn expect_is_a_subset_with_exact_arrays() {
    let actual = json!({"kind": "logic", "parts": [{"kind": "logic", "confidence": 1.0}]});
    assert!(failures(json!({"parts": [{"confidence": 1}]}), actual.clone()).is_empty());
    assert_eq!(
        failures(json!({"kind": "rename", "parts": []}), actual.clone()),
        [
            ".kind: expected \"rename\", got \"logic\"",
            ".parts: expected 0 elements, got 1"
        ]
    );
    assert_eq!(failures(json!({"x": 1}), actual), [".x: missing"]);
}

#[test]
fn operators_negate_and_quantify() {
    let actual = json!({"verdict": "behavior", "parts": [{"kind": "move"}, {"kind": "logic"}]});
    assert!(failures(json!({"verdict": {"$not": "structure"}}), actual.clone()).is_empty());
    assert_eq!(
        failures(json!({"verdict": {"$not": "behavior"}}), actual.clone()),
        [".verdict: expected not \"behavior\", got \"behavior\""]
    );
    assert!(
        failures(
            json!({"parts": {"$some": {"kind": "move"}}}),
            actual.clone()
        )
        .is_empty()
    );
    assert_eq!(
        failures(
            json!({"parts": {"$some": {"kind": "rename"}}}),
            actual.clone()
        ),
        [".parts: no element matches {\"kind\":\"rename\"}"]
    );
    assert_eq!(
        failures(json!({"parts": {"$every": {"kind": "logic"}}}), actual),
        [".parts[0].kind: expected \"logic\", got \"move\""]
    );
}

#[test]
fn bless_keeps_the_asserted_fields_with_current_values() {
    let actual = json!({"hunks": [{"kind": "logic", "verdict": "mixed"}, {"kind": "move", "verdict": "structure"}]});
    let expect = json!({"hunks": [{"kind": "rename"}], "summary": {"verdict": {"$not": "none"}}});
    assert_eq!(
        bless(&expect, &actual),
        json!({"hunks": [{"kind": "logic"}, {"kind": "move"}]})
    );
}

#[test]
fn case_json_is_checked_before_it_runs() {
    assert!(validate(&json!({"description": "d", "expect": {}})).is_ok());
    let invalid = [
        (json!({"expect": {}}), "no description"),
        (
            json!({"description": "d", "expcet": {}}),
            "unknown key \"expcet\"",
        ),
        (
            json!({"description": "d", "open": "yes"}),
            "`open` is true or false",
        ),
        (
            json!({"description": "d", "label": {"labeller": "bot"}}),
            "label.labeller is one of [\"human\", \"llm\", \"tool\"]",
        ),
    ];
    for (spec, why) in invalid {
        assert_eq!(validate(&spec), Err(why.to_string()));
    }
}
