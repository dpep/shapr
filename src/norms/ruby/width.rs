//! Layout/IndentationWidth: how far a body sits in from what opens it, in
//! spaces (`2`, `4`, ...).

use super::Walker;
use ruby_prism::{Location, Node};

pub const NORM: &str = "Layout/IndentationWidth";
pub const STYLES: &[&str] = &["2", "4"];

/// A body's first statement, against what opens it: a `def`, `class`,
/// `module` or block from the start of its line; an `if`, `unless`,
/// `while`, `until`, `case`, `when`, `else`, `begin`, `rescue` or `ensure`
/// from its keyword, when the keyword starts its line.
pub fn see(w: &mut Walker, node: &Node) {
    let (opener, first, from_keyword) = if let Some(def) = node.as_def_node() {
        (
            def.def_keyword_loc(),
            def.body().and_then(|b| first_statement(&b)),
            false,
        )
    } else if let Some(class) = node.as_class_node() {
        (
            class.class_keyword_loc(),
            class.body().and_then(|b| first_statement(&b)),
            false,
        )
    } else if let Some(module) = node.as_module_node() {
        (
            module.module_keyword_loc(),
            module.body().and_then(|b| first_statement(&b)),
            false,
        )
    } else if let Some(singleton) = node.as_singleton_class_node() {
        (
            singleton.class_keyword_loc(),
            singleton.body().and_then(|b| first_statement(&b)),
            false,
        )
    } else if let Some(block) = node.as_block_node() {
        (
            block.opening_loc(),
            block.body().and_then(|b| first_statement(&b)),
            false,
        )
    } else if let Some(node_if) = node.as_if_node() {
        let Some(keyword) = node_if.if_keyword_loc() else {
            return;
        };
        // A modifier `if` has no `end`; `elsif` is its own IfNode.
        if node_if.end_keyword_loc().is_none() && keyword.as_slice() != b"elsif" {
            return;
        }
        (
            keyword,
            node_if.statements().and_then(|s| s.body().iter().next()),
            true,
        )
    } else if let Some(unless) = node.as_unless_node() {
        if unless.end_keyword_loc().is_none() {
            return;
        }
        (
            unless.keyword_loc(),
            unless.statements().and_then(|s| s.body().iter().next()),
            true,
        )
    } else if let Some(node_while) = node.as_while_node() {
        if node_while.closing_loc().is_none() {
            return;
        }
        (
            node_while.keyword_loc(),
            node_while.statements().and_then(|s| s.body().iter().next()),
            true,
        )
    } else if let Some(until) = node.as_until_node() {
        if until.closing_loc().is_none() {
            return;
        }
        (
            until.keyword_loc(),
            until.statements().and_then(|s| s.body().iter().next()),
            true,
        )
    } else if let Some(when) = node.as_when_node() {
        (
            when.keyword_loc(),
            when.statements().and_then(|s| s.body().iter().next()),
            true,
        )
    } else if let Some(node_else) = node.as_else_node() {
        (
            node_else.else_keyword_loc(),
            node_else.statements().and_then(|s| s.body().iter().next()),
            true,
        )
    } else if let Some(begin) = node.as_begin_node() {
        let Some(keyword) = begin.begin_keyword_loc() else {
            return;
        };
        (
            keyword,
            begin.statements().and_then(|s| s.body().iter().next()),
            true,
        )
    } else if let Some(rescue) = node.as_rescue_node() {
        (
            rescue.keyword_loc(),
            rescue.statements().and_then(|s| s.body().iter().next()),
            true,
        )
    } else if let Some(ensure) = node.as_ensure_node() {
        (
            ensure.ensure_keyword_loc(),
            ensure.statements().and_then(|s| s.body().iter().next()),
            true,
        )
    } else {
        return;
    };
    let Some(first) = first else {
        return;
    };
    observe(w, &opener, &first, from_keyword);
}

/// A body's first statement; a `def` or block body with `rescue` is a
/// `begin` whose statements come first.
fn first_statement<'pr>(body: &Node<'pr>) -> Option<Node<'pr>> {
    if let Some(statements) = body.as_statements_node() {
        return statements.body().iter().next();
    }
    if let Some(begin) = body.as_begin_node()
        && begin.begin_keyword_loc().is_none()
    {
        return begin.statements()?.body().iter().next();
    }
    None
}

fn observe(w: &mut Walker, opener: &Location, first: &Node, from_keyword: bool) {
    let open_line = w.index.line(opener.start_offset());
    let start = first.location().start_offset();
    let line = w.index.line(start);
    if line <= open_line {
        return;
    }
    let indent = |line: usize| {
        let from = w.index.start(line);
        let text = &w.source[from..];
        let spaces = text.iter().take_while(|&&b| b == b' ').count();
        // Tabs are Layout/IndentationStyle's.
        (text.get(spaces) != Some(&b'\t')).then_some(spaces)
    };
    let (Some(open_indent), Some(body_indent)) = (indent(open_line), indent(line)) else {
        return;
    };
    let base = if from_keyword {
        let column = w.index.column(opener.start_offset()) - 1;
        if column != open_indent {
            return;
        }
        column
    } else {
        open_indent
    };
    // The statement must start its line.
    if w.index.column(start) - 1 != body_indent {
        return;
    }
    let Some(width) = body_indent.checked_sub(base).filter(|&n| n > 0) else {
        return;
    };
    let text = &w.source[start..w.index.start(line + 1).min(w.source.len())];
    let key = String::from_utf8_lossy(text).trim().to_string();
    w.push(
        NORM,
        &width.to_string(),
        start,
        (line, line),
        key,
        Vec::new(),
    );
}

#[cfg(test)]
mod tests {
    use super::super::tests::styles;

    #[test]
    fn bodies_measure_from_their_opener() {
        let src = "class A\n  def b\n      c\n  end\n\n  def d\n    if e\n     f\n    else\n      g\n    end\n    x = if y\n          z\n        end\n    h.each do |i|\n        j\n    end\n  end\nend\n";
        assert_eq!(styles(src, super::NORM), "2@2 4@3 2@7 1@8 2@10 4@16");
    }

    #[test]
    fn rescue_and_ensure_bodies_count() {
        let src = "begin
  a
rescue
    b
ensure
   c
end
";
        assert_eq!(styles(src, super::NORM), "2@2 4@4 3@6");
    }
}
