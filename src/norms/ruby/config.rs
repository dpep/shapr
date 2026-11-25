//! The RuboCop config a file falls under: the nearest `.rubocop.yml`, with
//! the local files it `inherit_from`s. Only the YAML RuboCop configs use is
//! read: top-level cop names, their scalar settings, their `Include` and
//! `Exclude` lists, and `inherit_from`. Gems (`inherit_gem`) and remote URLs
//! are not followed: a gem's config is whichever version this machine has
//! installed, not the repository's, and a URL needs the network. What isn't
//! read is listed in `Config::unread`.

use crate::norms::snapshot::{MAX_CONFIG_BYTES, Snapshot, join, parent, resolve, too_large};
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
    /// missing or remote file, lines that aren't YAML this reader reads, a
    /// pattern past the brace cap.
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
    if text.len() > MAX_CONFIG_BYTES {
        config.unread.push(too_large(path));
        return;
    }
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
    (config.unread).extend(own.unread.iter().map(|note| format!("{path}: {note}")));
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
    // Patterns past the brace cap, by their text.
    let mut wide = Vec::new();
    let mut pattern = |item: &str| {
        let item = unquote(item);
        let unreadable = item.starts_with('/') || item.starts_with('!') || item.contains("<%");
        if alternatives(item).is_none() {
            wide.push(item.to_string());
            return None;
        }
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
    config.unread = (wide.into_iter())
        .map(|p| format!("pattern {p} not read (over {MAX_ALTERNATIVES} alternatives)"))
        .collect();
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

/// Brace alternatives one path pattern may stand for: far more than a real
/// pattern lists, and a bound on one built to multiply.
const MAX_ALTERNATIVES: usize = 256;

/// RuboCop's path match (`PathUtil.match_path?`): the pattern itself, or a
/// pattern with a wildcard under `File.fnmatch?` with `FNM_PATHNAME` and
/// `FNM_EXTGLOB`, so `*` stays within a directory and skips a leading `.`,
/// `**/` spans directories but not hidden ones, and `{a,b}` nests; or a
/// hidden file in no hidden directory under `FNM_DOTMATCH` too.
pub fn glob(pattern: &str, path: &str) -> bool {
    let wild = pattern.contains(['*', '{', '[', '?']);
    if pattern == path || (wild && fnmatch(pattern, path, false)) {
        return true;
    }
    // RuboCop matches absolute paths, so every file sits under a separator.
    let (dirs, name) = path.rsplit_once('/').unwrap_or(("", path));
    let hidden_dir = dirs.split('/').any(|d| d.starts_with('.'));
    name.starts_with('.') && !hidden_dir && fnmatch(pattern, path, true)
}

/// `File.fnmatch?(pattern, path, FNM_PATHNAME | FNM_EXTGLOB)`, plus
/// `FNM_DOTMATCH` when DOTMATCH: any brace alternative matches.
fn fnmatch(pattern: &str, path: &str, dotmatch: bool) -> bool {
    (alternatives(pattern).unwrap_or_default().iter())
        .any(|p| pathname(p.as_bytes(), path.as_bytes(), dotmatch))
}

/// The patterns `{a,b}` stands for, braces expanded as Ruby's
/// `ruby_brace_expand` does: nested, `\` escaping, and none at all for an
/// unclosed `{`. `None` past [`MAX_ALTERNATIVES`].
fn alternatives(pattern: &str) -> Option<Vec<String>> {
    let mut done = Vec::new();
    let mut todo = vec![pattern.to_string()];
    while let Some(p) = todo.pop() {
        let b = p.as_bytes();
        let (mut open, mut close, mut nest, mut i) = (None, None, 0, 0);
        while i < b.len() {
            match b[i] {
                b'{' => {
                    if nest == 0 {
                        open = Some(i);
                    }
                    nest += 1;
                }
                b'}' if open.is_some() => {
                    nest -= 1;
                    if nest == 0 {
                        close = Some(i);
                        break;
                    }
                }
                b'\\' => i += 1,
                _ => {}
            }
            i += 1;
        }
        let (open, close) = match (open, close) {
            (None, _) => {
                done.push(p);
                continue;
            }
            (Some(open), Some(close)) => (open, close),
            (Some(_), None) => continue,
        };
        // Each expansion drops a brace pair, so the work ends.
        for alt in split_top(&p[open + 1..close]).into_iter().rev() {
            todo.push(format!("{}{alt}{}", &p[..open], &p[close + 1..]));
        }
        if done.len() + todo.len() > MAX_ALTERNATIVES {
            return None;
        }
    }
    Some(done)
}

/// INNER split at its commas outside nested braces and escapes.
fn split_top(inner: &str) -> Vec<&str> {
    let b = inner.as_bytes();
    let (mut out, mut from, mut nest, mut i) = (Vec::new(), 0, 0i32, 0);
    while i < b.len() {
        match b[i] {
            b'{' => nest += 1,
            b'}' => nest -= 1,
            b',' if nest == 0 => {
                out.push(&inner[from..i]);
                from = i + 1;
            }
            b'\\' => i += 1,
            _ => {}
        }
        i += 1;
    }
    out.push(&inner[from.min(inner.len())..]);
    out
}

/// Ruby's `fnmatch` under `FNM_PATHNAME` (dir.c): segment by segment, with
/// the last `**/` retried one directory further on a mismatch, never into a
/// hidden one unless DOTMATCH. Each retry advances, so no pattern
/// backtracks without bound.
fn pathname(p: &[u8], s: &[u8], dotmatch: bool) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut retry: Option<(usize, usize)> = None;
    loop {
        if p[pi..].starts_with(b"**/") {
            while p[pi..].starts_with(b"**/") {
                pi += 3;
            }
            retry = Some((pi, si));
        }
        if let Some((np, ns)) = segment(p, pi, s, si, dotmatch) {
            (pi, si) = (np, ns);
            while si < s.len() && s[si] != b'/' {
                si += 1;
            }
            if pi < p.len() && si < s.len() {
                (pi, si) = (pi + 1, si + 1);
                continue;
            }
            if pi == p.len() && si == s.len() {
                return true;
            }
        }
        match retry {
            Some((at, mut from)) if dotmatch || s.get(from) != Some(&b'.') => {
                while from < s.len() && s[from] != b'/' {
                    from += 1;
                }
                if from == s.len() {
                    return false;
                }
                retry = Some((at, from + 1));
                (pi, si) = (at, from + 1);
            }
            _ => return false,
        }
    }
}

/// Ruby's `fnmatch_helper`: P from PI against one segment of S from SI,
/// where only the last `*` retries; the positions it stopped at on a match.
fn segment(
    p: &[u8],
    mut pi: usize,
    s: &[u8],
    mut si: usize,
    dotmatch: bool,
) -> Option<(usize, usize)> {
    let end = |b: &[u8], i: usize| i >= b.len() || b[i] == b'/';
    let unescape = |i: usize| i + usize::from(p.get(i) == Some(&b'\\'));
    if !dotmatch && s.get(si) == Some(&b'.') && p.get(unescape(pi)) != Some(&b'.') {
        return None;
    }
    let mut star: Option<(usize, usize)> = None;
    loop {
        match p.get(pi) {
            Some(b'*') => {
                while p.get(pi) == Some(&b'*') {
                    pi += 1;
                }
                if end(p, unescape(pi)) {
                    return Some((unescape(pi), si));
                }
                if end(s, si) {
                    return None;
                }
                star = Some((pi, si));
                continue;
            }
            Some(b'?') => {
                if end(s, si) {
                    return None;
                }
                pi += 1;
                si += char_len(s, si);
                continue;
            }
            Some(b'[') => {
                if end(s, si) {
                    return None;
                }
                if let Some(next) = bracket(p, pi + 1, s, si) {
                    pi = next;
                    si += char_len(s, si);
                    continue;
                }
            }
            _ => {
                let q = unescape(pi);
                if end(s, si) {
                    return end(p, q).then_some((q, si));
                }
                if !end(p, q) {
                    let r = char_len(p, q);
                    if s[si..].starts_with(&p[q..q + r]) {
                        pi = q + r;
                        si += r;
                        continue;
                    }
                }
            }
        }
        let (at, from) = star?;
        let from = from + char_len(s, from);
        star = Some((at, from));
        (pi, si) = (at, from);
    }
}

/// Ruby's `bracket`: a `[...]` from after its `[` against the character at
/// S\[SI\]; the position after its `]` when it matches. Unclosed, it never does.
fn bracket(p: &[u8], mut i: usize, s: &[u8], si: usize) -> Option<usize> {
    let negate = matches!(p.get(i), Some(b'!' | b'^'));
    i += usize::from(negate);
    let c = &s[si..si + char_len(s, si)];
    let mut ok = false;
    while p.get(i) != Some(&b']') {
        let lo = i + usize::from(p.get(i) == Some(&b'\\'));
        if lo >= p.len() {
            return None;
        }
        i = lo + char_len(p, lo);
        if i >= p.len() {
            return None;
        }
        let lo = &p[lo..i];
        if p[i] == b'-' && p.get(i + 1) != Some(&b']') {
            let hi = i + 1 + usize::from(p.get(i + 1) == Some(&b'\\'));
            if hi >= p.len() {
                return None;
            }
            i = hi + char_len(p, hi);
            let hi = &p[hi..i];
            ok |= c == lo || c == hi || (codepoint(lo)..=codepoint(hi)).contains(&codepoint(c));
        } else {
            ok |= c == lo;
        }
    }
    (ok != negate).then_some(i + 1)
}

/// The length of the UTF-8 character at B\[I\].
fn char_len(b: &[u8], i: usize) -> usize {
    let len = match b[i] {
        0..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    len.min(b.len() - i)
}

fn codepoint(c: &[u8]) -> u32 {
    std::str::from_utf8(c)
        .ok()
        .and_then(|c| c.chars().next())
        .map_or(0, u32::from)
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

    /// Cases where a hand-rolled matcher parted from RuboCop's
    /// `match_path?` (expectations from Ruby 3.4 and RuboCop 1.92).
    #[test]
    fn globs_match_as_rubocop_matches_them() {
        let cases = [
            ("{lib,app/{models,jobs}}/**/*.rb", "app/jobs/x.rb", true),
            ("{lib,app/{models,jobs}}/**/*.rb", "lib/x.rb", true),
            ("{a,{b,c}}.rb", "c.rb", true),
            ("a{a*{a,b}", "ab", false),
            ("{}a\\***", "a*b", true),
            ("\\*", "\\", false),
            ("*", ".a", true),
            ("**/*", ".a/b", false),
            ("**/*", "a/.b", true),
            ("lib/*", "lib/.b", true),
            ("lib/*", ".lib/b", false),
            ("**/*", "x/.a/b", false),
            ("?{a,b}", ".a", true),
            ("[!a][^b]", ".a", true),
            ("[/{,a}[a-b]", "b", true),
            ("a[.**/[^b]", "ab", true),
            ("*a/*", ".a/b", false),
            ("**/**/[!a][ab]", "a/.b", true),
            ("lib/.*", "lib/.b", true),
            ("lib/**/x", "lib/.d/x", false),
        ];
        let wrong: Vec<_> = (cases.iter())
            .filter(|(p, s, want)| glob(p, s) != *want)
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn globs_built_to_backtrack_match_without_it() {
        let stars = format!("{}b", "*a".repeat(16));
        assert!(!glob(&stars, &"a".repeat(64)));
        assert!(glob(&stars, &format!("{}b", "a".repeat(64))));
        let deep = format!("{}x", "**/".repeat(16));
        assert!(!glob(&deep, &"a/".repeat(40)));
        assert!(glob(&deep, &format!("{}x", "a/".repeat(40))));
        assert_eq!(
            alternatives("{a,b}{c,d}").unwrap(),
            ["ac", "ad", "bc", "bd"]
        );
        assert_eq!(alternatives(&"{a,b}".repeat(9)), None);
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
