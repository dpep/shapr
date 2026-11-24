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
    Analysis, Language, LineIndex, NormDef, Observation, Replace, Scope, ScopeKind, Snapshot,
};
use crate::frontend::ruby::{Entered, generated};
use ruby_prism::{Location, Node, Visit};
use std::collections::BTreeMap;

pub struct Ruby;

static NORMS: &[NormDef] = &[
    NormDef {
        id: strings::NORM,
        styles: strings::STYLES,
    },
    NormDef {
        id: braces::NORM,
        styles: braces::STYLES,
    },
    NormDef {
        id: commas::ARRAY,
        styles: commas::STYLES,
    },
    NormDef {
        id: commas::HASH,
        styles: commas::STYLES,
    },
    NormDef {
        id: commas::ARGUMENTS,
        styles: commas::STYLES,
    },
    NormDef {
        id: hash_syntax::NORM,
        styles: hash_syntax::STYLES,
    },
    NormDef {
        id: indentation::NORM,
        styles: indentation::STYLES,
    },
    NormDef {
        id: whitespace::NORM,
        styles: whitespace::STYLES,
    },
    NormDef {
        id: final_newline::NORM,
        styles: final_newline::STYLES,
    },
    NormDef {
        id: width::NORM,
        styles: width::STYLES,
    },
    NormDef {
        id: naming::METHOD,
        styles: naming::STYLES,
    },
    NormDef {
        id: naming::VARIABLE,
        styles: naming::STYLES,
    },
    NormDef {
        id: operators::NORM,
        styles: operators::STYLES,
    },
    NormDef {
        id: def_spacing::NORM,
        styles: def_spacing::STYLES,
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
            errors: result.errors().next().is_some(),
            lines: index.count(),
        }
    }

    /// `.editorconfig`, then `.rubocop.yml`: where both set a norm, the
    /// Ruby-specific file wins.
    fn configured(&self, path: &str, snapshot: &dyn Snapshot) -> BTreeMap<&'static str, String> {
        let mut styles = from_editorconfig(&editorconfig::properties(path, snapshot));
        let Some(config) = config::find(path, snapshot).map(|p| config::load(&p, snapshot)) else {
            return styles;
        };
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
            if let Some(style) = style {
                styles.insert(setting.norm, style);
            }
        }
        styles
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
