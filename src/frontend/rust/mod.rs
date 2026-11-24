//! Rust, parsed with tree-sitter.
//!
//! The concrete syntax tree is mapped onto the shape the core expects:
//! punctuation stays in its parent's own text, a statement's `;` wrapper and
//! a module or impl body's braces fold into the enclosing node, attributes
//! become the first children of the item they precede, and an impl's
//! header (attributes, generics, trait, type) is its first child. A macro's token
//! tree can't be parsed, so every token in it is a leaf and every identifier
//! is [`crate::tree::Role::Opaque`].

mod roles;

use super::{Construct, Frontend};
use crate::norms::Snapshot;
use crate::tree::{Flags, NodeId, Semantics, Span, Spelling, Tree, TreeBuilder};
use roles::{Mode, Plan};
use std::collections::HashSet;
use std::sync::OnceLock;
use tree_sitter::{Node, Parser};

pub struct Rust;

/// The entry point, and methods of std traits that syntax or the prelude
/// reaches (`a + b` calls `add`, `{}` calls `fmt`, `x.clone()` prefers an
/// inherent `clone`): defining or renaming one reaches callers shapr can't
/// see.
const RESERVED: &[&str] = &[
    "add",
    "add_assign",
    "as_mut",
    "as_ref",
    "bitand",
    "bitand_assign",
    "bitor",
    "bitor_assign",
    "bitxor",
    "bitxor_assign",
    "borrow",
    "borrow_mut",
    "call",
    "call_mut",
    "call_once",
    "clamp",
    "clone",
    "clone_from",
    "cmp",
    "default",
    "deref",
    "deref_mut",
    "div",
    "div_assign",
    "drop",
    "eq",
    "extend",
    "fmt",
    "from",
    "from_iter",
    "from_str",
    "ge",
    "gt",
    "hash",
    "index",
    "index_mut",
    "into",
    "into_future",
    "into_iter",
    "le",
    "lt",
    "main",
    "max",
    "min",
    "mul",
    "mul_assign",
    "ne",
    "neg",
    "next",
    "next_back",
    "not",
    "partial_cmp",
    "poll",
    "rem",
    "rem_assign",
    "shl",
    "shl_assign",
    "shr",
    "shr_assign",
    "size_hint",
    "sub",
    "sub_assign",
    "to_owned",
    "to_string",
    "try_from",
    "try_into",
    "type_id",
];

impl Frontend for Rust {
    fn language(&self) -> &'static str {
        "rust"
    }

    fn handles(&self, path: &str) -> bool {
        path.ends_with(".rs")
    }

    fn parse(&self, source: Vec<u8>) -> Tree {
        let mut builder = TreeBuilder::new(source.clone());
        builder.semantics(Semantics {
            file_scoped: true,
            scoped_drop: true,
            private_by_default: true,
            reserved: RESERVED,
            spelling: Spelling {
                quotes: &['"', '\''],
                ..Spelling::default()
            },
        });
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .expect("the bundled grammar matches the tree-sitter version");
        match parser.parse(&source, None) {
            Some(cst) => Walker {
                src: &source,
                b: &mut builder,
                locals: vec![HashSet::new()],
                opaque_members: false,
                doctests: doctests(cst.root_node(), &source),
            }
            .children(cst.root_node(), Mode::Expr, &[]),
            None => builder.error(0..source.len()),
        }
        builder.finish()
    }

    /// Rust has no call by a computed name.
    fn dispatch_words(&self) -> &'static [&'static str] {
        &[]
    }

    fn search_globs(&self) -> &'static [&'static str] {
        &["*.rs"]
    }

    /// A library crate's source: the nearest `Cargo.toml` above it has a
    /// `[lib]` target or a `src/lib.rs`, isn't `publish = false`, and the file
    /// isn't one of its binaries, tests, examples, benches or build script.
    /// A private module's `pub` items count, as they may be re-exported.
    fn library(&self, path: &str, files: &dyn Snapshot) -> bool {
        let mut dir = crate::norms::snapshot::parent(path);
        loop {
            let manifest = crate::norms::join(dir, "Cargo.toml");
            if files.list(dir).contains(&manifest) {
                // Unread, its targets and `publish` are unknown: assume a library.
                let Some(toml) = files.read(&manifest) else {
                    return true;
                };
                let toml = String::from_utf8_lossy(&toml);
                let line = |want: &str| {
                    toml.lines()
                        .any(|l| l.split_whitespace().collect::<String>() == want)
                };
                let lib = line("[lib]")
                    || files
                        .list(&crate::norms::join(dir, "src"))
                        .contains(&crate::norms::join(dir, "src/lib.rs"));
                let own = path
                    .strip_prefix(dir)
                    .unwrap_or(path)
                    .trim_start_matches('/');
                let target = !["src/bin/", "tests/", "examples/", "benches/"]
                    .iter()
                    .any(|p| own.starts_with(p))
                    && !matches!(own, "src/main.rs" | "build.rs");
                return lib && target && !unpublished(&toml, dir, files);
            }
            if dir.is_empty() {
                return false;
            }
            dir = crate::norms::snapshot::parent(dir);
        }
    }

    fn construct(&self, tree: &Tree, id: NodeId) -> Option<Construct> {
        roles::construct(tree, id)
    }
}

/// Whether a crate's manifest allows no registry: `[package]` says
/// `publish = false` or `publish = []`, or `publish.workspace = true` under a
/// workspace root (above `dir`) whose `[workspace.package]` says so. Any
/// spelling this doesn't read leaves the crate published.
fn unpublished(toml: &str, dir: &str, files: &dyn Snapshot) -> bool {
    let compact = |l: &str| l.split_whitespace().collect::<String>();
    // The lines of table `header`, up to the next header.
    let table = |toml: &str, header: &str| -> Vec<String> {
        toml.lines()
            .skip_while(|l| compact(l) != header)
            .skip(1)
            .take_while(|l| !l.trim_start().starts_with('['))
            .map(compact)
            .collect()
    };
    let none = |lines: &[String]| {
        lines
            .iter()
            .any(|l| matches!(l.as_str(), "publish=false" | "publish=[]"))
    };
    let package = table(toml, "[package]");
    let inherits = package.iter().any(|l| {
        matches!(
            l.as_str(),
            "publish.workspace=true" | "publish={workspace=true}"
        )
    });
    if !inherits {
        return none(&package);
    }
    // The workspace root: this manifest, or the nearest one above with `[workspace]`.
    let workspace = |toml: &str| toml.lines().any(|l| compact(l) == "[workspace]");
    let mut root = workspace(toml).then(|| toml.to_string());
    let mut at = dir;
    while root.is_none() && !at.is_empty() {
        at = crate::norms::snapshot::parent(at);
        let read = files.read(&crate::norms::join(at, "Cargo.toml"));
        root = read
            .map(|t| String::from_utf8_lossy(&t).into_owned())
            .filter(|t| workspace(t));
    }
    root.is_some_and(|root| none(&table(&root, "[workspace.package]")))
}

fn span(n: &Node<'_>) -> Span {
    n.start_byte()..n.end_byte()
}

/// The node's kind, as the `&'static str` a tree keeps. tree-sitter ties
/// kind names to the grammar's lifetime; this grammar is static.
fn kind(n: &Node<'_>) -> &'static str {
    static KINDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    let kinds = KINDS.get_or_init(|| {
        let language = tree_sitter::Language::new(tree_sitter_rust::LANGUAGE);
        (0..language.node_kind_count() as u16)
            .map(|id| -> &'static str {
                Box::leak(language.node_kind_for_id(id).unwrap_or("").into())
            })
            .collect()
    });
    kinds
        .get(n.kind_id() as usize)
        .copied()
        .unwrap_or("unknown")
}

/// Whether a comma before `rest` (its later siblings) is a trailing one,
/// which never changes the parse: one before a closing bracket, or ending a
/// match arm. A one-element tuple's comma makes it a tuple, so it counts.
fn trailing(parent: &Node<'_>, rest: &[(Option<&str>, Node<'_>)]) -> bool {
    let next = rest.iter().map(|(_, c)| c).find(|c| !is_comment(c));
    let closes = match next {
        Some(c) => !c.is_named() && matches!(c.kind(), ")" | "]" | "}" | ">" | "|"),
        None => parent.kind() == "match_arm",
    };
    let tuple = matches!(
        parent.kind(),
        "tuple_expression" | "tuple_pattern" | "tuple_type"
    );
    closes && !(tuple && parent.named_child_count() == 1)
}

/// Doc comments that hold doctest code, by start byte: in a run of `///`
/// (or `//!`) lines, each line of a fenced or indented code block (fence
/// lines included); a `/** */` or `/*! */` block holding one.
fn doctests(root: Node<'_>, src: &[u8]) -> HashSet<usize> {
    let mut docs = Vec::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if is_comment(&n) {
            docs.push(n);
            continue;
        }
        let mut c = n.walk();
        let kids: Vec<_> = n.children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    let text = |n: &Node<'_>| std::str::from_utf8(&src[span(n)]).unwrap_or_default();
    let mut out = HashSet::new();
    // Line docs with one marker that rustdoc joins into one text, as (marker, nodes).
    let mut runs: Vec<(&str, Vec<Node<'_>>)> = Vec::new();
    for n in docs {
        let t = text(&n);
        let line_marker = ["///", "//!"]
            .into_iter()
            .find(|m| t.starts_with(m) && !t.starts_with("////"));
        let Some(marker) = line_marker else {
            let block_doc = (t.starts_with("/**") && !t.starts_with("/***") && t != "/**/")
                || t.starts_with("/*!");
            if block_doc {
                let inner = t[3..].strip_suffix("*/").unwrap_or(&t[3..]);
                // A ` * ` gutter isn't part of the text.
                let lines: Vec<&str> = inner
                    .lines()
                    .map(|l| match l.trim_start().strip_prefix('*') {
                        Some(rest) => rest.strip_prefix(' ').unwrap_or(rest),
                        None => l,
                    })
                    .collect();
                if code_lines(&lines).contains(&true) {
                    out.insert(n.start_byte());
                }
            }
            continue;
        };
        match runs.last_mut() {
            Some((m, nodes))
                if *m == marker && nodes.last().is_some_and(|last| joined(&n, last)) =>
            {
                nodes.push(n)
            }
            _ => runs.push((marker, vec![n])),
        }
    }
    for (marker, nodes) in runs {
        let bodies: Vec<&str> = nodes.iter().map(|n| &text(n)[marker.len()..]).collect();
        let code = code_lines(&bodies);
        out.extend(
            nodes
                .iter()
                .zip(code)
                .filter(|(_, c)| *c)
                .map(|(n, _)| n.start_byte()),
        );
    }
    out
}

/// Whether rustdoc reads doc comment `n` as a continuation of `last`: only
/// comments and attributes, which rustdoc joins or skips, lie between them.
fn joined(n: &Node<'_>, last: &Node<'_>) -> bool {
    let mut at = n.prev_sibling();
    while let Some(s) = at {
        if s.id() == last.id() {
            return true;
        }
        if !(is_comment(&s) || matches!(s.kind(), "attribute_item" | "inner_attribute_item")) {
            return false;
        }
        at = s.prev_sibling();
    }
    false
}

/// Which lines of a doc comment's text are code to rustdoc's Markdown,
/// read both as written and with `>` blockquote markers removed, so a
/// block either reading finds is code.
fn code_lines(lines: &[&str]) -> Vec<bool> {
    let unquoted: Vec<&str> = lines.iter().map(|l| unquote(l)).collect();
    let mut out = blocks(lines);
    if unquoted != lines {
        for (o, q) in out.iter_mut().zip(blocks(&unquoted)) {
            *o |= q;
        }
    }
    out
}

/// A line with its `>` blockquote markers (each with one optional space)
/// removed.
fn unquote(line: &str) -> &str {
    let mut rest = line;
    loop {
        let t = rest.trim_start_matches(' ');
        match t.strip_prefix('>') {
            Some(r) if rest.len() - t.len() < 4 => rest = r.strip_prefix(' ').unwrap_or(r),
            _ => return rest,
        }
    }
}

/// Which lines are a Rust code block: a fenced block with its fences,
/// unless its info string names only other languages (```text), or an
/// indented block (four columns past the text, or past an open list item's
/// text, where no paragraph continues). The text is first unindented by its
/// least indentation, as rustdoc does.
fn blocks(lines: &[&str]) -> Vec<bool> {
    let indent = |s: &str| -> usize {
        let lead = s.chars().take_while(|c| *c == ' ' || *c == '\t');
        lead.map(|c| if c == '\t' { 4 } else { 1 }).sum()
    };
    let blank = |s: &str| s.trim().is_empty();
    let least = lines.iter().filter(|l| !blank(l)).map(|l| indent(l)).min();
    let least = least.unwrap_or(0);
    let mut out = vec![false; lines.len()];
    // The open fence: its character, length, and whether it holds Rust.
    let mut fenced: Option<(char, usize, bool)> = None;
    let (mut indented, mut para, mut after_blank) = (false, false, true);
    // The column an open list item's text starts at.
    let mut item: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if blank(line) {
            out[i] = fenced.is_some_and(|(_, _, rust)| rust);
            (para, after_blank) = (false, true);
            continue;
        }
        let (n, text) = (indent(line) - least, line.trim_start());
        if after_blank && item.is_some_and(|c| n < c) {
            item = None;
        }
        let base = item.unwrap_or(0);
        if let Some((ch, len, rust)) = fenced {
            out[i] = rust;
            let run = text.chars().take_while(|&c| c == ch).count();
            let closes = n < base + 4 && run >= len && text[run..].trim().is_empty();
            if closes {
                fenced = None;
            }
        } else if n >= base + 4 && (indented || !para) {
            out[i] = true;
            indented = true;
        } else {
            indented = false;
            para = false;
            if let Some(fence) = fence(text) {
                out[i] = fence.2;
                fenced = Some(fence);
            } else if heading(text) {
            } else if let Some(at) = list_item(text) {
                item = Some(n + at);
                para = !text[at..].trim().is_empty();
            } else {
                para = true;
            }
        }
        after_blank = false;
    }
    out
}

/// A fence opening a code block: its character, length, and whether its
/// info string leaves it Rust (rustdoc's tags, or no language at all).
fn fence(text: &str) -> Option<(char, usize, bool)> {
    let ch = text.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = text.chars().take_while(|&c| c == ch).count();
    if len < 3 {
        return None;
    }
    let mut words = text[len..]
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .peekable();
    let rust = words.peek().is_none() || words.any(rust_tag);
    Some((ch, len, rust))
}

/// An info-string word rustdoc reads as a Rust block's, or one it might
/// (`{.class}` attributes).
fn rust_tag(word: &str) -> bool {
    let code = word
        .strip_prefix('E')
        .is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()));
    matches!(
        word,
        "rust"
            | "ignore"
            | "should_panic"
            | "no_run"
            | "compile_fail"
            | "test_harness"
            | "allow_fail"
            | "standalone_crate"
    ) || word.starts_with("ignore-")
        || word.starts_with("edition")
        || word.starts_with('{')
        || code
}

/// A line that ends any paragraph before it and starts none: an ATX
/// heading, a thematic break or a setext underline.
fn heading(text: &str) -> bool {
    let hashes = text.chars().take_while(|&c| c == '#').count();
    let atx = (1..=6).contains(&hashes)
        && text[hashes..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace);
    let rule = text.chars().next().is_some_and(|first| {
        matches!(first, '-' | '=' | '*' | '_')
            && text.chars().all(|c| c == first || c == ' ' || c == '\t')
    });
    atx || rule
}

/// Where a list item's text starts in `text` (`- a`, `1. a`, `2) a`).
fn list_item(text: &str) -> Option<usize> {
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let marker = match text.as_bytes().get(digits) {
        Some(b'-' | b'*' | b'+') if digits == 0 => 1,
        Some(b'.' | b')') if (1..=9).contains(&digits) => digits + 1,
        _ => return None,
    };
    let spaces = text[marker..].chars().take_while(|c| *c == ' ').count();
    match spaces {
        0 => None,
        1..=4 => Some(marker + spaces),
        _ => Some(marker + 1),
    }
}

/// A token directly inside a `{ }` group of a token tree, where it may be
/// field-init shorthand: tokens don't say whether `{` opens a struct.
fn in_braces(n: &Node<'_>) -> bool {
    n.parent().is_some_and(|p| {
        p.kind() == "token_tree" && p.child(0).is_some_and(|open| open.kind() == "{")
    })
}

fn is_comment(n: &Node<'_>) -> bool {
    matches!(n.kind(), "line_comment" | "block_comment")
}

struct Walker<'a> {
    src: &'a [u8],
    b: &'a mut TreeBuilder,
    /// Names bound by a pattern in each enclosing function, innermost last.
    locals: Vec<HashSet<String>>,
    /// The members being emitted are named by more than Rust code (a derive
    /// or attribute reads their names), so they aren't renamable.
    opaque_members: bool,
    /// Start bytes of doc comments holding doctest code.
    doctests: HashSet<usize>,
}

impl<'a> Walker<'a> {
    /// Marked plain `pub` (not `pub(crate)` or `pub(super)`), or an item of a
    /// public trait or a variant of a public enum, which take its visibility.
    fn public(&self, n: &Node<'_>) -> bool {
        let mut cursor = n.walk();
        let own = n
            .children(&mut cursor)
            .find(|c| c.kind() == "visibility_modifier");
        if let Some(own) = own {
            return self.text(&own) == "pub";
        }
        let container = n
            .parent()
            .and_then(|list| Some((list.kind(), list.parent()?)));
        match container {
            Some(("declaration_list", item)) if item.kind() == "trait_item" => self.public(&item),
            Some(("enum_variant_list", item)) if item.kind() == "enum_item" => self.public(&item),
            _ => false,
        }
    }

    fn text(&self, n: &Node<'_>) -> &'a str {
        std::str::from_utf8(&self.src[span(n)]).unwrap_or_default()
    }

    /// A doc comment's fenced code is a doctest, compiled and run: code.
    fn comment(&mut self, n: &Node<'_>) {
        if self.doctests.contains(&n.start_byte()) {
            self.b.directive(span(n));
        } else {
            self.b.comment(span(n));
        }
    }

    fn is_local(&self, name: &str) -> bool {
        self.locals.last().is_some_and(|l| l.contains(name))
    }

    /// `n`'s children in source order: comments and errors reported, named
    /// children emitted by `plan`, attributes folded into the next child.
    fn children(&mut self, n: Node<'_>, mode: Mode, skip: &[Node<'_>]) {
        self.children_with(n, &mut |_, _| Plan::Emit(mode), skip);
    }

    fn children_with(
        &mut self,
        n: Node<'_>,
        plan: &mut dyn FnMut(Option<&str>, &Node<'_>) -> Plan,
        skip: &[Node<'_>],
    ) {
        let mut attrs: Vec<Node<'_>> = Vec::new();
        let mut cursor = n.walk();
        let kids: Vec<_> = n
            .children(&mut cursor)
            .enumerate()
            .map(|(i, c)| (n.field_name_for_child(i as u32), c))
            .collect();
        for (i, &(field, child)) in kids.iter().enumerate() {
            if is_comment(&child) {
                self.comment(&child);
                continue;
            }
            if child.is_missing() {
                self.b.error(span(&child));
                continue;
            }
            if skip.iter().any(|s| s.id() == child.id()) {
                continue;
            }
            let chosen = plan(field, &child);
            if !child.is_named() && !child.is_error() {
                if chosen == Plan::Emit(Mode::Tokens) {
                    self.token(child);
                } else if child.kind() == "," && trailing(&n, &kids[i + 1..]) {
                    self.b.ignore(span(&child));
                }
                continue;
            }
            if child.kind() == "attribute_item" && chosen != Plan::Emit(Mode::Tokens) {
                attrs.push(child);
                continue;
            }
            let held = std::mem::take(&mut attrs);
            match chosen {
                Plan::Emit(mode) => self.emit(child, mode, &held),
                Plan::Name(role) => {
                    let name = self.text(&child);
                    self.leaf(&child, Some(name), role);
                }
                Plan::Splice => self.children(child, Mode::Expr, &[]),
                Plan::Annotate => {
                    self.b.open(
                        "annotation",
                        span(&child),
                        None,
                        crate::tree::Role::Annotation,
                    );
                    self.emit(child, Mode::Expr, &held);
                    self.b.close();
                }
            }
        }
        // Attributes with no item after them (the end of a body).
        for attr in attrs {
            self.emit(attr, Mode::Expr, &[]);
        }
    }

    /// Opens `kind` over `n` and the attributes before it, which become its
    /// first children.
    fn open(
        &mut self,
        n: &Node<'_>,
        attrs: &[Node<'_>],
        name: Option<&str>,
        role: crate::tree::Role,
    ) {
        let start = attrs.first().map_or(n.start_byte(), |a| a.start_byte());
        // The block owns every statement's `;`, a `let`'s as much as an
        // expression statement's, so statements compare alike.
        let end = match n.child(n.child_count().saturating_sub(1)) {
            Some(last) if n.kind() == "let_declaration" && last.kind() == ";" => last.start_byte(),
            _ => n.end_byte(),
        };
        self.b
            .open(kind(n), start..end, name.map(str::as_bytes), role);
        if role.defines().is_some() && self.public(n) {
            self.b.flag(Flags::PUBLIC);
        }
        // An attribute macro may read the item's tokens as written.
        if roles::reads_tokens(attrs, self.src) {
            self.b.flag(Flags::SPELLS);
        }
        for attr in attrs {
            self.emit(*attr, Mode::Expr, &[]);
        }
    }

    /// A node compared byte for byte.
    fn leaf(&mut self, n: &Node<'_>, name: Option<&str>, role: crate::tree::Role) {
        self.b.leaf(kind(n), span(n), name.map(str::as_bytes), role);
    }

    fn emit(&mut self, n: Node<'_>, mode: Mode, attrs: &[Node<'_>]) {
        if n.is_error() {
            self.b.error(span(&n));
            self.b
                .open("ERROR", span(&n), None, crate::tree::Role::Other);
            self.children(n, mode, &[]);
            self.b.close();
            return;
        }
        if mode == Mode::Tokens {
            return self.token(n);
        }
        self.node(n, mode, attrs);
    }

    /// Inside a token tree: every token a leaf, every identifier opaque.
    fn token(&mut self, n: Node<'_>) {
        use crate::tree::{Hazard, Role};
        if is_comment(&n) {
            return self.comment(&n);
        }
        match n.kind() {
            "identifier" => {
                let name = self.text(&n);
                let bang = n.next_sibling().is_some_and(|s| s.kind() == "!");
                let role = match name {
                    "line" | "column" if bang => Role::Hazard(Hazard::Line),
                    "file" | "module_path" if bang => Role::Hazard(Hazard::File),
                    _ => Role::Opaque,
                };
                if role == Role::Opaque && in_braces(&n) {
                    // Possibly `S { a }` shorthand, naming a field too.
                    self.member(&n, Some(name));
                    return self.b.close();
                }
                self.leaf(&n, Some(name), role);
            }
            "string_literal" => self.format_string(n),
            _ if n.child_count() == 0 || n.kind() == "raw_string_literal" => {
                let role = match (n.is_named(), n.kind()) {
                    (false, "return" | "?") => Role::Hazard(Hazard::Return),
                    (false, "break") => Role::Hazard(Hazard::Break),
                    (false, "continue") => Role::Hazard(Hazard::Next),
                    (false, "await") => Role::Hazard(Hazard::Await),
                    (false, "yield") => Role::Hazard(Hazard::Yield),
                    _ => Role::Other,
                };
                self.leaf(&n, None, role);
            }
            _ => {
                self.b.open(kind(&n), span(&n), None, Role::Other);
                // `name!(...)` nested in another macro's tokens.
                let bang = n.prev_sibling().filter(|b| b.kind() == "!");
                let name = bang
                    .and_then(|b| b.prev_sibling())
                    .filter(|i| i.kind() == "identifier");
                if name.is_some_and(|i| roles::spells(self.text(&i), Some(n))) {
                    self.b.flag(Flags::SPELLS);
                }
                self.children(n, Mode::Tokens, &[]);
                self.b.close();
            }
        }
    }

    /// A string in a token tree, which a formatting macro may read names
    /// from (`"{count}"`): its content is leaves, those names opaque.
    fn format_string(&mut self, n: Node<'_>) {
        use crate::tree::Role;
        let text = self.text(&n).as_bytes();
        let (Some(open), Some(close)) = (
            text.iter().position(|&b| b == b'"'),
            text.iter().rposition(|&b| b == b'"'),
        ) else {
            return self.leaf(&n, None, Role::Other);
        };
        let base = n.start_byte();
        self.b.open(kind(&n), span(&n), None, Role::Other);
        let mut piece = open + 1;
        let mut i = open + 1;
        while i < close {
            if text[i] == b'{' && text.get(i + 1) == Some(&b'{') {
                i += 2;
                continue;
            }
            let ident_len = text[i + 1..close]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
                .count();
            let starts_ident = text
                .get(i + 1)
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_');
            let ends = text.get(i + 1 + ident_len);
            if text[i] == b'{' && starts_ident && matches!(ends, Some(b'}' | b':')) {
                let from = i + 1;
                let to = from + ident_len;
                if piece < from {
                    self.b.leaf(
                        "string_content",
                        base + piece..base + from,
                        None,
                        Role::Other,
                    );
                }
                let name = &text[from..to];
                self.b.leaf(
                    "identifier",
                    base + from..base + to,
                    Some(name),
                    Role::Opaque,
                );
                piece = to;
                i = to;
                continue;
            }
            i += 1;
        }
        if piece < close {
            self.b.leaf(
                "string_content",
                base + piece..base + close,
                None,
                Role::Other,
            );
        }
        self.b.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Hazard, NameKind::*, Receiver, Role};

    /// Lists `Cargo.toml` and `src/lib.rs`, but reads nothing.
    struct Unreadable;

    impl Snapshot for Unreadable {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            None
        }

        fn list(&self, dir: &str) -> Vec<String> {
            match dir {
                "" => vec!["Cargo.toml".into()],
                "src" => vec!["src/lib.rs".into()],
                _ => Vec::new(),
            }
        }
    }

    #[test]
    fn an_unreadable_manifest_counts_as_a_library() {
        assert!(Rust.library("src/lib.rs", &Unreadable));
    }

    fn parse(src: &str) -> Tree {
        Rust.parse(src.as_bytes().to_vec())
    }

    /// Every node with a role, as (role, name).
    fn roles(src: &str) -> Vec<(Role, Option<String>)> {
        let tree = parse(src);
        assert!(tree.errors.is_empty(), "{src}");
        tree.nodes
            .iter()
            .filter(|n| n.role != Role::Other)
            .map(|n| (n.role, n.name.as_deref().map(str::to_string)))
            .collect()
    }

    fn assert_has(found: &[(Role, Option<String>)], want: &[(Role, &str)]) {
        for (role, name) in want {
            assert!(
                found
                    .iter()
                    .any(|(r, n)| r == role && n.as_deref() == Some(*name)),
                "{role:?} {name}: {found:?}"
            );
        }
    }

    #[test]
    fn recognizes_rust_paths() {
        assert!(Rust.handles("src/main.rs"));
        assert!(!Rust.handles("a.rb") && !Rust.handles("rs") && !Rust.handles("Cargo.toml"));
    }

    #[test]
    fn code_blocks_follow_rustdocs_markdown() {
        let code = |doc: &str| code_lines(&doc.lines().collect::<Vec<_>>());
        assert_eq!(
            code("````\n```\nx\n````\ny"),
            [true, true, true, true, false]
        );
        assert_eq!(code("```sh\nx\n```"), [false; 3]);
        assert_eq!(code("```should_panic,sh\nx\n```"), [true; 3]);
        assert_eq!(code("Title\n=====\n    x"), [false, false, true]);
        assert_eq!(code("a\n    b"), [false, false]);
        assert_eq!(code("> > ```\n> > x\n> > ```"), [true; 3]);
    }

    #[test]
    fn comments_and_doc_comments_are_comments_and_attributes_code() {
        let tree = parse(
            "//! Crate docs.\n#![allow(dead_code)]\n/// Item docs.\n/** Block docs. */\n\
             #[doc = \"x\"]\nfn a() {\n    // note\n    /* inline */\n}\n",
        );
        let comments = tree.nodes.iter().filter(|n| n.comment).count();
        assert_eq!(comments, 5);
        assert!(tree.line(1).comment && !tree.line(1).code);
        assert!(tree.line(2).code && !tree.line(2).comment);
        assert!(tree.line(5).code);
    }

    #[test]
    fn whitespace_is_insignificant_except_in_literals() {
        let root = |s: &str| parse(s).node(Tree::ROOT).hash;
        assert_eq!(root("fn a() { b(1,2) }"), root("fn a() {\n    b(1, 2)\n}"));
        assert_ne!(root("fn a() { \"x y\" }"), root("fn a() { \"x  y\" }"));
        // Macro tokens are compared one by one: `<<` is not `< <`.
        assert_ne!(root("m!(a << b);"), root("m!(a < < b);"));
        assert_eq!(root("m!(a, b);"), root("m!( a ,b );"));
    }

    #[test]
    fn attributes_belong_to_the_item_after_them() {
        let tree = parse("#[test]\nfn a() {}\nfn b() {}\n");
        let root = tree.node(Tree::ROOT);
        assert_eq!(root.children.len(), 2);
        let a = tree.node(root.children[0]);
        assert_eq!(a.name.as_deref(), Some("a"));
        assert_eq!(tree.node(a.children[0]).kind, "attribute_item");
    }

    #[test]
    fn roles_mark_definitions() {
        let found = roles(
            "mod shop {\n  pub struct Cart<T> { items: Vec<T> }\n  enum Size { Small, Large(u8) }\n  \
             trait Priced {}\n  union U { a: u8 }\n  type Id = u64;\n  const MAX: u32 = 3;\n  \
             static mut SEEN: u32 = 0;\n  macro_rules! twice { ($x:expr) => { $x * 2 }; }\n  \
             impl Cart<u8> {\n    fn add(&mut self, item: u8, (a, b): (u8, u8)) {\n      \
             let total = item;\n      let mut n: u8 = 1;\n      let (p, q) = (a, b);\n      \
             let f = |x, y: u8| x + y;\n      if let Some(v) = self.items.pop() {}\n      \
             for i in 0..n {}\n      match n { 0 => {}, m => {} }\n      n = total;\n      n += 1;\n    }\n  }\n}\n",
        );
        assert_has(
            &found,
            &[
                (Role::Definition(Module), "shop"),
                (Role::Reference(Module), "shop"),
                (Role::Definition(Class), "Cart"),
                (Role::Reference(Class), "Cart"),
                (Role::Definition(Member), "items"),
                (Role::Definition(Class), "Size"),
                (Role::Definition(Constant), "Small"),
                (Role::Definition(Constant), "Large"),
                (Role::Definition(Module), "Priced"),
                (Role::Definition(Class), "U"),
                (Role::Definition(Constant), "Id"),
                (Role::Definition(Constant), "MAX"),
                (Role::Definition(Constant), "SEEN"),
                (Role::Definition(Macro), "twice"),
                (Role::Definition(Method), "add"),
                (Role::Definition(SelfParameter), "self"),
                (Role::Definition(Parameter), "item"),
                (Role::Definition(Parameter), "a"),
                (Role::Definition(Parameter), "b"),
                (Role::Bind, "total"),
                (Role::Definition(Variable), "n"),
                (Role::Definition(Variable), "p"),
                (Role::Definition(Parameter), "x"),
                (Role::Definition(Parameter), "y"),
                (Role::Definition(Variable), "v"),
                (Role::Definition(Variable), "i"),
                (Role::Definition(Variable), "m"),
                (Role::Reference(Variable), "total"),
                (Role::Reference(Constant), "Some"),
            ],
        );
        let impl_role = found
            .iter()
            .any(|(r, n)| *r == Role::Definition(Class) && n.is_none());
        assert!(impl_role, "an impl is a nameless namespace");
        // `n = total` binds; `n += 1` writes.
        assert_eq!(
            found
                .iter()
                .filter(|(r, n)| *r == Role::Bind && n.as_deref() == Some("n"))
                .count(),
            1
        );
    }

    #[test]
    fn roles_mark_references_and_calls() {
        let found = roles(
            "fn run(list: Vec<u8>, p: Point) -> u8 {\n  helper(list.len());\n  p.x;\n  list.push(1);\n  \
             self.go();\n  Self::make();\n  Point::origin();\n  std::mem::take(&mut p);\n  \
             list.iter().map(helper);\n  MAX + Size::Small as u8\n}\n",
        );
        assert_has(
            &found,
            &[
                (Role::Call(Receiver::Implicit), "helper"),
                (Role::Call(Receiver::Explicit), "len"),
                (Role::Reference(Member), "x"),
                (Role::Call(Receiver::Explicit), "push"),
                (Role::Call(Receiver::SelfRef), "go"),
                (Role::Call(Receiver::SelfRef), "make"),
                (Role::Call(Receiver::Explicit), "origin"),
                (Role::Call(Receiver::Explicit), "take"),
                (Role::Reference(Module), "mem"),
                (Role::Reference(Method), "helper"),
                (Role::Reference(Constant), "MAX"),
                (Role::Reference(Constant), "Small"),
                (Role::Reference(Class), "Size"),
                (Role::Reference(Class), "Point"),
                (Role::Reference(Variable), "list"),
            ],
        );
        // A call's receiver is its first child, as the core expects.
        let tree = parse("fn a(v: Vec<u8>) { v.push(1); }");
        let call = tree
            .nodes
            .iter()
            .find(|n| n.role == Role::Call(Receiver::Explicit))
            .unwrap();
        assert_eq!(tree.node(call.children[0]).name.as_deref(), Some("v"));
    }

    #[test]
    fn roles_mark_jumps() {
        let found = roles(
            "async fn run(v: Vec<u8>) -> Result<u8, E> {\n  let a = parse(v)?;\n  fetch().await;\n  \
             'outer: loop { break 'outer; }\n  for x in v { continue; }\n  bail!(\"no\");\n  \
             println!(\"{}\", a);\n  return Ok(a);\n}\n",
        );
        for jump in [
            Hazard::Return,
            Hazard::Await,
            Hazard::Break,
            Hazard::Next,
            Hazard::Macro,
        ] {
            assert!(
                found.iter().any(|(r, _)| *r == Role::Hazard(jump)),
                "{jump:?}"
            );
        }
        assert_has(&found, &[(Role::Reference(Macro), "println")]);
        let position = roles("fn a() { line!(); file!(); m!(column!(), module_path!(), line); }");
        let position: Vec<Role> = position.into_iter().map(|(r, _)| r).collect();
        assert_eq!(
            position
                .iter()
                .filter(|r| matches!(r, Role::Hazard(Hazard::Line | Hazard::File)))
                .count(),
            4
        );
        let jumping_tokens = roles("fn a() { m!(return); m!(x?); }");
        assert_eq!(
            jumping_tokens
                .iter()
                .filter(|(r, _)| *r == Role::Hazard(Hazard::Return))
                .count(),
            2
        );
    }

    #[test]
    fn macro_tokens_and_format_names_are_opaque() {
        let found = roles("fn a(n: u8) { println!(\"{} {n:>4} {{n}}\", n + count); }");
        let opaque: Vec<&str> = found
            .iter()
            .filter(|(r, _)| *r == Role::Opaque)
            .filter_map(|(_, n)| n.as_deref())
            .collect();
        assert_eq!(opaque, ["n", "n", "count"]);
        assert_has(&roles("fn a(v: u8) { S { v }; }"), &[(Role::Opaque, "v")]);
    }

    #[test]
    fn an_assert_spells_its_condition_unless_a_message_surely_follows() {
        let spells = |src: &str| {
            parse(src)
                .nodes
                .iter()
                .any(|n| n.flags.contains(crate::tree::Flags::SPELLS))
        };
        assert!(spells("fn f() { assert!(a > 0); }"));
        assert!(spells("fn f() { assert!(a > 0,); }"));
        assert!(spells("fn f() { assert!(g::<u8, u16>(a)); }"));
        assert!(spells(
            "fn f() { assert!((|x, y| x > y)(a, 0) || a < b, \"m\"); }"
        ));
        assert!(!spells("fn f() { assert!(a > 0, \"m\"); }"));
        assert!(!spells("fn f() { assert!(f(a, b), \"m {}\", a < b); }"));
    }

    #[test]
    fn items_bound_locals_and_closures_do_not() {
        let tree =
            parse("mod m { #[derive(Clone)] struct S; impl S { fn a() { let f = |x| x; } } }");
        let scopes: Vec<&str> = tree
            .nodes
            .iter()
            .filter(|n| n.flags.contains(Flags::SCOPE))
            .map(|n| n.kind)
            .collect();
        assert_eq!(
            scopes,
            ["mod_item", "struct_item", "impl_item", "function_item"]
        );
    }

    #[test]
    fn plain_pub_items_and_their_public_traits_and_enums_are_marked() {
        let tree = parse(
            "pub fn a() {} pub(crate) fn b() {} fn c() {} \
             pub trait T { fn d(); } trait U { fn e(); } \
             pub enum E { F } pub struct S { pub g: u8, h: u8 } \
             impl S { pub fn i() {} fn j() {} }",
        );
        let public: Vec<&str> = tree
            .nodes
            .iter()
            .filter(|n| n.flags.contains(Flags::PUBLIC))
            .filter_map(|n| n.name.as_deref())
            .collect();
        assert_eq!(public, ["a", "T", "d", "E", "F", "S", "g", "i"]);
    }

    #[test]
    fn argument_lists_are_marked() {
        let tree = parse("fn a(x: u8) { f(x); self.g(); }");
        let lists = tree
            .nodes
            .iter()
            .filter(|n| n.flags.contains(Flags::ARGUMENTS))
            .count();
        assert_eq!(lists, 2);
    }

    #[test]
    fn annotations_mark_types_generics_and_visibility() {
        let tree = parse("pub fn a<T>(x: T) -> T where T: Copy { x }");
        let annotations = tree
            .nodes
            .iter()
            .filter(|n| n.role == Role::Annotation)
            .map(|n| n.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            annotations,
            [
                "visibility_modifier",
                "type_parameters",
                "annotation",
                "annotation",
                "where_clause"
            ]
        );
    }

    #[test]
    fn members_named_by_a_derive_are_not_definitions() {
        let named = |src: &str| {
            roles(src)
                .iter()
                .any(|(r, n)| *r == Role::Definition(Member) && n.as_deref() == Some("x"))
        };
        assert!(named("#[derive(Clone, PartialEq)]\nstruct P { x: u8 }"));
        assert!(!named("#[derive(Debug)]\nstruct P { x: u8 }"));
        assert!(!named("#[derive(serde::Serialize)]\nstruct P { x: u8 }"));
        assert!(!named(
            "#[serde(rename_all = \"camelCase\")]\nstruct P { x: u8 }"
        ));
        assert!(!named(
            "struct P {\n  #[serde(rename = \"y\")]\n  x: u8,\n}"
        ));
    }

    #[test]
    fn the_language_is_file_scoped_with_scoped_drops() {
        let semantics = parse("").semantics;
        assert!(semantics.file_scoped && semantics.scoped_drop);
    }

    #[test]
    fn syntax_errors_are_recorded() {
        assert!(!parse("fn a( {").errors.is_empty());
        assert!(parse("fn a() {}").errors.is_empty());
    }
}
