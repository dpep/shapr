//! Layout/SpaceAroundOperators: whitespace on both sides of a binary
//! operator, assignment or `=>` (`space`), or not (`no_space`), or two or
//! more spaces after it that align with nothing (`extra_space`).

use super::Walker;
use crate::norms::Replace;
use ruby_prism::{Location, Node};

pub const NORM: &str = "Layout/SpaceAroundOperators";
/// `extra_space` is counted only where seen, as def spacing's `empty_lines`.
pub const STYLES: &[&str] = &["space", "no_space"];

/// Binary operators a call can spell; `**` is RuboCop's own setting.
const BINARY: &[&[u8]] = &[
    b"+", b"-", b"*", b"/", b"%", b"==", b"!=", b"<", b">", b"<=", b">=", b"<=>", b"===", b"=~",
    b"!~", b"&", b"|", b"^", b"<<", b">>",
];

/// Any whitespace on both sides is `space`, except two or more spaces after
/// the operator where RuboCop's `AllowForAlignment` finds nothing they
/// align with. Extra space before an operator isn't judged (RuboCop's rule
/// for it looks across whole assignment groups). Unary operators, default
/// parameters (`a=1`) and rational literals (`1/3r`) aren't observed.
pub fn see(w: &mut Walker, node: &Node) {
    let Some(op) = operator(node) else {
        return;
    };
    let (start, end) = (op.start_offset(), op.end_offset());
    // `and` and `or` can't lose their spaces.
    if op.as_slice().starts_with(b"**") || matches!(op.as_slice(), b"and" | b"or") {
        return;
    }
    let source = w.source;
    let left = source[..start]
        .iter()
        .rev()
        .take_while(|b| b.is_ascii_whitespace())
        .count();
    let right = source[end..]
        .iter()
        .take_while(|b| b.is_ascii_whitespace())
        .count();
    let extra =
        right >= 2 && !source[end..end + right].contains(&b'\n') && !aligned(w, end + right);
    let style = match (left > 0 && right > 0, extra) {
        (false, _) => "no_space",
        (true, true) => "extra_space",
        (true, false) => "space",
    };
    let pad = |at: usize| Replace {
        span: at..at,
        text: " ".to_string(),
    };
    let fix: Vec<Replace> = [
        (left == 0).then(|| pad(start)),
        (right == 0).then(|| pad(end)),
        extra.then(|| Replace {
            span: end..end + right,
            text: " ".to_string(),
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    let loc = node.location();
    let key = [
        &source[loc.start_offset()..start - left],
        op.as_slice(),
        &source[(end + right).min(loc.end_offset())..loc.end_offset()],
    ]
    .concat();
    let line = w.index.line(start);
    // Removing whitespace can change a parse (`a <<b`): only `space` is offered.
    let fix = (!fix.is_empty()).then(|| ("space".to_string(), fix));
    w.push(
        NORM,
        style,
        start,
        (line, line),
        String::from_utf8_lossy(&key).into_owned(),
        fix.into_iter().collect(),
    );
}

/// RuboCop's `aligned_with_something?` for the operand at byte `at`: the
/// nearest code line above and below, and the nearest at this line's
/// indentation each way, has a word starting at its column or the operand's
/// first word there. Columns count characters, as RuboCop's do.
fn aligned(w: &Walker, at: usize) -> bool {
    let text = |line: usize| {
        let end = w.index.start(line + 1).min(w.source.len());
        String::from_utf8_lossy(&w.source[w.index.start(line)..end])
            .trim_end_matches(['\n', '\r'])
            .to_string()
    };
    let indent = |t: &str| t.find(|c: char| !c.is_whitespace());
    let line = w.index.line(at);
    let own = text(line);
    let prefix = String::from_utf8_lossy(&w.source[w.index.start(line)..at]);
    let column = prefix.chars().count();
    let word: String = own
        .chars()
        .skip(column)
        .take_while(|c| !c.is_whitespace())
        .collect();
    let base = indent(&own);
    let matches = |t: &str| {
        let chars: Vec<char> = t.chars().collect();
        let starts = column > 0
            && chars.get(column - 1).is_some_and(|c| c.is_whitespace())
            && chars.get(column).is_some_and(|c| !c.is_whitespace());
        starts
            || (!word.is_empty()
                && chars
                    .iter()
                    .skip(column)
                    .collect::<String>()
                    .starts_with(&word))
    };
    let above: Vec<usize> = (1..line).rev().collect();
    let below: Vec<usize> = (line + 1..=w.index.count()).collect();
    [above, below].iter().any(|lines| {
        // Blank lines and lines a comment begins are passed over.
        let code = || {
            (lines.iter())
                .map(|&l| text(l))
                .filter(|t| indent(t).is_some_and(|i| !t[i..].starts_with('#')))
        };
        let nearest = code().next().is_some_and(|t| matches(&t));
        let level = code()
            .take_while(|t| indent(t) >= base)
            .find(|t| indent(t) == base)
            .is_some_and(|t| matches(&t));
        nearest || level
    })
}

/// The operator of a binary call, `&&`, `||`, an assignment, or `=>` in a
/// pair or `rescue`.
fn operator<'pr>(node: &Node<'pr>) -> Option<Location<'pr>> {
    if let Some(assoc) = node.as_assoc_node() {
        return assoc.operator_loc();
    }
    if let Some(rescue) = node.as_rescue_node() {
        return rescue.operator_loc();
    }
    if let Some(call) = node.as_call_node() {
        // RuboCop's `RationalLiteral`: `1/3r` is a number.
        if call.name().as_slice() == b"/"
            && call
                .receiver()
                .is_some_and(|r| r.as_integer_node().is_some())
            && (call.arguments().and_then(|a| a.arguments().iter().next()))
                .is_some_and(|a| a.as_rational_node().is_some())
        {
            return None;
        }
        let binary = call.receiver().is_some()
            && call.call_operator_loc().is_none()
            && call.opening_loc().is_none()
            && BINARY.contains(&call.name().as_slice())
            && call
                .arguments()
                .is_some_and(|a| a.arguments().iter().count() == 1);
        return if binary {
            call.message_loc()
        } else {
            call.equal_loc()
        };
    }
    macro_rules! loc {
        ($($as:ident => $loc:ident),* $(,)?) => {
            $(if let Some(n) = node.$as() {
                return Some(n.$loc());
            })*
        };
    }
    loc!(
        as_and_node => operator_loc,
        as_or_node => operator_loc,
        as_local_variable_write_node => operator_loc,
        as_instance_variable_write_node => operator_loc,
        as_class_variable_write_node => operator_loc,
        as_global_variable_write_node => operator_loc,
        as_constant_write_node => operator_loc,
        as_constant_path_write_node => operator_loc,
        as_multi_write_node => operator_loc,
        as_local_variable_operator_write_node => binary_operator_loc,
        as_instance_variable_operator_write_node => binary_operator_loc,
        as_class_variable_operator_write_node => binary_operator_loc,
        as_global_variable_operator_write_node => binary_operator_loc,
        as_constant_operator_write_node => binary_operator_loc,
        as_index_operator_write_node => binary_operator_loc,
        as_call_operator_write_node => binary_operator_loc,
        as_local_variable_or_write_node => operator_loc,
        as_instance_variable_or_write_node => operator_loc,
        as_index_or_write_node => operator_loc,
        as_call_or_write_node => operator_loc,
        as_local_variable_and_write_node => operator_loc,
        as_instance_variable_and_write_node => operator_loc,
        as_index_and_write_node => operator_loc,
        as_call_and_write_node => operator_loc,
    );
    None
}

#[cfg(test)]
mod tests {
    use super::super::tests::styles;

    #[test]
    fn binary_operators_and_assignments_count() {
        let src = "a = 1\nbb   = 2\nc=a+b\nd = a &&b\ne = !a\nf = -a\ng = a**2\n\
                   def h(i=1, j: 2); end\nk ||= a.b\nl = a.\n  m\n@n += 1\nx.y = 1\nz[0]=1\n\
                   p(a and b)\n";
        assert_eq!(
            styles(src, super::NORM),
            "space@1 space@2 no_space@3 no_space@3 space@4 no_space@4 space@5 space@6 space@7 \
             space@9 space@10 space@12 space@13 no_space@14"
        );
    }

    #[test]
    fn rockets_and_extra_space_count_as_rubocop_reads_them() {
        let src = "h = { 'a'=>'b', 'c' => 'd' }\nx =  y\nfoo =  1\nbar = 22\nlong_name = 1\n\
                   b =         2\nr = 1/3r\nbegin\nrescue E=>e\nend\nz = a +\n  b\n";
        assert_eq!(
            styles(src, super::NORM),
            "space@1 no_space@1 space@1 extra_space@2 extra_space@3 space@4 space@5 space@6 \
             space@7 no_space@9 space@11 space@11"
        );
    }
}
