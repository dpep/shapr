//! A suggested edit as a unified diff against the file it was found in.

use super::{LineIndex, Replace};
use crate::tree::Tree;

/// Whether applying REPLACES to TREE's source (the file at PATH) leaves a
/// program its parser reads cleanly and, unless the edit RENAMES, the same
/// program: equal in canonical form, so only whitespace, comments and
/// respellings restyle proves the same differ. An edit that fails it is
/// never offered.
pub fn keeps(path: &str, tree: &Tree, replaces: &[Replace], renames: bool) -> bool {
    let Some(frontend) = crate::frontend::for_path(path) else {
        return false;
    };
    let mut sorted = replaces.to_vec();
    sorted.sort_by_key(|r| r.span.start);
    let mut edited = Vec::with_capacity(tree.source.len());
    let mut at = 0;
    for r in &sorted {
        edited.extend_from_slice(&tree.source[at..r.span.start]);
        edited.extend_from_slice(r.text.as_bytes());
        at = r.span.end;
    }
    edited.extend_from_slice(&tree.source[at..]);
    let after = frontend.parse(edited);
    after.errors.is_empty()
        && (renames || after.node(Tree::ROOT).canon == tree.node(Tree::ROOT).canon)
}

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
    use crate::frontend::{Frontend, Ruby};

    #[test]
    fn an_edit_is_kept_only_when_the_program_stays_the_same() {
        let keeps = |source: &str, from: &str, to: &str, renames: bool| {
            let tree = Ruby.parse(source.as_bytes().to_vec());
            let at = source.find(from).unwrap();
            let replace = Replace {
                span: at..at + from.len(),
                text: to.into(),
            };
            super::keeps("a.rb", &tree, &[replace], renames)
        };
        assert!(keeps("go(\n  \"x\"\n)\n", "\"x\"", "'x'", false));
        assert!(keeps("a = 1  \n", "1  ", "1", false));
        assert!(!keeps("go(\n  1,\n  &b\n)\n", "&b", "&b,", false));
        assert!(!keeps("x = /a  \nb/\n", "a  ", "a", false));
        assert!(!keeps("x = :\"a  \nb\"\n", "a  ", "a", false));
        assert!(!keeps("def fooBar; end\n", "fooBar", "foo_bar", false));
        assert!(keeps("def fooBar; end\n", "fooBar", "foo_bar", true));
        assert!(!keeps("def fooBar; end\n", "fooBar", "1x", true));
    }

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
