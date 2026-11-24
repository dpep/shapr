//! Rust, parsed with tree-sitter.
//!
//! The concrete syntax tree is mapped onto the shape the core expects:
//! punctuation stays in its parent's own text, a statement's `;` wrapper and
//! a module or impl body's braces fold into the enclosing node, attributes
//! become the first children of the item they precede, and a namespace's
//! header (attributes, name, generics) is its first child. A macro's token
//! tree can't be parsed, so every token in it is a leaf and every identifier
//! is [`crate::tree::Role::Opaque`].

mod roles;

use super::Frontend;
use crate::tree::{Semantics, Span, Tree, TreeBuilder};
use roles::{Mode, Plan};
use std::collections::HashSet;
use std::sync::OnceLock;
use tree_sitter::{Node, Parser};

pub struct Rust;

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
            }
            .children(cst.root_node(), Mode::Expr, &[]),
            None => builder.error(0..source.len()),
        }
        builder.finish()
    }
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
}

impl<'a> Walker<'a> {
    fn text(&self, n: &Node<'_>) -> &'a str {
        std::str::from_utf8(&self.src[span(n)]).unwrap_or_default()
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
        for (field, child) in kids {
            if is_comment(&child) {
                self.b.comment(span(&child));
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
        use crate::tree::{Jump, Role};
        if is_comment(&n) {
            return self.b.comment(span(&n));
        }
        match n.kind() {
            "identifier" => {
                let name = self.text(&n);
                self.leaf(&n, Some(name), Role::Opaque);
            }
            "string_literal" => self.format_string(n),
            _ if n.child_count() == 0 || n.kind() == "raw_string_literal" => {
                let role = match (n.is_named(), n.kind()) {
                    (false, "return" | "?") => Role::Jump(Jump::Return),
                    (false, "break") => Role::Jump(Jump::Break),
                    (false, "continue") => Role::Jump(Jump::Next),
                    (false, "await") => Role::Jump(Jump::Await),
                    (false, "yield") => Role::Jump(Jump::Yield),
                    _ => Role::Other,
                };
                self.leaf(&n, None, role);
            }
            _ => {
                self.b.open(kind(&n), span(&n), None, Role::Other);
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
    use crate::tree::{Jump, NameKind::*, Receiver, Role};

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
            Jump::Return,
            Jump::Await,
            Jump::Break,
            Jump::Next,
            Jump::Macro,
        ] {
            assert!(
                found.iter().any(|(r, _)| *r == Role::Jump(jump)),
                "{jump:?}"
            );
        }
        assert_has(&found, &[(Role::Reference(Macro), "println")]);
        let jumping_tokens = roles("fn a() { m!(return); m!(x?); }");
        assert_eq!(
            jumping_tokens
                .iter()
                .filter(|(r, _)| *r == Role::Jump(Jump::Return))
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
