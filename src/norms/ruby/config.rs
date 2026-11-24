//! The RuboCop config a file falls under: the nearest `.rubocop.yml`, with
//! the local files it `inherit_from`s. Only the YAML RuboCop configs use is
//! read: top-level cop names, their scalar settings, and `inherit_from`.
//! Gems (`inherit_gem`) and remote URLs are not followed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const NAME: &str = ".rubocop.yml";

/// Settings by cop, then by key; a later file overrides what it inherits.
#[derive(Debug, Default)]
pub struct Config(BTreeMap<String, BTreeMap<String, String>>);

impl Config {
    pub fn get(&self, cop: &str, key: &str) -> Option<&str> {
        let settings = self.0.get(cop)?;
        // A disabled cop sets no style.
        if settings.get("Enabled").map(String::as_str) == Some("false") {
            return None;
        }
        settings.get(key).map(String::as_str)
    }
}

/// The `.rubocop.yml` nearest `dir`, looking no higher than `root`.
pub fn find(dir: &Path, root: &Path) -> Option<PathBuf> {
    let mut dir = Some(dir);
    while let Some(d) = dir {
        let candidate = d.join(NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if d == root {
            break;
        }
        dir = d.parent();
    }
    None
}

pub fn load(path: &Path) -> Config {
    let mut config = Config::default();
    merge(path, &mut config, 0);
    config
}

fn merge(path: &Path, config: &mut Config, depth: usize) {
    // Deep enough for any real chain; a cycle stops here.
    if depth > 8 {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let (inherits, own) = parse(&text);
    let base = path.parent().unwrap_or(Path::new("."));
    for file in inherits {
        if !file.contains("://") {
            merge(&base.join(file), config, depth + 1);
        }
    }
    for (cop, settings) in own.0 {
        config.0.entry(cop).or_default().extend(settings);
    }
}

/// `inherit_from` entries and the cops' scalar settings.
fn parse(text: &str) -> (Vec<String>, Config) {
    let mut inherits = Vec::new();
    let mut config = Config::default();
    let mut cop: Option<String> = None;
    let mut in_inherit = false;
    // The indentation of a cop's settings; deeper lines are nested values.
    let mut indent: Option<usize> = None;
    for raw in text.lines() {
        let line = strip_comment(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "---" {
            continue;
        }
        let depth = line.len() - line.trim_start().len();
        if depth == 0 {
            let (key, value) = split(trimmed);
            in_inherit = key == "inherit_from";
            cop = None;
            indent = None;
            if in_inherit {
                inherits.extend(list(value));
            } else if value.is_empty() {
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
        let Some(cop) = &cop else { continue };
        if *indent.get_or_insert(depth) != depth {
            continue;
        }
        let (key, value) = split(trimmed);
        if !value.is_empty() {
            config
                .0
                .entry(cop.clone())
                .or_default()
                .insert(key.to_string(), unquote(value).to_string());
        }
    }
    (inherits, config)
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
        );
        assert_eq!(inherits, [".rubocop_todo.yml"]);
        assert_eq!(
            config.get("Style/StringLiterals", "EnforcedStyle"),
            Some("double_quotes")
        );
        assert_eq!(config.get("Style/HashSyntax", "EnforcedStyle"), None);
        assert_eq!(config.get("AllCops", "Exclude"), None);
        assert_eq!(
            parse("inherit_from: [a.yml, 'b.yml']\n").0,
            ["a.yml", "b.yml"]
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
        let sub = dir.path().join("lib");
        std::fs::create_dir(&sub).unwrap();
        let found = find(&sub, dir.path()).unwrap();
        assert_eq!(
            load(&found).get("Style/StringLiterals", "EnforcedStyle"),
            Some("double_quotes")
        );
    }
}
