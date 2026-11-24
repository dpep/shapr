//! The `.editorconfig` properties for a file (editorconfig.org): every
//! `.editorconfig` from the file's directory up to the repository's top, or
//! to the first that says `root = true`; nearer files and later sections
//! win. Section globs follow the spec: `*`, `**`, `?`, `[...]`, `[!...]`,
//! `{a,b}` and `{1..3}`; a glob without `/` matches the name at any depth.

use super::snapshot::{Snapshot, join, parent};
use std::collections::BTreeMap;

const NAME: &str = ".editorconfig";

/// Properties by lowercased key; `unset` removes one.
pub fn properties(path: &str, snapshot: &dyn Snapshot) -> BTreeMap<String, String> {
    let mut files = Vec::new();
    let mut dir = parent(path);
    loop {
        if let Some(text) = snapshot.read(&join(dir, NAME)) {
            let parsed = parse(&String::from_utf8_lossy(&text));
            let root = parsed.root;
            files.push((dir.to_string(), parsed));
            if root {
                break;
            }
        }
        if dir.is_empty() {
            break;
        }
        dir = parent(dir);
    }
    let mut out = BTreeMap::new();
    for (dir, file) in files.iter().rev() {
        let relative = match dir.as_str() {
            "" => path,
            dir => &path[dir.len() + 1..],
        };
        for (glob, props) in &file.sections {
            if matches(glob, relative) {
                for (key, value) in props {
                    if value == "unset" {
                        out.remove(key);
                    } else {
                        out.insert(key.clone(), value.clone());
                    }
                }
            }
        }
    }
    out
}

struct File {
    root: bool,
    sections: Vec<(String, Vec<(String, String)>)>,
}

fn parse(text: &str) -> File {
    let mut file = File {
        root: false,
        sections: Vec::new(),
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(glob) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            file.sections.push((glob.to_string(), Vec::new()));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim().to_lowercase(), value.trim().to_lowercase());
        match file.sections.last_mut() {
            Some((_, props)) => props.push((key, value)),
            None if key == "root" => file.root = value == "true",
            None => {}
        }
    }
    file
}

/// A section glob against a path relative to its `.editorconfig`.
fn matches(glob: &str, path: &str) -> bool {
    let glob = match glob.strip_prefix('/') {
        Some(rooted) => rooted.to_string(),
        None if !glob.contains('/') => format!("**/{glob}"),
        None => glob.to_string(),
    };
    expand(&glob).iter().any(|g| {
        let g: Vec<char> = g.chars().collect();
        let p: Vec<char> = path.chars().collect();
        // `**/x` also matches `x` at the top.
        wild(&g, &p) || (g.starts_with(&['*', '*', '/']) && wild(&g[3..], &p))
    })
}

/// The globs `{a,b}` and `{1..3}` stand for, braces expanded.
fn expand(glob: &str) -> Vec<String> {
    let chars: Vec<char> = glob.chars().collect();
    let Some(open) = chars.iter().position(|&c| c == '{') else {
        return vec![glob.to_string()];
    };
    let mut depth = 0;
    let mut close = None;
    for (i, &c) in chars.iter().enumerate().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return vec![glob.to_string()];
    };
    let head: String = chars[..open].iter().collect();
    let inner: String = chars[open + 1..close].iter().collect();
    let tail: String = chars[close + 1..].iter().collect();
    let options: Vec<String> = match inner
        .split_once("..")
        .map(|(a, b)| (a.parse::<i64>(), b.parse::<i64>()))
    {
        Some((Ok(a), Ok(b))) if (b - a).abs() < 1000 => {
            let (lo, hi) = (a.min(b), a.max(b));
            (lo..=hi).map(|n| n.to_string()).collect()
        }
        _ => split_top(&inner),
    };
    options
        .iter()
        .flat_map(|o| expand(&format!("{head}{o}{tail}")))
        .collect()
}

/// `a,b{c,d}` split at its top-level commas.
fn split_top(inner: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut depth = 0;
    for c in inner.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(String::new());
                continue;
            }
            _ => {}
        }
        out.last_mut().unwrap().push(c);
    }
    out
}

fn wild(g: &[char], p: &[char]) -> bool {
    match g.first() {
        None => p.is_empty(),
        Some('*') if g.get(1) == Some(&'*') => (0..=p.len()).any(|i| wild(&g[2..], &p[i..])),
        Some('*') => (0..=p.len())
            .take_while(|&i| i == 0 || p[i - 1] != '/')
            .any(|i| wild(&g[1..], &p[i..])),
        Some('?') => p.first().is_some_and(|&c| c != '/') && wild(&g[1..], &p[1..]),
        Some('[') => {
            let Some(end) = g.iter().position(|&c| c == ']') else {
                return p.first() == Some(&'[') && wild(&g[1..], &p[1..]);
            };
            let (negate, set) = match g[1..end].split_first() {
                Some(('!', rest)) => (true, rest),
                _ => (false, &g[1..end]),
            };
            let Some(&c) = p.first() else {
                return false;
            };
            let mut hit = false;
            let mut i = 0;
            while i < set.len() {
                if i + 2 < set.len() && set[i + 1] == '-' {
                    hit |= set[i] <= c && c <= set[i + 2];
                    i += 3;
                } else {
                    hit |= set[i] == c;
                    i += 1;
                }
            }
            hit != negate && c != '/' && wild(&g[end + 1..], &p[1..])
        }
        Some(&c) => p.first() == Some(&c) && wild(&g[1..], &p[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::norms::snapshot::Worktree;

    #[test]
    fn globs_match_as_the_spec_reads_them() {
        assert!(matches("*", "lib/a.rb"));
        assert!(matches("*.rb", "lib/deep/a.rb"));
        assert!(!matches("*.rb", "lib/a.rs"));
        assert!(matches("*.{rb,yml}", "a.yml"));
        assert!(matches("lib/*.rb", "lib/a.rb"));
        assert!(!matches("lib/*.rb", "lib/x/a.rb"));
        assert!(matches("lib/**.rb", "lib/x/a.rb"));
        assert!(matches("/Makefile", "Makefile"));
        assert!(!matches("/Makefile", "sub/Makefile"));
        assert!(matches("tests/norms/**", "tests/norms/case/a.rb"));
        assert!(matches("a[0-9].rb", "a5.rb"));
        assert!(!matches("a[!0-9].rb", "a5.rb"));
        assert!(matches("v{1..3}.rb", "v2.rb"));
        assert!(matches("?.rb", "x.rb"));
    }

    #[test]
    fn nearer_files_win_and_root_stops_the_climb() {
        let dir = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let full = dir.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, text).unwrap();
        };
        write(
            ".editorconfig",
            "[*]\nindent_size = 4\ninsert_final_newline = true\n",
        );
        write(
            "lib/.editorconfig",
            "[*.rb]\nindent_size = 2\n[a.rb]\ninsert_final_newline = unset\n",
        );
        write(
            "vendor/.editorconfig",
            "root = true\n[*]\ntrim_trailing_whitespace = false\n",
        );
        let snapshot = Worktree(dir.path().to_path_buf());
        let lib = properties("lib/a.rb", &snapshot);
        assert_eq!(lib.get("indent_size").map(String::as_str), Some("2"));
        assert_eq!(lib.get("insert_final_newline"), None);
        let vendor = properties("vendor/x.rb", &snapshot);
        assert_eq!(vendor.get("indent_size"), None);
        assert_eq!(
            vendor.get("trim_trailing_whitespace").map(String::as_str),
            Some("false")
        );
    }
}
