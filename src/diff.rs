//! Unified diff parsing: `git diff` output, or any unified diff.

/// One file's section of a diff.
#[derive(Clone, Debug, Default, PartialEq)]
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hunk {
    pub before: Lines,
    pub after: Lines,
    /// Line numbers of the `-` lines (before side) and `+` lines (after side).
    pub removed: Vec<usize>,
    pub added: Vec<usize>,
}

impl Hunk {
    /// The maximal runs of changed lines, no context between them, each as
    /// a hunk of its own: the edits git's context merged into this one.
    pub fn runs(&self) -> Vec<Hunk> {
        let (mut old, mut new) = (self.before.start, self.after.start);
        let (mut removed, mut added) =
            (self.removed.iter().peekable(), self.added.iter().peekable());
        let mut out = Vec::new();
        while removed.peek().is_some() || added.peek().is_some() {
            // Context lines pair one to one, so both sides skip them together.
            while removed.peek() != Some(&&old) && added.peek() != Some(&&new) {
                old += 1;
                new += 1;
            }
            let mut run = Hunk {
                before: Lines {
                    start: old,
                    count: 0,
                },
                after: Lines {
                    start: new,
                    count: 0,
                },
                ..Hunk::default()
            };
            // A tool other than git may interleave `-` and `+` lines.
            loop {
                if removed.next_if_eq(&&old).is_some() {
                    run.removed.push(old);
                    old += 1;
                } else if added.next_if_eq(&&new).is_some() {
                    run.added.push(new);
                    new += 1;
                } else {
                    break;
                }
            }
            run.before.count = run.removed.len();
            run.after.count = run.added.len();
            out.push(run);
        }
        out
    }
}

/// A hunk header that doesn't parse, or whose body's line numbers pass
/// usize: its line in the diff, from 1.
#[derive(Debug, PartialEq)]
pub struct Malformed(pub usize);

pub fn parse(text: &str) -> Result<Vec<FileDiff>, Malformed> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut lines = text.lines().enumerate().peekable();
    // Inside a `diff --git` header, `---` belongs to the current file.
    let mut in_header = false;
    // Past a file's `+++` and before the next `diff --git`: elsewhere (mail
    // headers, a commit message) a line starting `@@ ` is text.
    let mut in_file = false;
    while let Some((at, line)) = lines.next() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            in_file = false;
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
            in_file = true;
            if let Some(file) = files.last_mut() {
                file.new_path = side_path(rest, "b/");
            }
        } else if line.starts_with("@@ ") {
            in_header = false;
            let malformed = Malformed(at + 1);
            let Some((old_start, new_start)) = parse_range_header(line) else {
                if in_file {
                    return Err(malformed);
                }
                continue;
            };
            let mut body = Vec::new();
            while let Some((_, next)) = lines.peek() {
                match next.as_bytes().first() {
                    Some(b' ' | b'-' | b'+') => body.push(*next),
                    Some(b'\\') => {}
                    // An empty context line, from a tool that trims spaces.
                    None => body.push(" "),
                    _ => break,
                }
                lines.next();
            }
            match (hunk(old_start, new_start, &body), files.last_mut()) {
                (Some(hunk), Some(file)) => file.hunks.push(hunk),
                (None, _) if in_file => return Err(malformed),
                _ => {}
            }
        } else if let Some(file) = files.last_mut().filter(|_| in_header) {
            header_line(file, line);
        }
    }
    Ok(files)
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

fn hunk(old_start: usize, new_start: usize, body: &[&str]) -> Option<Hunk> {
    let changed = |l: &&str| !l.starts_with(' ');
    let (Some(first), Some(last)) = (
        body.iter().position(changed),
        body.iter().rposition(changed),
    ) else {
        return Some(Hunk::default());
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
                old = old.checked_add(1)?;
                out.before.count += 1;
            }
            b'+' => {
                out.added.push(new);
                new = new.checked_add(1)?;
                out.after.count += 1;
            }
            _ => {
                if in_region {
                    out.before.count += 1;
                    out.after.count += 1;
                }
                old = old.checked_add(1)?;
                new = new.checked_add(1)?;
            }
        }
    }
    Some(out)
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
        if count == 0 {
            start.checked_add(1)
        } else {
            Some(start)
        }
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
            b'r' => out.push(b'\r'),
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'v' => out.push(0x0b),
            // Up to three octal digits, read as bytes: a path's next
            // character may be multibyte.
            b'0'..=b'7' => {
                let mut value = u32::from(escaped - b'0');
                for _ in 0..2 {
                    match bytes.get(i) {
                        Some(&d @ b'0'..=b'7') => value = value * 8 + u32::from(d - b'0'),
                        _ => break,
                    }
                    i += 1;
                }
                out.push(u8::try_from(value).unwrap_or(b'?'));
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
        let files = parse(DIFF).unwrap();
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
    fn splits_a_hunk_into_runs_of_changed_lines() {
        let files =
            parse("--- a/x\n+++ b/x\n@@ -1,7 +1,7 @@\n-a\n+A\n+B\n c\n d\n-e\n f\n-g\n+G\n-h\n")
                .unwrap();
        let runs = files[0].hunks[0].runs();
        let sides: Vec<_> = runs
            .iter()
            .map(|r| (r.before, r.after, r.removed.clone(), r.added.clone()))
            .collect();
        let lines = |start, count| Lines { start, count };
        assert_eq!(
            sides,
            [
                (lines(1, 1), lines(1, 2), vec![1], vec![1, 2]),
                // A pure removal sits before the after side's next line.
                (lines(4, 1), lines(5, 0), vec![4], vec![]),
                // Interleaved `-` and `+` lines are one run.
                (lines(6, 2), lines(6, 1), vec![6, 7], vec![6]),
            ]
        );
    }

    #[test]
    fn reads_plain_unified_diffs_and_quoted_paths() {
        let files = parse(
            "--- \"a/sp\\303\\244ce.rb\"\n+++ \"b/sp\\303\\244ce.rb\"\n@@ -1 +1 @@\n-a\n+b\n",
        )
        .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path(), "späce.rb");
        assert_eq!(files[0].hunks[0].removed, [1]);
    }

    #[test]
    fn hostile_headers_and_paths_parse_without_panicking() {
        let max = usize::MAX;
        // Line numbers past usize: malformed, like any header that doesn't
        // parse.
        for body in [
            format!("@@ -{max},0 +1 @@\n+a\n"),
            format!("@@ -1 +{max} @@\n-a\n+b\n+c\n"),
            format!("@@ -{max} +1 @@\n a\n-b\n"),
            "@@ -99999999999999999999999 +1 @@\n-a\n".into(),
        ] {
            let text = format!("--- a/a.rb\n+++ b/a.rb\n{body}");
            assert_eq!(parse(&text).err(), Some(Malformed(3)), "{text}");
        }
        // (diff, the file's path, its hunks' first after-side lines)
        let cases: &[(String, &str, &[usize])] = &[
            (format!("@@ -1,{max} +1,{max} @@\n-a\n+b\n"), "a.rb", &[1]),
            (
                format!("@@ -1 +{} @@\n-a\n+b\n", max - 1),
                "a.rb",
                &[max - 1],
            ),
        ];
        for (body, path, starts) in cases {
            let text = format!("--- a/a.rb\n+++ b/a.rb\n{body}");
            let files = parse(&text).unwrap();
            assert_eq!(files[0].path(), *path, "{text}");
            let got: Vec<usize> = files[0].hunks.iter().map(|h| h.after.start).collect();
            assert_eq!(got, *starts, "{text}");
        }
        // Quoted paths: escapes are bytes, so a short octal run before a
        // multibyte character, or a stray backslash, decodes in place.
        let paths = [
            ("\"a/\\12\u{e9}.rb\"", "\n\u{e9}.rb"),
            ("\"a/\\7\"", "\u{7}"),
            ("\"a/x\\\u{e9}\"", "x\u{e9}"),
            ("\"a/\\777\"", "?"),
            ("\"a/\\303\\251\"", "\u{e9}"),
        ];
        for (quoted, path) in paths {
            let text = format!("--- {quoted}\n+++ /dev/null\n@@ -1 +0,0 @@\n-a\n");
            assert_eq!(parse(&text).unwrap()[0].path(), path, "{text}");
            let git = format!("diff --git {quoted} {quoted}\n");
            assert_eq!(
                parse(&git).unwrap()[0].old_path.as_deref(),
                Some(path),
                "{git}"
            );
        }
    }
}
