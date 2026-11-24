//! A suggested edit as a unified diff against the file it was found in.

use super::{LineIndex, Replace};

/// How a unified diff marks a last line with no line break.
const NO_NEWLINE: &str = "\\ No newline at end of file\n";

/// The lines `replaces` touch, before and after, as one `@@` section.
pub fn unified(path: &str, source: &[u8], replaces: &[Replace]) -> Option<String> {
    let start = replaces.iter().map(|r| r.span.start).min()?;
    let end = replaces.iter().map(|r| r.span.end).max()?;
    let index = LineIndex::new(source);
    let first = index.line(start);
    let from = index.start(first);
    let to = source[end..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(source.len(), |i| end + i + 1);
    let mut sorted = replaces.to_vec();
    sorted.sort_by_key(|r| r.span.start);
    let mut new = Vec::new();
    let mut at = from;
    for r in &sorted {
        new.extend_from_slice(&source[at..r.span.start]);
        new.extend_from_slice(r.text.as_bytes());
        at = r.span.end;
    }
    new.extend_from_slice(&source[at..to]);
    let old_text = String::from_utf8_lossy(&source[from..to]);
    let new_text = String::from_utf8_lossy(&new);
    let old: Vec<&str> = old_text.lines().collect();
    let new: Vec<&str> = new_text.lines().collect();
    let mut out = format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{first},{} +{first},{} @@\n",
        old.len(),
        new.len()
    );
    let bare = |text: &str| !text.is_empty() && !text.ends_with('\n');
    for line in old {
        out.push_str(&format!("-{line}\n"));
    }
    if bare(&old_text) {
        out.push_str(NO_NEWLINE);
    }
    for line in new {
        out.push_str(&format!("+{line}\n"));
    }
    if bare(&new_text) {
        out.push_str(NO_NEWLINE);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replacement_is_shown_as_its_whole_lines() {
        let source = b"a = 1\nb = \"x\"\nc = 3\n";
        let replaces = [
            Replace {
                span: 10..11,
                text: "'".into(),
            },
            Replace {
                span: 12..13,
                text: "'".into(),
            },
        ];
        assert_eq!(
            unified("lib/a.rb", source, &replaces).unwrap(),
            "--- a/lib/a.rb\n+++ b/lib/a.rb\n@@ -2,1 +2,1 @@\n-b = \"x\"\n+b = 'x'\n"
        );
        assert_eq!(unified("a.rb", source, &[]), None);
        let bare = b"a = 1";
        let append = [Replace {
            span: 5..5,
            text: "\n".into(),
        }];
        assert_eq!(
            unified("a.rb", bare, &append).unwrap(),
            "--- a/a.rb\n+++ b/a.rb\n@@ -1,1 +1,1 @@\n-a = 1\n\\ No newline at end of file\n+a = 1\n"
        );
    }
}
