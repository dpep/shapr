//! The `.editorconfig` properties for a file (editorconfig.org): every
//! `.editorconfig` from the file's directory up to the repository's top, or
//! to the first that says `root = true`; nearer files and later sections
//! win. Section globs follow the spec: `*`, `**`, `?`, `[...]`, `[!...]`,
//! `{a,b}` and `{1..3}` (an integer in the range); a glob without `/`
//! matches the name at any depth.

use super::snapshot::{Snapshot, join, parent};
use std::collections::BTreeMap;

const NAME: &str = ".editorconfig";

/// The longest section glob read: far past a real one, and a bound on the
/// matcher's work.
const MAX_GLOB: usize = 1024;

/// What the `.editorconfig` files above a file set for it.
#[derive(Default)]
pub struct Properties {
    /// By lowercased key; `unset` removes one.
    pub values: BTreeMap<String, String>,
    /// What wasn't read, one note each: a section past the glob cap.
    pub unread: Vec<String>,
}

pub fn properties(path: &str, snapshot: &dyn Snapshot) -> Properties {
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
    let mut out = Properties::default();
    for (dir, file) in files.iter().rev() {
        let relative = match dir.as_str() {
            "" => path,
            dir => &path[dir.len() + 1..],
        };
        if !file.long.is_empty() {
            let lines: Vec<String> = file.long.iter().map(usize::to_string).collect();
            let s = if lines.len() == 1 { "" } else { "s" };
            out.unread.push(format!(
                "{}: line{s} {} not read (a section glob over {MAX_GLOB} characters)",
                join(dir, NAME),
                lines.join(", ")
            ));
        }
        for (glob, props) in &file.sections {
            if matches(glob, relative) {
                for (key, value) in props {
                    if value == "unset" {
                        out.values.remove(key);
                    } else {
                        out.values.insert(key.clone(), value.clone());
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
    /// Lines of sections past [`MAX_GLOB`], 1-based: ignored with their
    /// properties.
    long: Vec<usize>,
}

fn parse(text: &str) -> File {
    let mut file = File {
        root: false,
        sections: Vec::new(),
        long: Vec::new(),
    };
    // Properties of a section past the cap go nowhere.
    let mut skipping = false;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(glob) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            skipping = glob.chars().count() > MAX_GLOB;
            if skipping {
                file.long.push(n + 1);
            } else {
                file.sections.push((glob.to_string(), Vec::new()));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim().to_lowercase(), value.trim().to_lowercase());
        match file.sections.last_mut() {
            _ if skipping => {}
            Some((_, props)) => props.push((key, value)),
            None if key == "root" => file.root = value == "true",
            None => {}
        }
    }
    file
}

/// A section glob against a path relative to its `.editorconfig`.
fn matches(glob: &str, path: &str) -> bool {
    let glob: Vec<char> = match glob.strip_prefix('/') {
        Some(rooted) => rooted.chars().collect(),
        None if !glob.contains('/') => format!("**/{glob}").chars().collect(),
        None => glob.chars().collect(),
    };
    let path: Vec<char> = path.chars().collect();
    // `**/x` also matches `x` at the top.
    run(&compile(&glob), &path)
        || (glob.starts_with(&['*', '*', '/']) && run(&compile(&glob[3..]), &path))
}

/// A glob compiled to a program for [`run`]: each instruction consumes
/// one character, or a number, or branches.
#[derive(Debug)]
enum Inst {
    Char(char),
    /// Any one character but `/`.
    Any,
    /// Any one character.
    AnyAll,
    /// `[...]`: ranges, negated or not; never `/`.
    Class(bool, Vec<(char, char)>),
    /// `{lo..hi}`: an integer between them.
    Number(i64, i64),
    Split(usize, usize),
    Jump(usize),
    Match,
}

fn compile(glob: &[char]) -> Vec<Inst> {
    let mut prog = Vec::new();
    sequence(glob, &mut prog);
    prog.push(Inst::Match);
    prog
}

fn sequence(glob: &[char], prog: &mut Vec<Inst>) {
    let mut i = 0;
    while i < glob.len() {
        let c = glob[i];
        i += 1;
        match c {
            '*' => {
                let any = if glob.get(i) == Some(&'*') {
                    i += 1;
                    Inst::AnyAll
                } else {
                    Inst::Any
                };
                let at = prog.len();
                prog.extend([Inst::Split(at + 1, at + 3), any, Inst::Jump(at)]);
            }
            '?' => prog.push(Inst::Any),
            '[' => match class(&glob[i..]) {
                Some((inst, len)) => {
                    prog.push(inst);
                    i += len;
                }
                None => prog.push(Inst::Char('[')),
            },
            '{' => match closing(&glob[i..]) {
                Some(len) => {
                    let inner = &glob[i..i + len];
                    i += len + 1;
                    match range(inner) {
                        Some((a, b)) => prog.push(Inst::Number(a.min(b), a.max(b))),
                        None => alternatives(&split_top(inner), prog),
                    }
                }
                None => prog.push(Inst::Char('{')),
            },
            c => prog.push(Inst::Char(c)),
        }
    }
}

/// `{a,b}`: each alternative, then on past them all.
fn alternatives(options: &[&[char]], prog: &mut Vec<Inst>) {
    let mut ends = Vec::new();
    for (k, option) in options.iter().enumerate() {
        let last = k + 1 == options.len();
        let split = prog.len();
        if !last {
            prog.push(Inst::Split(split + 1, 0));
        }
        sequence(option, prog);
        if !last {
            ends.push(prog.len());
            prog.push(Inst::Jump(0));
            prog[split] = Inst::Split(split + 1, prog.len());
        }
    }
    let end = prog.len();
    for at in ends {
        prog[at] = Inst::Jump(end);
    }
}

/// The length of a brace's contents, nested braces included; `None`
/// unclosed.
fn closing(glob: &[char]) -> Option<usize> {
    let mut depth = 0;
    for (i, &c) in glob.iter().enumerate() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// `n..m`, both integers.
fn range(inner: &[char]) -> Option<(i64, i64)> {
    let inner: String = inner.iter().collect();
    let (a, b) = inner.split_once("..")?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// `a,b{c,d}` split at its top-level commas.
fn split_top(inner: &[char]) -> Vec<&[char]> {
    let mut out = Vec::new();
    let (mut depth, mut from) = (0, 0);
    for (i, &c) in inner.iter().enumerate() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&inner[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    out.push(&inner[from..]);
    out
}

/// A `[...]` after its `[`, and the characters it spans, `]` included;
/// `None` when unclosed (a literal `[`).
fn class(glob: &[char]) -> Option<(Inst, usize)> {
    let end = glob.iter().position(|&c| c == ']')?;
    let (negate, set) = match glob[..end].split_first() {
        Some(('!', rest)) => (true, rest),
        _ => (false, &glob[..end]),
    };
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < set.len() {
        if i + 2 < set.len() && set[i + 1] == '-' {
            ranges.push((set[i], set[i + 2]));
            i += 3;
        } else {
            ranges.push((set[i], set[i]));
            i += 1;
        }
    }
    Some((Inst::Class(negate, ranges), end + 1))
}

/// Whether `prog` matches all of `path`: every thread advanced together,
/// so no glob backtracks.
fn run(prog: &[Inst], path: &[char]) -> bool {
    let n = path.len();
    let mut pending: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
    pending[0].push(0);
    // The position each instruction was last reached at.
    let mut seen = vec![usize::MAX; prog.len()];
    for i in 0..=n {
        let mut stack = std::mem::take(&mut pending[i]);
        while let Some(s) = stack.pop() {
            if seen[s] == i {
                continue;
            }
            seen[s] = i;
            let c = path.get(i).copied();
            let step = match &prog[s] {
                Inst::Split(a, b) => {
                    stack.extend([*b, *a]);
                    false
                }
                Inst::Jump(a) => {
                    stack.push(*a);
                    false
                }
                Inst::Match if i == n => return true,
                Inst::Match => false,
                Inst::Char(want) => c == Some(*want),
                Inst::Any => c.is_some_and(|c| c != '/'),
                Inst::AnyAll => c.is_some(),
                Inst::Class(negate, ranges) => c.is_some_and(|c| {
                    c != '/' && ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c)) != *negate
                }),
                Inst::Number(lo, hi) => {
                    for (end, value) in numbers(&path[i..]) {
                        if (*lo..=*hi).contains(&value) {
                            pending[i + end].push(s + 1);
                        }
                    }
                    false
                }
            };
            if step {
                pending[i + 1].push(s + 1);
            }
        }
    }
    false
}

/// Each integer `[+-]?[0-9]+` that `path` starts with, by its length,
/// up to the first that doesn't fit an `i64`.
fn numbers(path: &[char]) -> Vec<(usize, i64)> {
    let negative = path.first() == Some(&'-');
    let sign = usize::from(matches!(path.first(), Some('+' | '-')));
    let mut value: i64 = 0;
    let mut out = Vec::new();
    for (k, c) in path[sign..].iter().enumerate() {
        let Some(digit) = c.to_digit(10).map(i64::from) else {
            break;
        };
        let next = value.checked_mul(10).and_then(|v| match negative {
            true => v.checked_sub(digit),
            false => v.checked_add(digit),
        });
        let Some(next) = next else { break };
        value = next;
        out.push((sign + k + 1, value));
    }
    out
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
        assert!(matches("v{-3..+3}.rb", "v-3.rb"));
        assert!(matches("v{-3..+3}.rb", "v+2.rb"));
        assert!(!matches("v{-3..+3}.rb", "v4.rb"));
        assert!(matches("v{1..3000}.rb", "v2999.rb"));
        let widest = "{-9223372036854775808..9223372036854775807}";
        assert!(matches(
            &format!("v{widest}.rb"),
            "v-9223372036854775808.rb"
        ));
        assert!(!matches(
            &format!("v{widest}.rb"),
            "v9223372036854775808.rb"
        ));
        assert!(matches("{0..999}{0..999}{0..999}", "1234"));
        assert!(!matches("{0..999}{0..999}", "12a"));
        assert!(matches("{a,{b,c}d}.rb", "cd.rb"));
        assert!(!matches("{a,{b,c}d}.rb", "c.rb"));
        assert!(matches(
            &format!("{}b", "*a".repeat(16)),
            &format!("{}b", "a".repeat(64))
        ));
        assert!(!matches(&format!("{}b", "*a".repeat(16)), &"a".repeat(64)));
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
        let lib = properties("lib/a.rb", &snapshot).values;
        assert_eq!(lib.get("indent_size").map(String::as_str), Some("2"));
        assert_eq!(lib.get("insert_final_newline"), None);
        let vendor = properties("vendor/x.rb", &snapshot).values;
        assert_eq!(vendor.get("indent_size"), None);
        assert_eq!(
            vendor.get("trim_trailing_whitespace").map(String::as_str),
            Some("false")
        );
    }
}
