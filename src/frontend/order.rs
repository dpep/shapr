//! What reordering a container's elements can change, per language: the
//! facts the `reorder` pass reads. A container this module doesn't name
//! (a positional argument list, `match` arms) isn't a reorder at all.

use super::{Frontend, Ruby};
use crate::tree::{Flags, NodeId, Role, Tree};

/// What the order of a container's elements can be observed through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dependence {
    /// Iterating the collection sees insertion order (a Ruby hash, keyword
    /// arguments a `**opts` collects).
    Iteration,
    /// Positions are part of the value: an array, a struct's declared fields
    /// (layout, derived `Ord`, `Debug`).
    Elements,
    /// Things load in written order (`require`, Gemfile `gem`), or a Rust
    /// `macro_rules!` is in scope only after its definition.
    Load,
    /// Code in the elements runs in written order.
    Evaluation,
}

/// How reordering `moved`, elements of `container` in a tree `language`
/// parsed, can be observed: empty when it can't, `None` when the container's
/// order is its meaning or it can't be judged.
pub fn reorder(
    language: &str,
    tree: &Tree,
    container: NodeId,
    moved: &[NodeId],
) -> Option<Vec<Dependence>> {
    match language {
        "ruby" => ruby(tree, container, moved),
        "rust" => rust(tree, container, moved),
        _ => None,
    }
}

fn ruby(tree: &Tree, container: NodeId, moved: &[NodeId]) -> Option<Vec<Dependence>> {
    let mut out = match tree.node(container).kind {
        "hash" | "keyword_hash" if distinct_keys(tree, container) => vec![Dependence::Iteration],
        "array" => vec![Dependence::Elements],
        "statements" => {
            // Definitions are the move pass's.
            if moved.iter().any(|&m| tree.node(m).role.is_declaration()) {
                return None;
            }
            let (load, run): (Vec<NodeId>, Vec<NodeId>) =
                moved.iter().partition(|&&m| loads(tree, m));
            let load = (!load.is_empty()).then_some(Dependence::Load);
            let run = (!run.is_empty()).then_some(Dependence::Evaluation);
            return Some(load.into_iter().chain(run).collect());
        }
        _ => return None,
    };
    if moved.iter().any(|&m| !ruby_pure(tree, m)) {
        out.push(Dependence::Evaluation);
    }
    Some(out)
}

/// Every entry is `key => value` with a plain literal key, no two alike: a
/// duplicate's later value wins, and a `**splat` may hold any key.
fn distinct_keys(tree: &Tree, hash: NodeId) -> bool {
    let mut keys = Vec::new();
    for entry in tree.code_children(hash) {
        if tree.node(entry).kind != "assoc" {
            return false;
        }
        let Some(key) = tree.code_children(entry).next() else {
            return false;
        };
        if !Ruby.plain_literal(tree, key) {
            return false;
        }
        // Canonical, so `'a'` and `"a"` are one key.
        keys.push(tree.node(key).canon);
    }
    let count = keys.len();
    keys.sort_unstable();
    keys.dedup();
    keys.len() == count
}

/// A `require` or a gem dependency (Gemfile `gem`, gemspec `add_*dependency`).
fn loads(tree: &Tree, id: NodeId) -> bool {
    let node = tree.node(id);
    let Role::Call(_) = node.role else {
        return false;
    };
    matches!(
        node.name.as_deref(),
        Some(
            "require"
                | "require_relative"
                | "gem"
                | "add_dependency"
                | "add_runtime_dependency"
                | "add_development_dependency"
        )
    )
}

/// Nothing in `id` runs code: no call (operators are calls), no
/// interpolation (it calls `to_s`), no control flow.
fn ruby_pure(tree: &Tree, id: NodeId) -> bool {
    tree.descendants(id).all(|d| {
        let node = tree.node(d);
        !matches!(
            node.role,
            Role::Call(_) | Role::Hazard(_) | Role::DynamicName
        ) && !node.kind.starts_with("embedded")
    })
}

/// Rust items, whose order in a module, `impl`, trait or block doesn't
/// matter to name resolution.
const ITEMS: &[&str] = &[
    "use_declaration",
    "function_item",
    "function_signature_item",
    "struct_item",
    "enum_item",
    "union_item",
    "const_item",
    "static_item",
    "type_item",
    "associated_type",
    "trait_item",
    "impl_item",
    "mod_item",
    "extern_crate_declaration",
    "foreign_mod_item",
];

fn rust(tree: &Tree, container: NodeId, moved: &[NodeId]) -> Option<Vec<Dependence>> {
    let kind = tree.node(container).kind;
    let out = match kind {
        "root" | "mod_item" | "impl_item" | "trait_item" | "foreign_mod_item" | "block" => {
            let mut out = Vec::new();
            // `macro_rules!` is in scope only below its definition, as is
            // what a `#[macro_use]` module or crate brings in; a macro or an
            // attribute macro (`SPELLS`) may expand differently by order.
            let textual = tree.code_children(container).any(|c| macro_scoped(tree, c));
            let expands = moved.iter().any(|&m| {
                let node = tree.node(m);
                node.kind == "macro_invocation" || node.flags.contains(Flags::SPELLS)
            });
            if textual || expands {
                out.push(Dependence::Load);
            }
            // In a block a macro call is a statement that runs (`assert!`).
            let statement = |m: &NodeId| {
                let node = tree.node(*m);
                let item_macro = node.kind == "macro_invocation" && kind != "block";
                !ITEMS.contains(&node.kind) && node.kind != "macro_definition" && !item_macro
            };
            if moved.iter().any(statement) {
                if kind != "block" {
                    return None;
                }
                out.push(Dependence::Evaluation);
            }
            out
        }
        "field_declaration_list" | "ordered_field_declaration_list" | "enum_variant_list" => {
            vec![Dependence::Elements]
        }
        "array_expression" if moved.iter().all(|&m| rust_pure(tree, m)) => {
            vec![Dependence::Elements]
        }
        "array_expression" => vec![Dependence::Elements, Dependence::Evaluation],
        // The value is the same whatever the order; only evaluation shows it.
        "field_initializer_list" if moved.iter().all(|&m| rust_pure(tree, m)) => Vec::new(),
        "field_initializer_list" => vec![Dependence::Evaluation],
        _ => return None,
    };
    Some(out)
}

/// A `macro_rules!`, or an item under `#[macro_use]`.
fn macro_scoped(tree: &Tree, id: NodeId) -> bool {
    let node = tree.node(id);
    if node.kind == "macro_definition" {
        return true;
    }
    let attributes = node
        .children
        .iter()
        .filter(|&&c| tree.node(c).kind == "attribute_item");
    attributes
        .flat_map(|&a| tree.descendants(a))
        .any(|d| tree.node(d).name.as_deref() == Some("macro_use"))
}

/// Only literals and names: nothing runs (an operator, an index or a
/// field read through `Deref` can call user code).
fn rust_pure(tree: &Tree, id: NodeId) -> bool {
    tree.descendants(id).all(|d| {
        let node = tree.node(d);
        let name = matches!(node.role, Role::Reference(_)) && node.kind != "field_expression";
        let literal = node.kind.ends_with("_literal")
            || matches!(
                node.kind,
                "string_content" | "escape_sequence" | "true" | "false"
            );
        name || literal || node.kind == "shorthand_field_initializer"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::Rust;

    /// The first node of `kind`, and its code children.
    fn container(tree: &Tree, kind: &str) -> (NodeId, Vec<NodeId>) {
        let id = tree
            .descendants(Tree::ROOT)
            .find(|&d| tree.node(d).kind == kind)
            .unwrap();
        (id, tree.code_children(id).collect())
    }

    fn ruby(source: &str, kind: &str) -> Option<Vec<Dependence>> {
        let tree = Ruby.parse(source.as_bytes().to_vec());
        let (id, children) = container(&tree, kind);
        reorder("ruby", &tree, id, &children)
    }

    fn rust(source: &str, kind: &str) -> Option<Vec<Dependence>> {
        let tree = Rust.parse(source.as_bytes().to_vec());
        let (id, children) = container(&tree, kind);
        reorder("rust", &tree, id, &children)
    }

    use Dependence::*;

    #[test]
    fn ruby_containers() {
        assert_eq!(ruby("{ a: 1, 'b' => 2 }", "hash"), Some(vec![Iteration]));
        assert_eq!(
            ruby("{ a: f, b: 2 }", "hash"),
            Some(vec![Iteration, Evaluation])
        );
        assert_eq!(ruby("{ 'a' => 1, \"a\" => 2 }", "hash"), None);
        assert_eq!(ruby("{ k => 1, j => 2 }", "hash"), None);
        assert_eq!(ruby("{ a: 1, **o }", "hash"), None);
        assert_eq!(
            ruby("[1, \"#{x}\"]", "array"),
            Some(vec![Elements, Evaluation])
        );
        assert_eq!(
            ruby("require 'a'\ngem 'b'\n", "statements"),
            Some(vec![Load])
        );
        assert_eq!(
            ruby("require 'a'\nx = 1\n", "statements"),
            Some(vec![Load, Evaluation])
        );
        assert_eq!(ruby("def a; end\nx = 1\n", "statements"), None);
        assert_eq!(ruby("f(a, b)", "arguments"), None);
    }

    #[test]
    fn rust_containers() {
        assert_eq!(rust("use a::b;\nmod c;\nfn d() {}\n", "root"), Some(vec![]));
        assert_eq!(
            rust("use a::b;\nmacro_rules! m { () => {} }\n", "root"),
            Some(vec![Load])
        );
        assert_eq!(
            rust("#[macro_use]\nmod a;\nmod b;\n", "root"),
            Some(vec![Load])
        );
        assert_eq!(
            rust("fn f() { let a = 1; g(); }", "block"),
            Some(vec![Evaluation])
        );
        assert_eq!(
            rust("struct S { a: u8, b: u8 }", "field_declaration_list"),
            Some(vec![Elements])
        );
        assert_eq!(
            rust("fn f() { S { a: 1, b: x }; }", "field_initializer_list"),
            Some(vec![])
        );
        assert_eq!(
            rust("fn f() { S { a: g(), b: x.y }; }", "field_initializer_list"),
            Some(vec![Evaluation])
        );
        assert_eq!(
            rust("fn f() { assert!(a); assert!(b); }", "block"),
            Some(vec![Load, Evaluation])
        );
        assert_eq!(
            rust("fn f() { [1, 2]; }", "array_expression"),
            Some(vec![Elements])
        );
        assert_eq!(rust("fn f() { g(a, b); }", "arguments"), None);
    }
}
