//! Ruby's norms, observed on a prism parse; RuboCop's cops name them. Each
//! norm is a module with `NORM`, `STYLES` and a `see` the walk calls per node.

mod config;
mod strings;

use super::{Analysis, Language, LineIndex, NormDef, Observation, Replace, Scope, ScopeKind};
use ruby_prism::{Location, Node, Visit};
use std::collections::BTreeMap;
use std::path::Path;

pub struct Ruby;

static NORMS: &[NormDef] = &[NormDef {
    id: strings::NORM,
    styles: strings::STYLES,
}];

/// Per-node observers, in `NORMS` order.
static OBSERVERS: &[fn(&mut Walker, &Node)] = &[strings::see];

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
            index: &index,
            observations: Vec::new(),
            scopes: Vec::new(),
            embedded: Vec::new(),
        };
        walker.visit(&result.node());
        Analysis {
            observations: walker.observations,
            scopes: walker.scopes,
            errors: result.errors().next().is_some(),
            lines: index.count(),
        }
    }

    fn configured(&self, dir: &Path, root: &Path) -> BTreeMap<&'static str, String> {
        let Some(config) = config::find(dir, root).map(|p| config::load(&p)) else {
            return BTreeMap::new();
        };
        let mut styles = BTreeMap::new();
        let style = config.get(strings::NORM, "EnforcedStyle");
        if let Some(s @ ("single_quotes" | "double_quotes")) = style {
            styles.insert(strings::NORM, s.to_string());
        }
        styles
    }
}

pub struct Walker<'a> {
    pub index: &'a LineIndex,
    observations: Vec<Observation>,
    scopes: Vec<Scope>,
    /// Per open branch node: whether it is an interpolation (`#{}`).
    pub embedded: Vec<bool>,
}

impl<'pr> Visit<'pr> for Walker<'_> {
    fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
        self.see(&node);
        self.embedded
            .push(node.as_embedded_statements_node().is_some());
    }

    fn visit_branch_node_leave(&mut self) {
        self.embedded.pop();
    }

    fn visit_leaf_node_enter(&mut self, node: Node<'pr>) {
        self.see(&node);
    }
}

impl Walker<'_> {
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
