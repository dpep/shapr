//! Layout/SpaceAroundOperators: whitespace on both sides of a binary
//! operator or assignment (`space`), or not (`no_space`).

use super::Walker;
use crate::norms::Replace;
use ruby_prism::{Location, Node};

pub const NORM: &str = "Layout/SpaceAroundOperators";
pub const STYLES: &[&str] = &["space", "no_space"];

/// Binary operators a call can spell; `**` is RuboCop's own setting.
const BINARY: &[&[u8]] = &[
    b"+", b"-", b"*", b"/", b"%", b"==", b"!=", b"<", b">", b"<=", b">=", b"<=>", b"===", b"=~",
    b"!~", b"&", b"|", b"^", b"<<", b">>",
];

/// Any whitespace on both sides is `space`, so aligned `=` never reads as
/// another style. Unary operators and default parameters (`a=1`) aren't
/// observed.
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
    let style = if left > 0 && right > 0 {
        "space"
    } else {
        "no_space"
    };
    let pad = |at: usize| Replace {
        span: at..at,
        text: " ".to_string(),
    };
    let fix: Vec<Replace> = [
        (left == 0).then(|| pad(start)),
        (right == 0).then(|| pad(end)),
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

/// The operator of a binary call, `&&`, `||`, or an assignment.
fn operator<'pr>(node: &Node<'pr>) -> Option<Location<'pr>> {
    if let Some(call) = node.as_call_node() {
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
}
