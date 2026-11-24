//! The RuboCop config a file falls under: the nearest `.rubocop.yml`, with
//! the local files it `inherit_from`s. Only the YAML RuboCop configs use is
//! read: top-level cop names, their scalar settings, their `Include` and
//! `Exclude` lists, and `inherit_from`. Gems (`inherit_gem`) and remote URLs
//! are not followed: a gem's config is whichever version this machine has
//! installed, not the repository's, and a URL needs the network. What isn't
//! read is listed in `Config::unread`.

use crate::norms::snapshot::{Snapshot, join, parent, resolve};
use std::collections::BTreeMap;

const NAME: &str = ".rubocop.yml";
/// Where `rubocop --auto-gen-config` parks the offenses a project has yet to fix.
const TODO: &str = ".rubocop_todo.yml";

/// Settings by cop, then by key; a later file overrides what it inherits.
#[derive(Debug, Default)]
pub struct Config {
    settings: BTreeMap<String, BTreeMap<String, String>>,
    /// Path lists (`Include`, `Exclude`) by cop, then by key: each pattern
    /// relative to the repository's top, `None` where it can't be read (a
    /// regexp, an absolute path, ERB).
    lists: BTreeMap<String, BTreeMap<String, Vec<Option<String>>>>,
    /// Lines of the file that aren't YAML this reader understands, 1-based.
    lines: Vec<usize>,
    /// `inherit_gem` gems, by name.
    gems: Vec<String>,
    /// What the chain holds that wasn't read, one note each: a gem, a
    /// missing or remote file, lines that aren't YAML this reader reads.
    pub unread: Vec<String>,
    /// Cops a `.rubocop_todo.yml` in the chain lists: offenses the project
    /// has parked rather than fixed.
    todo: std::collections::BTreeSet<String>,
}

impl Config {
    pub fn get(&self, cop: &str, key: &str) -> Option<&str> {
        let settings = self.settings.get(cop)?;
        // A disabled cop sets no style.
        if settings.get("Enabled").map(String::as_str) == Some("false") {
            return None;
        }
        settings.get(key).map(String::as_str)
    }

    /// A `.rubocop_todo.yml` the config inherits lists `cop`.
    pub fn todo(&self, cop: &str) -> bool {
        self.todo.contains(cop)
    }

    /// RuboCop would inspect the file at `path` with `cop`: it is outside
    /// `AllCops`' and the cop's `Exclude`, and inside the cop's `Include` if
    /// it has one. A pattern that can't be read counts against the file.
    pub fn inspects(&self, cop: &str, path: &str) -> bool {
        let list = |cop: &str, key: &str| self.lists.get(cop).and_then(|l| l.get(key));
        let hit = |patterns: &Vec<Option<String>>, unread: bool| {
            (patterns.iter()).any(|p| p.as_ref().map_or(unread, |p| glob(p, path)))
        };
        let excluded = |cop: &str| list(cop, "Exclude").is_some_and(|p| hit(p, true));
        let included = list(cop, "Include").is_none_or(|p| p.is_empty() || hit(p, false));
        included && !excluded("AllCops") && !excluded(cop)
    }
}

/// The `.rubocop.yml` nearest the file at `path`, looking no higher than
/// the repository's top.
pub fn find(path: &str, snapshot: &dyn Snapshot) -> Option<String> {
    let mut dir = parent(path);
    loop {
        let candidate = join(dir, NAME);
        if snapshot.read(&candidate).is_some() {
            return Some(candidate);
        }
        if dir.is_empty() {
            return None;
        }
        dir = parent(dir);
    }
}

/// Config files one chain reads, at most: plenty for a real project, and a
/// bound on a chain built to fan out.
const MAX_FILES: usize = 32;

pub fn load(path: &str, snapshot: &dyn Snapshot) -> Config {
    let mut config = Config::default();
    merge(path, snapshot, &mut config, &mut Vec::new(), &mut 0);
    config
}

/// Merges the file at `path` under what it inherits; `chain` holds the
/// files inheriting it, `read` counts the files read so far.
fn merge(
    path: &str,
    snapshot: &dyn Snapshot,
    config: &mut Config,
    chain: &mut Vec<String>,
    read: &mut usize,
) {
    let Some(text) = snapshot.read(path) else {
        return;
    };
    *read += 1;
    chain.push(path.to_string());
    // RuboCop reads a `.rubocop*` file's paths from its own directory, any
    // other config's from where it runs: the top, here.
    let name = path.rsplit('/').next().unwrap_or_default();
    let base = if name.starts_with(".rubocop") {
        parent(path)
    } else {
        ""
    };
    let (inherits, own) = parse(&String::from_utf8_lossy(&text), base);
    if !own.lines.is_empty() {
        let lines: Vec<String> = own.lines.iter().map(usize::to_string).collect();
        let s = if lines.len() == 1 { "" } else { "s" };
        (config.unread).push(format!("{path}: line{s} {} not read", lines.join(", ")));
    }
    (config.unread).extend(own.gems.iter().map(|g| format!("inherit_gem: {g}")));
    for entry in inherits {
        let file = resolve(parent(path), &entry).filter(|f| snapshot.read(f).is_some());
        let note = match file {
            _ if entry.contains("://") => format!("inherit_from: {entry} (remote)"),
            None => format!("inherit_from: {entry} (not found)"),
            Some(file) if chain.contains(&file) => format!("inherit_from: {entry} (cycle)"),
            Some(_) if *read >= MAX_FILES => {
                format!("inherit_from: {entry} (past {MAX_FILES} config files)")
            }
            Some(file) => {
                merge(&file, snapshot, config, chain, read);
                continue;
            }
        };
        if !config.unread.contains(&note) {
            config.unread.push(note);
        }
    }
    chain.pop();
    if name == TODO {
        let cops = own.settings.keys().chain(own.lists.keys());
        config.todo.extend(cops.cloned());
    }
    for (cop, settings) in own.settings {
        config.settings.entry(cop).or_default().extend(settings);
    }
    for (cop, lists) in own.lists {
        config.lists.entry(cop).or_default().extend(lists);
    }
}

/// `inherit_from` entries and the cops' scalar settings and path lists,
/// those resolved from `base`.
fn parse(text: &str, base: &str) -> (Vec<String>, Config) {
    let mut inherits = Vec::new();
    let mut config = Config::default();
    let mut cop: Option<String> = None;
    let mut in_inherit = false;
    let mut in_gem = false;
    // The indentation of a cop's settings; deeper lines are nested values.
    let mut indent: Option<usize> = None;
    // The setting whose block list the following `- ` lines extend.
    let mut items: Option<String> = None;
    let pattern = |item: &str| {
        let item = unquote(item);
        let unreadable = item.starts_with('/') || item.starts_with('!') || item.contains("<%");
        (!unreadable).then(|| resolve(base, item)).flatten()
    };
    for (n, raw) in text.lines().enumerate() {
        let line = strip_comment(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "---" {
            continue;
        }
        if !readable(trimmed) {
            config.lines.push(n + 1);
            continue;
        }
        let depth = line.len() - line.trim_start().len();
        if depth == 0 {
            let (key, value) = split(trimmed);
            in_inherit = key == "inherit_from";
            in_gem = key == "inherit_gem";
            cop = None;
            indent = None;
            items = None;
            if in_inherit {
                inherits.extend(list(value));
            } else if value.is_empty() && !in_gem {
                cop = Some(key.to_string());
            }
            continue;
        }
        if in_inherit {
            if let Some(item) = trimmed.strip_prefix("- ") {
                inherits.push(unquote(item).to_string());
            }
            continue;
        }
        if in_gem {
            config.gems.push(split(trimmed).0.to_string());
            continue;
        }
        let Some(cop) = &cop else { continue };
        let indent = *indent.get_or_insert(depth);
        // A block list's items sit deeper than its key, or level with it.
        if let (Some(key), Some(item)) = (&items, trimmed.strip_prefix("- "))
            && depth >= indent
        {
            let list = config.lists.entry(cop.clone()).or_default();
            list.entry(key.clone()).or_default().push(pattern(item));
            continue;
        }
        if depth != indent {
            continue;
        }
        let (key, value) = split(trimmed);
        items = None;
        if value.is_empty() {
            items = Some(key.to_string());
            let list = config.lists.entry(cop.clone()).or_default();
            list.insert(key.to_string(), Vec::new());
        } else if value.starts_with('[') {
            let list = config.lists.entry(cop.clone()).or_default();
            list.insert(
                key.to_string(),
                self::list(value).iter().map(|v| pattern(v)).collect(),
            );
        } else {
            config
                .settings
                .entry(cop.clone())
                .or_default()
                .insert(key.to_string(), unquote(value).to_string());
        }
    }
    (inherits, config)
}

/// A line is a list item or a `key: value` with a key, and a flow value
/// closes on its line.
fn readable(line: &str) -> bool {
    if line.starts_with("- ") {
        return true;
    }
    let Some((key, value)) = line.split_once(':') else {
        return false;
    };
    let value = value.trim();
    let open = |a: char, b: char| value.starts_with(a) && !value.ends_with(b);
    !key.trim().is_empty() && !open('[', ']') && !open('{', '}')
}

/// RuboCop's path match: `File.fnmatch?` with `FNM_PATHNAME` and
/// `FNM_EXTGLOB`, so `*` stays within a directory, `**/` spans any number of
/// them and `{a,b}` is either; a pattern with no wildcard matches only itself.
pub fn glob(pattern: &str, path: &str) -> bool {
    if let Some(open) = pattern.find('{')
        && let Some(close) = pattern[open..].find('}').map(|i| open + i)
    {
        return (pattern[open + 1..close].split(',')).any(|alt| {
            glob(
                &format!("{}{alt}{}", &pattern[..open], &pattern[close + 1..]),
                path,
            )
        });
    }
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments(&pattern, &path)
}

fn segments(pattern: &[&str], path: &[&str]) -> bool {
    match (pattern.split_first(), path.split_first()) {
        (None, None) => true,
        (Some((&"**", rest)), _) if !rest.is_empty() => {
            (0..=path.len()).any(|skip| segments(rest, &path[skip..]))
        }
        (Some((p, rest)), Some((name, tail))) => {
            wildcard(p.as_bytes(), name.as_bytes()) && segments(rest, tail)
        }
        _ => false,
    }
}

/// One path segment against `*`, `?` and `[...]`.
fn wildcard(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|i| wildcard(rest, &name[i..])),
        Some((b'?', rest)) => !name.is_empty() && wildcard(rest, &name[1..]),
        Some((b'[', rest)) => {
            let Some(end) = rest.iter().position(|&b| b == b']') else {
                return name.first() == Some(&b'[') && wildcard(rest, &name[1..]);
            };
            let (class, rest) = (&rest[..end], &rest[end + 1..]);
            let (negate, class) = match class.split_first() {
                Some((b'!' | b'^', class)) => (true, class),
                _ => (false, class),
            };
            let Some(&c) = name.first() else { return false };
            let mut hit = false;
            let mut i = 0;
            while i < class.len() {
                if i + 2 < class.len() && class[i + 1] == b'-' {
                    hit |= (class[i]..=class[i + 2]).contains(&c);
                    i += 3;
                } else {
                    hit |= class[i] == c;
                    i += 1;
                }
            }
            hit != negate && wildcard(rest, &name[1..])
        }
        Some((&b, rest)) => name.first() == Some(&b) && wildcard(rest, &name[1..]),
    }
}

fn strip_comment(line: &str) -> &str {
    if line.trim_start().starts_with('#') {
        return "";
    }
    line.find(" #").map_or(line, |i| &line[..i])
}

fn split(line: &str) -> (&str, &str) {
    match line.split_once(':') {
        Some((key, value)) => (unquote(key.trim()), value.trim()),
        None => (line, ""),
    }
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    for q in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(q).and_then(|v| v.strip_suffix(q)) {
            return inner;
        }
    }
    value
}

/// A scalar or a flow list (`[a.yml, b.yml]`).
fn list(value: &str) -> Vec<String> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }
    let inner = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split(',')
        .map(|v| unquote(v).to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cop_settings_and_inherit_from() {
        let (inherits, config) = parse(
            "inherit_from:\n  - .rubocop_todo.yml\n\nAllCops:\n  TargetRubyVersion: 3.2\n  Exclude:\n    - 'vendor/**/*'\n\
             Style/StringLiterals:\n  EnforcedStyle: double_quotes # house style\n\
             Style/HashSyntax:\n  Enabled: false\n  EnforcedStyle: hash_rockets\n",
            "",
        );
        assert_eq!(inherits, [".rubocop_todo.yml"]);
        assert_eq!(
            config.get("Style/StringLiterals", "EnforcedStyle"),
            Some("double_quotes")
        );
        assert_eq!(config.get("Style/HashSyntax", "EnforcedStyle"), None);
        assert_eq!(config.get("AllCops", "Exclude"), None);
        assert_eq!(
            parse("inherit_from: [a.yml, 'b.yml']\n", "").0,
            ["a.yml", "b.yml"]
        );
    }

    #[test]
    fn lines_this_reader_cant_read_are_noted() {
        let (_, config) = parse(
            "Style/StringLiterals: [oops\n  : :\nA:\n  B: c\nloose\n",
            "",
        );
        assert_eq!(config.lines, [1, 2, 5]);
    }

    #[test]
    fn paths_scope_a_cop_as_rubocop_matches_them() {
        let (_, config) = parse(
            "AllCops:\n  Exclude:\n    - 'db/**/*'\n\
             Style/HashSyntax:\n  Exclude:\n    - !ruby/regexp /tmp/\n\
             Style/StringLiterals:\n  Include:\n  - '**/*.{rb,rake}'\n  Exclude: [lib/legacy/*.rb]\n",
            "sub",
        );
        let inspects = |path| config.inspects("Style/StringLiterals", path);
        assert!(inspects("sub/lib/a.rb"));
        assert!(inspects("sub/a.rake"));
        assert!(!inspects("sub/Gemfile"));
        assert!(!inspects("sub/db/schema.rb"));
        assert!(!inspects("sub/lib/legacy/old.rb"));
        assert!(inspects("sub/lib/legacy/deeper/old.rb"));
        // The regexp can't be read, so it may exclude anything.
        assert!(!config.inspects("Style/HashSyntax", "sub/lib/a.rb"));
        assert!(glob("a/**/*.rb", "a/b.rb"));
        assert!(glob("a/[b-c]?.rb", "a/cd.rb"));
        assert!(!glob("a/*", "a/b/c.rb"));
        assert!(!glob("a", "a/b.rb"));
    }

    /// A chain that fans out without a cycle stops at the file cap.
    #[test]
    fn a_fanned_out_chain_stops_at_the_file_cap() {
        struct Fan;
        impl Snapshot for Fan {
            fn read(&self, path: &str) -> Option<Vec<u8>> {
                let n: usize = path.strip_prefix('f')?.strip_suffix(".yml")?.parse().ok()?;
                let next = format!("  - f{}.yml\n", n + 1).repeat(12);
                Some(format!("inherit_from:\n{next}A:\n  B: c\n").into_bytes())
            }
            fn list(&self, _: &str) -> Vec<String> {
                Vec::new()
            }
        }
        let config = load("f0.yml", &Fan);
        assert_eq!(config.get("A", "B"), Some("c"));
        assert!(
            (config.unread.iter()).any(|n| n.ends_with("(past 32 config files)")),
            "{:?}",
            config.unread
        );
    }

    #[test]
    fn a_file_overrides_what_it_inherits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("base.yml"),
            "Style/StringLiterals:\n  EnforcedStyle: single_quotes\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(NAME),
            "inherit_from: base.yml\nStyle/StringLiterals:\n  EnforcedStyle: double_quotes\n",
        )
        .unwrap();
        let snapshot = crate::norms::snapshot::Worktree(dir.path().to_path_buf());
        let found = find("lib/a.rb", &snapshot).unwrap();
        assert_eq!(found, NAME);
        assert_eq!(
            load(&found, &snapshot).get("Style/StringLiterals", "EnforcedStyle"),
            Some("double_quotes")
        );
    }
}
