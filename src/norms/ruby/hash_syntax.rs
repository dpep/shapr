//! Style/HashSyntax: `a: 1` (`ruby19`) or `:a => 1` (`hash_rockets`).

use super::Walker;
use crate::norms::Replace;
use ruby_prism::Node;

pub const NORM: &str = "Style/HashSyntax";
pub const STYLES: &[&str] = &["ruby19", "hash_rockets"];

/// Each pair of a hash (or keyword arguments) with a symbol key that can be
/// written either way. As in RuboCop, a rocket is a choice only when every
/// key is such a symbol; a label is one in any hash.
pub fn see(w: &mut Walker, node: &Node) {
    let elements = match (node.as_hash_node(), node.as_keyword_hash_node()) {
        (Some(hash), _) => hash.elements(),
        (_, Some(kwargs)) => kwargs.elements(),
        _ => return,
    };
    let mut pairs = Vec::new();
    let mut mixed = false;
    for element in elements.iter() {
        let Some(assoc) = element.as_assoc_node() else {
            continue;
        };
        let value = assoc.value();
        // `{a:}` omits the value: it has no rocket spelling.
        if value.as_implicit_node().is_some() {
            return;
        }
        let key = assoc.key();
        let Some(symbol) = key.as_symbol_node() else {
            mixed = true;
            continue;
        };
        let name = String::from_utf8_lossy(symbol.unescaped()).into_owned();
        let opening = symbol.opening_loc().map(|l| l.as_slice().to_vec());
        let source = String::from_utf8_lossy(key.location().as_slice()).into_owned();
        let pair = match (opening.as_deref(), assoc.operator_loc()) {
            (Some(b":"), Some(_)) if label(&name) => Pair::Rocket(format!("{name}: ")),
            (None, None) if label(&name) => Pair::Label(format!(":{name} => ")),
            // `"a-b": 1`: a label RuboCop's `hash_rockets` rewrites.
            (Some(b"\"" | b"'"), None) => {
                mixed = true;
                let quoted = source.trim_end_matches(':');
                Pair::Label(format!(":{quoted} => "))
            }
            _ => {
                mixed = true;
                continue;
            }
        };
        pairs.push((key.location().start_offset(), value, name, pair));
    }
    if mixed {
        pairs.retain(|(.., pair)| matches!(pair, Pair::Label(_)));
    }
    for (start, value, name, pair) in pairs {
        let to = value.location().start_offset();
        let (style, other, text) = match pair {
            Pair::Rocket(text) => ("hash_rockets", "ruby19", text),
            Pair::Label(text) => ("ruby19", "hash_rockets", text),
        };
        let lines = (w.index.line(start), w.last_line(&value.location()));
        let value = String::from_utf8_lossy(value.location().as_slice());
        let fix = vec![Replace {
            span: start..to,
            text,
        }];
        let key = format!("{name} {value}");
        w.push(
            NORM,
            style,
            start,
            lines,
            key,
            vec![(other.to_string(), fix)],
        );
    }
}

/// A pair's spelling, with the text its key and operator take in the other.
enum Pair {
    Rocket(String),
    Label(String),
}

/// A symbol a label can spell: an identifier, maybe ending in `?` or `!`.
fn label(name: &str) -> bool {
    let name = name.as_bytes();
    let body = match name.split_last() {
        Some((b'?' | b'!', body)) => body,
        _ => name,
    };
    matches!(body.first(), Some(b) if b.is_ascii_alphabetic() || *b == b'_')
        && body.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

#[cfg(test)]
mod tests {
    use super::super::tests::styles;

    #[test]
    fn pairs_of_all_symbol_hashes_count() {
        let src = "a = { b: 1, c?: 2 }\nd = { :e => 1 }\nf = { :g => 1, 'h' => 2 }\n\
                   i = { :\"j-k\" => 1 }\nl = { m: }\nn(o: 1, **p)\nq = { \"r\": 1 }\nr('s', t: 1, 'u' => 2)\n";
        assert_eq!(
            styles(src, super::NORM),
            "ruby19@1 ruby19@1 hash_rockets@2 ruby19@6 ruby19@7 ruby19@8"
        );
    }
}
