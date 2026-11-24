//! Style/StringLiterals: a quoted string that could be written either way
//! with the same value.

use super::{Walker, replace};
use ruby_prism::Node;

pub const NORM: &str = crate::frontend::ruby::variants::STRING_LITERALS;
pub const STYLES: &[&str] = &["single_quotes", "double_quotes"];

/// Strings in an interpolation are another cop's.
pub fn see(w: &mut Walker, node: &Node) {
    let Some(string) = node.as_string_node() else {
        return;
    };
    let (Some(opening), Some(closing)) = (string.opening_loc(), string.closing_loc()) else {
        return;
    };
    if w.embedded.iter().any(|&e| e) {
        return;
    }
    let raw = string.content_loc().as_slice();
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
    let line = w.index.line(at);
    w.push(
        NORM,
        style,
        at,
        (line, line),
        String::from_utf8_lossy(raw).into_owned(),
        vec![(
            other.to_string(),
            vec![replace(&opening, quote), replace(&closing, quote)],
        )],
    );
}

#[cfg(test)]
mod tests {
    use super::super::tests::styles;

    #[test]
    fn only_strings_that_could_take_either_quote_count() {
        let src = "a = 'x'\nb = \"y\"\nc = \"it's\"\nd = 'say \"hi\"'\ne = \"#{a} z\"\n\
                   f = \"tab\\t\"\ng = %q(x)\nh = 'a#b'\ni = \"#{'nested'}\"\nj = ?x\nk = 'a#{b}'\nl = 'a#@b'\n";
        assert_eq!(
            styles(src, super::NORM),
            "single_quotes@1 double_quotes@2 single_quotes@8"
        );
    }
}
