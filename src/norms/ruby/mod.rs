//! Ruby's norms, observed on a prism parse; RuboCop's cops name them. Each
//! norm is a module with `NORM`, `STYLES` and a `see` the walk calls per node.

mod braces;
mod commas;
mod config;
mod def_spacing;
mod final_newline;
mod hash_syntax;
mod indentation;
mod naming;
mod operators;
mod strings;
mod whitespace;
mod width;

use super::editorconfig;
use super::{
    Analysis, Configured, Language, LineIndex, NormDef, Observation, Replace, Scope, ScopeKind,
    Snapshot,
};
use crate::frontend::ruby::{Entered, generated};
use ruby_prism::{Location, Node, Visit};
use std::collections::{BTreeMap, BTreeSet};

pub struct Ruby;

static NORMS: &[NormDef] = &[
    NormDef {
        id: strings::NORM,
        styles: strings::STYLES,
        renames: false,
        indents: false,
        shared: false,
    },
    NormDef {
        id: braces::NORM,
        styles: braces::STYLES,
        renames: false,
        indents: false,
        shared: false,
    },
    NormDef {
        id: commas::ARRAY,
        styles: commas::STYLES,
        renames: false,
        indents: false,
        shared: false,
    },
    NormDef {
        id: commas::HASH,
        styles: commas::STYLES,
        renames: false,
        indents: false,
        shared: false,
    },
    NormDef {
        id: commas::ARGUMENTS,
        styles: commas::STYLES,
        renames: false,
        indents: false,
        shared: false,
    },
    NormDef {
        id: hash_syntax::NORM,
        styles: hash_syntax::STYLES,
        renames: false,
        indents: false,
        shared: false,
    },
    NormDef {
        id: indentation::NORM,
        styles: indentation::STYLES,
        renames: false,
        indents: true,
        shared: false,
    },
    NormDef {
        id: whitespace::NORM,
        styles: whitespace::STYLES,
        renames: false,
        indents: false,
        shared: false,
    },
    NormDef {
        id: final_newline::NORM,
        styles: final_newline::STYLES,
        renames: false,
        indents: false,
        shared: false,
    },
    NormDef {
        id: width::NORM,
        styles: width::STYLES,
        renames: false,
        indents: true,
        shared: false,
    },
    NormDef {
        id: naming::METHOD,
        styles: naming::STYLES,
        renames: true,
        indents: false,
        shared: true,
    },
    NormDef {
        id: naming::VARIABLE,
        styles: naming::STYLES,
        renames: true,
        indents: false,
        shared: false,
    },
    NormDef {
        id: operators::NORM,
        styles: operators::STYLES,
        renames: false,
        indents: false,
        shared: false,
    },
    NormDef {
        id: def_spacing::NORM,
        styles: def_spacing::STYLES,
        renames: false,
        indents: false,
        shared: false,
    },
];

/// Per-node observers.
static OBSERVERS: &[fn(&mut Walker, &Node)] = &[
    strings::see,
    braces::see,
    commas::see_array,
    commas::see_hash,
    commas::see_arguments,
    hash_syntax::see,
    indentation::see,
    whitespace::see,
    width::see,
    naming::see,
    naming::see_variables,
    operators::see,
    def_spacing::see,
];

/// A norm's RuboCop setting, and the styles its values map to: a value
/// not listed sets no style.
struct Setting {
    norm: &'static str,
    key: &'static str,
    values: &'static [(&'static str, &'static str)],
}

static SETTINGS: &[Setting] = &[
    Setting {
        norm: strings::NORM,
        key: "EnforcedStyle",
        values: &[
            ("single_quotes", "single_quotes"),
            ("double_quotes", "double_quotes"),
        ],
    },
    Setting {
        norm: braces::NORM,
        key: "EnforcedStyle",
        // `compact` differs from `space` only between nested braces.
        values: &[
            ("space", "space"),
            ("no_space", "no_space"),
            ("compact", "space"),
        ],
    },
    Setting {
        norm: commas::ARRAY,
        key: "EnforcedStyleForMultiline",
        values: COMMA_VALUES,
    },
    Setting {
        norm: commas::HASH,
        key: "EnforcedStyleForMultiline",
        values: COMMA_VALUES,
    },
    Setting {
        norm: commas::ARGUMENTS,
        key: "EnforcedStyleForMultiline",
        values: COMMA_VALUES,
    },
    Setting {
        norm: hash_syntax::NORM,
        key: "EnforcedStyle",
        // `no_mixed_keys` constrains a hash's pairs to agree, not a syntax.
        values: &[
            ("ruby19", "ruby19"),
            ("ruby19_no_mixed_keys", "ruby19"),
            ("hash_rockets", "hash_rockets"),
        ],
    },
    Setting {
        norm: indentation::NORM,
        key: "EnforcedStyle",
        values: &[
            ("normal", "normal"),
            ("indented_internal_methods", "indented_internal_methods"),
        ],
    },
    Setting {
        norm: whitespace::NORM,
        key: "Enabled",
        values: &[("true", "no_trailing_whitespace")],
    },
    Setting {
        norm: final_newline::NORM,
        key: "EnforcedStyle",
        values: &[
            ("final_newline", "final_newline"),
            ("final_blank_line", "final_blank_line"),
        ],
    },
    Setting {
        norm: width::NORM,
        key: "Width",
        values: ANY_NUMBER,
    },
    Setting {
        norm: naming::METHOD,
        key: "EnforcedStyle",
        values: NAMING_VALUES,
    },
    Setting {
        norm: naming::VARIABLE,
        key: "EnforcedStyle",
        values: NAMING_VALUES,
    },
    Setting {
        norm: operators::NORM,
        key: "Enabled",
        values: &[("true", "space")],
    },
    Setting {
        norm: def_spacing::NORM,
        key: "Enabled",
        values: &[("true", "empty_line")],
    },
];

const NAMING_VALUES: &[(&str, &str)] = &[("snake_case", "snake_case"), ("camelCase", "camelCase")];

/// A setting whose value, a number, is the style.
const ANY_NUMBER: &[(&str, &str)] = &[("*", "*")];

/// Every comma style wants one where commas are observed: each item on its
/// own line.
const COMMA_VALUES: &[(&str, &str)] = &[
    ("comma", "comma"),
    ("consistent_comma", "comma"),
    ("diff_comma", "comma"),
    ("no_comma", "no_comma"),
];

impl Language for Ruby {
    fn name(&self) -> &'static str {
        "ruby"
    }

    fn handles(&self, path: &str) -> bool {
        crate::frontend::for_path(path).is_some_and(|f| f.language() == "ruby")
    }

    fn norms(&self) -> &'static [NormDef] {
        NORMS
    }

    fn analyze(&self, source: &[u8]) -> Analysis {
        let result = ruby_prism::parse(source);
        let index = LineIndex::new(source);
        let mut walker = Walker {
            source,
            index: &index,
            observations: Vec::new(),
            scopes: Vec::new(),
            embedded: Vec::new(),
            literals: Vec::new(),
            names: naming::Names::default(),
            entered: Entered::default(),
        };
        walker.visit(&result.node());
        whitespace::lines(&mut walker);
        final_newline::file(&mut walker);
        naming::methods(&mut walker);
        naming::variables(&mut walker);
        Analysis {
            observations: walker.observations,
            scopes: walker.scopes,
            disabled: directives(source, &result, &index),
            errors: result.errors().next().is_some(),
            lines: index.count(),
        }
    }

    /// `.editorconfig`, then `.rubocop.yml`: where both set a norm, the
    /// Ruby-specific file wins.
    fn configured(&self, path: &str, snapshot: &dyn Snapshot) -> Configured {
        let mut styles = from_editorconfig(&editorconfig::properties(path, snapshot));
        let Some(config) = config::find(path, snapshot).map(|p| config::load(&p, snapshot)) else {
            return Configured {
                styles,
                ..Configured::default()
            };
        };
        // A file RuboCop skips for a cop has no configured style there.
        let off: BTreeSet<&'static str> = (NORMS.iter())
            .map(|n| n.id)
            .filter(|id| !config.inspects(id, path))
            .collect();
        styles.retain(|id, _| !off.contains(id));
        for setting in SETTINGS {
            let value = config.get(setting.norm, setting.key);
            let style = match value {
                Some(n) if setting.values == ANY_NUMBER => {
                    n.parse::<u32>().ok().map(|n| n.to_string())
                }
                _ => (setting.values.iter())
                    .find(|(v, _)| Some(*v) == value)
                    .map(|(_, style)| style.to_string()),
            };
            if let (Some(style), false) = (style, off.contains(setting.norm)) {
                styles.insert(setting.norm, style);
            }
        }
        // `Enabled` sets one blank line unless `NumberOfEmptyLines` says
        // otherwise: two or more is `empty_lines`; a range sets no style.
        match config.get(def_spacing::NORM, "NumberOfEmptyLines") {
            None | Some("1") => {}
            Some(n) if n.parse::<u32>().is_ok_and(|n| n > 1) => {
                if styles.contains_key(def_spacing::NORM) {
                    styles.insert(def_spacing::NORM, "empty_lines".to_string());
                }
            }
            Some(_) => {
                styles.remove(def_spacing::NORM);
            }
        }
        Configured { styles, off }
    }
}

/// The norms `.editorconfig` properties set.
fn from_editorconfig(properties: &BTreeMap<String, String>) -> BTreeMap<&'static str, String> {
    let mut styles = BTreeMap::new();
    let get = |key: &str| properties.get(key).map(String::as_str);
    // `false` only stops an editor trimming; it asks for no whitespace.
    if get("trim_trailing_whitespace") == Some("true") {
        styles.insert(whitespace::NORM, "no_trailing_whitespace".to_string());
    }
    // A tab-indented file measures in tabs, which this norm doesn't.
    let size = match get("indent_size") {
        Some("tab") => get("tab_width"),
        size => size,
    };
    if let (false, Some(size)) = (get("indent_style") == Some("tab"), size)
        && let Ok(size) = size.parse::<u32>()
    {
        styles.insert(width::NORM, size.to_string());
    }
    match get("insert_final_newline") {
        Some("true") => styles.insert(final_newline::NORM, "final_newline".to_string()),
        Some("false") => styles.insert(final_newline::NORM, "no_final_newline".to_string()),
        _ => None,
    };
    styles
}

/// The lines each `# rubocop:disable` or `# rubocop:todo` turns its cops
/// off for: alone on its line, to the matching `# rubocop:enable` or the
/// file's end; after code, that line.
fn directives(
    source: &[u8],
    result: &ruby_prism::ParseResult,
    index: &LineIndex,
) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut open: Vec<(String, usize)> = Vec::new();
    for comment in result.comments() {
        let loc = comment.location();
        let text = String::from_utf8_lossy(&source[loc.start_offset()..loc.end_offset()]);
        let Some(rest) = text.trim_start_matches('#').trim().strip_prefix("rubocop:") else {
            continue;
        };
        let (mode, cops) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        // RuboCop reads a ` -- ` and what follows as the reason.
        let cops = cops.split(" --").next().unwrap_or_default();
        let cops = cops.split(',').map(str::trim).filter(|c| !c.is_empty());
        let line = index.line(loc.start_offset());
        let alone = source[index.start(line)..loc.start_offset()]
            .iter()
            .all(u8::is_ascii_whitespace);
        match mode {
            "disable" | "todo" if alone => open.extend(cops.map(|c| (c.to_string(), line))),
            "disable" | "todo" => out.extend(cops.map(|c| (c.to_string(), line, line))),
            "enable" => {
                for cop in cops {
                    let (closed, kept) =
                        open.drain(..).partition(|(c, _)| cop == "all" || c == cop);
                    open = kept;
                    out.extend(
                        closed
                            .into_iter()
                            .map(|(c, first): (String, usize)| (c, first, line)),
                    );
                }
            }
            _ => {}
        }
    }
    let last = index.count().max(1);
    out.extend(open.into_iter().map(|(c, first)| (c, first, last)));
    out
}

pub struct Walker<'a> {
    pub source: &'a [u8],
    pub index: &'a LineIndex,
    observations: Vec<Observation>,
    scopes: Vec<Scope>,
    /// Per open branch node: whether it is an interpolation (`#{}`).
    pub embedded: Vec<bool>,
    /// Byte ranges of string and heredoc contents.
    pub literals: Vec<std::ops::Range<usize>>,
    names: naming::Names,
    entered: Entered,
}

impl<'pr> Visit<'pr> for Walker<'_> {
    generated::concrete_visits!('pr);

    fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
        self.entered.enter(&node);
        self.see(&node);
        self.embedded
            .push(node.as_embedded_statements_node().is_some());
    }

    fn visit_branch_node_leave(&mut self) {
        self.embedded.pop();
    }

    fn visit_leaf_node_enter(&mut self, node: Node<'pr>) {
        self.entered.enter(&node);
        self.see(&node);
    }
}

impl Walker<'_> {
    fn dispatched(&mut self, node: &Node<'_>) -> bool {
        self.entered.dispatched(node)
    }

    fn see(&mut self, node: &Node<'_>) {
        let kind = if node.as_def_node().is_some() {
            Some(ScopeKind::Method)
        } else if node.as_class_node().is_some()
            || node.as_module_node().is_some()
            || node.as_singleton_class_node().is_some()
        {
            Some(ScopeKind::Class)
        } else {
            None
        };
        if let Some(kind) = kind {
            let loc = node.location();
            self.scopes.push(Scope {
                kind,
                first: self.index.line(loc.start_offset()),
                last: self.last_line(&loc),
            });
        }
        for observe in OBSERVERS {
            observe(self, node);
        }
    }

    /// The line a location ends on.
    pub fn last_line(&self, loc: &Location) -> usize {
        self.index
            .line(loc.end_offset().saturating_sub(1).max(loc.start_offset()))
    }

    pub fn push(
        &mut self,
        norm: &'static str,
        style: &str,
        at: usize,
        lines: (usize, usize),
        key: String,
        fix: Vec<(String, Vec<Replace>)>,
    ) {
        self.observations.push(Observation {
            norm,
            style: style.to_string(),
            lines,
            line: self.index.line(at),
            column: self.index.column(at),
            key,
            fix,
        });
    }
}

pub fn replace(loc: &Location, text: &str) -> Replace {
    Replace {
        span: loc.start_offset()..loc.end_offset(),
        text: text.to_string(),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// One norm's observations in `src`, as `style@line` words.
    pub fn styles(src: &str, norm: &str) -> String {
        let words: Vec<String> = (Ruby.analyze(src.as_bytes()).observations.iter())
            .filter(|o| o.norm == norm)
            .map(|o| format!("{}@{}", o.style, o.line))
            .collect();
        words.join(" ")
    }

    #[test]
    fn rubocop_directives_turn_checks_off_by_line() {
        let analysis = Ruby.analyze(
            b"a = 1 # rubocop:disable Style/StringLiterals -- reason\n\
              # rubocop:disable all\nb = 2\n# rubocop:enable all\n\
              # rubocop:todo Naming, Layout/TrailingWhitespace\nc = 3\n",
        );
        let off = |norm, line| analysis.disabled(norm, line);
        assert!(off("Style/StringLiterals", 1));
        assert!(!off("Style/HashSyntax", 1));
        assert!(off("Style/HashSyntax", 3));
        assert!(!off("Style/StringLiterals", 5));
        assert!(off("Naming/MethodName", 6));
        assert!(off("Layout/TrailingWhitespace", 6));
        assert!(!off("Layout/IndentationWidth", 6));
    }

    #[test]
    fn methods_and_classes_are_scopes() {
        let analysis = Ruby.analyze(b"class A\n  def b\n    1\n  end\nend\n");
        let scopes: Vec<(ScopeKind, usize, usize)> = analysis
            .scopes
            .iter()
            .map(|s| (s.kind, s.first, s.last))
            .collect();
        assert_eq!(
            scopes,
            [(ScopeKind::Class, 1, 5), (ScopeKind::Method, 2, 4)]
        );
    }
}
