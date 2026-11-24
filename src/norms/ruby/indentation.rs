//! Layout/IndentationConsistency: methods after `private` or `protected`
//! at the modifier's column (`normal`) or a step deeper
//! (`indented_internal_methods`).

use super::Walker;
use crate::norms::Replace;
use ruby_prism::Node;

pub const NORM: &str = "Layout/IndentationConsistency";
pub const STYLES: &[&str] = &["normal", "indented_internal_methods"];

/// The step `indented_internal_methods` adds, for a suggested edit.
const STEP: usize = 2;

pub fn see(w: &mut Walker, node: &Node) {
    let body = if let Some(class) = node.as_class_node() {
        class.body()
    } else if let Some(module) = node.as_module_node() {
        module.body()
    } else if let Some(singleton) = node.as_singleton_class_node() {
        singleton.body()
    } else {
        return;
    };
    let Some(statements) = body.and_then(|b| b.as_statements_node()) else {
        return;
    };
    // The column of the bare `private`/`protected` the section follows: as
    // RuboCop reads it, a method's indentation is relative to its modifier.
    let mut section: Option<usize> = None;
    for statement in statements.body().iter() {
        if let Some(name) = modifier(&statement) {
            section = (name != b"public").then(|| column(w, statement.location().start_offset()));
            continue;
        }
        let (Some(base), Some(def)) = (section, statement.as_def_node()) else {
            continue;
        };
        let start = def.location().start_offset();
        let col = column(w, start);
        let style = match col.checked_sub(base) {
            Some(0) => "normal",
            Some(_) => "indented_internal_methods",
            None => continue,
        };
        let line = w.index.line(start);
        let last = w.last_line(&def.location());
        let text = &w.source[w.index.start(line)..def.location().end_offset()];
        let key = String::from_utf8_lossy(text)
            .lines()
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        let fix = shift(w, line, last, style, col - base);
        w.push(
            NORM,
            style,
            start,
            (line, line),
            key,
            fix.into_iter().collect(),
        );
    }
}

/// A bare `private`, `protected` or `public` on its own: its name.
fn modifier<'a>(node: &Node<'a>) -> Option<&'a [u8]> {
    let call = node.as_call_node()?;
    if call.receiver().is_some() || call.arguments().is_some() || call.block().is_some() {
        return None;
    }
    let name = call.name().as_slice();
    matches!(name, b"private" | b"protected" | b"public").then_some(name)
}

fn column(w: &Walker, offset: usize) -> usize {
    w.index.column(offset) - 1
}

/// The method's lines moved to the other style: a step in, or back to the
/// modifier's column. None when a line inside (a heredoc body) can't move.
fn shift(
    w: &Walker,
    first: usize,
    last: usize,
    style: &str,
    depth: usize,
) -> Option<(String, Vec<Replace>)> {
    let span = &w.source[w.index.start(first)..w.index.start(last + 1).min(w.source.len())];
    if span.windows(2).any(|p| p == b"<<") {
        return None;
    }
    let mut replaces = Vec::new();
    for line in first..=last {
        let start = w.index.start(line);
        let text = &w.source[start..w.index.start(line + 1).min(w.source.len())];
        if text.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let indent = text.iter().take_while(|&&b| b == b' ').count();
        match style {
            "normal" => replaces.push(Replace {
                span: start..start,
                text: " ".repeat(STEP),
            }),
            _ if indent >= depth => replaces.push(Replace {
                span: start..start + depth,
                text: String::new(),
            }),
            _ => return None,
        }
    }
    let other = if style == "normal" {
        "indented_internal_methods"
    } else {
        "normal"
    };
    Some((other.to_string(), replaces))
}

#[cfg(test)]
mod tests {
    use super::super::tests::styles;

    #[test]
    fn an_outdented_modifier_still_measures() {
        let src = "module A\n    def a; end\n\n  private\n\n    def b; end\nend\n";
        assert_eq!(styles(src, super::NORM), "indented_internal_methods@6");
    }

    #[test]
    fn methods_after_a_modifier_count() {
        let src = "class A\n  def a; end\n\n  private\n\n  def b; end\n\n  protected\n\n    def c\n    end\n\n  public\n\n  def d; end\nend\n";
        assert_eq!(
            styles(src, super::NORM),
            "normal@6 indented_internal_methods@10"
        );
    }
}
