//! Which Rust syntax plays which [`Role`], and what each node keeps in its
//! own text (the name it declares or calls) instead of as a child.

use super::{Walker, span};
use crate::frontend::Construct;
use crate::tree::{Hazard, NameKind, NodeId, Receiver, Role, Tree};
use std::collections::HashSet;
use tree_sitter::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Expr,
    /// A pattern: a lowercase identifier binds a name of this kind.
    Pattern(NameKind),
    /// A path's prefix (`a::b` in `a::b::c`): modules and types.
    Path,
    /// A macro's token tree or an attribute's arguments.
    Tokens,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Plan {
    Emit(Mode),
    /// A leaf carrying its text as its name, in this role.
    Name(Role),
    /// Its children stand in its place (a body's braces fold into the item).
    Splice,
    /// Wrapped in an `annotation` node: a type.
    Annotate,
}

/// Macros from std that expand to no control flow of their own.
const PLAIN_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "cfg",
    "compile_error",
    "concat",
    "dbg",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "env",
    "eprint",
    "eprintln",
    "format",
    "format_args",
    "include_bytes",
    "include_str",
    "matches",
    "option_env",
    "panic",
    "print",
    "println",
    "stringify",
    "todo",
    "unimplemented",
    "unreachable",
    "vec",
    "write",
    "writeln",
];

/// Derives that never read a member's name; any other derive (`Debug`,
/// `Serialize`, a CLI parser) makes field and variant names observable.
const NAMELESS_DERIVES: &[&str] = &[
    "Clone",
    "Copy",
    "Default",
    "Eq",
    "Hash",
    "Ord",
    "PartialEq",
    "PartialOrd",
];

/// Attributes that don't read the names of the item or its members.
const NAMELESS_ATTRIBUTES: &[&str] = &[
    "allow",
    "cfg",
    "cold",
    "deny",
    "deprecated",
    "doc",
    "expect",
    "forbid",
    "ignore",
    "inline",
    "must_use",
    "non_exhaustive",
    "repr",
    "rustfmt::skip",
    "should_panic",
    "test",
    "track_caller",
    "warn",
];

fn upper(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
}

impl Walker<'_> {
    pub(super) fn node(&mut self, n: Node<'_>, mode: Mode, attrs: &[Node<'_>]) {
        use NameKind::*;
        let field = |f: &str| n.child_by_field_name(f);
        match (mode, n.kind()) {
            (_, "expression_statement") => self.children(n, mode, &[]),

            // Items.
            (_, "function_item" | "function_signature_item") => {
                let name = field("name");
                let locals = bindings(n, self.src);
                self.locals.push(locals);
                self.open(
                    &n,
                    attrs,
                    name.map(|x| self.text(&x)),
                    Role::Definition(Method),
                );
                self.children_with(
                    n,
                    &mut |f, _| match f {
                        Some("return_type") => Plan::Annotate,
                        _ => Plan::Emit(Mode::Expr),
                    },
                    &name.into_iter().collect::<Vec<_>>(),
                );
                self.b.close();
                self.locals.pop();
            }
            (_, "struct_item" | "union_item" | "enum_item") => self.namespace(n, attrs, Class),
            (_, "trait_item" | "mod_item") => self.namespace(n, attrs, Module),
            (_, "impl_item") => {
                self.open(&n, &[], None, Role::Definition(Class));
                let body = field("body");
                let start = attrs.first().map_or(n.start_byte(), |a| a.start_byte());
                let end = body.map_or(n.end_byte(), |b| b.start_byte());
                self.b.open("header", start..end, None, Role::Other);
                for attr in attrs {
                    self.emit(*attr, Mode::Expr, &[]);
                }
                self.children(n, Mode::Expr, &body.into_iter().collect::<Vec<_>>());
                self.b.close();
                if let Some(body) = body {
                    self.children(body, Mode::Expr, &[]);
                }
                self.b.close();
            }
            (_, "const_item" | "static_item" | "type_item") => {
                self.named(n, attrs, Role::Definition(Constant))
            }
            (_, "macro_definition") => {
                let name = field("name");
                self.open(
                    &n,
                    attrs,
                    name.map(|x| self.text(&x)),
                    Role::Definition(Macro),
                );
                self.children(n, Mode::Tokens, &name.into_iter().collect::<Vec<_>>());
                self.b.close();
            }
            (_, "enum_variant") => {
                let role = if self.opaque_members || !attrs.is_empty() {
                    Role::Other
                } else {
                    Role::Definition(Constant)
                };
                self.named(n, attrs, role);
            }
            (_, "field_declaration") => {
                let role = if self.opaque_members || !attrs.is_empty() {
                    Role::Other
                } else {
                    Role::Definition(Member)
                };
                self.named(n, attrs, role);
            }
            (_, "attribute_item" | "inner_attribute_item") => {
                let exposes = n.kind() == "attribute_item" && names_observed(&[n], self.src);
                let role = if exposes { Role::Exposes } else { Role::Other };
                self.open(&n, &[], None, role);
                self.children(n, Mode::Tokens, &[]);
                self.b.close();
            }
            (_, "type_parameters" | "where_clause" | "visibility_modifier") => {
                self.branch(n, attrs, None, Role::Annotation, mode, &[])
            }

            // Bindings.
            (_, "let_declaration") => {
                let pattern = field("pattern");
                let simple = pattern.filter(|p| p.kind() == "identifier");
                let role = match simple {
                    Some(_)
                        if field("value").is_some()
                            && field("type").is_none()
                            && field("alternative").is_none() =>
                    {
                        Role::Bind
                    }
                    _ => Role::Definition(Variable),
                };
                let name = simple.map(|p| self.text(&p));
                self.open(&n, attrs, name, role);
                let mut skip: Vec<Node<'_>> = simple.into_iter().collect();
                if simple.is_some() {
                    let mut c = n.walk();
                    skip.extend(
                        n.children(&mut c)
                            .filter(|k| k.kind() == "mutable_specifier"),
                    );
                }
                self.children_with(
                    n,
                    &mut |f, _| match f {
                        Some("type") => Plan::Annotate,
                        Some("pattern") => Plan::Emit(Mode::Pattern(Variable)),
                        _ => Plan::Emit(Mode::Expr),
                    },
                    &skip,
                );
                self.b.close();
            }
            (_, "parameter") => {
                let pattern = field("pattern").filter(|p| p.kind() == "identifier");
                let (name, role) = match pattern {
                    Some(p) => (Some(self.text(&p)), Role::Definition(Parameter)),
                    None => (None, Role::Other),
                };
                self.open(&n, attrs, name, role);
                self.children_with(
                    n,
                    &mut |f, _| match f {
                        Some("type") => Plan::Annotate,
                        Some("pattern") => Plan::Emit(Mode::Pattern(Parameter)),
                        _ => Plan::Emit(Mode::Expr),
                    },
                    &pattern.into_iter().collect::<Vec<_>>(),
                );
                self.b.close();
            }
            (_, "self_parameter") => self.branch(
                n,
                attrs,
                Some("self"),
                Role::Definition(SelfParameter),
                mode,
                &[],
            ),
            (_, "closure_parameters") => {
                self.branch(n, attrs, None, Role::Other, Mode::Pattern(Parameter), &[])
            }
            (_, "let_condition" | "for_expression") => {
                self.open(&n, attrs, None, Role::Other);
                self.children_with(
                    n,
                    &mut |f, _| match f {
                        Some("pattern") => Plan::Emit(Mode::Pattern(Variable)),
                        _ => Plan::Emit(Mode::Expr),
                    },
                    &[],
                );
                self.b.close();
            }
            (_, "match_pattern") => {
                self.open(&n, attrs, None, Role::Other);
                self.children_with(
                    n,
                    &mut |f, _| match f {
                        Some("condition") => Plan::Emit(Mode::Expr),
                        _ => Plan::Emit(Mode::Pattern(Variable)),
                    },
                    &[],
                );
                self.b.close();
            }
            (_, "assignment_expression" | "compound_assignment_expr") => {
                let left = field("left")
                    .filter(|l| l.kind() == "identifier" && self.is_local(self.text(l)));
                let role = match (left, n.kind()) {
                    (None, _) => Role::Other,
                    (Some(_), "assignment_expression") => Role::Bind,
                    (Some(_), _) => Role::Definition(Variable),
                };
                let name = left.map(|l| self.text(&l));
                self.branch(
                    n,
                    attrs,
                    name,
                    role,
                    Mode::Expr,
                    &left.into_iter().collect::<Vec<_>>(),
                );
            }

            // Patterns.
            (Mode::Pattern(kind), "identifier") => {
                let name = self.text(&n);
                let role = if upper(name) {
                    Role::Reference(Constant)
                } else {
                    Role::Definition(kind)
                };
                self.leaf(&n, Some(name), role);
            }
            (Mode::Pattern(kind), "tuple_struct_pattern") => {
                self.open(&n, attrs, None, Role::Other);
                self.children_with(
                    n,
                    &mut |f, _| match f {
                        Some("type") => Plan::Emit(Mode::Expr),
                        _ => Plan::Emit(Mode::Pattern(kind)),
                    },
                    &[],
                );
                self.b.close();
            }
            (Mode::Pattern(_), "field_pattern") => match field("pattern") {
                Some(_) => {
                    let name = field("name");
                    let label = name.map(|x| self.text(&x));
                    self.branch(
                        n,
                        attrs,
                        label,
                        Role::Reference(Member),
                        mode,
                        &name.into_iter().collect::<Vec<_>>(),
                    );
                }
                // `S { a }` binds `a` and names the field `a`.
                None => {
                    let name = field("name").map(|x| self.text(&x));
                    self.leaf(&n, name, Role::Opaque);
                }
            },
            (Mode::Pattern(_), "scoped_identifier") => {
                self.path(n, attrs, Role::Reference(Constant))
            }

            // Paths and names.
            (Mode::Path, "identifier") => {
                let name = self.text(&n);
                let role = if upper(name) && name != "Self" {
                    Role::Reference(Class)
                } else if name == "Self" {
                    Role::Other
                } else {
                    Role::Reference(Module)
                };
                self.leaf(&n, Some(name), role);
            }
            (_, "identifier") => {
                let name = self.text(&n);
                let role = if name == "Self" {
                    Role::Other
                } else if self.is_local(name) {
                    Role::Reference(Variable)
                } else if upper(name) {
                    Role::Reference(Constant)
                } else {
                    Role::Reference(Method)
                };
                self.leaf(&n, Some(name), role);
            }
            (_, "type_identifier") => {
                let name = self.text(&n);
                let role = if name == "Self" {
                    Role::Other
                } else {
                    Role::Reference(Class)
                };
                self.leaf(&n, Some(name), role);
            }
            (_, "field_identifier") => self.leaf(&n, Some(self.text(&n)), Role::Reference(Member)),
            (_, "scoped_identifier") => {
                let last = field("name").map(|x| self.text(&x)).unwrap_or_default();
                let role = match mode {
                    Mode::Path if upper(last) => Role::Reference(Class),
                    Mode::Path => Role::Reference(Module),
                    _ if upper(last) => Role::Reference(Constant),
                    _ => Role::Reference(Method),
                };
                self.path(n, attrs, role)
            }
            (_, "scoped_type_identifier") => self.path(n, attrs, Role::Reference(Class)),
            (_, "field_expression") => {
                let member = field("field").filter(|f| f.kind() == "field_identifier");
                let role = if member.is_some() {
                    Role::Reference(Member)
                } else {
                    Role::Other
                };
                let name = member.map(|m| self.text(&m));
                self.branch(
                    n,
                    attrs,
                    name,
                    role,
                    Mode::Expr,
                    &member.into_iter().collect::<Vec<_>>(),
                );
            }
            (_, "field_initializer") => {
                let member = field("field").filter(|f| f.kind() == "field_identifier");
                let role = if member.is_some() {
                    Role::Reference(Member)
                } else {
                    Role::Other
                };
                let name = member.map(|m| self.text(&m));
                self.branch(
                    n,
                    attrs,
                    name,
                    role,
                    Mode::Expr,
                    &member.into_iter().collect::<Vec<_>>(),
                );
            }
            (_, "shorthand_field_initializer") => self.leaf(&n, Some(self.text(&n)), Role::Opaque),
            (_, "call_expression") => self.call(n, attrs),
            (_, "macro_invocation") => {
                let mac = field("macro");
                let name = mac.map(|m| {
                    let t = self.text(&m);
                    t.rsplit("::").next().unwrap_or(t)
                });
                let role = match name {
                    Some("line" | "column") => Role::Hazard(Hazard::Line),
                    Some("file" | "module_path") => Role::Hazard(Hazard::File),
                    Some(name) if PLAIN_MACROS.contains(&name) => Role::Reference(Macro),
                    _ => Role::Hazard(Hazard::Macro),
                };
                self.open(&n, attrs, name, role);
                self.children_with(
                    n,
                    &mut |_, c| {
                        if c.kind() == "token_tree" {
                            Plan::Emit(Mode::Tokens)
                        } else {
                            Plan::Emit(Mode::Expr)
                        }
                    },
                    &mac.into_iter().collect::<Vec<_>>(),
                );
                self.b.close();
            }

            // Control flow bound to the enclosing function.
            (_, "return_expression" | "try_expression") => {
                self.branch(n, attrs, None, Role::Hazard(Hazard::Return), mode, &[])
            }
            (_, "break_expression") => {
                self.branch(n, attrs, None, Role::Hazard(Hazard::Break), mode, &[])
            }
            (_, "continue_expression") => {
                self.branch(n, attrs, None, Role::Hazard(Hazard::Next), mode, &[])
            }
            (_, "await_expression") => {
                self.branch(n, attrs, None, Role::Hazard(Hazard::Await), mode, &[])
            }
            (_, "yield_expression") => {
                self.branch(n, attrs, None, Role::Hazard(Hazard::Yield), mode, &[])
            }

            (
                _,
                "string_literal" | "raw_string_literal" | "char_literal" | "label" | "lifetime",
            ) => self.leaf(&n, None, Role::Other),
            _ if n.child_count() == 0 => self.leaf(&n, None, Role::Other),
            _ => self.branch(n, attrs, None, Role::Other, mode, &[]),
        }
    }

    fn branch(
        &mut self,
        n: Node<'_>,
        attrs: &[Node<'_>],
        name: Option<&str>,
        role: Role,
        mode: Mode,
        skip: &[Node<'_>],
    ) {
        self.open(&n, attrs, name, role);
        self.children(n, mode, skip);
        self.b.close();
    }

    /// An item declaring its `name` field, which stays in its own text.
    fn named(&mut self, n: Node<'_>, attrs: &[Node<'_>], role: Role) {
        let name = n.child_by_field_name("name");
        let label = name.filter(|_| role != Role::Other).map(|x| self.text(&x));
        self.open(&n, attrs, label, role);
        self.children_with(
            n,
            &mut |f, _| match f {
                Some("type") if n.kind() != "type_item" => Plan::Annotate,
                _ => Plan::Emit(Mode::Expr),
            },
            &name.into_iter().collect::<Vec<_>>(),
        );
        self.b.close();
    }

    /// `a::b::c`: the last name in its own text, the prefix a path child.
    fn path(&mut self, n: Node<'_>, attrs: &[Node<'_>], role: Role) {
        let name = n.child_by_field_name("name");
        let label = name.map(|x| self.text(&x));
        self.open(&n, attrs, label, role);
        self.children_with(
            n,
            &mut |f, _| match f {
                Some("path") => Plan::Emit(Mode::Path),
                _ => Plan::Emit(Mode::Expr),
            },
            &name.into_iter().collect::<Vec<_>>(),
        );
        self.b.close();
    }

    /// A struct, enum, union, trait or module. Its name is a child, a
    /// reference to it as the core expects (`class A` in Ruby); a module's
    /// or impl's braces fold into it, so its items are its children.
    fn namespace(&mut self, n: Node<'_>, attrs: &[Node<'_>], name_kind: NameKind) {
        let name = n.child_by_field_name("name");
        let label = name.map(|x| self.text(&x));
        self.open(&n, attrs, label, Role::Definition(name_kind));
        let opaque = std::mem::replace(&mut self.opaque_members, names_observed(attrs, self.src));
        self.children_with(
            n,
            &mut |f, c| match f {
                Some("name") => Plan::Name(Role::Reference(name_kind)),
                Some("body") if c.kind() == "declaration_list" => Plan::Splice,
                _ => Plan::Emit(Mode::Expr),
            },
            &[],
        );
        self.opaque_members = opaque;
        self.b.close();
    }

    fn call(&mut self, n: Node<'_>, attrs: &[Node<'_>]) {
        let Some(function) = n.child_by_field_name("function") else {
            return self.branch(n, attrs, None, Role::Other, Mode::Expr, &[]);
        };
        fn name_of<'t>(f: Node<'t>, field: &str) -> Option<Node<'t>> {
            f.child_by_field_name(field)
        }
        match function.kind() {
            "identifier" => {
                let name = self.text(&function);
                if self.is_local(name) || upper(name) {
                    return self.branch(n, attrs, None, Role::Other, Mode::Expr, &[]);
                }
                self.branch(
                    n,
                    attrs,
                    Some(name),
                    Role::Call(Receiver::Implicit),
                    Mode::Expr,
                    &[function],
                );
            }
            "field_expression" => {
                let (Some(value), Some(method)) =
                    (name_of(function, "value"), name_of(function, "field"))
                else {
                    return self.branch(n, attrs, None, Role::Other, Mode::Expr, &[]);
                };
                let receiver = if value.kind() == "self" {
                    Receiver::SelfRef
                } else {
                    Receiver::Explicit
                };
                let name = self.text(&method);
                self.open(&n, attrs, Some(name), Role::Call(receiver));
                // The receiver becomes the call's first child; `.name` its own text.
                self.emit(value, Mode::Expr, &[]);
                self.children(n, Mode::Expr, &[function]);
                self.b.close();
            }
            "scoped_identifier" => {
                let (path, Some(method)) = (name_of(function, "path"), name_of(function, "name"))
                else {
                    return self.branch(n, attrs, None, Role::Other, Mode::Expr, &[]);
                };
                let name = self.text(&method);
                if upper(name) {
                    return self.branch(n, attrs, None, Role::Other, Mode::Expr, &[]);
                }
                let receiver = match path {
                    Some(p) if self.text(&p) == "Self" => Receiver::SelfRef,
                    _ => Receiver::Explicit,
                };
                self.open(&n, attrs, Some(name), Role::Call(receiver));
                if let Some(path) = path {
                    self.emit(path, Mode::Path, &[]);
                }
                self.children(n, Mode::Expr, &[function]);
                self.b.close();
            }
            _ => self.branch(n, attrs, None, Role::Other, Mode::Expr, &[]),
        }
    }
}

/// Whether an item's attributes read its members' names: a derive beyond
/// [`NAMELESS_DERIVES`], or any attribute beyond [`NAMELESS_ATTRIBUTES`].
fn names_observed(attrs: &[Node<'_>], src: &[u8]) -> bool {
    attrs.iter().any(|attr| {
        let text = String::from_utf8_lossy(&src[span(attr)]).into_owned();
        let inner = text
            .trim_start_matches('#')
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim();
        let head: String = inner
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
            .collect();
        if head == "derive" {
            let args = inner["derive".len()..]
                .trim()
                .trim_start_matches('(')
                .trim_end_matches(')');
            return args.split(',').map(str::trim).any(|d| {
                let d = d.rsplit("::").next().unwrap_or(d);
                !d.is_empty() && !NAMELESS_DERIVES.contains(&d)
            });
        }
        !NAMELESS_ATTRIBUTES.contains(&head.as_str())
    })
}

/// Every name a pattern binds in function `f`, closures included and nested
/// functions skipped: an identifier there names a local, not an item.
fn bindings(f: Node<'_>, src: &[u8]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut stack = vec![f];
    while let Some(n) = stack.pop() {
        let mut c = n.walk();
        for (i, child) in n.children(&mut c).enumerate() {
            let field = n.field_name_for_child(i as u32);
            if child.kind() == "function_item" {
                continue;
            }
            let pattern = match (n.kind(), field) {
                (
                    "let_declaration" | "parameter" | "let_condition" | "for_expression",
                    Some("pattern"),
                ) => true,
                ("closure_parameters", _) => true,
                ("match_pattern", f) => f != Some("condition"),
                _ => false,
            };
            if pattern {
                pattern_names(child, src, &mut out);
            } else {
                stack.push(child);
            }
        }
    }
    out
}

fn pattern_names(p: Node<'_>, src: &[u8], out: &mut HashSet<String>) {
    let text = |n: &Node<'_>| String::from_utf8_lossy(&src[span(n)]).into_owned();
    match p.kind() {
        "identifier" | "shorthand_field_identifier" => {
            let name = text(&p);
            if !upper(&name) {
                out.insert(name);
            }
        }
        "scoped_identifier" | "type_identifier" => {}
        _ => {
            let mut c = p.walk();
            for (i, child) in p.children(&mut c).enumerate() {
                let field = p.field_name_for_child(i as u32);
                let skip = match (p.kind(), field) {
                    ("tuple_struct_pattern" | "struct_pattern" | "parameter", Some("type")) => true,
                    ("field_pattern", Some("name")) => child.kind() == "field_identifier",
                    _ => false,
                };
                if !skip {
                    pattern_names(child, src, out);
                }
            }
        }
    }
}

/// Which tree-sitter nodes are which [`Construct`], by kind, name and own
/// text (a binary operator is its node's own text).
pub(super) fn construct(tree: &Tree, id: NodeId) -> Option<Construct> {
    let node = tree.node(id);
    let name = node.name.as_deref();
    let own = || -> String { tree.own_bytes(id).iter().map(|&(_, b)| b as char).collect() };
    let first_named = |want: &str| {
        node.children
            .first()
            .is_some_and(|&c| tree.node(c).name.as_deref() == Some(want))
    };
    Some(match node.kind {
        "if_expression" | "while_expression" | "match_expression" | "match_arm" => {
            Construct::Branch { test: 1 }
        }
        "binary_expression"
            if matches!(
                own().as_str(),
                "==" | "!=" | "<" | ">" | "<=" | ">=" | "&&" | "||"
            ) =>
        {
            Construct::Comparison
        }
        "unary_expression" if own() == "!" => Construct::Comparison,
        "integer_literal" | "float_literal" | "string_literal" | "raw_string_literal"
        | "char_literal" | "boolean_literal" => Construct::Literal,
        "try_expression" => Construct::Error,
        "macro_invocation" if matches!(name, Some(n) if ERROR_MACROS.contains(&n)) => {
            Construct::Error
        }
        "call_expression" if matches!(name, Some(n) if ERROR_METHODS.contains(&n)) => {
            Construct::Error
        }
        "call_expression" | "tuple_struct_pattern" if first_named("Err") => Construct::Error,
        "parameters" => Construct::Parameters,
        "function_item" | "mod_item" if test_item(tree, id) => Construct::Test,
        _ => return None,
    })
}

/// Macros that panic or return an error.
const ERROR_MACROS: &[&str] = &[
    "panic",
    "unreachable",
    "todo",
    "unimplemented",
    "bail",
    "ensure",
    "anyhow",
];

/// Methods that unwrap, convert or recover from an error or a missing value.
const ERROR_METHODS: &[&str] = &[
    "unwrap",
    "expect",
    "unwrap_err",
    "expect_err",
    "map_err",
    "ok_or",
    "ok_or_else",
    "unwrap_or",
    "unwrap_or_else",
    "unwrap_or_default",
    "context",
    "with_context",
];

/// A `#[test]` (or `#[tokio::test]`, `#[rstest]`) fn, or a `#[cfg(test)]` mod.
fn test_item(tree: &Tree, id: NodeId) -> bool {
    tree.node(id)
        .children
        .iter()
        .filter(|&&c| tree.node(c).kind == "attribute_item")
        .any(|&c| {
            let text: String = String::from_utf8_lossy(tree.text(&tree.node(c).extent))
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            matches!(text.as_str(), "#[test]" | "#[cfg(test)]" | "#[rstest]")
                || text.ends_with("::test]")
        })
}
