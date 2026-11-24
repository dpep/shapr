//! A suggested edit as a unified diff against the file it was found in.

use super::{LineIndex, Replace};

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
    let old = String::from_utf8_lossy(&source[from..to]);
    let new = String::from_utf8_lossy(&new);
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let mut out = format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{first},{} +{first},{} @@\n",
        old.len(),
        new.len()
    );
    for line in old {
        out.push_str(&format!("-{line}\n"));
    }
    for line in new {
        out.push_str(&format!("+{line}\n"));
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
    }
}
