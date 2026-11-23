//! Unified diff parsing: `git diff` output, or any unified diff.

/// One file's section of a diff.
#[derive(Debug, Default, PartialEq)]
pub struct FileDiff {
    /// `None` for an added file.
    pub old_path: Option<String>,
    /// `None` for a deleted file.
    pub new_path: Option<String>,
    /// Blob ids from the `index` line, possibly abbreviated; all zeros for a
    /// side that doesn't exist.
    pub old_blob: Option<String>,
    pub new_blob: Option<String>,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// The path a reader knows the file by: the new one, unless it was deleted.
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or_default()
    }
}

/// A run of 1-based lines on one side. `count` 0 is an empty run sitting
/// immediately before line `start`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Lines {
    pub start: usize,
    pub count: usize,
}

impl Lines {
    /// The last line, for a non-empty run.
    pub fn last(&self) -> Option<usize> {
        (self.count > 0).then(|| self.start + self.count - 1)
    }
}

/// A hunk, narrowed to its changed region: first changed line to last, with
/// the context between them but not around them.
#[derive(Debug, Default, PartialEq)]
pub struct Hunk {
    pub before: Lines,
    pub after: Lines,
    /// Line numbers of the `-` lines (before side) and `+` lines (after side).
    pub removed: Vec<usize>,
    pub added: Vec<usize>,
}

pub fn parse(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut lines = text.lines().peekable();
    // Inside a `diff --git` header, `---` belongs to the current file.
    let mut in_header = false;
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let (old, new) = split_git_header(rest);
            files.push(FileDiff {
                old_path: Some(old),
                new_path: Some(new),
                ..FileDiff::default()
            });
            in_header = true;
        } else if let Some(rest) = line.strip_prefix("--- ") {
            if !in_header {
                files.push(FileDiff::default());
            }
            in_header = false;
            if let Some(file) = files.last_mut() {
                file.old_path = side_path(rest, "a/");
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            if let Some(file) = files.last_mut() {
                file.new_path = side_path(rest, "b/");
            }
        } else if line.starts_with("@@ ") {
            in_header = false;
            let Some((old_start, new_start)) = parse_range_header(line) else {
                continue;
            };
            let mut body = Vec::new();
            while let Some(next) = lines.peek() {
                match next.as_bytes().first() {
                    Some(b' ' | b'-' | b'+') => body.push(*next),
                    Some(b'\\') => {}
                    // An empty context line, from a tool that trims spaces.
                    None => body.push(" "),
                    _ => break,
                }
                lines.next();
            }
            if let Some(file) = files.last_mut() {
                file.hunks.push(hunk(old_start, new_start, &body));
            }
        } else if let Some(file) = files.last_mut().filter(|_| in_header) {
            header_line(file, line);
        }
    }
    files
}

fn header_line(file: &mut FileDiff, line: &str) {
    if line.starts_with("new file mode") {
        file.old_path = None;
    } else if line.starts_with("deleted file mode") {
        file.new_path = None;
    } else if let Some(path) = line.strip_prefix("rename from ") {
        file.old_path = Some(unquote(path));
    } else if let Some(path) = line.strip_prefix("rename to ") {
        file.new_path = Some(unquote(path));
    } else if let Some(ids) = line.strip_prefix("index ") {
        let ids = ids.split(' ').next().unwrap_or_default();
        if let Some((old, new)) = ids.split_once("..") {
            file.old_blob = Some(old.to_string());
            file.new_blob = Some(new.to_string());
        }
    } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
        file.binary = true;
    }
}

fn hunk(old_start: usize, new_start: usize, body: &[&str]) -> Hunk {
    let changed = |l: &&str| !l.starts_with(' ');
    let (Some(first), Some(last)) = (
        body.iter().position(changed),
        body.iter().rposition(changed),
    ) else {
        return Hunk::default();
    };
    let mut old = old_start;
    let mut new = new_start;
    let mut out = Hunk::default();
    for (i, line) in body.iter().enumerate() {
        if i == first {
            out.before.start = old;
            out.after.start = new;
        }
        let in_region = (first..=last).contains(&i);
        match line.as_bytes()[0] {
            b'-' => {
                out.removed.push(old);
                old += 1;
                out.before.count += 1;
            }
            b'+' => {
                out.added.push(new);
                new += 1;
                out.after.count += 1;
            }
            _ => {
                if in_region {
                    out.before.count += 1;
                    out.after.count += 1;
                }
                old += 1;
                new += 1;
            }
        }
    }
    out
}

/// `@@ -a[,b] +c[,d] @@` → the first line number of each side's hunk body.
/// An empty side (`-a,0`) names the line *after which* it sits.
fn parse_range_header(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split(' ').skip(1);
    let side = |part: Option<&str>, sign: char| -> Option<usize> {
        let spec = part?.strip_prefix(sign)?;
        let (start, count) = spec.split_once(',').unwrap_or((spec, "1"));
        let start: usize = start.parse().ok()?;
        let count: usize = count.parse().ok()?;
        Some(if count == 0 { start + 1 } else { start })
    };
    Some((side(parts.next(), '-')?, side(parts.next(), '+')?))
}

fn split_git_header(rest: &str) -> (String, String) {
    if let Some(end) = closing_quote(rest).filter(|_| rest.starts_with('"')) {
        let old = unquote(&rest[..=end]);
        let new = unquote(rest[end + 1..].trim_start());
        return (strip(old, "a/"), strip(new, "b/"));
    }
    // Unquoted paths may hold spaces; ` b/` is the separator git writes.
    match rest.find(" b/") {
        Some(at) => (
            strip(rest[..at].to_string(), "a/"),
            rest[at + 3..].to_string(),
        ),
        None => {
            let (old, new) = rest.split_once(' ').unwrap_or((rest, rest));
            (old.to_string(), new.to_string())
        }
    }
}

fn side_path(rest: &str, prefix: &str) -> Option<String> {
    // GNU diff appends a tab and a timestamp.
    let rest = rest.split('\t').next().unwrap_or(rest);
    (rest != "/dev/null").then(|| strip(unquote(rest), prefix))
}

fn strip(path: String, prefix: &str) -> String {
    path.strip_prefix(prefix)
        .map(str::to_string)
        .unwrap_or(path)
}

fn closing_quote(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Undoes git's C-style quoting of unusual paths.
fn unquote(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return s.to_string();
    };
    let bytes = inner.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 == bytes.len() {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let escaped = bytes[i + 1];
        i += 2;
        match escaped {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'0'..=b'7' => {
                let digits = &inner[i - 1..(i + 2).min(inner.len())];
                out.push(u8::from_str_radix(digits, 8).unwrap_or(b'?'));
                i += 2;
            }
            other => out.push(other),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "\
diff --git a/lib/a.rb b/lib/a.rb
index 1111111..2222222 100644
--- a/lib/a.rb
+++ b/lib/a.rb
@@ -1,6 +1,6 @@
 class A
-  def x
+  def y
     1
-  end
+  end
 end

@@ -10,0 +11,2 @@ end
+# one
+# two
diff --git a/old.rb b/new.rb
similarity index 100%
rename from old.rb
rename to new.rb
diff --git a/gone.rb b/gone.rb
deleted file mode 100644
index 3333333..0000000
--- a/gone.rb
+++ /dev/null
@@ -1 +0,0 @@
-x
diff --git a/img.png b/img.png
index 4444444..5555555 100644
Binary files a/img.png and b/img.png differ
";

    #[test]
    fn parses_files_and_narrows_hunks() {
        let files = parse(DIFF);
        assert_eq!(files.len(), 4);
        let a = &files[0];
        assert_eq!(a.path(), "lib/a.rb");
        assert_eq!(a.old_blob.as_deref(), Some("1111111"));
        assert_eq!(a.hunks.len(), 2);
        // Lines 2..=4 changed; the interior context line 3 is included.
        assert_eq!(a.hunks[0].before, Lines { start: 2, count: 3 });
        assert_eq!(a.hunks[0].after, Lines { start: 2, count: 3 });
        assert_eq!(a.hunks[0].removed, [2, 4]);
        assert_eq!(a.hunks[0].added, [2, 4]);
        // A pure insertion: empty before side, sitting before line 11.
        assert_eq!(
            a.hunks[1].before,
            Lines {
                start: 11,
                count: 0
            }
        );
        assert_eq!(
            a.hunks[1].after,
            Lines {
                start: 11,
                count: 2
            }
        );

        assert_eq!(
            (files[1].old_path.as_deref(), files[1].new_path.as_deref()),
            (Some("old.rb"), Some("new.rb"))
        );
        assert!(files[1].hunks.is_empty());
        assert_eq!(
            (files[2].path(), files[2].new_path.as_deref()),
            ("gone.rb", None)
        );
        assert_eq!(files[2].hunks[0].after, Lines { start: 1, count: 0 });
        assert!(files[3].binary);
    }

    #[test]
    fn reads_plain_unified_diffs_and_quoted_paths() {
        let files = parse(
            "--- \"a/sp\\303\\244ce.rb\"\n+++ \"b/sp\\303\\244ce.rb\"\n@@ -1 +1 @@\n-a\n+b\n",
        );
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path(), "späce.rb");
        assert_eq!(files[0].hunks[0].removed, [1]);
    }
}
