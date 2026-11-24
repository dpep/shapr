//! Layout/EmptyLineBetweenDefs: one blank line between adjacent method
//! definitions (`empty_line`), two or more (`empty_lines`), or none
//! (`no_empty_line`). RuboCop's `NumberOfEmptyLines` counts them exactly.

use super::Walker;
use crate::norms::Replace;
use ruby_prism::Node;

pub const NORM: &str = "Layout/EmptyLineBetweenDefs";
/// `empty_lines` is counted only where seen, as width's numbers are: a body
/// with one blank line everywhere weighs two styles, not three.
pub const STYLES: &[&str] = &["empty_line", "no_empty_line"];

/// The gap between two adjacent `def`s in a body: a blank line in it, or
/// only comments (or nothing); blank lines both before and after a comment
/// choose nothing, as RuboCop's `multiple_blank_lines_groups?`. A one-line def chooses nothing: beyond
/// RuboCop, which allows two adjacent, it often sits tight against a
/// longer one (`def version; 1 end`). Nor do defs sharing a line.
pub fn see(w: &mut Walker, node: &Node) {
    let Some(statements) = node.as_statements_node() else {
        return;
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
        // RuboCop skips a gap with a blank line after a comment line, so a
        // blank around a comment block is never an offense.
        if (gap.clone()).any(|l| !blank.contains(&l) && blank.iter().any(|&b| b > l)) {
            continue;
        }
        let comments: Vec<String> = (gap.clone())
            .filter(|l| !blank.contains(l))
            .map(|l| String::from_utf8_lossy(line_text(l)).trim().to_string())
            .collect();
        let remove = |lines: &[usize]| -> Vec<Replace> {
            (lines.iter())
                .map(|&l| Replace {
                    span: w.index.start(l)..w.index.start(l + 1).min(w.source.len()),
                    text: String::new(),
                })
                .collect()
        };
        let (style, fix) = match blank.len() {
            0 => {
                let at = w.index.start(gap.start);
                let insert = vec![Replace {
                    span: at..at,
                    text: "\n".to_string(),
                }];
                ("no_empty_line", vec![("empty_line".to_string(), insert)])
            }
            1 => (
                "empty_line",
                vec![("no_empty_line".to_string(), remove(&blank))],
            ),
            _ => (
                "empty_lines",
                vec![
                    ("empty_line".to_string(), remove(&blank[1..])),
                    ("no_empty_line".to_string(), remove(&blank)),
                ],
            ),
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
            fix,
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
    fn defs_in_conditional_and_begin_bodies_count() {
        let pair = "  def a\n    1\n  end\n  def b\n    2\n  end\n";
        for (open, close) in [
            ("if c\n", "end\n"),
            ("while c\n", "end\n"),
            ("begin\n", "end\n"),
        ] {
            let src = format!("{open}{pair}{close}");
            assert_eq!(styles(&src, super::NORM), "no_empty_line@5", "{src}");
        }
        let src = format!("if c\n  1\nelse\n{pair}end\n");
        assert_eq!(styles(&src, super::NORM), "no_empty_line@7", "{src}");
    }

    #[test]
    fn gaps_between_adjacent_defs_count() {
        let src = "class A\n  def a\n  end\n\n  def b\n  end\n  # c\n  def c\n  end\n  def d; end\n  \
                   def e; end\n  x = 1\n  def f; end\n  def g\n  end\n\n\n  # h\n  def h; end\nend\n";
        assert_eq!(styles(src, super::NORM), "empty_line@5 no_empty_line@8");
        let src = "def a\n  1\nend\n\n\ndef b\n  2\nend\n";
        assert_eq!(styles(src, super::NORM), "empty_lines@6");
    }
}
