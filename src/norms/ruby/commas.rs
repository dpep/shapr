//! Trailing commas in multiline lists: Style/TrailingCommaInArrayLiteral,
//! Style/TrailingCommaInHashLiteral and Style/TrailingCommaInArguments.

use super::Walker;
use crate::norms::Replace;
use ruby_prism::Node;

pub const ARRAY: &str = crate::frontend::ruby::variants::TRAILING_COMMA_ARRAY;
pub const HASH: &str = crate::frontend::ruby::variants::TRAILING_COMMA_HASH;
pub const ARGUMENTS: &str = crate::frontend::ruby::variants::TRAILING_COMMA_ARGUMENTS;
pub const STYLES: &[&str] = &["comma", "no_comma"];

pub fn see_array(w: &mut Walker, node: &Node) {
    let Some(array) = node.as_array_node() else {
        return;
    };
    // `%w[]` and `%i[]` take no commas.
    let Some(opening) = array.opening_loc().filter(|o| o.as_slice() == b"[") else {
        return;
    };
    let Some(closing) = array.closing_loc() else {
        return;
    };
    let items: Vec<Node> = array.elements().iter().collect();
    list(
        w,
        ARRAY,
        opening.start_offset(),
        &items,
        closing.start_offset(),
    );
}

pub fn see_hash(w: &mut Walker, node: &Node) {
    let Some(hash) = node.as_hash_node() else {
        return;
    };
    let items: Vec<Node> = hash.elements().iter().collect();
    let (open, close) = (hash.opening_loc(), hash.closing_loc());
    list(w, HASH, open.start_offset(), &items, close.start_offset());
}

/// A bracketed call's arguments, `f(...)` or `Set[...]`. One ending in a
/// block argument (`&blk`) chooses nothing: a comma can't follow it.
pub fn see_arguments(w: &mut Walker, node: &Node) {
    let Some(call) = node.as_call_node() else {
        return;
    };
    let (Some(opening), Some(closing)) = (call.opening_loc(), call.closing_loc()) else {
        return;
    };
    if !matches!(opening.as_slice(), b"(" | b"[") {
        return;
    }
    // Keyword arguments are items each, as RuboCop reads them.
    let mut items: Vec<Node> = Vec::new();
    for arg in call.arguments().iter().flat_map(|a| a.arguments().iter()) {
        match arg.as_keyword_hash_node() {
            Some(kwargs) => items.extend(kwargs.elements().iter()),
            None => items.push(arg),
        }
    }
    if call
        .block()
        .is_some_and(|b| b.as_block_argument_node().is_some())
    {
        return;
    }
    let (open, close) = (opening.start_offset(), closing.start_offset());
    list(w, ARGUMENTS, open, &items, close);
}

/// A list chooses when each item starts on its own line and the closing
/// bracket sits on a line after the last: there RuboCop's `comma` and
/// `consistent_comma` both want the comma, and `no_comma` doesn't.
fn list(w: &mut Walker, norm: &'static str, open: usize, items: &[Node], close: usize) {
    let (Some(first), Some(last)) = (items.first(), items.last()) else {
        return;
    };
    let starts: Vec<usize> = items
        .iter()
        .map(|i| w.index.line(i.location().start_offset()))
        .collect();
    if starts.windows(2).any(|p| p[0] >= p[1]) || starts[0] == w.index.line(open) {
        return;
    }
    let end = last.location().end_offset();
    let end_line = w.last_line(&last.location());
    let close_line = w.index.line(close);
    if close_line <= end_line {
        return;
    }
    // Between the last item and the bracket: a comma, comments, space. Any
    // other text (a heredoc's body) and the list chooses nothing.
    let gap = &w.source[end..close];
    let mut comma = None;
    let mut i = 0;
    while i < gap.len() {
        match gap[i] {
            b',' if comma.is_none() => comma = Some(end + i),
            b'#' => {
                while i < gap.len() && gap[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b if b.is_ascii_whitespace() => {}
            _ => return,
        }
        i += 1;
    }
    let key = String::from_utf8_lossy(&w.source[first.location().start_offset()..end]).into_owned();
    let (style, other, fix) = match comma {
        Some(at) => (
            "comma",
            "no_comma",
            Replace {
                span: at..at + 1,
                text: String::new(),
            },
        ),
        None => (
            "no_comma",
            "comma",
            Replace {
                span: end..end,
                text: ",".to_string(),
            },
        ),
    };
    w.push(
        norm,
        style,
        comma.unwrap_or(end),
        (end_line, close_line),
        key,
        vec![(other.to_string(), vec![fix])],
    );
}

#[cfg(test)]
mod tests {
    use super::super::tests::styles;
    use super::*;

    #[test]
    fn multiline_lists_with_an_item_per_line_count() {
        let src = "a = [\n  1,\n  2,\n]\nb = [\n  1,\n  2\n]\nc = [1, 2]\nd = [\n  1, 2,\n]\n\
                   e = [1,\n  2]\nf = %w[\n  x\n]\n";
        assert_eq!(styles(src, ARRAY), "comma@3 no_comma@7");
    }

    #[test]
    fn a_comment_after_the_comma_still_counts() {
        let src = "a = {\n  b: 1, # one\n}\nc = [\n  <<~X,\n  body\n  X\n]\n";
        assert_eq!(styles(src, HASH), "comma@2");
        assert_eq!(styles(src, ARRAY), "");
    }

    #[test]
    fn parenthesized_arguments_count() {
        let src = "f(\n  1,\n  &blk\n)\ng(\n  a: 1,\n  b: 2,\n)\nh 1,\n  2\nk(\n  a: 1, b: 2,\n)\nS[\n  :x,\n]\n";
        assert_eq!(styles(src, ARGUMENTS), "comma@7 comma@15");
        assert_eq!(styles(src, HASH), "");
    }
}
