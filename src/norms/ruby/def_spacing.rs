//! Layout/EmptyLineBetweenDefs: a blank line between adjacent method
//! definitions (`empty_line`), or none (`no_empty_line`).

use super::Walker;
use crate::norms::Replace;
use ruby_prism::Node;

pub const NORM: &str = "Layout/EmptyLineBetweenDefs";
pub const STYLES: &[&str] = &["empty_line", "no_empty_line"];

/// The gap between two adjacent `def`s in a body: a blank line in it, or
/// only comments (or nothing). A one-line def chooses nothing: beyond
/// RuboCop, which allows two adjacent, it often sits tight against a
/// longer one (`def version; 1 end`). Nor do defs sharing a line.
pub fn see(w: &mut Walker, node: &Node) {
    // The walk reaches the program's statements only through the program.
    let statements = match node.as_program_node() {
        Some(program) => program.statements(),
        None => match node.as_statements_node() {
            Some(statements) => statements,
            None => return,
        },
    };
    let body: Vec<Node> = statements.body().iter().collect();
    for pair in body.windows(2) {
        let (Some(prev), Some(next)) = (pair[0].as_def_node(), pair[1].as_def_node()) else {
            continue;
        };
        let (prev_loc, next_loc) = (prev.location(), next.location());
        let prev_first = w.index.line(prev_loc.start_offset());
        let prev_last = w.last_line(&prev_loc);
        let next_first = w.index.line(next_loc.start_offset());
        let next_last = w.last_line(&next_loc);
        if next_first <= prev_last || prev_first == prev_last || next_first == next_last {
            continue;
        }
        let gap = prev_last + 1..next_first;
        let line_text = |line: usize| {
            let end = w.index.start(line + 1).min(w.source.len());
            &w.source[w.index.start(line)..end]
        };
        let blank: Vec<usize> = (gap.clone())
            .filter(|&l| line_text(l).iter().all(u8::is_ascii_whitespace))
            .collect();
        let comments: Vec<String> = (gap.clone())
            .filter(|l| !blank.contains(l))
            .map(|l| String::from_utf8_lossy(line_text(l)).trim().to_string())
            .collect();
        let (style, other, fix) = if blank.is_empty() {
            let at = w.index.start(gap.start);
            let insert = vec![Replace {
                span: at..at,
                text: "\n".to_string(),
            }];
            ("no_empty_line", "empty_line", insert)
        } else {
            let remove = (blank.iter())
                .map(|&l| Replace {
                    span: w.index.start(l)..w.index.start(l + 1).min(w.source.len()),
                    text: String::new(),
                })
                .collect();
            ("empty_line", "no_empty_line", remove)
        };
        let names =
            |d: &ruby_prism::DefNode| String::from_utf8_lossy(d.name().as_slice()).into_owned();
        let key = format!(
            "{}\n{}\n{}",
            names(&prev),
            comments.join("\n"),
            names(&next)
        );
        w.push(
            NORM,
            style,
            next_loc.start_offset(),
            (prev_last, next_first),
            key,
            vec![(other.to_string(), fix)],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::styles;

    #[test]
    fn top_level_defs_count() {
        let src = "def a\n  1\nend\ndef b\n  2\nend\n";
        assert_eq!(styles(src, super::NORM), "no_empty_line@4");
    }

    #[test]
    fn gaps_between_adjacent_defs_count() {
        let src = "class A\n  def a\n  end\n\n  def b\n  end\n  # c\n  def c\n  end\n  def d; end\n  \
                   def e; end\n  x = 1\n  def f; end\n  def g\n  end\n\n\n  # h\n  def h; end\nend\n";
        assert_eq!(styles(src, super::NORM), "empty_line@5 no_empty_line@8");
    }
}
