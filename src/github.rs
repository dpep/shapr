//! GitHub input: a pull request or commit named by URL or shorthand, its
//! metadata from the REST API, and its objects in a cache of bare partial
//! clones. The user's own repository is only read (remotes, revisions).

use crate::claims::{self, Message};
use crate::git::{self, Git};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The accepted forms, for a usage error.
pub const FORMS: &str = "a pull request (https://github.com/OWNER/REPO/pull/N, OWNER/REPO#N, #N) \
or a commit (https://github.com/OWNER/REPO/commit/SHA, OWNER/REPO@SHA)";

const HOST: &str = "github.com";
const API_URL: &str = "https://api.github.com";
const GIT_URL: &str = "https://github.com";

#[derive(Clone, Debug, PartialEq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl fmt::Display for Repo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

impl Repo {
    /// GitHub's character sets, which also keep the name safe as a path.
    fn parse(owner: &str, name: &str) -> Option<Repo> {
        let owner_ok = !owner.is_empty()
            && owner.len() <= 39
            && owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-');
        let name_ok = !matches!(name, "" | "." | "..")
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        (owner_ok && name_ok).then(|| Repo {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    fn from_path(path: &str) -> Option<Repo> {
        let (owner, name) = path.split_once('/')?;
        Repo::parse(owner, name)
    }
}

#[derive(Debug, PartialEq)]
pub enum Target {
    Pull {
        repo: Repo,
        number: u64,
    },
    /// A commit, alone; `pull` when it was named inside a pull request.
    Commit {
        repo: Repo,
        sha: String,
        pull: Option<u64>,
    },
}

#[derive(Debug, PartialEq)]
enum Parsed {
    NotGitHub,
    Target(Target),
    /// `#N`: a pull request in the current repository's GitHub remote.
    ThisRepo(u64),
    /// Shaped like a GitHub reference, but not one we accept.
    Malformed,
}

/// What a RANGE argument names: `None` for local revisions, a target for a
/// GitHub form. A form that's malformed or can't be resolved is a usage
/// error. A shorthand that also names a local revision is that revision.
pub fn resolve(arg: &str, git: &Git) -> Result<Option<Target>, String> {
    let parsed = parse(arg);
    if parsed == Parsed::NotGitHub {
        return Ok(None);
    }
    if !arg.contains("://") && git.resolves(arg) {
        crate::verbose!("`{arg}` is a local revision, not a GitHub reference");
        return Ok(None);
    }
    match parsed {
        Parsed::NotGitHub => Ok(None),
        Parsed::Target(target) => Ok(Some(target)),
        Parsed::ThisRepo(number) => {
            this_repo(git, number).map(|repo| Some(Target::Pull { repo, number }))
        }
        Parsed::Malformed => Err(format!("`{arg}` is not a revision or {FORMS}")),
    }
}

fn parse(arg: &str) -> Parsed {
    let lower = arg.to_ascii_lowercase();
    let scheme = ["https://", "http://"]
        .iter()
        .find(|s| lower.starts_with(*s))
        .map_or(0, |s| s.len());
    let rest = &arg[scheme..];
    let rest = strip_prefix_ignore_case(rest, "www.").unwrap_or(rest);
    if let Some(path) = strip_prefix_ignore_case(rest, "github.com/") {
        return url_path(path).map_or(Parsed::Malformed, Parsed::Target);
    }
    if scheme > 0 {
        return Parsed::Malformed;
    }
    if let Some(number) = arg.strip_prefix('#') {
        return number_of(number).map_or(Parsed::Malformed, Parsed::ThisRepo);
    }
    if let Some((path, number)) = arg.split_once('#') {
        return pull(path, number).map_or(Parsed::Malformed, Parsed::Target);
    }
    // `origin/main@{u}` is git's reflog syntax, not a commit form.
    if let Some((path, sha)) = arg.split_once('@')
        && path.contains('/')
        && !sha.starts_with('{')
    {
        return commit(path, sha, None).map_or(Parsed::Malformed, Parsed::Target);
    }
    Parsed::NotGitHub
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// The path after `github.com/`.
fn url_path(path: &str) -> Option<Target> {
    let path = path.split('?').next().unwrap_or(path);
    // `github.com/o/r#12` puts the number where a URL keeps its fragment.
    if let Some((repo, number)) = path.split_once('#')
        && !repo.trim_end_matches('/').contains("/pull/")
        && let Some(target) = pull(repo.trim_end_matches('/'), number)
    {
        return Some(target);
    }
    let path = path.split('#').next().unwrap_or(path);
    let segments: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    match segments.as_slice() {
        [owner, name, "pull", number, "commits", sha, ..] => {
            commit(&format!("{owner}/{name}"), sha, Some(number_of(number)?))
        }
        [owner, name, "pull", number, ..] => pull(&format!("{owner}/{name}"), number),
        [owner, name, "commit", sha] => commit(&format!("{owner}/{name}"), sha, None),
        _ => None,
    }
}

fn number_of(digits: &str) -> Option<u64> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|&n| n > 0)
}

fn pull(path: &str, number: &str) -> Option<Target> {
    Some(Target::Pull {
        repo: Repo::from_path(path)?,
        number: number_of(number)?,
    })
}

fn commit(path: &str, sha: &str, pull: Option<u64>) -> Option<Target> {
    let hex = (7..=40).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex {
        return None;
    }
    Some(Target::Commit {
        repo: Repo::from_path(path)?,
        sha: sha.to_ascii_lowercase(),
        pull,
    })
}

/// The repository `#N` means: `origin` when it is on GitHub, else the only
/// GitHub remote.
fn this_repo(git: &Git, number: u64) -> Result<Repo, String> {
    let mut found: Vec<(String, Repo)> = git
        .remote_urls()
        .into_iter()
        .filter_map(|(name, url)| Some((name, github_remote(&url)?)))
        .collect();
    if let Some((_, repo)) = found.iter().find(|(name, _)| name == "origin") {
        return Ok(repo.clone());
    }
    found.dedup_by(|a, b| a.1 == b.1);
    match found.as_slice() {
        [(_, repo)] => Ok(repo.clone()),
        [] => Err(format!(
            "`#{number}` needs a GitHub remote in this repository: name it, OWNER/REPO#{number}"
        )),
        many => {
            let names: Vec<String> = many.iter().map(|(n, r)| format!("{n} ({r})")).collect();
            Err(format!(
                "`#{number}` is ambiguous: remotes {} are on GitHub and none is origin; name it, OWNER/REPO#{number}",
                names.join(", ")
            ))
        }
    }
}

/// The repository a remote URL names, when it is on github.com:
/// `git@github.com:o/r.git`, `https://github.com/o/r`, `ssh://git@github.com/o/r.git`.
fn github_remote(url: &str) -> Option<Repo> {
    let (authority, path) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        None => url.split_once(':')?,
    };
    let host = authority.rsplit('@').next()?.split(':').next()?;
    if !host.eq_ignore_ascii_case(HOST) {
        return None;
    }
    let path = path.trim_matches('/');
    Repo::from_path(path.strip_suffix(".git").unwrap_or(path))
}

/// What a remote run analyzed, as `meta.source` reports it.
#[derive(Clone, Debug, Serialize)]
pub struct Source {
    pub kind: &'static str,
    pub url: String,
    pub repo: String,
    pub number: Option<u64>,
    pub title: String,
    pub base: CommitId,
    pub head: CommitId,
    pub merge: bool,
}

impl Source {
    /// `o/r#12` or `o/r@abc1234`, for the text summary.
    pub fn name(&self) -> String {
        match (self.kind, self.number) {
            ("pr", Some(n)) => format!("{}#{n}", self.repo),
            _ => format!("{}@{}", self.repo, self.head.short(7)),
        }
    }
}

/// A fetched target, ready to diff.
pub struct Remote {
    pub source: Source,
    /// Reads the cached clone.
    pub git: Git,
    /// `base..head`; for a pull request, base is the merge base, so this is
    /// `git diff BASE...HEAD`, GitHub's Files tab.
    pub range: String,
    /// The pull request's commit messages (the first 250, as the compare
    /// API lists them), or the commit's.
    pub commits: Vec<Message>,
    /// The pull request's title and body.
    pub description: Option<Message>,
    api: Api,
}

/// `#N` references looked up per run: each is one API request.
const REFERENCE_LOOKUPS: usize = 10;

impl Remote {
    /// Whether each of the first `#N` references is an issue or a pull
    /// request, from the issues endpoint (which serves both, a pull request
    /// with a `pull_request` key). A failed lookup is left out: the
    /// reference stays neutral.
    pub fn reference_kinds(&self, numbers: &[u64]) -> BTreeMap<u64, claims::HintKind> {
        let repo = &self.source.repo;
        let mut kinds = BTreeMap::new();
        for &n in numbers.iter().take(REFERENCE_LOOKUPS) {
            let what = format!("{repo}#{n}");
            match self
                .api
                .get::<IssueJson>(&format!("/repos/{repo}/issues/{n}"), &what)
            {
                Ok(found) => {
                    let kind = match found.pull_request {
                        Some(_) => claims::HintKind::PullRequest,
                        None => claims::HintKind::Issue,
                    };
                    kinds.insert(n, kind);
                }
                Err(error) => crate::verbose!("{what} stays a reference: {error:#}"),
            }
        }
        kinds
    }
}

#[derive(Deserialize)]
struct IssueJson {
    pull_request: Option<serde::de::IgnoredAny>,
}

/// Looks the target up on GitHub and fetches its two commits into the cache.
pub fn fetch(target: &Target, cache: &Cache) -> Result<Remote> {
    let api = Api::new();
    let (repo, source, description, commits) = match target {
        Target::Pull { repo, number } => {
            let what = format!("pull request {repo}#{number}");
            let pr: PullJson = api.get(&format!("/repos/{repo}/pulls/{number}"), &what)?;
            let (base, head) = (
                CommitId::parse(&pr.base.sha)?,
                CommitId::parse(&pr.head.sha)?,
            );
            let compare: CompareJson =
                api.get(&format!("/repos/{repo}/compare/{base}...{head}"), &what)?;
            let source = Source {
                kind: "pr",
                url: format!("https://{HOST}/{repo}/pull/{number}"),
                repo: repo.to_string(),
                number: Some(*number),
                title: pr.title,
                base: CommitId::parse(&compare.merge_base_commit.sha)?,
                head,
                merge: false,
            };
            let description = Message {
                source: claims::Source::PullRequest,
                commit: None,
                text: format!("{}\n\n{}", source.title, pr.body.unwrap_or_default()),
            };
            let commits = (compare.commits.into_iter())
                .map(Message::try_from)
                .collect::<Result<_>>()?;
            (repo, source, Some(description), commits)
        }
        Target::Commit {
            repo,
            sha: short,
            pull,
        } => {
            let what = format!("commit {repo}@{short}");
            let found: CommitJson = api.get(&format!("/repos/{repo}/commits/{short}"), &what)?;
            let head = CommitId::parse(&found.sha)?;
            let base = CommitId::parse(match found.parents.first() {
                Some(parent) => &parent.sha,
                None => git::EMPTY_TREE,
            })?;
            let source = Source {
                kind: "commit",
                url: match pull {
                    Some(n) => format!("https://{HOST}/{repo}/pull/{n}/commits/{head}"),
                    None => format!("https://{HOST}/{repo}/commit/{head}"),
                },
                repo: repo.to_string(),
                number: *pull,
                title: found
                    .commit
                    .message
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string(),
                base,
                head,
                merge: found.parents.len() > 1,
            };
            (repo, source, None, vec![Message::try_from(found)?])
        }
    };
    crate::verbose!(
        "{} \"{}\": diffing {}..{}{}",
        source.name(),
        source.title,
        source.base.short(7),
        source.head.short(7),
        if source.merge {
            " (merge: against its first parent)"
        } else {
            ""
        }
    );
    let wanted: Vec<&str> = [source.base.as_str(), source.head.as_str()]
        .into_iter()
        .filter(|s| *s != git::EMPTY_TREE)
        .collect();
    let (git_dir, lock) = cache.fetch(repo, &wanted)?;
    Ok(Remote {
        git: Git::bare(git_dir, source.head.as_str().to_string(), lock),
        range: format!("{}..{}", source.base, source.head),
        source,
        commits,
        description,
        api,
    })
}

/// A full commit id from the API, checked before it reaches git: 40 hex
/// digits, lowercase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CommitId(String);

impl CommitId {
    fn parse(value: &str) -> Result<CommitId> {
        if value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(CommitId(value.to_ascii_lowercase()))
        } else {
            bail!("unexpected response from GitHub: `{value}` is not a commit id")
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Its first N digits.
    pub fn short(&self, n: usize) -> &str {
        &self.0[..n.min(self.0.len())]
    }
}

impl std::fmt::Display for CommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Deserialize)]
struct PullJson {
    title: String,
    body: Option<String>,
    base: ShaJson,
    head: ShaJson,
}

#[derive(Deserialize)]
struct ShaJson {
    sha: String,
}

#[derive(Deserialize)]
struct CompareJson {
    merge_base_commit: ShaJson,
    #[serde(default)]
    commits: Vec<CommitJson>,
}

#[derive(Deserialize)]
struct CommitJson {
    sha: String,
    commit: MessageJson,
    #[serde(default)]
    parents: Vec<ShaJson>,
}

#[derive(Deserialize)]
struct MessageJson {
    message: String,
}

impl TryFrom<CommitJson> for Message {
    type Error = anyhow::Error;

    fn try_from(c: CommitJson) -> Result<Self> {
        Ok(Message {
            source: claims::Source::Commit,
            commit: Some(CommitId::parse(&c.sha)?.short(12).to_string()),
            text: c.commit.message,
        })
    }
}

/// A token and where it came from. Never displayed: errors name the source.
struct Token {
    value: String,
    source: &'static str,
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Token(from {}, redacted)", self.source)
    }
}

/// gh's own order: GH_TOKEN, GITHUB_TOKEN, then `gh auth token`.
fn token() -> Option<Token> {
    for name in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Some(value) = std::env::var(name).ok().filter(|v| !v.trim().is_empty()) {
            return Some(Token {
                value: value.trim().to_string(),
                source: name,
            });
        }
    }
    let value = gh_auth_token()?;
    Some(Token {
        value,
        source: "`gh auth token`",
    })
}

/// `gh auth token`, if gh is installed and logged in; bounded, since a
/// credential store can hang.
fn gh_auth_token() -> Option<String> {
    let mut child = Command::new("gh")
        .args(["auth", "token", "--hostname", HOST])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !value.is_empty()).then_some(value)
}

struct Api {
    base: String,
    /// Named in errors: `api.github.com`.
    host: String,
    token: Option<Token>,
    agent: ureq::Agent,
}

impl Api {
    fn new() -> Self {
        let base = env_url("SHAPR_GITHUB_API_URL", API_URL);
        let host = base
            .split_once("://")
            .map_or(base.as_str(), |(_, rest)| rest)
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string();
        let token = token();
        crate::verbose!(
            "GitHub API at {base}, {}",
            token
                .as_ref()
                .map_or("unauthenticated".to_string(), |t| format!(
                    "token from {}",
                    t.source
                ))
        );
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent(concat!("shapr/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Api {
            base,
            host,
            token,
            agent,
        }
    }

    fn get<T: serde::de::DeserializeOwned>(&self, path: &str, what: &str) -> Result<T> {
        let mut request = self
            .agent
            .get(format!("{}{path}", self.base))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {}", token.value));
        }
        // Only the host and status are reported: no error string carries
        // the request, so none can carry the token.
        let mut response = request
            .call()
            .map_err(|e| anyhow!("can't reach {}: {}", self.host, transport(&e)))?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let hint = match &self.token {
            Some(_) => String::new(),
            None => " (unauthenticated: set GITHUB_TOKEN or run `gh auth login`)".to_string(),
        };
        match status {
            200 => response
                .body_mut()
                .with_config()
                .limit(64 << 20)
                .read_json()
                .map_err(|_| anyhow!("unexpected response from {} for {what}", self.host)),
            401 => {
                let source = self.token.as_ref().map_or("the request", |t| t.source);
                bail!(
                    "GitHub rejected the token from {source} (401): refresh it or run `gh auth login`"
                )
            }
            403 | 429 if header("x-ratelimit-remaining").as_deref() == Some("0") => {
                bail!("GitHub's API rate limit is used up{hint}")
            }
            404 => bail!("{what} not found on GitHub{hint}"),
            _ => bail!("{} answered {status} for {what}{hint}", self.host),
        }
    }
}

/// A transport failure in a few words; no ureq error carries headers.
fn transport(error: &ureq::Error) -> String {
    match error {
        ureq::Error::HostNotFound => "host not found".to_string(),
        ureq::Error::ConnectionFailed => "connection failed".to_string(),
        ureq::Error::Timeout(_) => "timed out".to_string(),
        ureq::Error::Io(e) => e.kind().to_string(),
        other => other.to_string(),
    }
}

/// An absolute URL from the environment, without a trailing `/`.
fn env_url(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Bare partial clones of GitHub repositories, one per repository, holding
/// only the commits shapr has diffed and the blobs it read.
pub struct Cache {
    root: PathBuf,
}

/// What `--status` and `--clear-cache` report.
#[derive(Serialize)]
pub struct CacheStatus {
    record: &'static str,
    pub path: String,
    pub repositories: Vec<String>,
    pub bytes: u64,
}

impl Cache {
    /// `--cache-dir`, else `SHAPR_CACHE_DIR`, else `$XDG_CACHE_HOME/shapr`,
    /// else `~/.cache/shapr`. A relative path is a usage error: it would
    /// mean a different cache in every directory.
    pub fn resolve(flag: Option<PathBuf>) -> Result<Cache, String> {
        let env = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let (root, from) = if let Some(dir) = flag {
            (dir, "--cache-dir")
        } else if let Some(dir) = env("SHAPR_CACHE_DIR") {
            (dir, "SHAPR_CACHE_DIR")
        } else if let Some(dir) = env("XDG_CACHE_HOME").filter(|d| d.is_absolute()) {
            (dir.join("shapr"), "XDG_CACHE_HOME")
        } else if let Some(home) = env("HOME") {
            (home.join(".cache/shapr"), "HOME")
        } else {
            return Err("no cache directory: set SHAPR_CACHE_DIR or HOME".to_string());
        };
        if root.is_relative() {
            return Err(format!(
                "{from} must be an absolute path, not `{}` (try $PWD/{})",
                root.display(),
                root.display()
            ));
        }
        Ok(Cache { root })
    }

    /// The only directory `--clear-cache` removes, so a cache root that is
    /// also something else (`SHAPR_CACHE_DIR=$HOME`) loses nothing else.
    fn hosts(&self) -> PathBuf {
        self.root.join(HOST)
    }

    fn repository(&self, repo: &Repo) -> PathBuf {
        // GitHub names are case-insensitive: one clone per repository.
        self.hosts()
            .join(repo.owner.to_ascii_lowercase())
            .join(format!("{}.git", repo.name.to_ascii_lowercase()))
    }

    /// The clone of REPO holding WANTED, fetching only the commits it lacks,
    /// and the lock that keeps other runs out of it until dropped: git's own
    /// locks fail a concurrent fetch or config write rather than wait.
    fn fetch(&self, repo: &Repo, wanted: &[&str]) -> Result<(PathBuf, std::fs::File)> {
        let path = self.repository(repo);
        let url = format!("{}/{repo}.git", env_url("SHAPR_GITHUB_GIT_URL", GIT_URL));
        if !path.exists() {
            create(&path).with_context(|| format!("creating the cache at {}", tilde(&path)))?;
            crate::verbose!("created {}", tilde(&path));
        }
        let lock = std::fs::File::create(path.join("shapr.lock"))
            .and_then(|lock| lock.lock().map(|()| lock))
            .with_context(|| format!("locking {}", tilde(&path)))?;
        // The clone follows SHAPR_GITHUB_GIT_URL; written only when it moved.
        let current = git::run(cache_git(&path).args(["config", "--get", "remote.origin.url"]))
            .unwrap_or_default();
        if String::from_utf8_lossy(&current).trim() != url {
            git::run(cache_git(&path).args(["config", "remote.origin.url", &url]))?;
        }
        let missing: Vec<&str> = wanted
            .iter()
            .copied()
            .filter(|sha| !has_commit(&path, sha))
            .collect();
        if missing.is_empty() {
            crate::verbose!("{repo}: both commits already cached");
            return Ok((path, lock));
        }
        crate::verbose!("{repo}: fetching {} commit(s) from {url}", missing.len());
        let mut cmd = cache_git(&path);
        cmd.args([
            "fetch",
            "--quiet",
            "--depth=1",
            "--filter=blob:none",
            "--no-tags",
            "--no-write-fetch-head",
            "origin",
        ])
        .args(&missing);
        git::run(&mut cmd).map_err(|e| {
            let text = e.to_string();
            let last = text.lines().last().unwrap_or_default().trim();
            let hint = if text.contains("Username")
                || text.contains("not found")
                || text.contains("Authentication")
            {
                " (private? let git reach it, e.g. `gh auth setup-git`)"
            } else {
                ""
            };
            anyhow!("can't fetch {repo} from {HOST}: {last}{hint}")
        })?;
        Ok((path, lock))
    }

    pub fn status(&self) -> CacheStatus {
        let mut repositories = Vec::new();
        for owner in read_dirs(&self.hosts()) {
            for repo in read_dirs(&owner) {
                let (Some(o), Some(r)) = (owner.file_name(), repo.file_name()) else {
                    continue;
                };
                let r = r.to_string_lossy();
                if let Some(r) = r.strip_suffix(".git") {
                    repositories.push(format!("{HOST}/{}/{r}", o.to_string_lossy()));
                }
            }
        }
        repositories.sort();
        CacheStatus {
            record: "cache",
            path: tilde(&self.root),
            repositories,
            bytes: size(&self.hosts()),
        }
    }

    /// Removes every cached clone; returns how many there were.
    pub fn clear(&self) -> Result<usize> {
        let count = self.status().repositories.len();
        match std::fs::remove_dir_all(self.hosts()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(anyhow!("clearing {}: {e}", tilde(&self.hosts())))
            }
            _ => Ok(count),
        }
    }
}

/// git on a cache clone, never prompting for credentials.
fn cache_git(git_dir: &Path) -> Command {
    let mut cmd = git::command();
    cmd.arg("--git-dir")
        .arg(git_dir)
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

/// Without fetching it on demand, which a partial clone otherwise would.
fn has_commit(git_dir: &Path, sha: &str) -> bool {
    cache_git(git_dir)
        .env("GIT_NO_LAZY_FETCH", "1")
        .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A bare repository whose `origin` fetches lazily, built beside PATH and
/// renamed into place so a concurrent run never sees half of one.
fn create(path: &Path) -> Result<()> {
    let parent = path.parent().context("no parent")?;
    std::fs::create_dir_all(parent)?;
    let staging = tempfile_dir(parent)?;
    git::run(
        git::command()
            .args(["init", "--quiet", "--bare"])
            .arg(&staging)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE"),
    )?;
    for (key, value) in [
        ("remote.origin.promisor", "true"),
        ("remote.origin.partialclonefilter", "blob:none"),
    ] {
        git::run(cache_git(&staging).args(["config", key, value]))?;
    }
    if std::fs::rename(&staging, path).is_err() {
        // Another run got there first.
        let _ = std::fs::remove_dir_all(&staging);
        if !path.exists() {
            bail!("couldn't move the new clone into place");
        }
    }
    Ok(())
}

fn tempfile_dir(parent: &Path) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let path = parent.join(format!(".new-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&path)?;
    Ok(path)
}

fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

fn size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| size(&e.path()))
        .sum()
}

/// PATH with the home directory as `~`, for reading rather than pasting.
pub fn tilde(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty())
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return Path::new("~").join(rest).display().to_string();
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(owner: &str, name: &str) -> Repo {
        Repo::parse(owner, name).unwrap()
    }

    fn pr(owner: &str, name: &str, number: u64) -> Parsed {
        Parsed::Target(Target::Pull {
            repo: repo(owner, name),
            number,
        })
    }

    fn commit_at(owner: &str, name: &str, sha: &str, pull: Option<u64>) -> Parsed {
        Parsed::Target(Target::Commit {
            repo: repo(owner, name),
            sha: sha.to_string(),
            pull,
        })
    }

    #[test]
    fn pull_request_forms() {
        for arg in [
            "https://github.com/acme/widgets/pull/12",
            "https://github.com/acme/widgets/pull/12/",
            "https://github.com/acme/widgets/pull/12/files",
            "https://github.com/acme/widgets/pull/12/commits",
            "https://github.com/acme/widgets/pull/12/files?w=1#diff-abc",
            "http://www.github.com/acme/widgets/pull/12",
            "HTTPS://GitHub.com/acme/widgets/pull/12",
            "github.com/acme/widgets/pull/12",
            "github.com/acme/widgets#12",
            "https://github.com/acme/widgets#12",
            "acme/widgets#12",
        ] {
            assert_eq!(parse(arg), pr("acme", "widgets", 12), "{arg}");
        }
        assert_eq!(parse("#12"), Parsed::ThisRepo(12));
        assert_eq!(parse("my-org/my.repo_2#7"), pr("my-org", "my.repo_2", 7));
    }

    #[test]
    fn commit_forms() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            parse(&format!("https://github.com/acme/widgets/commit/{sha}")),
            commit_at("acme", "widgets", sha, None)
        );
        assert_eq!(
            parse("acme/widgets@ABCDEF1"),
            commit_at("acme", "widgets", "abcdef1", None)
        );
        assert_eq!(
            parse(&format!(
                "https://github.com/acme/widgets/pull/12/commits/{sha}"
            )),
            commit_at("acme", "widgets", sha, Some(12))
        );
    }

    #[test]
    fn local_revisions_are_not_github() {
        for arg in [
            "main",
            "main..HEAD",
            "main...feature",
            "HEAD~2",
            "HEAD@{1}",
            "origin/main@{upstream}",
            "v1.2.3",
            "abc1234",
            "feature/login",
        ] {
            assert_eq!(parse(arg), Parsed::NotGitHub, "{arg}");
        }
    }

    #[test]
    fn github_shaped_but_wrong_is_malformed() {
        for arg in [
            "https://github.com/acme/widgets",
            "https://github.com/acme/widgets/issues/3",
            "https://github.com/acme/widgets/pull/abc",
            "https://github.com/acme/widgets/compare/a...b",
            "https://gitlab.com/acme/widgets/pull/12",
            "github.com/acme",
            "acme/widgets#",
            "acme/widgets#0",
            "acme/widgets#x1",
            "#",
            "#abc",
            "acme/widgets@xyz1234",
            "acme/widgets@abc12",
            "../etc#1",
            "acme/../x#1",
        ] {
            assert_eq!(parse(arg), Parsed::Malformed, "{arg}");
        }
    }

    #[test]
    fn remote_urls_name_their_github_repository() {
        for url in [
            "git@github.com:acme/widgets.git",
            "https://github.com/acme/widgets",
            "https://github.com/acme/widgets.git",
            "ssh://git@github.com/acme/widgets.git",
            "https://user@GitHub.com/acme/widgets/",
        ] {
            assert_eq!(github_remote(url), Some(repo("acme", "widgets")), "{url}");
        }
        for url in [
            "git@gitlab.com:acme/widgets.git",
            "/srv/git/widgets.git",
            "https://github.com/acme",
        ] {
            assert_eq!(github_remote(url), None, "{url}");
        }
    }

    #[test]
    fn a_token_never_shows_in_debug() {
        let token = Token {
            value: "ghp_secretvalue".to_string(),
            source: "GH_TOKEN",
        };
        assert!(!format!("{token:?}").contains("ghp_"));
    }

    #[test]
    fn a_relative_cache_dir_is_refused() {
        let error = Cache::resolve(Some(PathBuf::from("cache"))).err().unwrap();
        assert!(
            error.contains("--cache-dir must be an absolute path"),
            "{error}"
        );
    }
}
