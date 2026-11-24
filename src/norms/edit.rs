//! A suggested edit as a unified diff against the file it was found in.

use super::{LineIndex, Replace};
use crate::classify::proof::{Outside, Search};
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
    let after = frontend.parse(apply(&tree.source, replaces));
    after.errors.is_empty()
        && (renames || after.node(Tree::ROOT).canon == tree.node(Tree::ROOT).canon)
}

/// `source` with `replaces`, which don't overlap, applied.
pub fn apply(source: &[u8], replaces: &[Replace]) -> Vec<u8> {
    let mut sorted = replaces.to_vec();
    sorted.sort_by_key(|r| (r.span.start, r.span.end));
    let mut edited = Vec::with_capacity(source.len());
    let mut at = 0;
    for r in &sorted {
        edited.extend_from_slice(&source[at..r.span.start]);
        edited.extend_from_slice(r.text.as_bytes());
        at = r.span.end;
    }
    edited.extend_from_slice(&source[at..]);
    edited
}

/// A rename's old and new names appear in no file of the repository but
/// `path`: what the search can't settle counts as shared.
pub fn unshared(outside: &dyn Outside, path: &str, source: &[u8], replaces: &[Replace]) -> bool {
    let Some(first) = replaces.first() else {
        return false;
    };
    // The whole identifier: a rename keeps a leading `_` outside its span.
    let start = source[..first.span.start]
        .iter()
        .rposition(|&b| b != b'_')
        .map_or(0, |i| i + 1);
    let prefix = String::from_utf8_lossy(&source[start..first.span.start]);
    let old = String::from_utf8_lossy(&source[start..first.span.end]);
    [old.into_owned(), format!("{prefix}{}", first.text)]
        .iter()
        .all(|word| match outside.uses(path, word) {
            Search::Done {
                found, mentioned, ..
            } => found.iter().all(|f| f == path) && mentioned.is_empty(),
            Search::Capped | Search::Unavailable => false,
        })
}

/// Unchanged lines a [`patch`] shows around each change, as `diff -u` does.
const CONTEXT: usize = 3;

/// `replaces` as a unified diff of the whole file, one `@@` section per
/// cluster of changed lines with three lines of context, so `git apply`
/// takes it as is.
pub fn patch(path: &str, source: &[u8], replaces: &[Replace]) -> Option<String> {
    let index = LineIndex::new(source);
    let old: Vec<&[u8]> = source.split_inclusive(|&b| b == b'\n').collect();
    let mut sorted = replaces.to_vec();
    sorted.sort_by_key(|r| (r.span.start, r.span.end));
    // Runs of whole lines (first, last, 1-based) and what they become.
    let mut runs: Vec<(usize, usize, Vec<Replace>)> = Vec::new();
    for r in sorted {
        let first = index.line(r.span.start).max(1);
        let last = index.line(r.span.end.max(r.span.start + 1) - 1).max(first);
        match runs.last_mut() {
            Some(run) if first <= run.1 => {
                run.1 = run.1.max(last);
                run.2.push(r);
            }
            _ => runs.push((first, last, vec![r])),
        }
    }
    if runs.is_empty() || old.is_empty() {
        return None;
    }
    let runs: Vec<(usize, usize, Vec<u8>)> = (runs.into_iter())
        .map(|(first, last, replaces)| {
            let from = index.start(first);
            let to = index.start(last + 1);
            let shifted: Vec<Replace> = (replaces.iter())
                .map(|r| Replace {
                    span: r.span.start - from..r.span.end - from,
                    text: r.text.clone(),
                })
                .collect();
            (first, last, apply(&source[from..to], &shifted))
        })
        .collect();
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    let line = |out: &mut String, mark: char, text: &[u8]| {
        out.push(mark);
        out.push_str(&String::from_utf8_lossy(text));
        if !text.ends_with(b"\n") {
            out.push('\n');
            out.push_str(NO_NEWLINE);
        }
    };
    // Lines the new side gains over the old, before the section at hand.
    let mut delta: isize = 0;
    let mut i = 0;
    while i < runs.len() {
        let mut j = i;
        while j + 1 < runs.len() && runs[j + 1].0 - runs[j].1 - 1 <= 2 * CONTEXT {
            j += 1;
        }
        let start = runs[i].0.saturating_sub(CONTEXT).max(1);
        let end = (runs[j].1 + CONTEXT).min(old.len());
        let mut body = String::new();
        let mut gained: isize = 0;
        let mut at = start;
        for (first, last, new) in &runs[i..=j] {
            for l in at..*first {
                line(&mut body, ' ', old[l - 1]);
            }
            for l in *first..=*last {
                line(&mut body, '-', old[l - 1]);
            }
            let new: Vec<&[u8]> = new.split_inclusive(|&b| b == b'\n').collect();
            for text in &new {
                line(&mut body, '+', text);
            }
            gained += new.len() as isize - (last - first + 1) as isize;
            at = last + 1;
        }
        for l in at..=end {
            line(&mut body, ' ', old[l - 1]);
        }
        let old_count = end - start + 1;
        let new_count = (old_count as isize + gained) as usize;
        let new_start = (start as isize + delta) as usize;
        let new_start = if new_count == 0 {
            new_start - 1
        } else {
            new_start
        };
        out.push_str(&format!(
            "@@ -{start},{old_count} +{new_start},{new_count} @@\n{body}"
        ));
        delta += gained;
        i = j + 1;
    }
    Some(out)
}

/// `replaces` with each line break they insert written as the file's
/// first one is: `\r\n` in a CRLF file.
pub fn endings(source: &[u8], replaces: &[Replace]) -> Vec<Replace> {
    let crlf =
        (source.iter().position(|&b| b == b'\n')).is_some_and(|i| i > 0 && source[i - 1] == b'\r');
    (replaces.iter())
        .map(|r| Replace {
            span: r.span.clone(),
            text: match crlf {
                true => r.text.replace("\r\n", "\n").replace('\n', "\r\n"),
                false => r.text.clone(),
            },
        })
        .collect()
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
    // Each line keeps its own break, so a CRLF file's edit applies.
    let old: Vec<&str> = old_text.split_inclusive('\n').collect();
    let new: Vec<&str> = new_text.split_inclusive('\n').collect();
    // An empty side names the line before it, as `diff -u` does.
    let new_first = if new.is_empty() { first - 1 } else { first };
    let mut out = format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{first},{} +{new_first},{} @@\n",
        old.len(),
        new.len()
    );
    for (mark, lines) in [('-', old), ('+', new)] {
        for line in lines {
            out.push(mark);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push('\n');
                out.push_str(NO_NEWLINE);
            }
        }
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
    fn a_patch_has_context_and_a_section_per_cluster() {
        let source: String = (1..=12).map(|n| format!("l{n}\n")).collect();
        let at = |line: &str| source.find(line).unwrap();
        let replaces = [
            Replace {
                span: at("l2\n")..at("l2\n") + 2,
                text: "L2".into(),
            },
            Replace {
                span: at("l11\n")..at("l12\n"),
                text: String::new(),
            },
        ];
        assert_eq!(
            patch("a.rb", source.as_bytes(), &replaces).unwrap(),
            "--- a/a.rb\n+++ b/a.rb\n@@ -1,5 +1,5 @@\n l1\n-l2\n+L2\n l3\n l4\n l5\n\
             @@ -8,5 +8,4 @@\n l8\n l9\n l10\n-l11\n l12\n"
        );
        let bare = [Replace {
            span: 5..5,
            text: "\n".into(),
        }];
        assert_eq!(
            patch("a.rb", b"a = 1", &bare).unwrap(),
            "--- a/a.rb\n+++ b/a.rb\n@@ -1,1 +1,1 @@\n-a = 1\n\\ No newline at end of file\n+a = 1\n"
        );
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
        let gap = [Replace {
            span: 5..5,
            text: "\n".into(),
        }];
        assert_eq!(endings(b"a = 1\r\n", &gap)[0].text, "\r\n");
        let drop = [Replace {
            span: 6..7,
            text: String::new(),
        }];
        assert_eq!(
            unified("a.rb", b"a = 1\n\n", &drop).unwrap(),
            "--- a/a.rb\n+++ b/a.rb\n@@ -2,1 +1,0 @@\n-\n"
        );
        assert_eq!(endings(b"a = 1\n", &gap)[0].text, "\n");
        let crlf = b"a = \"x\"\r\n";
        let quote = |at: usize| Replace {
            span: at..at + 1,
            text: "'".into(),
        };
        assert_eq!(
            unified("a.rb", crlf, &[quote(4), quote(6)]).unwrap(),
            "--- a/a.rb\n+++ b/a.rb\n@@ -1,1 +1,1 @@\n-a = \"x\"\r\n+a = 'x'\r\n"
        );
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
