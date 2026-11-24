//! GitHub input end to end, offline: the API is a canned HTTP server on
//! 127.0.0.1 and the "GitHub" git remote is a local bare repository served
//! over file://, both reached through shapr's URL overrides.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

mod common;
use common::*;

/// Canned API responses by path; anything else is 404. Records each
/// request's head, lowercased.
struct Server {
    url: String,
    routes: Arc<Mutex<HashMap<String, (u16, String)>>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let routes: Arc<Mutex<HashMap<String, (u16, String)>>> = Arc::default();
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let (r, q) = (routes.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                q.lock().unwrap().push(head.to_lowercase());
                let (status, body) = r
                    .lock()
                    .unwrap()
                    .get(&path)
                    .cloned()
                    .unwrap_or((404, r#"{"message":"Not Found"}"#.to_string()));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Server {
            url,
            routes,
            requests,
        }
    }

    fn route(&self, path: &str, status: u16, body: Value) {
        self.routes
            .lock()
            .unwrap()
            .insert(path.to_string(), (status, body.to_string()));
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

const BEFORE: &str = "class Greeter\n  def greet(name)\n    \"hi #{name}\"\n  end\nend\n";
const REFORMATTED: &str = "class Greeter\n  def greet( name )\n      \"hi #{name}\"\n  end\nend\n";
const LOGIC: &str = "def count\n  1\nend\n";

/// A working repository and its "GitHub" copy, acme/widgets: main has a
/// base commit and moves on after `feature` branches off it, so a two-dot
/// diff of the tips would differ from the pull request's.
struct Fixture {
    tmp: tempfile::TempDir,
    work: PathBuf,
    remote: PathBuf,
    server: Server,
    fork_point: String,
    main: String,
    feature: String,
}

impl Fixture {
    fn new() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        std::fs::create_dir(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["config", "user.email", "test@example.com"]);
        git(&work, &["config", "user.name", "Test"]);
        let fork_point = commit(&work, &[("lib/greeter.rb", BEFORE)], "base");
        git(&work, &["checkout", "-q", "-b", "feature"]);
        commit(&work, &[("lib/greeter.rb", REFORMATTED)], "Reformat greet");
        let feature = commit(&work, &[("lib/count.rb", LOGIC)], "Add count");
        git(&work, &["checkout", "-q", "main"]);
        let main = commit(&work, &[("lib/other.rb", LOGIC)], "Main moves on");

        let remote = tmp.path().join("remotes/acme/widgets.git");
        let remote_str = remote.to_str().unwrap();
        git(
            tmp.path(),
            &["clone", "-q", "--bare", work.to_str().unwrap(), remote_str],
        );
        // What github.com's server allows: partial clones.
        git(&remote, &["config", "uploadpack.allowFilter", "true"]);
        git(&remote, &["update-ref", "refs/pull/12/head", &feature]);

        let server = Server::start();
        let fixture = Fixture {
            tmp,
            work,
            remote,
            server,
            fork_point,
            main,
            feature,
        };
        fixture.pull(12, &fixture.feature.clone(), "Tidy the greeter");
        fixture
    }

    /// Serves pull request NUMBER from main's tip to HEAD.
    fn pull(&self, number: u64, head: &str, title: &str) {
        let base = &self.main;
        let merge_base = git(&self.work, &["merge-base", base, head])
            .trim()
            .to_string();
        self.server.route(
            &format!("/repos/acme/widgets/pulls/{number}"),
            200,
            json!({"title": title, "body": "Why it changed.", "state": "open",
                   "base": {"sha": base}, "head": {"sha": head}}),
        );
        self.server.route(
            &format!("/repos/acme/widgets/compare/{base}...{head}"),
            200,
            json!({"merge_base_commit": {"sha": merge_base},
                   "commits": [{"sha": head, "commit": {"message": "Add count"}, "parents": []}]}),
        );
    }

    /// Serves the commit SHA under the name ASKED.
    fn commit(&self, asked: &str, sha: &str) {
        let parents = git(&self.work, &["rev-list", "--parents", "-n1", sha])
            .trim()
            .to_string();
        let parents: Vec<Value> = parents
            .split_whitespace()
            .skip(1)
            .map(|p| json!({"sha": p}))
            .collect();
        let message = git(&self.work, &["log", "-1", "--format=%B", sha])
            .trim()
            .to_string();
        self.server.route(
            &format!("/repos/acme/widgets/commits/{asked}"),
            200,
            json!({"sha": sha, "commit": {"message": message}, "parents": parents}),
        );
    }

    fn cache(&self) -> PathBuf {
        self.tmp.path().join("cache")
    }

    /// shapr in DIR, its GitHub pointed at the fixture, no token, and no
    /// `gh` on PATH (git only), so a developer's own login never leaks in.
    fn shapr_in(&self, dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let bin = self.tmp.path().join("bin");
        if !bin.exists() {
            std::fs::create_dir(&bin).unwrap();
            let found = Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap();
            let git_path = String::from_utf8(found.stdout).unwrap();
            std::os::unix::fs::symlink(git_path.trim(), bin.join("git")).unwrap();
        }
        let remotes = format!("file://{}", self.tmp.path().join("remotes").display());
        let mut cmd = command(env!("CARGO_BIN_EXE_shapr"), dir);
        cmd.args(args)
            .env("PATH", &bin)
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("XDG_CACHE_HOME")
            .env("SHAPR_GITHUB_API_URL", &self.server.url)
            .env("SHAPR_GITHUB_GIT_URL", remotes)
            .env("SHAPR_CACHE_DIR", self.cache());
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn shapr(&self, args: &[&str]) -> Output {
        self.shapr_in(&self.work, args, &[])
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Each hunk's file and kind: what must match a local run.
fn labels(report: &Value) -> Vec<(String, String)> {
    report["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["file"].as_str().unwrap().to_string(),
                h["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn a_pull_request_matches_the_local_three_dot_range() {
    let f = Fixture::new();
    let remote = f.shapr(&["-j", "https://github.com/acme/widgets/pull/12/files"]);
    let local = f.shapr(&["-j", "main...feature"]);
    let (remote_report, local_report) = (report(&remote), report(&local));
    assert_eq!(labels(&remote_report), labels(&local_report));
    assert_eq!(
        labels(&remote_report),
        [
            ("lib/count.rb".to_string(), "logic".to_string()),
            ("lib/greeter.rb".to_string(), "format_only".to_string())
        ]
    );
    assert_eq!(remote.status.code(), local.status.code());
    assert_eq!(local_report["meta"]["source"], Value::Null);
    let source = &remote_report["meta"]["source"];
    assert_eq!(
        *source,
        json!({"kind": "pr", "url": "https://github.com/acme/widgets/pull/12",
               "repo": "acme/widgets", "number": 12, "title": "Tidy the greeter",
               "base": f.fork_point, "head": f.feature, "merge": false})
    );
    // The user's repository gained nothing.
    assert!(git(&f.work, &["for-each-ref"]).lines().count() == 2);
}

fn hints(report: &Value) -> Vec<(String, String, String)> {
    report["summary"]["claims"]["hints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            let s = |k: &str| h[k].as_str().unwrap_or("").to_string();
            (s("source"), s("value"), s("commit"))
        })
        .collect()
}

#[test]
fn claims_read_the_pull_requests_commits_and_description() {
    let f = Fixture::new();
    let r = report(&f.shapr(&["-j", "acme/widgets#12"]));
    assert_eq!(r["summary"]["claims"]["messages"], 2);
    let commit = f.feature[..12].to_string();
    assert_eq!(
        hints(&r),
        [
            ("commit".to_string(), "add".to_string(), commit),
            (
                "pull_request".to_string(),
                "tidy".to_string(),
                String::new()
            ),
        ]
    );

    // --message replaces the pull request's own description.
    let file = f.tmp.path().join("pr.md");
    std::fs::write(&file, "Fix the greeting\n").unwrap();
    let r = report(&f.shapr(&["-j", "--message", file.to_str().unwrap(), "acme/widgets#12"]));
    let sources: Vec<String> = hints(&r).into_iter().map(|h| h.0).collect();
    assert_eq!(sources, ["commit", "message"]);
    assert_eq!(r["summary"]["claims"]["messages"], 2);
}

#[test]
fn hash_references_are_looked_up_once_and_failures_stay_references() {
    let f = Fixture::new();
    let refs: Vec<String> = (1..=12).map(|n| format!("#{n}")).collect();
    let body = format!(
        "In the spirit of #1, see #2, #3 and #1 again. {}",
        refs.join(" ")
    );
    let pull = "/repos/acme/widgets/pulls/12".to_string();
    let mut canned: Value =
        serde_json::from_str(&f.server.routes.lock().unwrap()[&pull].1).unwrap();
    canned["body"] = json!(body);
    f.server.route(&pull, 200, canned);
    f.server.route(
        "/repos/acme/widgets/issues/1",
        200,
        json!({"number": 1, "pull_request": {"url": "x"}}),
    );
    f.server
        .route("/repos/acme/widgets/issues/2", 200, json!({"number": 2}));
    f.server.route(
        "/repos/acme/widgets/issues/3",
        500,
        json!({"message": "boom"}),
    );
    let out = f.shapr(&["-j", "acme/widgets#12"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let r = report(&out);
    let kinds: Vec<(String, String)> = r["summary"]["claims"]["hints"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["value"].as_str().unwrap().starts_with('#'))
        .map(|h| {
            (
                h["value"].as_str().unwrap().into(),
                h["hint"].as_str().unwrap().into(),
            )
        })
        .take(4)
        .collect();
    let kind = |v: &str, k: &str| (v.to_string(), k.to_string());
    assert_eq!(
        kinds,
        [
            kind("#1", "pull_request"),
            kind("#2", "issue"),
            kind("#3", "reference"),
            kind("#4", "reference"),
        ]
    );
    let asked: Vec<String> = f
        .server
        .requests()
        .iter()
        .filter_map(|r| r.split_whitespace().nth(1).map(str::to_string))
        .filter(|p| p.contains("/issues/"))
        .collect();
    // Each number once, the first ten only.
    let expected: Vec<String> = (1..=10)
        .map(|n| format!("/repos/acme/widgets/issues/{n}"))
        .collect();
    assert_eq!(asked, expected);
}

#[test]
fn claims_read_a_commits_message() {
    let f = Fixture::new();
    f.commit(&f.feature, &f.feature);
    let r = report(&f.shapr(&["-j", &format!("acme/widgets@{}", f.feature)]));
    assert_eq!(r["summary"]["claims"]["messages"], 1);
    assert_eq!(
        hints(&r),
        [(
            "commit".to_string(),
            "add".to_string(),
            f.feature[..12].to_string()
        )]
    );
}

#[test]
fn the_text_summary_names_the_pull_request() {
    let f = Fixture::new();
    let out = f.shapr(&["acme/widgets#12"]);
    let text = String::from_utf8(out.stdout).unwrap();
    let summary = text.lines().next().unwrap();
    assert!(
        summary.starts_with("acme/widgets#12: mixed: 2 hunks"),
        "{summary}"
    );
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn a_fork_head_is_fetched_from_the_pull_ref() {
    let f = Fixture::new();
    // The head lives only under refs/pull/13/head, as a fork's does.
    let forked = git(
        &f.work,
        &[
            "commit-tree",
            "-p",
            &f.fork_point,
            "-m",
            "fork",
            &format!("{}^{{tree}}", f.feature),
        ],
    )
    .trim()
    .to_string();
    git(
        &f.work,
        &[
            "push",
            "-q",
            f.remote.to_str().unwrap(),
            &format!("{forked}:refs/pull/13/head"),
        ],
    );
    assert!(!git(&f.work, &["branch", "--contains", &forked]).contains("feature"));
    f.pull(13, &forked, "From a fork");
    let out = f.shapr(&["-j", "acme/widgets#13"]);
    let r = report(&out);
    assert_eq!(r["meta"]["source"]["head"], forked);
    assert_eq!(labels(&r).len(), 2, "{}", stderr(&out));
}

#[test]
fn a_commit_is_diffed_against_its_parent() {
    let f = Fixture::new();
    let reformat = git(&f.work, &["rev-parse", "feature~1"]).trim().to_string();
    let short = &reformat[..8];
    f.commit(short, &reformat);
    let out = f.shapr(&["-j", &format!("acme/widgets@{short}")]);
    let r = report(&out);
    assert_eq!(
        labels(&r),
        [("lib/greeter.rb".to_string(), "format_only".to_string())]
    );
    let source = &r["meta"]["source"];
    assert_eq!(source["kind"], "commit");
    assert_eq!(source["title"], "Reformat greet");
    assert_eq!(source["base"], f.fork_point);
    assert_eq!(source["number"], Value::Null);
    assert_eq!(
        source["url"],
        format!("https://github.com/acme/widgets/commit/{reformat}")
    );
    assert_eq!(out.status.code(), Some(0));

    // Inside a pull request, it keeps the number.
    f.commit(&f.feature, &f.feature);
    let url = format!(
        "https://github.com/acme/widgets/pull/12/commits/{}",
        f.feature
    );
    let r = report(&f.shapr(&["-j", &url]));
    assert_eq!(
        labels(&r),
        [("lib/count.rb".to_string(), "logic".to_string())]
    );
    assert_eq!(r["meta"]["source"]["number"], 12);
    assert_eq!(r["meta"]["source"]["url"], url);
}

#[test]
fn a_merge_commit_is_diffed_against_its_first_parent_and_says_so() {
    let f = Fixture::new();
    git(
        &f.work,
        &["merge", "-q", "--no-ff", "-m", "Merge feature", "feature"],
    );
    let merge = git(&f.work, &["rev-parse", "HEAD"]).trim().to_string();
    git(&f.work, &["push", "-q", f.remote.to_str().unwrap(), "main"]);
    f.commit(&merge, &merge);
    let out = f.shapr(&["-j", &format!("acme/widgets@{merge}")]);
    let r = report(&out);
    let local = report(&f.shapr(&["-j", &format!("{merge}^1..{merge}")]));
    assert_eq!(labels(&r), labels(&local));
    assert_eq!(r["meta"]["source"]["merge"], true);
    assert_eq!(r["meta"]["source"]["base"], f.main);

    let text = f.shapr(&[&format!("https://github.com/acme/widgets/commit/{merge}")]);
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(
        text.contains(&format!(
            "acme/widgets@{} (a merge, against its first parent): ",
            &merge[..7]
        )),
        "{text}"
    );
}

#[test]
fn hash_number_uses_origin_or_the_only_github_remote() {
    let f = Fixture::new();
    git(
        &f.work,
        &[
            "remote",
            "add",
            "fork",
            "git@github.com:someone/widgets.git",
        ],
    );
    git(
        &f.work,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
    );
    let r = report(&f.shapr(&["-j", "#12"]));
    assert_eq!(r["meta"]["source"]["repo"], "acme/widgets");

    // Without origin, two GitHub repositories is a question, not a guess.
    git(&f.work, &["remote", "remove", "origin"]);
    git(
        &f.work,
        &[
            "remote",
            "add",
            "upstream",
            "https://github.com/acme/widgets.git",
        ],
    );
    let out = f.shapr(&["#12"]);
    assert_eq!(out.status.code(), Some(64));
    assert!(
        stderr(&out).contains("`#12` is ambiguous"),
        "{}",
        stderr(&out)
    );
    assert!(f.server.requests().iter().all(|r| !r.contains("someone")));

    git(&f.work, &["remote", "remove", "fork"]);
    let r = report(&f.shapr(&["-j", "#12"]));
    assert_eq!(r["meta"]["source"]["repo"], "acme/widgets");
}

#[test]
fn a_local_revision_that_looks_like_github_stays_local() {
    let f = Fixture::new();
    git(&f.work, &["branch", "acme/widgets#12", "feature"]);
    let out = f.shapr(&["-j", "main..acme/widgets#12"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = f.shapr(&["-j", "acme/widgets#12"]);
    let r = report(&out);
    assert_eq!(r["meta"]["source"], Value::Null);
    assert!(f.server.requests().is_empty());
}

#[test]
fn an_unknown_pull_request_unauthenticated_says_how_to_authenticate() {
    let f = Fixture::new();
    let out = f.shapr(&["acme/widgets#99"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        stderr(&out).trim(),
        "shapr: pull request acme/widgets#99 not found on GitHub (unauthenticated: set GITHUB_TOKEN or run `gh auth login`)"
    );
}

#[test]
fn the_token_is_sent_but_never_printed() {
    let f = Fixture::new();
    let token = "ghp_FAKEtoken0123456789";
    let ok = f.shapr_in(
        &f.work,
        &["-v", "-j", "acme/widgets#12"],
        &[("GH_TOKEN", token)],
    );
    let missing = f.shapr_in(
        &f.work,
        &["-v", "-J", "acme/widgets#99"],
        &[("GITHUB_TOKEN", token)],
    );
    f.server.route(
        "/repos/acme/widgets/pulls/98",
        401,
        json!({"message": "Bad credentials"}),
    );
    let rejected = f.shapr_in(&f.work, &["-v", "acme/widgets#98"], &[("GH_TOKEN", token)]);
    assert!(ok.status.success());
    assert_eq!(missing.status.code(), Some(2));
    assert!(
        stderr(&missing).contains("not found on GitHub\n"),
        "{}",
        stderr(&missing)
    );
    assert!(stderr(&rejected).contains("rejected the token from GH_TOKEN (401)"));
    for out in [&ok, &missing, &rejected] {
        assert!(!String::from_utf8_lossy(&out.stdout).contains("FAKE"));
        assert!(!stderr(out).contains("FAKE"), "{}", stderr(out));
    }
    let sent = format!("authorization: bearer {}", token.to_lowercase());
    assert!(f.server.requests().iter().all(|r| r.contains(&sent)));
}

#[test]
fn an_unreachable_api_is_one_line() {
    let f = Fixture::new();
    let out = f.shapr_in(
        &f.work,
        &["-J", "acme/widgets#12"],
        &[("SHAPR_GITHUB_API_URL", "http://127.0.0.1:1")],
    );
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.starts_with("shapr: can't reach 127.0.0.1:1"), "{err}");
    assert_eq!(err.lines().count(), 1);
    assert_eq!(report(&out)["code"], 2);
}

#[test]
fn a_malformed_github_form_lists_the_accepted_ones() {
    let f = Fixture::new();
    let out = f.shapr(&["https://github.com/acme/widgets/issues/3"]);
    assert_eq!(out.status.code(), Some(64));
    let err = stderr(&out);
    assert!(
        err.contains("OWNER/REPO#N") && err.contains("OWNER/REPO@SHA"),
        "{err}"
    );
    assert!(f.server.requests().is_empty());
}

#[test]
fn a_relative_cache_dir_is_a_usage_error() {
    let f = Fixture::new();
    let out = f.shapr_in(
        &f.work,
        &["acme/widgets#12"],
        &[("SHAPR_CACHE_DIR", "cache")],
    );
    assert_eq!(out.status.code(), Some(64));
    assert!(stderr(&out).contains("SHAPR_CACHE_DIR must be an absolute path"));
}

#[test]
fn gitattributes_come_from_the_head_commit() {
    let f = Fixture::new();
    git(&f.work, &["checkout", "-q", "-b", "generated", "main"]);
    commit(
        &f.work,
        &[
            (".gitattributes", "lib/schema.rb linguist-generated\n"),
            ("lib/schema.rb", "SCHEMA = 1\n"),
        ],
        "Add the schema",
    );
    let head = commit(
        &f.work,
        &[("lib/schema.rb", "SCHEMA = 2\n")],
        "Bump the schema",
    );
    git(
        &f.work,
        &["push", "-q", f.remote.to_str().unwrap(), "generated"],
    );
    f.commit(&head, &head);
    // Outside any repository: only the head commit can say it's generated.
    let out = f.shapr_in(f.tmp.path(), &["-j", &format!("acme/widgets@{head}")], &[]);
    let r = report(&out);
    assert_eq!(r["hunks"][0]["file"], "lib/schema.rb");
    assert_eq!(r["hunks"][0]["category"], "generated", "{}", stderr(&out));
    assert_eq!(r["summary"]["categories"]["generated"], 1);
}

#[test]
fn a_rename_is_verified_in_the_cached_clone() {
    let f = Fixture::new();
    let meter = |name: &str| {
        format!(
            "class Meter\n  def read\n    {name} + 1\n  end\n\n  def {name}\n    41\n  end\nend\n"
        )
    };
    git(&f.work, &["checkout", "-q", "-b", "rename", "main"]);
    commit(
        &f.work,
        &[
            ("lib/meter.rb", &meter("sample")),
            (
                "lib/report.rb",
                "class Report\n  def show\n    Meter.new.sample\n  end\nend\n",
            ),
        ],
        "Add the meter",
    );
    let head = commit(
        &f.work,
        &[("lib/meter.rb", &meter("probe"))],
        "Rename sample",
    );
    git(
        &f.work,
        &["push", "-q", f.remote.to_str().unwrap(), "rename"],
    );
    f.commit(&head, &head);
    // Outside any repository, the cache's partial clone is all there is: the
    // caller's blob was never fetched for the diff.
    let out = f.shapr_in(f.tmp.path(), &["-j", &format!("acme/widgets@{head}")], &[]);
    let r = report(&out);
    let residual = &r["hunks"][0]["parts"][0];
    assert_eq!(residual["kind"], "logic", "{r:#} {}", stderr(&out));
    assert!(
        residual["reason"]
            .as_str()
            .unwrap()
            .contains("lib/report.rb")
    );
    let local = report(&f.shapr(&["-j", &format!("{head}^..{head}")]));
    assert_eq!(labels(&r), labels(&local));
}

#[test]
fn a_second_run_reuses_the_cache_and_clear_cache_empties_it() {
    let f = Fixture::new();
    let first = report(&f.shapr(&["-j", "acme/widgets#12"]));
    // With the remote gone, only the cache can answer.
    std::fs::remove_dir_all(&f.remote).unwrap();
    let out = f.shapr(&["-j", "-v", "acme/widgets#12"]);
    assert_eq!(report(&out), first);
    assert!(stderr(&out).contains("already cached"));

    let status = report(&f.shapr(&["-j", "--status"]));
    assert_eq!(status["record"], "cache");
    assert_eq!(status["repositories"], json!(["github.com/acme/widgets"]));
    assert!(status["bytes"].as_u64().unwrap() > 0);

    std::fs::write(f.cache().join("keep.txt"), "not ours").unwrap();
    let cleared = report(&f.shapr(&["-j", "--clear-cache"]));
    assert_eq!(cleared["repositories"], json!([]));
    assert_eq!(cleared["bytes"], 0);
    assert!(!f.cache().join("github.com").exists());
    assert!(f.cache().join("keep.txt").exists());
}

#[test]
fn source_fields_are_documented() {
    let schema =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/schema.md")).unwrap();
    let f = Fixture::new();
    let r = report(&f.shapr(&["-j", "acme/widgets#12"]));
    let status = report(&f.shapr(&["-j", "--status"]));
    let records = [&r["meta"]["source"], &status];
    for record in records {
        for key in record.as_object().unwrap().keys() {
            assert!(
                schema.contains(&format!("`{key}`")),
                "docs/schema.md doesn't name `{key}`"
            );
        }
    }
}
