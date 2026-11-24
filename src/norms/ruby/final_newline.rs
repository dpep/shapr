//! Layout/TrailingEmptyLines: how a file ends, with one line break
//! (`final_newline`), a blank line (`final_blank_line`) or none
//! (`no_final_newline`).

use super::Walker;
use crate::norms::Replace;

pub const NORM: &str = "Layout/TrailingEmptyLines";
pub const STYLES: &[&str] = &["final_newline", "final_blank_line", "no_final_newline"];

/// One observation per non-empty file, on its last line.
pub fn file(w: &mut Walker) {
    let source = w.source;
    let breaks = source.iter().rev().take_while(|&&b| b == b'\n').count();
    let end = source.len() - breaks;
    if end == 0 {
        return;
    }
    let style = match breaks {
        0 => "no_final_newline",
        1 => "final_newline",
        _ => "final_blank_line",
    };
    let fix = |text: &str| Replace {
        span: end..source.len(),
        text: text.to_string(),
    };
    let fixes = [
        ("final_newline", "\n"),
        ("final_blank_line", "\n\n"),
        ("no_final_newline", ""),
    ]
    .into_iter()
    .filter(|(other, _)| *other != style)
    .map(|(other, text)| (other.to_string(), vec![fix(text)]))
    .collect();
    let line = w.index.count();
    let key =
        String::from_utf8_lossy(&source[w.index.start(w.index.line(end - 1))..end]).into_owned();
    w.push(NORM, style, end - 1, (line, line), key, fixes);
}

#[cfg(test)]
mod tests {
    use super::super::tests::styles;

    #[test]
    fn a_file_ends_one_way() {
        assert_eq!(styles("a = 1\n", super::NORM), "final_newline@1");
        assert_eq!(styles("a = 1", super::NORM), "no_final_newline@1");
        assert_eq!(styles("a = 1\n\n", super::NORM), "final_blank_line@1");
        assert_eq!(styles("", super::NORM), "");
    }
}
