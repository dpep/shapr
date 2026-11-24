//! Ruby's norms, observed on a prism parse; RuboCop's cops name them.

mod config;

use super::{Analysis, Language, LineIndex, NormDef, Observation, Replace, Scope, ScopeKind};
use ruby_prism::{Location, Node, Visit};
use std::collections::BTreeMap;
use std::path::Path;

pub struct Ruby;

const STRING_LITERALS: &str = "Style/StringLiterals";

static NORMS: &[NormDef] = &[NormDef {
    id: STRING_LITERALS,
    styles: &["single_quotes", "double_quotes"],
}];

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
        let style = config.get(STRING_LITERALS, "EnforcedStyle");
        if let Some(s @ ("single_quotes" | "double_quotes")) = style {
            styles.insert(STRING_LITERALS, s.to_string());
        }
        styles
    }
}

struct Walker<'a> {
    index: &'a LineIndex,
    observations: Vec<Observation>,
    scopes: Vec<Scope>,
    /// Per open branch node: whether it is an interpolation (`#{}`).
    embedded: Vec<bool>,
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
                last: self.index.line(loc.end_offset().saturating_sub(1)),
            });
        }
        if let Some(string) = node.as_string_node() {
            self.string(
                string.opening_loc(),
                string.content_loc(),
                string.closing_loc(),
            );
        }
    }

    /// Style/StringLiterals: a quoted string that could be written either
    /// way with the same value. Strings in an interpolation are another cop's.
    fn string(&mut self, opening: Option<Location>, content: Location, closing: Option<Location>) {
        let (Some(opening), Some(closing)) = (opening, closing) else {
            return;
        };
        if self.embedded.iter().any(|&e| e) {
            return;
        }
        let raw = content.as_slice();
        let (style, other, quote) = match opening.as_slice() {
            // No escapes, `"` or `#{`, `#$`, `#@` (an interpolation in `"`).
            b"'" if !raw.iter().any(|b| matches!(b, b'\\' | b'"'))
                && !raw
                    .windows(2)
                    .any(|w| w[0] == b'#' && matches!(w[1], b'{' | b'$' | b'@')) =>
            {
                ("single_quotes", "double_quotes", "\"")
            }
            b"\"" if !raw.iter().any(|b| matches!(b, b'\\' | b'\'')) => {
                ("double_quotes", "single_quotes", "'")
            }
            _ => return,
        };
        let at = opening.start_offset();
        let line = self.index.line(at);
        self.observations.push(Observation {
            norm: STRING_LITERALS,
            style: style.to_string(),
            lines: (line, line),
            line,
            column: self.index.column(at),
            key: String::from_utf8_lossy(raw).into_owned(),
            fix: vec![(
                other.to_string(),
                vec![replace(&opening, quote), replace(&closing, quote)],
            )],
        });
    }
}

fn replace(loc: &Location, text: &str) -> Replace {
    Replace {
        span: loc.start_offset()..loc.end_offset(),
        text: text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(src: &str) -> Vec<(String, usize)> {
        Ruby.analyze(src.as_bytes())
            .observations
            .into_iter()
            .map(|o| (o.style, o.line))
            .collect()
    }

    #[test]
    fn only_strings_that_could_take_either_quote_count() {
        let src = "a = 'x'\nb = \"y\"\nc = \"it's\"\nd = 'say \"hi\"'\ne = \"#{a} z\"\n\
                   f = \"tab\\t\"\ng = %q(x)\nh = 'a#b'\ni = \"#{'nested'}\"\nj = ?x\nk = 'a#{b}'\nl = 'a#@b'\n";
        assert_eq!(
            styles(src),
            [
                ("single_quotes".into(), 1),
                ("double_quotes".into(), 2),
                ("single_quotes".into(), 8)
            ]
        );
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
