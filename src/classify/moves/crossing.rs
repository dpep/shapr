//! What a move may cross: which statements a moved definition must keep
//! its order with. [`Needs::by`] is the one predicate; [`dependencies`]
//! applies it within a file (siblings, then the statement lists enclosing
//! the container, #171) and [`window`] across files (the statements between
//! where the unit ran and where its new file now loads).
//!
//! A statement depends on the unit, or the unit on it, when:
//!
//! - the unit runs where it stands (anything but a method body, or any
//!   definition where definitions run: Ruby) and the statement runs code,
//!   not [`inert`]: `moves/method_between_class_reopenings_past_top_level_call`,
//!   `moves/method_to_new_file_past_include`, `soundness/constants_running_code_keep_order`;
//! - it defines a definition hook (`method_added`) and the unit is a method:
//!   `moves/method_past_method_added_hook`, `moves/method_to_new_file_past_method_added`;
//! - it [`expands`]: a macro invocation or an item an attribute macro
//!   rewrites: `moves/rust_fn_past_item_macro_in_impl`;
//! - the unit is a hook, or below one in its statement list or an enclosing
//!   one, and the statement is a method: their order is what the hook sees
//!   (#147): `moves/methods_swapped_under_method_added_hook`,
//!   `moves/singleton_methods_swapped_under_outer_singleton_method_added`;
//! - it declares the unit's name, or names it at load time:
//!   `moves/singleton_method_past_class_self_definition`, `soundness/mv4_window_reads_moved_class`;
//! - it declares a name the unit's load-time code reads;
//! - it defines a macro the unit invokes (`macro_rules!` resolves to the
//!   definition textually before it, #171):
//!   `moves/rust_method_past_macro_redefinition_between_impls`.

use super::members;
use crate::classify::align::{key, lcs, transparent};
use crate::tree::{Flags, Hazard, NameKind, NodeId, Role, Tree, spells};

/// Whether `unit`, of the old file's before tree, can run where the old
/// file's after tree loads its new file: at top-level statement `at`, or
/// after the file's last statement when `None`. Every statement that now
/// runs on the other side of it must not depend on it ([`Needs`]). `Some`
/// says whether it now runs later than before.
pub(super) fn window(before: &Tree, unit: NodeId, after: &Tree, at: Option<usize>) -> Option<bool> {
    let (tb, ta): (Vec<NodeId>, Vec<NodeId>) = (
        members(before, Tree::ROOT).collect(),
        members(after, Tree::ROOT).collect(),
    );
    let top = std::iter::once(unit)
        .chain(before.ancestors(unit))
        .find(|a| tb.contains(a))?;
    let t = tb.iter().position(|&n| n == top)?;
    let matched = lcs(
        tb.len(),
        ta.len(),
        |i, j| key(before, tb[i]) == key(after, ta[j]),
        |_| 1,
    );
    let r = at.unwrap_or(ta.len());
    // Where the unit's top-level statement now stands: kept, or for a unit
    // at the top level itself, the gap it left.
    let (later, span) = if top == unit {
        let slot = matched
            .iter()
            .filter(|(i, _)| *i < t)
            .map(|(_, j)| j + 1)
            .max()
            .unwrap_or(0);
        (r >= slot, if r >= slot { slot..r } else { r + 1..slot })
    } else {
        let &(_, j) = matched.iter().find(|(i, _)| *i == t)?;
        (r > j, if r > j { j + 1..r } else { r + 1..j })
    };
    let needs = Needs::of(before, unit);
    if ta[span].iter().any(|&w| needs.by(after, w)) {
        return None;
    }
    // Inside the top-level statement, the unit's siblings past it (or
    // before it) in each statement list up to it.
    let mut x = unit;
    while x != top {
        let parent = before.node(x).parent?;
        if transparent(before, parent) {
            let siblings: Vec<NodeId> = before.code_children(parent).collect();
            let at = siblings.iter().position(|&s| s == x)?;
            let side = if later {
                &siblings[at + 1..]
            } else {
                &siblings[..at]
            };
            if side.iter().any(|&w| needs.by(before, w)) {
                return None;
            }
        }
        x = parent;
    }
    Some(later)
}

/// Siblings whose order relative to `node` matters, as (hash, comes first),
/// sorted: those that name what it declares at load time (`alias_method`,
/// `private :x`, a constant read), that declare the same name, or that
/// declare a name its own load-time code reads. For anything but a method,
/// whose body runs later, every statement that runs code counts (not a
/// declaration or a bare `private`): a `require` or a call may read it. So
/// does it for a method where definitions run ([`crate::tree::Semantics::definitions_run`]),
/// as does a sibling defining a definition hook (`method_added`) and, below
/// one, every other method.
pub(super) fn dependencies(tree: &Tree, node: NodeId) -> Vec<(u64, bool)> {
    let Some(parent) = tree.node(node).parent else {
        return Vec::new();
    };
    let needs = Needs::of(tree, node);
    let mut before = true;
    let mut out = Vec::new();
    for &sibling in &tree.node(parent).children {
        if sibling == node {
            before = false;
            continue;
        }
        if tree.node(sibling).comment {
            continue;
        }
        if needs.by(tree, sibling) {
            out.push((tree.node(sibling).hash, before));
        }
    }
    // The statement lists enclosing its container count too: a method
    // moved between two `impl`s crosses what stands between them (a
    // `macro_rules!` redefinition its body expands).
    let mut path = parent;
    for outer in tree.ancestors(parent) {
        if outer == Tree::ROOT || transparent(tree, outer) {
            let at = tree.node(path).span.start;
            for &sibling in &tree.node(outer).children {
                if sibling == path || tree.node(sibling).comment {
                    continue;
                }
                if needs.by(tree, sibling) {
                    out.push((tree.node(sibling).hash, tree.node(sibling).span.start < at));
                }
            }
        }
        path = outer;
    }
    out.sort_unstable();
    out
}

/// What a unit needs of the statements around it, to tell which depend on
/// it ([`dependencies`]).
struct Needs<'t> {
    tree: &'t Tree,
    node: NodeId,
    declared: Option<&'t str>,
    reads: Vec<&'t str>,
    /// A method body runs later; anything else runs where it stands, so a
    /// statement beside it may need it (or change it) by the time it runs.
    runs_now: bool,
    /// A method defined after a definition hook: the hook sees every method
    /// definition after it in order, so their order among themselves counts.
    hooked: bool,
}

impl<'t> Needs<'t> {
    fn of(tree: &'t Tree, node: NodeId) -> Self {
        let runs_now = tree.node(node).role != Role::Definition(NameKind::Method);
        let hooks = tree.semantics.definition_hooks;
        // A hook before it in its own statement list or an enclosing one (a
        // class body's `def self.singleton_method_added` above `class << self`).
        let hook_before = |path: NodeId| {
            let Some(list) = tree.node(path).parent else {
                return false;
            };
            tree.node(list)
                .children
                .iter()
                .take_while(|&&c| c != path)
                .any(|&c| declared_name(tree, c).is_some_and(|n| hooks.contains(&n)))
        };
        let hooked = !runs_now
            && std::iter::once(node)
                .chain(tree.ancestors(node))
                .any(hook_before);
        Needs {
            tree,
            node,
            declared: declared_name(tree, node),
            reads: mentions(tree, node),
            runs_now,
            hooked,
        }
    }

    /// Whether statement `other`, of `tree`, depends on the unit or it on
    /// `other`.
    fn by(&self, tree: &Tree, other: NodeId) -> bool {
        let theirs = declared_name(tree, other);
        let semantics = &tree.semantics;
        let hook =
            |name: Option<&str>| name.is_some_and(|n| semantics.definition_hooks.contains(&n));
        // A hook sees the method definitions after it, either way round.
        let other_method = tree.node(other).role == Role::Definition(NameKind::Method);
        ((self.runs_now || semantics.definitions_run) && !inert(tree, other))
            || (hook(theirs) && !self.runs_now)
            || expands(tree, other)
            || ((hook(self.declared) || self.hooked) && other_method)
            || self.declared.is_some_and(|d| {
                theirs == Some(d) || mentions(tree, other).iter().any(|m| spells(m, d))
            })
            || theirs.is_some_and(|t| self.reads.contains(&t))
            || macro_name(tree, other).is_some_and(|m| invokes(self.tree, self.node, m))
    }
}

/// A statement that runs no code another could depend on: a declaration
/// (but not a constant whose value calls something, as `X = Counter.next`
/// does), or a bare visibility statement (`private`), which only marks a
/// section.
fn inert(tree: &Tree, node: NodeId) -> bool {
    let n = tree.node(node);
    let calls = || {
        tree.descendants(node)
            .any(|d| matches!(tree.node(d).role, Role::Call(_) | Role::Hazard(_)))
    };
    if n.flags.contains(Flags::DIRECTIVE) {
        return true;
    }
    match n.role {
        Role::Definition(NameKind::Constant) => !calls(),
        Role::Visibility(_) => n.children.iter().all(|&c| tree.node(c).comment),
        role => role.is_declaration(),
    }
}

/// A macro invocation, or an item an attribute macro rewrites: it may
/// expand to anything, depending on what is beside it.
fn expands(tree: &Tree, node: NodeId) -> bool {
    let n = tree.node(node);
    matches!(
        n.role,
        Role::Reference(NameKind::Macro) | Role::Hazard(Hazard::Macro)
    ) || n.flags.contains(Flags::SPELLS)
}

/// The macro a sibling defines (`macro_rules!`): an invocation resolves to
/// the definition textually before it, function bodies included.
fn macro_name(tree: &Tree, node: NodeId) -> Option<&str> {
    let n = tree.node(node);
    n.name
        .as_deref()
        .filter(|_| n.role == Role::Definition(NameKind::Macro))
}

/// Whether the subtree invokes macro `name`, or may in a token tree.
fn invokes(tree: &Tree, node: NodeId, name: &str) -> bool {
    tree.descendants(node).any(|d| {
        let n = tree.node(d);
        n.name.as_deref() == Some(name)
            && matches!(
                n.role,
                Role::Reference(NameKind::Macro) | Role::Hazard(Hazard::Macro) | Role::Opaque
            )
    })
}

fn declared_name(tree: &Tree, node: NodeId) -> Option<&str> {
    let n = tree.node(node);
    n.role.defines().and(n.name.as_deref())
}

/// Names the subtree uses when it runs: references, calls, dynamic names,
/// and the text of any literal leaf (`:x` in `memoize :x`). Method bodies
/// run later, so they're skipped.
fn mentions(tree: &Tree, node: NodeId) -> Vec<&str> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(id) = stack.pop() {
        let n = tree.node(id);
        if n.comment || n.role == Role::Definition(NameKind::Method) {
            continue;
        }
        match n.role {
            Role::Reference(_) | Role::Call(_) | Role::DynamicName => {
                out.extend(n.name.as_deref());
            }
            _ => {}
        }
        if n.children.is_empty() {
            let text = std::str::from_utf8(tree.text(&n.span)).unwrap_or_default();
            out.push(tree.semantics.spelling.word(text));
        }
        stack.extend(&n.children);
    }
    out
}
