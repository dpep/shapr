//! A codebase's norms, for `shapr norms PATH...`: the files under the paths,
//! each norm pooled over all of them, the deviations from it, and the code
//! least like the rest (hotspots).

use super::{
    Analysis, Cached, Configured, Counts, Language, NormDef, Observation, ScopeKind, Worktree,
    count, for_path, infer, skipped,
};
use crate::change::Category;
use crate::git::Linguist;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// One file read for its norms.
pub struct Source {
    /// As named, or as found under a named directory.
    pub path: String,
    /// From the top of its repository, for config and the repository search.
    pub rel: String,
    /// The repository's top, or the filesystem root outside one.
    pub root: PathBuf,
    pub bytes: Vec<u8>,
    pub language: &'static dyn Language,
    pub analysis: Analysis,
    pub configured: Configured,
}

#[derive(Serialize)]
pub struct FileRecord {
    pub record: &'static str,
    pub file: String,
    pub language: Option<&'static str>,
    pub skipped: Option<&'static str>,
}

/// Files the paths hold: those judged, a record for each file named or
/// judged, and how many were skipped and why.
pub struct Collected {
    pub sources: Vec<Source>,
    pub files: Vec<FileRecord>,
    pub skipped: BTreeMap<&'static str, usize>,
}

pub const SKIPS: [&str; 6] = [
    "generated",
    "vendored",
    "docs",
    "fixture",
    "syntax_error",
    "unsupported",
];

/// Reads PATHS: a file as named, a directory as every file under it a
/// language has norms for (git's tracked and unignored files inside a
/// repository). A file found under a directory that is generated, vendored
/// or docs is counted, not read or listed. Errs with a message on a path
/// that can't be read.
pub fn collect(paths: &[PathBuf]) -> Result<Collected, String> {
    let mut named: Vec<(String, bool)> = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut found: Vec<String> = list(path)
                .into_iter()
                .filter(|p| for_path(p).is_some())
                .collect();
            found.sort();
            named.extend(found.into_iter().map(|p| (p, false)));
        } else {
            named.push((path.display().to_string(), true));
        }
    }
    let mut seen = BTreeSet::new();
    named.retain(|(p, _)| seen.insert(p.clone()));
    let mut roots = HashMap::new();
    let located: Vec<(PathBuf, String)> = (named.iter())
        .map(|(p, _)| locate(Path::new(p), &mut roots))
        .collect();
    let linguist = linguist(&located);
    let mut out = Collected {
        sources: Vec::new(),
        files: Vec::new(),
        skipped: SKIPS.iter().map(|s| (*s, 0)).collect(),
    };
    // Config files read once per repository, however many files they configure.
    let mut snapshots: HashMap<PathBuf, Cached<Worktree>> = HashMap::new();
    for ((path, explicit), (root, rel)) in named.into_iter().zip(located) {
        let language = for_path(&path);
        let category = Category::of(&rel, false, linguist.get(&rel).copied().unwrap_or_default());
        // A link out of the repository, or anything but a file, isn't read.
        let outside = std::fs::symlink_metadata(&path).is_ok()
            && super::snapshot::inside(&root, &rel).is_none_or(|f| !f.is_file());
        let mut reason = match language {
            None => Some("unsupported"),
            Some(_) if outside => Some("unsupported"),
            Some(_) => skipped(&rel, category),
        };
        let mut source = None;
        if reason.is_none() {
            let bytes = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
            let language = language.expect("a language");
            let analysis = language.analyze(&bytes);
            if analysis.errors {
                reason = Some("syntax_error");
            } else {
                let snapshot = (snapshots.entry(root.clone()))
                    .or_insert_with(|| Cached::new(Worktree(root.clone())));
                let configured = language.configured(&rel, snapshot);
                source = Some(Source {
                    path: path.clone(),
                    rel,
                    root,
                    bytes,
                    language,
                    analysis,
                    configured,
                });
            }
        }
        if let Some(reason) = reason {
            *out.skipped.entry(reason).or_default() += 1;
        }
        let listed = explicit || matches!(reason, None | Some("syntax_error"));
        if listed {
            out.files.push(FileRecord {
                record: "file",
                file: path,
                language: language.map(|l| l.name()),
                skipped: reason,
            });
        }
        out.sources.extend(source);
    }
    Ok(out)
}

/// The files under DIR, as DIR joined with each one's path below it.
fn list(dir: &Path) -> Vec<String> {
    let shown = |rel: &str| {
        let joined = dir.join(rel);
        let text = joined.display().to_string();
        text.strip_prefix("./").map(str::to_string).unwrap_or(text)
    };
    let tracked = crate::git::command()
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotePath=false", "ls-files", "-z", "--cached"])
        .args(["--others", "--exclude-standard", "--", "."])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output();
    if let Ok(out) = tracked
        && out.status.success()
    {
        return String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(shown)
            .filter(|p| Path::new(p).is_file())
            .collect();
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.iter()
        .filter_map(|p| p.strip_prefix(dir).ok())
        .map(|p| shown(&p.display().to_string()))
        .collect()
}

/// Every file under DIR outside a repository, hidden directories left out.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        match entry.file_type() {
            Ok(t) if t.is_dir() && !hidden => walk(&path, out),
            Ok(t) if t.is_file() => out.push(path),
            _ => {}
        }
    }
}

/// The repository top above PATH, and PATH from it.
fn locate(path: &Path, roots: &mut HashMap<PathBuf, PathBuf>) -> (PathBuf, String) {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = full.parent().unwrap_or(Path::new("")).to_path_buf();
    let root = (roots.entry(dir))
        .or_insert_with_key(|dir| repo_root(dir))
        .clone();
    let rel = full.strip_prefix(&root).unwrap_or(&full);
    let rel: Vec<String> = (rel.components())
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (root, rel.join("/"))
}

/// The enclosing git working tree, where a linter config search stops; the
/// filesystem root outside one.
pub fn repo_root(dir: &Path) -> PathBuf {
    let mut at = Some(dir);
    while let Some(d) = at {
        if d.join(".git").exists() {
            return d.to_path_buf();
        }
        at = d.parent();
    }
    dir.ancestors().last().unwrap_or(dir).to_path_buf()
}

/// Each file's linguist attributes, one `git check-attr` per repository.
fn linguist(located: &[(PathBuf, String)]) -> BTreeMap<String, Linguist> {
    let mut out = BTreeMap::new();
    let roots: BTreeSet<&PathBuf> = located.iter().map(|(r, _)| r).collect();
    for root in roots {
        let rels: Vec<&str> = (located.iter())
            .filter(|(r, _)| r == root)
            .map(|(_, rel)| rel.as_str())
            .collect();
        let git = crate::git::Git::new(Some(root.clone()));
        out.extend(git.linguist(&rels, None));
    }
    out
}

/// What a file's config says of one norm at one observation.
pub struct Checked<'a> {
    /// The style config sets where it checks this line.
    pub style: Option<&'a str>,
    /// Config doesn't check this line: the file is excluded for the norm's
    /// check, or a directive turns it off.
    pub withheld: bool,
}

pub fn checked<'a>(source: &'a Source, def: &NormDef, o: &Observation) -> Checked<'a> {
    let withheld =
        source.configured.off.contains(def.id) || source.analysis.disabled(def.id, o.line);
    let style = (source.configured.styles.get(def.id))
        .filter(|_| !withheld)
        .map(String::as_str);
    Checked { style, withheld }
}

/// One norm over every file.
pub struct Pooled<'a> {
    pub def: &'static NormDef,
    pub counts: Counts,
    pub files: usize,
    pub inferred: Option<super::Norm>,
    /// The style config sets, the same wherever it sets one.
    pub configured: Option<String>,
    /// Per file, the observations judged and the style each is held to;
    /// a deviation when it differs from the observation's.
    pub judged: Vec<Vec<(&'a Observation, String)>>,
    pub config: Option<ConfigRecord>,
}

impl Pooled<'_> {
    pub fn expected(&self) -> Option<String> {
        self.inferred
            .as_ref()
            .map(|n| n.style.clone())
            .or(self.configured.clone())
    }

    /// Observations judged, and those in the style they're held to.
    pub fn adherence(&self) -> (usize, usize) {
        let all = self.judged.iter().flatten();
        let matching = all.clone().filter(|(o, e)| o.style == *e).count();
        (matching, all.count())
    }

    pub fn deviations(&self) -> impl Iterator<Item = (usize, &Observation, &str)> {
        (self.judged.iter().enumerate())
            .flat_map(|(f, judged)| judged.iter().map(move |(o, e)| (f, *o, e.as_str())))
            .filter(|(_, o, e)| o.style != *e)
    }
}

/// How far the code is from its config, for a norm config sets.
#[derive(Serialize)]
pub struct ConfigRecord {
    /// Observations config checks, and those in the configured style.
    pub checked: usize,
    pub matching: usize,
    pub adherence: Option<f64>,
    /// Observations where config doesn't check the norm's cop.
    pub withheld: usize,
    /// The project's `.rubocop_todo.yml` lists the cop.
    pub todo: bool,
    /// `agrees` or `disagrees` with the inferred style; `config_only` when
    /// none is inferred.
    pub agreement: &'static str,
}

/// Each norm of the sources' languages, pooled over every file.
pub fn pool<'a>(sources: &'a [Source], threshold: f64) -> Vec<Pooled<'a>> {
    let mut defs: Vec<&'static NormDef> = Vec::new();
    for s in sources {
        for def in s.language.norms() {
            if !defs.iter().any(|d| d.id == def.id) {
                defs.push(def);
            }
        }
    }
    defs.into_iter()
        .map(|def| {
            let of = |s: &'a Source| s.analysis.observations.iter().filter(|o| o.norm == def.id);
            let counts = count(def, sources.iter().flat_map(of));
            let files = sources.iter().filter(|s| of(s).next().is_some()).count();
            let inferred = infer(&counts, threshold);
            let styles: BTreeSet<&String> = (sources.iter())
                .filter_map(|s| s.configured.styles.get(def.id))
                .collect();
            let configured = match styles.len() {
                1 => styles.into_iter().next().cloned(),
                _ => None,
            };
            let judged: Vec<Vec<(&Observation, String)>> = (sources.iter())
                .map(|s| {
                    of(s)
                        .filter_map(|o| {
                            let style = match &inferred {
                                Some(n) => n.style.clone(),
                                None => checked(s, def, o).style?.to_string(),
                            };
                            Some((o, style))
                        })
                        .collect()
                })
                .collect();
            let config = configured.as_ref().map(|style| {
                let (mut checked_n, mut matching, mut withheld) = (0, 0, 0);
                for s in sources {
                    for o in of(s) {
                        let c = checked(s, def, o);
                        withheld += c.withheld as usize;
                        if c.style.is_some() {
                            checked_n += 1;
                            matching += (o.style == *style) as usize;
                        }
                    }
                }
                ConfigRecord {
                    checked: checked_n,
                    matching,
                    adherence: share(matching, checked_n),
                    withheld,
                    todo: sources.iter().any(|s| s.configured.todo.contains(def.id)),
                    agreement: match &inferred {
                        None => "config_only",
                        Some(n) if n.style == *style => "agrees",
                        Some(_) => "disagrees",
                    },
                }
            });
            Pooled {
                def,
                counts,
                files,
                inferred,
                configured,
                judged,
                config,
            }
        })
        .collect()
}

/// K of N as a share to 2 decimals; `None` of nothing. Rounding never
/// claims all or none when it isn't.
pub fn share(k: usize, n: usize) -> Option<f64> {
    let rounded = |s: f64| (s * 100.0).round() / 100.0;
    (n > 0).then(|| match k {
        0 => 0.0,
        k if k == n => 1.0,
        k => rounded(k as f64 / n as f64).clamp(0.01, 0.99),
    })
}

#[derive(Serialize)]
pub struct Lines {
    pub start: usize,
    pub count: usize,
}

#[derive(Serialize)]
pub struct Hotspot {
    pub record: &'static str,
    pub kind: &'static str,
    pub rank: usize,
    pub file: String,
    pub name: Option<String>,
    pub lines: Lines,
    pub deviations: usize,
    pub norms: Vec<&'static str>,
    pub per_100_lines: f64,
}

/// Files, classes and methods holding deviations config doesn't withhold
/// (where it does, the project chose not to check), each kind ranked by the
/// norms it breaks, then deviations per line. A class counts its own lines
/// and deviations, not a nested class's; a method is its innermost `def`.
pub fn hotspots(sources: &[Source], pooled: &[Pooled]) -> Vec<Hotspot> {
    // Per file, each deviation's line and norm.
    let mut per_file: Vec<Vec<(usize, &'static str)>> = vec![Vec::new(); sources.len()];
    for p in pooled {
        for (f, o, _) in p.deviations() {
            if !checked(&sources[f], p.def, o).withheld {
                per_file[f].push((o.line, p.def.id));
            }
        }
    }
    let mut units: Vec<(Hotspot, usize)> = Vec::new();
    for (source, devs) in sources.iter().zip(per_file) {
        if devs.is_empty() {
            continue;
        }
        let analysis = &source.analysis;
        let innermost = |kind: ScopeKind, line: usize| {
            (analysis.scopes.iter().enumerate())
                .filter(|(_, s)| s.kind == kind && s.first <= line && line <= s.last)
                .min_by_key(|(_, s)| s.last - s.first)
                .map(|(i, _)| i)
        };
        let lines = analysis.lines.max(1);
        let mut unit = |kind: &'static str,
                        name: Option<String>,
                        start: usize,
                        count: usize,
                        hits: Vec<&'static str>| {
            let norms: BTreeSet<&'static str> = hits.iter().copied().collect();
            let rate = hits.len() as f64 / count.max(1) as f64;
            units.push((
                Hotspot {
                    record: "hotspot",
                    kind,
                    rank: 0,
                    file: source.path.clone(),
                    name,
                    lines: Lines { start, count },
                    deviations: hits.len(),
                    norms: norms.into_iter().collect(),
                    per_100_lines: (rate * 1000.0).round() / 10.0,
                },
                (rate * 1e9) as usize,
            ));
        };
        unit("file", None, 1, lines, devs.iter().map(|d| d.1).collect());
        // A class's own lines: those it is the innermost class of.
        let mut own: BTreeMap<usize, usize> = BTreeMap::new();
        for line in 1..=lines {
            if let Some(c) = innermost(ScopeKind::Class, line) {
                *own.entry(c).or_default() += 1;
            }
        }
        let mut by_scope: BTreeMap<(usize, bool), Vec<&'static str>> = BTreeMap::new();
        for (line, norm) in &devs {
            if let Some(c) = innermost(ScopeKind::Class, *line) {
                by_scope.entry((c, false)).or_default().push(norm);
            }
            if let Some(m) = innermost(ScopeKind::Method, *line) {
                by_scope.entry((m, true)).or_default().push(norm);
            }
        }
        for ((i, method), hits) in by_scope {
            let scope = &analysis.scopes[i];
            let span = scope.last - scope.first + 1;
            if method {
                // `def self.x` reads as a singleton method of its owner.
                let (singleton, method) = match scope.name.strip_prefix("self.") {
                    Some(name) => (true, name),
                    None => (false, scope.name.as_str()),
                };
                let name = match owner(analysis, scope.first) {
                    Some((owner, s)) if s || singleton => format!("{owner}.{method}"),
                    Some((owner, _)) => format!("{owner}#{method}"),
                    None => scope.name.clone(),
                };
                unit("method", Some(name), scope.first, span, hits);
            } else {
                let count = own.get(&i).copied().unwrap_or(span);
                unit("class", Some(scope.name.clone()), scope.first, count, hits);
            }
        }
    }
    let mut out = Vec::new();
    for kind in ["file", "class", "method"] {
        let mut of: Vec<(Hotspot, usize)> = Vec::new();
        let mut rest = Vec::new();
        for u in units.drain(..) {
            if u.0.kind == kind {
                of.push(u);
            } else {
                rest.push(u);
            }
        }
        units = rest;
        of.sort_by(|(a, ra), (b, rb)| {
            (b.norms.len().cmp(&a.norms.len()))
                .then(rb.cmp(ra))
                .then(b.deviations.cmp(&a.deviations))
                .then(a.file.cmp(&b.file))
                .then(a.lines.start.cmp(&b.lines.start))
        });
        for (n, (mut h, _)) in of.into_iter().enumerate() {
            h.rank = n + 1;
            out.push(h);
        }
    }
    out
}

/// The class or module a method at LINE belongs to, by name, and whether it
/// sits in a `class << self` (a singleton method).
fn owner(analysis: &Analysis, line: usize) -> Option<(String, bool)> {
    let mut around: Vec<&super::Scope> = (analysis.scopes.iter())
        .filter(|s| s.kind == ScopeKind::Class && s.first <= line && line <= s.last)
        .collect();
    around.sort_by_key(|s| s.last - s.first);
    let singleton = around
        .first()
        .is_some_and(|s| s.name.starts_with("class << "));
    let named = around
        .into_iter()
        .find(|s| !s.name.starts_with("class << "))?;
    Some((named.name.clone(), singleton))
}
