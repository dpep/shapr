//! Which Rust syntax plays which [`Role`], and what each node keeps in its
//! own text (the name it declares or calls) instead of as a child.

use super::{Walker, is_comment, kind, span, trailing};
use crate::frontend::{Construct, Pattern, Surface};
use crate::tree::{Canonical, Flags, Hazard, NameKind, NodeId, Receiver, Role, Tree};
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
    /// A module's or trait's items: [`Walker::body`].
    Body,
    /// Wrapped in an `annotation` node: a type.
    Annotate,
}

/// Spelling variants `restyle` normalizes, by the rustfmt option or lint
/// that chooses among them.
const FIELD_INIT_SHORTHAND: &str = "use_field_init_shorthand";
const UNUSED_PARENS: &str = "unused_parens";
const REORDER_IMPORTS: &str = "reorder_imports";

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
pub(super) const NAMELESS_DERIVES: &[&str] = &[
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
            (_, "arguments") => {
                self.open(&n, attrs, None, Role::Other);
                self.b.flag(Flags::ARGUMENTS);
                self.children(n, mode, &[]);
                self.b.close();
            }

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
                // Its locals are the ones `self.locals` now holds.
                self.b.flag(Flags::SCOPE);
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
                self.b.flag(Flags::SCOPE);
                if reads_tokens(attrs, self.src) {
                    self.b.flag(Flags::SPELLS);
                }
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
                    self.body(body);
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
                if n.kind() == "inner_attribute_item" {
                    self.b.flag(Flags::HEADER);
                }
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
                Some(pattern) => {
                    let name = field("name");
                    let label = name.map(|x| self.text(&x));
                    self.open(&n, attrs, label, Role::Reference(Member));
                    let same = label == Some(self.text(&pattern));
                    if attrs.is_empty() && pattern.kind() == "identifier" && same {
                        self.shorthand(b"pattern", label);
                    }
                    self.children(n, mode, &name.into_iter().collect::<Vec<_>>());
                    self.b.close();
                }
                // `S { a }` binds `a` and names the field `a`.
                None => {
                    let name = field("name").map(|x| self.text(&x));
                    self.member(&n, name);
                    if name == Some(self.text(&n)) {
                        self.shorthand(b"pattern", name);
                    }
                    self.b.close();
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
                self.open(&n, attrs, name, role);
                let value = field("value").filter(|v| v.kind() == "identifier");
                if attrs.is_empty() && name.is_some() && value.map(|v| self.text(&v)) == name {
                    self.shorthand(b"init", name);
                }
                self.children(n, Mode::Expr, &member.into_iter().collect::<Vec<_>>());
                self.b.close();
            }
            (_, "shorthand_field_initializer") => {
                let name = self.text(&n);
                self.member(&n, Some(name));
                self.shorthand(b"init", Some(name));
                self.b.close();
            }
            (_, "parenthesized_expression") => {
                self.open(&n, attrs, None, Role::Other);
                // `(s.f)()` calls a field; `s.f()` a method.
                let callee = n.parent().is_some_and(|p| {
                    p.kind() == "call_expression"
                        && p.child_by_field_name("function")
                            .is_some_and(|f| f.id() == n.id())
                });
                if attrs.is_empty() && !callee && n.named_child_count() == 1 {
                    self.b.respell(UNUSED_PARENS, Canonical::Transparent);
                }
                self.children(n, mode, &[]);
                self.b.close();
            }
            (_, "call_expression") => self.call(n, attrs),
            // Imports resolve by name, never by order: the canon reads the
            // list's items as a set. They stay in source order, so a rename
            // pairs each item with itself.
            (_, "use_list") => {
                self.open(&n, attrs, None, Role::Other);
                self.b
                    .respell(REORDER_IMPORTS, Canonical::Unordered(b"{}".to_vec()));
                let mut c = n.walk();
                let kids: Vec<_> = n.children(&mut c).map(|k| (None, k)).collect();
                for (i, &(_, k)) in kids.iter().enumerate() {
                    if is_comment(&k) {
                        self.comment(&k);
                    } else if k.is_missing() {
                        self.b.error(span(&k));
                    } else if k.kind() == "," && trailing(&n, &kids[i + 1..]) {
                        self.b.ignore(span(&k));
                    } else if k.is_named() {
                        self.emit(k, mode, &[]);
                    }
                }
                self.b.close();
            }
            (_, "macro_invocation") => {
                let mac = field("macro");
                let name = mac.map(|m| {
                    let t = self.text(&m);
                    t.rsplit("::").next().unwrap_or(t)
                });
                let role = match name {
                    // `dbg!` prints its own line and column.
                    Some("line" | "column" | "dbg") => Role::Hazard(Hazard::Line),
                    Some("file" | "module_path") => Role::Hazard(Hazard::File),
                    Some(name) if PLAIN_MACROS.contains(&name) => Role::Reference(Macro),
                    _ => Role::Hazard(Hazard::Macro),
                };
                self.open(&n, attrs, name, role);
                let mut c = n.walk();
                let tokens = n.children(&mut c).find(|c| c.kind() == "token_tree");
                if let Some(tokens) = tokens.filter(|t| spells(name.unwrap_or_default(), Some(*t)))
                {
                    self.b.flag(Flags::SPELLS);
                    self.b.verbatim(span(&tokens));
                }
                let mut skip: Vec<Node<'_>> = mac.into_iter().collect();
                // `vec!` takes a trailing comma or none alike.
                let vec = tokens.filter(|_| name == Some("vec"));
                skip.extend(vec);
                self.children_with(
                    n,
                    &mut |_, c| {
                        if c.kind() == "token_tree" {
                            Plan::Emit(Mode::Tokens)
                        } else {
                            Plan::Emit(Mode::Expr)
                        }
                    },
                    &skip,
                );
                if let Some(tokens) = vec {
                    self.b.open(kind(&tokens), span(&tokens), None, Role::Other);
                    let mut c = tokens.walk();
                    let kids: Vec<_> = tokens.children(&mut c).map(|k| (None, k)).collect();
                    for (i, &(_, k)) in kids.iter().enumerate() {
                        if k.kind() == "," && trailing(&tokens, &kids[i + 1..]) {
                            self.b.ignore(span(&k));
                        } else {
                            self.token(k);
                        }
                    }
                    self.b.close();
                }
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

    /// Opens `S { a }`'s `a`: a field's name and a local at once, so opaque
    /// to a rename and marked as naming a member.
    pub(super) fn member(&mut self, n: &Node<'_>, name: Option<&str>) {
        self.open(n, &[], name, Role::Opaque);
        self.b.flag(Flags::MEMBER);
    }

    /// `S { a: a }` and `S { a }`, in an expression (`init`) or a pattern:
    /// the field and the local read or bound are one name either way.
    fn shorthand(&mut self, place: &[u8], name: Option<&str>) {
        let Some(name) = name else { return };
        let mut value = place.to_vec();
        value.push(0);
        value.extend_from_slice(name.as_bytes());
        self.b
            .respell(FIELD_INIT_SHORTHAND, Canonical::Value(value));
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
        self.b.flag(Flags::SCOPE);
        let opaque = std::mem::replace(&mut self.opaque_members, names_observed(attrs, self.src));
        self.children_with(
            n,
            &mut |f, c| match f {
                Some("name") => Plan::Name(Role::Reference(name_kind)),
                Some("body") if c.kind() == "declaration_list" => Plan::Body,
                _ => Plan::Emit(Mode::Expr),
            },
            &[],
        );
        self.opaque_members = opaque;
        self.b.close();
    }

    /// A module's, trait's or impl's items, under a node with no text of its
    /// own (its braces fold into the item), as a Ruby class body is: the core
    /// tells the items from the header by it.
    pub(super) fn body(&mut self, list: Node<'_>) {
        let mut cursor = list.walk();
        let items: Vec<Node<'_>> = list
            .named_children(&mut cursor)
            .filter(|c| !super::is_comment(c))
            .collect();
        let (Some(first), Some(last)) = (items.first(), items.last()) else {
            return self.children(list, Mode::Expr, &[]);
        };
        self.b.open(
            "body",
            first.start_byte()..last.end_byte(),
            None,
            Role::Other,
        );
        self.children(list, Mode::Expr, &[]);
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
                // `Location::caller()` reports a line: its own, or under
                // `#[track_caller]` its caller's.
                let location = path.is_some_and(|p| {
                    let p = self.text(&p);
                    p.rsplit("::").next() == Some("Location")
                }) && name == "caller";
                let role = match location {
                    true => Role::Hazard(Hazard::Stack),
                    false => Role::Call(receiver),
                };
                self.open(&n, attrs, Some(name), role);
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
    beyond(attrs, src, NAMELESS_DERIVES)
}

/// Whether an item's attributes may read any of its tokens, as an attribute
/// macro reads its whole item: a derive outside std, or any attribute beyond
/// [`NAMELESS_ATTRIBUTES`]. std's derives read only names.
pub(super) fn reads_tokens(attrs: &[Node<'_>], src: &[u8]) -> bool {
    let std = [NAMELESS_DERIVES, &["Debug"]].concat();
    beyond(attrs, src, &std)
}

/// Whether any attribute is a derive beyond `derives`, or another attribute
/// beyond [`NAMELESS_ATTRIBUTES`].
fn beyond(attrs: &[Node<'_>], src: &[u8], derives: &[&str]) -> bool {
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
                !d.is_empty() && !derives.contains(&d)
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

/// Whether macro `name`'s expansion may spell out its `tokens` as text:
/// `stringify!`, `dbg!` (prints the expression), `assert!` without a message
/// (panics with the condition), and any macro outside std, unseen.
pub(super) fn spells(name: &str, tokens: Option<Node<'_>>) -> bool {
    match name {
        "stringify" | "dbg" => true,
        "assert" | "debug_assert" => !tokens.is_some_and(|t| {
            let mut c = t.walk();
            let kinds: Vec<&str> = t.children(&mut c).map(|t| t.kind()).collect();
            let Some(comma) = kinds.iter().position(|&k| k == ",") else {
                return false;
            };
            // A `<` or `|` before it may open generics or closure
            // parameters, whose commas a token tree doesn't nest.
            let ambiguous = kinds[..comma]
                .iter()
                .any(|k| k.starts_with('<') || *k == "|");
            !ambiguous && !matches!(kinds.get(comma + 1), None | Some(&(")" | "]" | "}")))
        }),
        "line" | "column" | "file" | "module_path" => false,
        name => !PLAIN_MACROS.contains(&name),
    }
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
        "unary_expression" if own() == "!" => Construct::Negation,
        "integer_literal" | "float_literal" | "string_literal" | "raw_string_literal"
        | "char_literal" => Construct::Literal,
        "boolean_literal" => Construct::Flag,
        "try_expression" => Construct::Error,
        "macro_invocation" if matches!(name, Some(n) if ERROR_MACROS.contains(&n)) => {
            Construct::Error
        }
        "call_expression" if matches!(name, Some(n) if ERROR_METHODS.contains(&n)) => {
            Construct::Error
        }
        "call_expression" | "tuple_struct_pattern" if first_named("Err") => Construct::Error,
        "call_expression" if matches!(name, Some("is_none" | "is_some" | "is_empty")) => {
            Construct::Guard
        }
        "parameters" => Construct::Parameters,
        "function_item" | "mod_item" if test_item(tree, id) => Construct::Test,
        "macro_invocation"
            if matches!(name, Some(n) if ["assert", "debug_assert", "prop_assert"]
                .iter()
                .any(|p| n.starts_with(p))) =>
        {
            Construct::Assertion
        }
        _ => return None,
    })
}

/// Rust's printed, logged and panicked text, and serde's keys.
pub(super) fn surface(tree: &Tree, id: NodeId) -> Option<Surface> {
    let node = tree.node(id);
    let name = node.name.as_deref();
    match node.kind {
        "macro_invocation" if matches!(name, Some(n) if TEXT_MACROS.contains(&n)) => {
            Some(Surface::Text)
        }
        "call_expression" if matches!(name, Some("expect" | "context")) => Some(Surface::Text),
        "attribute_item" => match attribute(tree, id) {
            (head, _) if head == "error" => Some(Surface::Text),
            (head, _) if head == "serde" && serialized(tree, node.parent?) => {
                Some(Surface::Key(vec![id]))
            }
            _ => None,
        },
        "struct_item" | "enum_item" if serialized(tree, id) => Some(Surface::Serialized),
        // Its name and attributes; a field's type, a variant's fields aside.
        "field_declaration" | "enum_variant" => Some(Surface::Key(
            tree.code_children(id)
                .filter(|&c| {
                    let c = tree.node(c);
                    c.role != Role::Annotation
                        && !matches!(
                            c.kind,
                            "field_declaration_list" | "ordered_field_declaration_list"
                        )
                })
                .collect(),
        )),
        _ => None,
    }
}

/// Whether item `id` derives serde's `Serialize` or `Deserialize`.
fn serialized(tree: &Tree, id: NodeId) -> bool {
    let item = tree.node(id);
    matches!(item.kind, "struct_item" | "enum_item")
        && (tree.code_children(id))
            .filter(|&c| tree.node(c).kind == "attribute_item")
            .any(|c| {
                let (head, args) = attribute(tree, c);
                head == "derive"
                    && args.split(',').any(|d| {
                        let d = d.trim();
                        matches!(d.rsplit("::").next(), Some("Serialize" | "Deserialize"))
                    })
            })
}

/// An attribute's path's last segment and its arguments' text:
/// `#[serde(rename = "a")]` is `("serde", "rename = \"a\"")`.
fn attribute(tree: &Tree, id: NodeId) -> (String, String) {
    let text = String::from_utf8_lossy(tree.text(&tree.node(id).extent)).into_owned();
    let inner = text
        .trim_start_matches('#')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    let path: String = inner
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
        .collect();
    let args = inner[path.len()..]
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let head = path.rsplit("::").next().unwrap_or_default().to_string();
    (head, args.to_string())
}

/// Macros that print, log, panic or build an error from a message.
const TEXT_MACROS: &[&str] = &[
    "println",
    "eprintln",
    "print",
    "eprint",
    "write",
    "writeln",
    "panic",
    "unreachable",
    "todo",
    "unimplemented",
    "bail",
    "ensure",
    "anyhow",
    "trace",
    "debug",
    "info",
    "warn",
    "error",
];

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

/// The pattern `id` spells when it is a string literal passed first to
/// `Regex::new`: a raw string's text, or a plain string's with `\\` and
/// `\"` unescaped (`None` for other escapes, which the regex would read
/// another way).
pub(super) fn pattern(tree: &Tree, id: NodeId) -> Option<Pattern> {
    let node = tree.node(id);
    let args = node.parent?;
    let call = tree.node(args).parent?;
    let callee = tree.node(call);
    let regex = callee.name.as_deref() == Some("new")
        && tree.node(args).kind == "arguments"
        && tree.code_children(args).next() == Some(id)
        && (callee.children.first()).is_some_and(|&c| {
            let c = tree.node(c);
            c.role == Role::Reference(NameKind::Class) && c.name.as_deref() == Some("Regex")
        });
    if !regex {
        return None;
    }
    let text = std::str::from_utf8(tree.text(&node.extent)).ok()?;
    let body = match node.kind {
        "raw_string_literal" => {
            let rest = text.strip_prefix('r')?;
            let hashes = "#".repeat(rest.len() - rest.trim_start_matches('#').len());
            let inner = rest
                .strip_prefix(hashes.as_str())?
                .strip_suffix(hashes.as_str())?;
            inner.strip_prefix('"')?.strip_suffix('"')?.to_string()
        }
        "string_literal" => {
            let inner = text.strip_prefix('"')?.strip_suffix('"')?;
            let mut out = String::new();
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => match chars.next()? {
                        e @ ('\\' | '"') => out.push(e),
                        _ => return None,
                    },
                    _ => out.push(c),
                }
            }
            out
        }
        _ => return None,
    };
    Some(Pattern {
        body,
        flags: String::new(),
        extended: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{Frontend, Rust};

    fn patterns(source: &str) -> Vec<String> {
        let tree = Rust.parse(source.as_bytes().to_vec());
        (tree.descendants(Tree::ROOT))
            .filter_map(|id| pattern(&tree, id))
            .map(|p| p.body)
            .collect()
    }

    #[test]
    fn a_string_passed_first_to_regex_new_is_a_pattern() {
        assert_eq!(
            patterns(
                "fn f() {\n    regex::Regex::new(r#\"a\"{2}\"#);\n    Regex::new(\"\\\\d+\");\n}\n"
            ),
            ["a\"{2}", "\\d+"]
        );
        for source in [
            "fn f() { Regex::new(\"\\n\"); }\n",
            "fn f() { Other::new(\"a{2}\"); }\n",
            "fn f() { Regex::with(\"a{2}\"); }\n",
            "fn f() { Regex::new(&s); }\n",
        ] {
            assert!(patterns(source).is_empty(), "{source}");
        }
    }
}
