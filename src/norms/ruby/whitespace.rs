//! Layout/TrailingWhitespace: lines that end in spaces or tabs, or don't.

use super::Walker;
use crate::norms::Replace;
use ruby_prism::Node;

pub const NORM: &str = "Layout/TrailingWhitespace";
pub const STYLES: &[&str] = &["no_trailing_whitespace", "trailing_whitespace"];

/// String, heredoc, regexp and symbol contents: whitespace there is data,
/// not style.
pub fn see(w: &mut Walker, node: &Node) {
    let content = if let Some(string) = node.as_string_node() {
        string.content_loc()
    } else if let Some(xstring) = node.as_x_string_node() {
        xstring.content_loc()
    } else if let Some(regexp) = node.as_regular_expression_node() {
        regexp.content_loc()
    } else if let Some(value) = node.as_symbol_node().and_then(|s| s.value_loc()) {
        value
    } else {
        return;
    };
    w.literals
        .push(content.start_offset()..content.end_offset());
}

/// Each non-empty line, after the walk: up to `__END__`, and not one whose
/// line break sits inside a literal.
pub fn lines(w: &mut Walker) {
    for line in 1..=w.index.count() {
        let start = w.index.start(line);
        let next = w.index.start(line + 1).min(w.source.len());
        let mut end = next;
        while end > start && matches!(w.source[end - 1], b'\n' | b'\r') {
            end -= 1;
        }
        let text = &w.source[start..end];
        if text == b"__END__" {
            break;
        }
        if text.is_empty() || w.literals.iter().any(|l| l.start < end && end < l.end) {
            continue;
        }
        let trailing = text
            .iter()
            .rev()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        let key = String::from_utf8_lossy(&text[..text.len() - trailing]).into_owned();
        if trailing == 0 {
            w.push(
                NORM,
                "no_trailing_whitespace",
                start,
                (line, line),
                key,
                Vec::new(),
            );
        } else {
            let at = end - trailing;
            let fix = Replace {
                span: at..end,
                text: String::new(),
            };
            let fix = vec![("no_trailing_whitespace".to_string(), vec![fix])];
            w.push(NORM, "trailing_whitespace", at, (line, line), key, fix);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::styles;

    #[test]
    fn lines_outside_literals_count() {
        let src = "a = 1  \n\nb = <<~X\n  body  \nX\n  \nc = 'x \n y'\n__END__\ndata  \n";
        assert_eq!(
            styles(src, super::NORM),
            "trailing_whitespace@1 no_trailing_whitespace@3 no_trailing_whitespace@5 \
             trailing_whitespace@6 no_trailing_whitespace@8"
        );
    }
}
