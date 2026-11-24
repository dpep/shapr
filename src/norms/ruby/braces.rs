//! Layout/SpaceInsideHashLiteralBraces: `{ a: 1 }` or `{a: 1}`.

use super::Walker;
use crate::norms::Replace;
use ruby_prism::Node;

pub const NORM: &str = "Layout/SpaceInsideHashLiteralBraces";
pub const STYLES: &[&str] = &["space", "no_space"];

/// A non-empty hash literal whose braces agree: spaces inside both or
/// neither. A brace followed or preceded by a line break chooses nothing.
pub fn see(w: &mut Walker, node: &Node) {
    let Some(hash) = node.as_hash_node() else {
        return;
    };
    if hash.elements().iter().next().is_none() {
        return;
    }
    let open = hash.opening_loc().end_offset();
    let close = hash.closing_loc().start_offset();
    let inner = &w.source[open..close];
    let left = inner.iter().take_while(|b| b.is_ascii_whitespace()).count();
    let right = inner
        .iter()
        .rev()
        .take_while(|b| b.is_ascii_whitespace())
        .count();
    if inner[..left].contains(&b'\n') || inner[inner.len() - right..].contains(&b'\n') {
        return;
    }
    let (style, other, pad) = match (left, right) {
        (0, 0) => ("no_space", "space", " "),
        (l, r) if l > 0 && r > 0 => ("space", "no_space", ""),
        _ => return,
    };
    let at = hash.opening_loc().start_offset();
    let lines = (w.index.line(at), w.last_line(&hash.location()));
    let key = String::from_utf8_lossy(&inner[left..inner.len() - right]).into_owned();
    let fix = vec![
        Replace {
            span: open..open + left,
            text: pad.to_string(),
        },
        Replace {
            span: close - right..close,
            text: pad.to_string(),
        },
    ];
    w.push(NORM, style, at, lines, key, vec![(other.to_string(), fix)]);
}

#[cfg(test)]
mod tests {
    use super::super::tests::styles;

    #[test]
    fn hashes_whose_braces_agree_count() {
        let src =
            "a = { b: 1 }\nc = {d: 2}\ne = {}\nf = { g: 3}\nh = {\n  i: 4\n}\nj = {  k: 5  }\n";
        assert_eq!(styles(src, super::NORM), "space@1 no_space@2 space@8");
    }
}
