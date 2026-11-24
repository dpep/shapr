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
        vec!["-j", "--merge-base", "HEAD~1"],
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

/// Clean code beside the messy file: enough single quotes, labels and clean
/// lines for each norm to be inferred.
const CLEAN: &str = "class Shelf\n  def a\n    { a: 'a', b: 'b' }\n  end\n\n  def b\n    { c: 'c', d: 'd' }\n  end\n\n  def c\n    ['e', 'f', 'g']\n  end\n\n  def d\n    { x: 1, y: 2, z: 3 }\n  end\nend\n";
const MESSY: &str = "class Cart\n  def total(items)\n    sum = 0  \n    items.each { |i| sum += i }\n    sum\n  end\n\n  def kind\n    { :k => \"kind\" }\n  end\n\n  def label\n    'cart'\n  end\nend\n";

fn norms_json(dir: &std::path::Path, args: &[&str]) -> Value {
    let mut all = vec!["norms", "-j"];
    all.extend(args);
    let out = shapr(dir, &all);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    report(&out)
}

fn norm<'a>(report: &'a Value, id: &str) -> &'a Value {
    (report["norms"].as_array().unwrap().iter())
        .find(|n| n["norm"] == id)
        .unwrap_or_else(|| panic!("no {id}"))
}

/// A directory recurses over the Ruby files shapr judges; generated,
/// vendored and docs files are counted, not read. No path is `.`.
#[test]
fn a_directory_recurses_over_the_files_it_judges() {
    let dir = repo(&[&[
        ("lib/a.rb", CLEAN),
        ("lib/deep/b.rb", MESSY),
        ("lib/notes.txt", "hi\n"),
        ("vendor/gems/x.rb", "X = \"x\"\n"),
        ("db/schema.rb", "S = \"s\"\n"),
        ("docs/example.rb", "E = \"e\"\n"),
        ("spec/fixtures/f.rb", "F = \"f\"\n"),
    ]]);
    let files = |report: &Value| -> Vec<String> {
        (report["files"].as_array().unwrap().iter())
            .map(|f| f["file"].as_str().unwrap().to_string())
            .collect()
    };
    let lib = norms_json(dir.path(), &["lib"]);
    assert_eq!(files(&lib), ["lib/a.rb", "lib/deep/b.rb"]);
    let all = norms_json(dir.path(), &[]);
    assert_eq!(files(&all), ["lib/a.rb", "lib/deep/b.rb"]);
    assert_eq!(
        all["summary"]["skipped"],
        serde_json::json!({"generated": 1, "vendored": 1, "docs": 1, "fixture": 1, "syntax_error": 0, "unsupported": 0})
    );
    assert_eq!(norms_json(dir.path(), &["."])["norms"], all["norms"]);
}

/// Each norm pools every file named: its style, adherence with the counts
/// behind it, and the deviations by file and line.
#[test]
fn a_norm_reports_adherence_over_every_file() {
    let dir = repo(&[&[("lib/a.rb", CLEAN), ("lib/b.rb", MESSY)]]);
    let report = norms_json(dir.path(), &["lib"]);
    let quotes = norm(&report, "Style/StringLiterals");
    assert_eq!(quotes["inferred"], "single_quotes");
    assert_eq!(quotes["scope"], "paths");
    assert_eq!(quotes["files"], 2);
    assert_eq!(
        (quotes["matching"].as_u64(), quotes["judged"].as_u64()),
        (Some(8), Some(9))
    );
    assert_eq!(quotes["adherence"], 0.89);
    assert_eq!(quotes["config"], Value::Null);
    assert_eq!(
        quotes["deviations"],
        serde_json::json!([{"file": "lib/b.rb", "line": 9, "column": 13, "style": "double_quotes", "expected": "single_quotes"}])
    );
    let text = String::from_utf8(shapr(dir.path(), &["norms", "lib"]).stdout).unwrap();
    assert!(text.contains("Style/StringLiterals"), "{text}");
    assert!(text.contains("single_quotes 89% (8/9)"), "{text}");
}

/// Hotspots rank where the code is least like itself: the method breaking
/// the most norms leads, with its deviations, lines and norms.
#[test]
fn hotspots_rank_the_least_consistent_code_first() {
    let dir = repo(&[&[("lib/a.rb", CLEAN), ("lib/b.rb", MESSY)]]);
    let report = norms_json(dir.path(), &["lib"]);
    let hotspots = report["hotspots"].as_array().unwrap();
    let first = |kind: &str| hotspots.iter().find(|h| h["kind"] == kind).unwrap();
    let method = first("method");
    assert_eq!(method["name"], "Cart#kind");
    assert_eq!(method["file"], "lib/b.rb");
    assert_eq!(method["rank"], 1);
    assert_eq!(method["lines"], serde_json::json!({"start": 8, "count": 3}));
    assert_eq!(method["deviations"], 2);
    assert_eq!(
        method["norms"],
        serde_json::json!(["Style/HashSyntax", "Style/StringLiterals"])
    );
    assert_eq!(method["per_100_lines"], 66.7);
    let file = first("file");
    assert_eq!(
        (file["file"].as_str(), file["name"].as_str()),
        (Some("lib/b.rb"), None)
    );
    assert_eq!(first("class")["name"], "Cart");
    // Clean code isn't a hotspot.
    assert!(
        hotspots.iter().all(|h| h["file"] == "lib/b.rb"),
        "{hotspots:?}"
    );
}

/// The plan's steps, applied tier by tier and committed, are what shapr's
/// own classifier calls them: layout is `format_only`, lexical `restyle`.
/// Applied in full, they leave nothing to plan.
#[test]
fn each_tier_of_the_plan_classifies_as_its_kind() {
    let dir = repo(&[&[("lib/a.rb", CLEAN), ("lib/b.rb", MESSY)]]);
    let plan = norms_json(dir.path(), &["lib"]);
    let steps = plan["steps"].as_array().unwrap();
    let summary: Vec<(u64, &str, &str)> = (steps.iter())
        .map(|s| {
            (
                s["tier"].as_u64().unwrap(),
                s["norm"].as_str().unwrap(),
                s["expect_kind"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (1, "Layout/TrailingWhitespace", "format_only"),
            (2, "Style/HashSyntax", "restyle"),
            (2, "Style/StringLiterals", "restyle"),
        ]
    );
    for (n, step) in steps.iter().enumerate() {
        assert_eq!(step["step"], n + 1);
    }
    let patches = tempfile::tempdir().unwrap();
    for (tier, kind) in [(1, "format_only"), (2, "restyle")] {
        for (n, step) in steps.iter().enumerate().filter(|(_, s)| s["tier"] == tier) {
            for (e, edit) in step["edits"].as_array().unwrap().iter().enumerate() {
                let path = patches.path().join(format!("{n}-{e}.diff"));
                std::fs::write(&path, edit["diff"].as_str().unwrap()).unwrap();
                git(dir.path(), &["apply", path.to_str().unwrap()]);
            }
        }
        commit(dir.path(), &[], &format!("tier {tier}"));
        let shape = report(&shapr(dir.path(), &["-j", "HEAD~1..HEAD"]));
        assert_eq!(
            shape["summary"]["verdict"], "structure",
            "tier {tier}: {shape}"
        );
        let kinds: Vec<&str> = (shape["hunks"].as_array().unwrap().iter())
            .map(|h| h["kind"].as_str().unwrap())
            .collect();
        assert!(
            !kinds.is_empty() && kinds.iter().all(|k| *k == kind),
            "tier {tier}: {kinds:?}"
        );
    }
    let after = norms_json(dir.path(), &["lib"]);
    assert_eq!(after["steps"], serde_json::json!([]), "{after}");
    assert_eq!(norm(&after, "Style/StringLiterals")["adherence"], 1.0);
}

/// A step says what it gains: deviations fixed, lines touched, adherence
/// before and after, and the files with their lines and diffs.
#[test]
fn a_step_names_its_norm_files_lines_and_gain() {
    let dir = repo(&[&[("lib/a.rb", CLEAN), ("lib/b.rb", MESSY)]]);
    let plan = norms_json(dir.path(), &["lib"]);
    let step = &plan["steps"][0];
    assert_eq!(step["norm"], "Layout/TrailingWhitespace");
    assert_eq!(step["style"], "no_trailing_whitespace");
    assert_eq!(step["risk"], "layout");
    assert_eq!(
        (step["fixes"].as_u64(), step["unfixed"].as_u64()),
        (Some(1), Some(0))
    );
    assert_eq!(step["lines_touched"], 1);
    assert_eq!(step["adherence_after"], 1.0);
    assert_eq!(step["edits"][0]["file"], "lib/b.rb");
    assert_eq!(step["edits"][0]["lines"], serde_json::json!([3]));
    assert!(
        step["edits"][0]["diff"]
            .as_str()
            .unwrap()
            .contains("-    sum = 0  \n+    sum = 0\n")
    );
    assert_eq!(norm(&plan, "Layout/TrailingWhitespace")["step"], 1);
}

/// Where config sets a style the code doesn't follow, the norm reports how
/// far the code is from config, and the plan moves it neither way: whether
/// to change the code or the config is a person's call. A cop the project
/// parks in .rubocop_todo.yml is reported as such, its files withheld.
#[test]
fn config_trajectory_reports_distance_and_disagreement() {
    let dir = repo(&[&[
        ("lib/a.rb", CLEAN),
        ("lib/b.rb", MESSY),
        (
            ".rubocop.yml",
            "inherit_from: .rubocop_todo.yml\n\nStyle/StringLiterals:\n  EnforcedStyle: double_quotes\nLayout/TrailingWhitespace:\n  Enabled: true\n",
        ),
        (
            ".rubocop_todo.yml",
            "Layout/TrailingWhitespace:\n  Exclude:\n    - 'lib/b.rb'\n",
        ),
    ]]);
    let report = norms_json(dir.path(), &["lib"]);
    let quotes = norm(&report, "Style/StringLiterals");
    assert_eq!(quotes["configured"], "double_quotes");
    let config = &quotes["config"];
    assert_eq!(config["agreement"], "disagrees");
    assert_eq!(
        (config["matching"].as_u64(), config["checked"].as_u64()),
        (Some(1), Some(9))
    );
    assert_eq!(config["adherence"], 0.11);
    assert_eq!(config["todo"], false);
    assert_eq!(quotes["step"], Value::Null);
    assert_eq!(quotes["unplanned"], "config_disagrees");
    let whitespace = norm(&report, "Layout/TrailingWhitespace");
    assert_eq!(whitespace["config"]["todo"], true);
    assert_eq!(whitespace["config"]["agreement"], "agrees");
    assert!(whitespace["config"]["withheld"].as_u64().unwrap() > 0);
    // Config withholds the edit in the file it excludes.
    assert_eq!(whitespace["unplanned"], "withheld");
    let steps = report["steps"].as_array().unwrap();
    assert!(
        steps.iter().all(
            |s| s["norm"] != "Style/StringLiterals" && s["norm"] != "Layout/TrailingWhitespace"
        ),
        "{steps:?}"
    );
}

/// A change's summary says whether it moves the code it touches toward its
/// config or away, by the offenses against the configured style.
#[test]
fn a_change_moves_toward_or_away_from_config() {
    let config = ".rubocop.yml";
    let rule = "Style/StringLiterals:\n  EnforcedStyle: double_quotes\n";
    let before = "A = \"a\"\nB = \"b\"\nC = 'c'\n";
    let away = "A = \"a\"\nB = 'b'\nC = 'c'\nD = 'd'\n";
    let dir = repo(&[&[(config, rule), ("a.rb", before)], &[("a.rb", away)]]);
    let summary = &report(&shapr(dir.path(), &["-j", "HEAD~1..HEAD"]))["summary"];
    assert_eq!(
        summary["config_trajectory"],
        serde_json::json!([{
            "norm": "Style/StringLiterals",
            "configured": "double_quotes",
            "before": {"checked": 3, "offenses": 1},
            "after": {"checked": 4, "offenses": 3},
            "fixed": 0,
            "added": 2,
            "direction": "away"
        }])
    );
    let text = String::from_utf8(shapr(dir.path(), &["HEAD~1..HEAD"]).stdout).unwrap();
    assert!(
        text.contains(
            "config Style/StringLiterals double_quotes: offenses 1 -> 3 (away: 0 fixed, 2 added)"
        ),
        "{text}"
    );
    // Opposite moves in two files net to level, and both still show.
    let dir = repo(&[
        &[(config, rule), ("a.rb", before), ("b.rb", before)],
        &[
            ("a.rb", "A = \"a\"\nB = \"b\"\nC = \"c\"\n"),
            ("b.rb", "A = 'a'\nB = \"b\"\nC = 'c'\n"),
        ],
    ]);
    let summary = &report(&shapr(dir.path(), &["-j", "HEAD~1..HEAD"]))["summary"];
    let t = &summary["config_trajectory"][0];
    assert_eq!(
        (&t["fixed"], &t["added"], &t["direction"]),
        (
            &serde_json::json!(1),
            &serde_json::json!(1),
            &serde_json::json!("level")
        )
    );
    let text = String::from_utf8(shapr(dir.path(), &["HEAD~1..HEAD"]).stdout).unwrap();
    assert!(
        text.contains("offenses 2 -> 2 (level: 1 fixed, 1 added)"),
        "{text}"
    );
}

/// Config shapr can't read (a gem's, a missing or remote file, lines that
/// aren't YAML it reads) is named on stderr and in the summary, never
/// silently dropped.
#[test]
fn config_it_cannot_read_is_named() {
    let config = "inherit_gem:\n  rubocop-shopify: rubocop.yml\n\
                  inherit_from:\n  - missing.yml\n  - https://example.com/rubocop.yml\n\
                  Style/StringLiterals: [oops\n";
    let dir = repo(&[
        &[(".rubocop.yml", config), ("a.rb", "def a\n  'x'\nend\n")],
        &[("a.rb", "def a\n  'y'\nend\n")],
    ]);
    let notes = [
        ".rubocop.yml: line 6 not read",
        "inherit_gem: rubocop-shopify",
        "inherit_from: missing.yml (not found)",
        "inherit_from: https://example.com/rubocop.yml (remote)",
    ];
    let out = shapr(dir.path(), &["norms", "-j", "a.rb"]);
    assert_eq!(
        report(&out)["summary"]["config_unread"],
        serde_json::json!(notes)
    );
    for args in [vec!["norms", "a.rb"], vec!["HEAD~1..HEAD"]] {
        let stderr = String::from_utf8(shapr(dir.path(), &args).stderr).unwrap();
        for note in notes {
            let line = format!("shapr: config not read, norms judge without it: {note}\n");
            assert_eq!(stderr.matches(&line).count(), 1, "{args:?}: {stderr}");
        }
    }
}

/// Deviations config doesn't check (a `.rubocop_todo.yml` exclude) count as
/// withheld, edit or not, read as withheld in the plan's text, and rank no
/// hotspot.
#[test]
fn deviations_config_excludes_are_withheld_and_rank_no_hotspot() {
    let snake = "class C\n  def a_b\n  end\n\n  def c_d\n  end\n\n  def e_f\n  end\n\n  def g_h\n  end\nend\n";
    let dir = repo(&[&[
        (".rubocop.yml", "inherit_from: .rubocop_todo.yml\n"),
        (
            ".rubocop_todo.yml",
            "Naming/MethodName:\n  Exclude:\n    - 'lib/b.rb'\n",
        ),
        ("lib/a.rb", snake),
        ("lib/c.rb", snake),
        ("lib/e.rb", snake),
        ("lib/b.rb", "class B\n  def doThing\n  end\nend\n"),
        (
            "lib/d.rb",
            "class D\n  def run_it\n    goNow\n  end\n\n  private\n\n  def goNow\n  end\nend\n",
        ),
    ]]);
    let report = norms_json(dir.path(), &[]);
    let step = (report["steps"].as_array().unwrap().iter())
        .find(|s| s["norm"] == "Naming/MethodName")
        .expect("a step");
    assert_eq!(
        (step["fixes"].as_u64(), step["withheld"].as_u64()),
        (Some(1), Some(1))
    );
    let hot: Vec<&str> = (report["hotspots"].as_array().unwrap().iter())
        .map(|h| h["file"].as_str().unwrap())
        .collect();
    assert!(!hot.contains(&"lib/b.rb"), "{hot:?}");
    let text = String::from_utf8(shapr(dir.path(), &["norms"]).stdout).unwrap();
    assert!(
        text.contains(", 1 withheld (config doesn't check them)"),
        "{text}"
    );
    assert!(!text.contains("left without an edit"), "{text}");
    assert!(
        text.contains(
            "lib/d.rb  1 norm, 1 deviation in 10 lines (10.0 per 100): Naming/MethodName"
        ),
        "{text}"
    );
}

/// `shapr` in `dir`, which must exit within `secs`: config built to blow up
/// is refused, not run to exhaustion.
fn shapr_within(dir: &std::path::Path, args: &[&str], secs: u64) -> std::process::Output {
    let out = tempfile::tempdir().unwrap();
    let (stdout, stderr) = (out.path().join("out"), out.path().join("err"));
    let mut child = command(env!("CARGO_BIN_EXE_shapr"), dir)
        .args(args)
        .stdout(std::fs::File::create(&stdout).unwrap())
        .stderr(std::fs::File::create(&stderr).unwrap())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > std::time::Duration::from_secs(secs) {
            child.kill().unwrap();
            panic!("shapr {args:?} ran past {secs}s");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    std::process::Output {
        status,
        stdout: std::fs::read(stdout).unwrap(),
        stderr: std::fs::read(stderr).unwrap(),
    }
}

/// An `inherit_from` that lists itself, many times over, reads each file
/// once and names the cycle.
#[test]
fn an_inherit_from_cycle_is_read_once_and_named() {
    let config = format!(
        "inherit_from:\n{}Style/StringLiterals:\n  EnforcedStyle: double_quotes\n",
        "  - .rubocop.yml\n".repeat(12)
    );
    let dir = repo(&[&[(".rubocop.yml", &config), ("a.rb", "def a\n  'x'\nend\n")]]);
    let out = shapr_within(dir.path(), &["norms", "-j", "a.rb"], 30);
    let report = report(&out);
    assert_eq!(
        report["summary"]["config_unread"],
        serde_json::json!(["inherit_from: .rubocop.yml (cycle)"])
    );
    assert_eq!(
        norm(&report, "Style/StringLiterals")["configured"],
        "double_quotes"
    );
}
