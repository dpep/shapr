//! `facets`: what kind of change a `logic` hunk makes, as facts the code
//! shows. A facet never changes a hunk's kind, and a hunk can carry several.
//!
//! Facets compare the constructs a frontend reports ([`Construct`]) on the lines the hunk
//! changed, on each side: a construct present on one side and not the other
//! (by kind, name, own text and what it tests) is a change to it.

mod regex;

use super::{Label, Labels, Method, Relocation};
use crate::change::{Category, Change, Trees};
use crate::diff::{Hunk, Lines};
use crate::frontend::{self, Construct, Frontend, Pattern, Surface};
use crate::tree::{NameKind, NodeId, Role, Span, Tree};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Facet {
    /// Every changed code line is inside a test item (a test file is a
    /// file category instead).
    Test,
    /// A method, class or module that didn't exist before.
    AddsDefinition,
    /// A method, class or module that no longer exists.
    RemovesDefinition,
    /// An existing method's parameters, return type, generics or visibility.
    Signature,
    /// A conditional added, removed or testing something else, or a
    /// comparison operator changed.
    Condition,
    /// An error raised, propagated, unwrapped or rescued, added or removed or
    /// changed.
    ErrorHandling,
    /// Only literal values differ (numbers, strings, symbols, booleans).
    LiteralOnly,
    /// Test code left holding fewer assertions than it removed.
    RemovesAssertions,
    /// A literal regular expression changed so that it matches everything
    /// it did, and more where it changed.
    RegexWidened,
    /// A literal regular expression changed, not shown to widen.
    RegexChanged,
    /// A literal in printed, logged or raised text changed, the call the same.
    OutputText,
    /// A key a serialized definition writes was added, removed or renamed.
    SerializedKey,
}

impl Facet {
    pub const ALL: [Facet; 12] = [
        Facet::Test,
        Facet::AddsDefinition,
        Facet::RemovesDefinition,
        Facet::Signature,
        Facet::Condition,
        Facet::ErrorHandling,
        Facet::LiteralOnly,
        Facet::RemovesAssertions,
        Facet::RegexWidened,
        Facet::RegexChanged,
        Facet::OutputText,
        Facet::SerializedKey,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Facet::Test => "test",
            Facet::AddsDefinition => "adds_definition",
            Facet::RemovesDefinition => "removes_definition",
            Facet::Signature => "signature",
            Facet::Condition => "condition",
            Facet::ErrorHandling => "error_handling",
            Facet::LiteralOnly => "literal_only",
            Facet::RemovesAssertions => "removes_assertions",
            Facet::RegexWidened => "regex_widened",
            Facet::RegexChanged => "regex_changed",
            Facet::OutputText => "output_text",
            Facet::SerializedKey => "serialized_key",
        }
    }
}

pub fn run(change: &Change, labels: &mut Labels) {
    for (file, labels) in change.files.iter().zip(labels) {
        let parsed = match (&file.trees, frontend::for_path(file.diff.path())) {
            (Ok(trees), Some(frontend)) if trees.clean() => Some(Parsed::new(trees, frontend)),
            _ => None,
        };
        for (hunk, label) in file.diff.hunks.iter().zip(labels) {
            label.tests = parsed.as_ref().is_some_and(|p| p.touches_tests(hunk));
            if !label.has_residual() {
                continue;
            }
            let mut facets = BTreeSet::new();
            if let Some(parsed) = parsed
                .as_ref()
                .filter(|_| label.residual.evidence.method == Method::Ast)
            {
                let hunk = residual(hunk, label);
                let test = file.category == Category::Test;
                let condition = parsed.facets(&hunk, test, &mut facets);
                if !condition.is_empty() {
                    label.residual.detail.insert(
                        "condition".to_string(),
                        json!(condition.iter().map(|c| c.as_str()).collect::<Vec<_>>()),
                    );
                }
                let test = test || facets.contains(&Facet::Test);
                let (removed, added) = parsed.assertions(&hunk, &label.residual.relocated);
                if test && added < removed {
                    facets.insert(Facet::RemovesAssertions);
                    label.residual.detail.insert(
                        "assertions".to_string(),
                        json!({"removed": removed, "added": added}),
                    );
                }
            }
            label.residual.facets = facets.into_iter().collect();
        }
    }
}

/// The hunk narrowed to its residual's lines: what the facets describe.
fn residual(hunk: &Hunk, label: &Label) -> Hunk {
    let lines = &label.residual.lines;
    if *lines == label.changed {
        return Hunk {
            before: hunk.before,
            after: hunk.after,
            removed: hunk.removed.clone(),
            added: hunk.added.clone(),
        };
    }
    let run = |lines: &BTreeSet<usize>, region: Lines| match (lines.first(), lines.last()) {
        (Some(&first), Some(&last)) => Lines {
            start: first,
            count: last - first + 1,
        },
        _ => Lines {
            start: region.start,
            count: 0,
        },
    };
    Hunk {
        before: run(&lines.before, hunk.before),
        after: run(&lines.after, hunk.after),
        removed: lines.before.iter().copied().collect(),
        added: lines.after.iter().copied().collect(),
    }
}

/// What a `condition` change did, where the code shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ConditionChange {
    /// A comparison kept its operands and changed its operator.
    OperatorChanged,
    /// A value tested is now negated.
    NegationAdded,
    /// A negated value is now tested as it is.
    NegationRemoved,
    /// A branch now tests whether a value is missing or empty.
    GuardAdded,
    /// A branch no longer tests whether a value is missing or empty.
    GuardRemoved,
    /// A call's `true` or `false` argument is now computed.
    FlagComputed,
}

impl ConditionChange {
    fn as_str(self) -> &'static str {
        match self {
            ConditionChange::OperatorChanged => "operator_changed",
            ConditionChange::NegationAdded => "negation_added",
            ConditionChange::NegationRemoved => "negation_removed",
            ConditionChange::GuardAdded => "guard_added",
            ConditionChange::GuardRemoved => "guard_removed",
            ConditionChange::FlagComputed => "flag_computed",
        }
    }
}

/// A parsed file's per-tree facts, computed once for all its hunks.
struct Parsed<'a> {
    frontend: &'static dyn Frontend,
    before: TreeFacts<'a>,
    after: TreeFacts<'a>,
}

struct TreeFacts<'a> {
    tree: &'a Tree,
    /// Method, class and module definitions by qualified name.
    definitions: BTreeMap<Vec<String>, Vec<NodeId>>,
    /// Line ranges of test items.
    tests: Vec<(usize, usize)>,
    /// The first line of each assertion, ascending.
    assertions: Vec<usize>,
    /// Each literal regular expression's first line and pattern, in order.
    patterns: Vec<(usize, Pattern)>,
}

impl<'a> TreeFacts<'a> {
    fn new(tree: &'a Tree, frontend: &dyn Frontend) -> Self {
        let mut definitions: BTreeMap<Vec<String>, Vec<NodeId>> = BTreeMap::new();
        let mut tests = Vec::new();
        let mut assertions = Vec::new();
        let mut patterns = Vec::new();
        for id in tree.descendants(Tree::ROOT) {
            if let Some(pattern) = frontend.pattern(tree, id) {
                patterns.push((tree.node(id).extent.start, pattern));
            }
            if definition(tree, id) {
                definitions.entry(qualified(tree, id)).or_default().push(id);
            }
            match frontend.construct(tree, id) {
                Some(Construct::Test) => tests.push(tree.lines(id).into_inner()),
                Some(Construct::Assertion) => assertions.push(*tree.lines(id).start()),
                _ => {}
            }
        }
        assertions.sort_unstable();
        patterns.sort_by_key(|(at, _)| *at);
        let patterns = (patterns.into_iter())
            .map(|(at, p)| (tree.line_of(at), p))
            .collect();
        TreeFacts {
            tree,
            definitions,
            tests,
            assertions,
            patterns,
        }
    }
}

/// A method, class or module definition: what `adds_definition` counts.
fn definition(tree: &Tree, id: NodeId) -> bool {
    definition_role(tree.node(id).role)
}

fn definition_role(role: Role) -> bool {
    matches!(
        role,
        Role::Definition(NameKind::Method | NameKind::Class | NameKind::Module)
    )
}

/// The definition's name under its enclosing definitions' names. A nameless
/// one (a Rust `impl`) is named by its header's hash.
fn qualified(tree: &Tree, id: NodeId) -> Vec<String> {
    let mut path: Vec<String> = tree
        .path(id)
        .into_iter()
        .filter(|step| definition_role(step.role))
        .map(|step| match step.name {
            Some(name) => name.to_string(),
            None => format!("{:x}", step.header.unwrap_or(0)),
        })
        .collect();
    path.reverse();
    path
}

/// One side of a hunk: its tree and the lines it changed, ascending.
struct Side<'s> {
    facts: &'s TreeFacts<'s>,
    changed: &'s [usize],
    region: Lines,
}

impl Side<'_> {
    fn tree(&self) -> &Tree {
        self.facts.tree
    }

    fn touches(&self, (first, last): (usize, usize)) -> bool {
        let i = self.changed.partition_point(|&l| l < first);
        self.changed.get(i).is_some_and(|&l| l <= last)
    }

    fn touches_span(&self, span: &Span) -> bool {
        self.touches(self.tree().span_lines(span).into_inner())
    }

    /// Definitions here whose own syntax (the line naming it) the change
    /// touches, and whose name the other side doesn't define.
    fn unmatched_definitions(&self, other: &Side) -> Vec<NodeId> {
        self.facts
            .definitions
            .iter()
            .filter(|(key, _)| !other.facts.definitions.contains_key(*key))
            .flat_map(|(_, ids)| ids.iter().copied())
            .filter(|&id| {
                let own = self.tree().own_bytes(id);
                own.iter().any(|&(line, _)| self.touches((line, line)))
            })
            .collect()
    }
}

/// What makes two constructs the same: kind, name, own text and what it
/// tests or names.
type Key = (Construct, &'static str, Option<String>, Vec<u8>, Vec<u64>);

impl<'a> Parsed<'a> {
    fn new(trees: &'a Trees, frontend: &'static dyn Frontend) -> Self {
        Parsed {
            frontend,
            before: TreeFacts::new(&trees.before, frontend),
            after: TreeFacts::new(&trees.after, frontend),
        }
    }

    /// Adds the hunk's facets; returns what its condition change did, where
    /// the code shows it.
    /// In a test file (`test`) or test item, a `true` or `false` argument is
    /// an expected value, not a mode: no `flag_computed`.
    fn facets(
        &self,
        hunk: &Hunk,
        test: bool,
        facets: &mut BTreeSet<Facet>,
    ) -> BTreeSet<ConditionChange> {
        let before = Side {
            facts: &self.before,
            changed: &hunk.removed,
            region: hunk.before,
        };
        let after = Side {
            facts: &self.after,
            changed: &hunk.added,
            region: hunk.after,
        };
        if self.in_tests(&before) && self.in_tests(&after) && !self.no_code(&before, &after) {
            facets.insert(Facet::Test);
        }
        let added = after.unmatched_definitions(&before);
        let removed = before.unmatched_definitions(&after);
        if !added.is_empty() {
            facets.insert(Facet::AddsDefinition);
        }
        if !removed.is_empty() {
            facets.insert(Facet::RemovesDefinition);
        }
        if self.signature_changed(&before, &after) {
            facets.insert(Facet::Signature);
        }
        // A new definition's insides are new too: the definition says it.
        let (b, a) = (
            self.keys(&before, &skips(&before, &removed)),
            self.keys(&after, &skips(&after, &added)),
        );
        let differs = |pick: fn(&Construct) -> bool| {
            let side = |keys: &[Key]| {
                keys.iter()
                    .filter(|k| pick(&k.0))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            side(&b) != side(&a)
        };
        if differs(|c| {
            matches!(
                c,
                Construct::Branch { .. } | Construct::Comparison | Construct::Negation
            )
        }) {
            facets.insert(Facet::Condition);
        }
        let (skip_before, skip_after) = (skips(&before, &removed), skips(&after, &added));
        let (b_ids, a_ids) = (
            self.touched(&before, &skip_before),
            self.touched(&after, &skip_after),
        );
        let test = test || facets.contains(&Facet::Test);
        let condition = self.condition_changes((&before, &b_ids), (&after, &a_ids), test);
        // A computed flag is a branch moved into the callee.
        if !condition.is_empty() {
            facets.insert(Facet::Condition);
        }
        if self.output_text((&before, &b_ids), (&after, &a_ids)) {
            facets.insert(Facet::OutputText);
        }
        if self.keys_written(&before, &b_ids) != self.keys_written(&after, &a_ids) {
            facets.insert(Facet::SerializedKey);
        }
        if differs(|c| *c == Construct::Error) {
            facets.insert(Facet::ErrorHandling);
        }
        if self.literal_only(&before, &after) {
            facets.insert(Facet::LiteralOnly);
        }
        match regexes(&before, &after) {
            Some(regex::Change::Widened) => facets.insert(Facet::RegexWidened),
            Some(regex::Change::Changed) => facets.insert(Facet::RegexChanged),
            None => false,
        };
        condition
    }

    /// Text surfaces whose literals changed while the rest stayed: a surface
    /// on each side, unequal, with equal hashes once literals are masked.
    fn output_text(&self, (before, b): (&Side, &[NodeId]), (after, a): (&Side, &[NodeId])) -> bool {
        let surfaces = |t: &Tree, ids: &[NodeId]| -> Vec<(u64, u64)> {
            (ids.iter().copied())
                .filter(|&id| self.frontend.surface(t, id) == Some(Surface::Text))
                .map(|id| (self.masked(t, id, id), t.node(id).hash))
                .collect()
        };
        let (old, new) = (surfaces(before.tree(), b), surfaces(after.tree(), a));
        old.iter().any(|&(mask, hash)| {
            new.iter().any(|&(m, h)| m == mask && h != hash) && !new.iter().any(|&(_, h)| h == hash)
        })
    }

    /// `id`'s hash with every literal under `surface` masked, except under
    /// another call, whose result is what's printed.
    fn masked(&self, tree: &Tree, surface: NodeId, id: NodeId) -> u64 {
        tree.hash_with(id, |c| {
            let node = tree.node(c);
            Some(match self.frontend.construct(tree, c) {
                Some(Construct::Literal | Construct::Flag) if node.role != Role::DynamicName => 0,
                _ if c != surface && matches!(node.role, Role::Call(_)) => node.hash,
                _ => self.masked(tree, surface, c),
            })
        })
    }

    /// The keys serialized definitions write, of those the side's changed
    /// lines touch, as their naming nodes' hashes, sorted.
    fn keys_written(&self, side: &Side, ids: &[NodeId]) -> Vec<Vec<u64>> {
        let tree = side.tree();
        let mut keys: Vec<Vec<u64>> = (ids.iter().copied())
            .filter_map(|id| match self.frontend.surface(tree, id) {
                Some(Surface::Key(names)) => Some((id, names)),
                _ => None,
            })
            .filter(|(_, names)| (names.iter()).any(|&n| side.touches_span(&tree.node(n).extent)))
            .filter(|&(id, _)| {
                (tree.ancestors(id))
                    .any(|p| self.frontend.surface(tree, p) == Some(Surface::Serialized))
            })
            .map(|(_, names)| names.iter().map(|&n| tree.node(n).hash).collect())
            .collect();
        keys.sort_unstable();
        keys
    }

    /// Nodes on the side whose extent touches its changed lines, outside `skip`.
    fn touched(&self, side: &Side, skip: &[Span]) -> Vec<NodeId> {
        let tree = side.tree();
        let mut out = Vec::new();
        let mut stack = vec![Tree::ROOT];
        while let Some(id) = stack.pop() {
            let node = tree.node(id);
            if !side.touches_span(&node.extent) || skip.iter().any(|s| contains(s, &node.extent)) {
                continue;
            }
            stack.extend(node.children.iter().copied());
            out.push(id);
        }
        out
    }

    /// The construct `id` is, when the change touches its own syntax.
    fn own_construct(&self, side: &Side, id: NodeId) -> Option<Construct> {
        let own = side.tree().own_bytes(id);
        let touched = own.iter().any(|&(line, _)| side.touches((line, line)));
        self.frontend.construct(side.tree(), id).filter(|_| touched)
    }

    fn condition_changes(
        &self,
        (before, b): (&Side, &[NodeId]),
        (after, a): (&Side, &[NodeId]),
        test: bool,
    ) -> BTreeSet<ConditionChange> {
        let (b, a) = (b.to_vec(), a.to_vec());
        let mut out = BTreeSet::new();
        let picked = |side: &Side, ids: &[NodeId], want: Construct| -> Vec<NodeId> {
            (ids.iter().copied())
                .filter(|&id| self.own_construct(side, id) == Some(want))
                .collect()
        };
        let hashes = |t: &Tree, ids: &[NodeId]| -> Vec<u64> {
            ids.iter().map(|&id| t.node(id).hash).collect()
        };
        // Operator: equal operands, other own text.
        let operands = |t: &Tree, id: NodeId| hashes(t, &t.code_children(id).collect::<Vec<_>>());
        let own =
            |t: &Tree, id: NodeId| -> Vec<u8> { t.own_bytes(id).iter().map(|&(_, c)| c).collect() };
        let (bt, at) = (before.tree(), after.tree());
        let old = picked(before, &b, Construct::Comparison);
        let new = picked(after, &a, Construct::Comparison);
        if old.iter().any(|&o| {
            new.iter().any(|&n| {
                operands(bt, o) == operands(at, n)
                    && own(bt, o) != own(at, n)
                    && bt.node(o).hash != at.node(n).hash
            })
        }) {
            out.insert(ConditionChange::OperatorChanged);
        }
        // Negation: an operand negated on one side, tested bare on the other.
        let negated = |side: &Side, ids: &[NodeId]| -> Vec<u64> {
            (picked(side, ids, Construct::Negation).into_iter())
                .filter_map(|n| side.tree().code_children(n).next())
                .map(|c| side.tree().node(c).hash)
                .collect()
        };
        let (nb, na) = (negated(before, &b), negated(after, &a));
        let present = |t: &Tree, ids: &[NodeId], h: u64| ids.iter().any(|&id| t.node(id).hash == h);
        if minus(&na, &nb).iter().any(|&h| present(bt, &b, h)) {
            out.insert(ConditionChange::NegationAdded);
        }
        if minus(&nb, &na).iter().any(|&h| present(at, &a, h)) {
            out.insert(ConditionChange::NegationRemoved);
        }
        // Guard: a missing-or-empty check a branch tests.
        let guards = |side: &Side, ids: &[NodeId]| -> Vec<u64> {
            (picked(side, ids, Construct::Guard).into_iter())
                .filter(|&g| self.in_branch_test(side.tree(), g))
                .map(|g| side.tree().node(g).hash)
                .collect()
        };
        let (gb, ga) = (guards(before, &b), guards(after, &a));
        if !minus(&ga, &gb).is_empty() {
            out.insert(ConditionChange::GuardAdded);
        }
        if !minus(&gb, &ga).is_empty() {
            out.insert(ConditionChange::GuardRemoved);
        }
        if !test && self.flag_computed((before, &b), (after, &a)) {
            out.insert(ConditionChange::FlagComputed);
        }
        out
    }

    /// `id` lies inside what an enclosing conditional tests.
    fn in_branch_test(&self, tree: &Tree, id: NodeId) -> bool {
        let mut child = id;
        for parent in tree.ancestors(id) {
            if let Some(Construct::Branch { test }) = self.frontend.construct(tree, parent)
                && tree.code_children(parent).take(test).any(|c| c == child)
            {
                return true;
            }
            child = parent;
        }
        false
    }

    /// A call passing `true` or `false` on the before side passes a computed
    /// value in its place on the after side, everything else equal.
    fn flag_computed(
        &self,
        (before, b): (&Side, &[NodeId]),
        (after, a): (&Side, &[NodeId]),
    ) -> bool {
        let (bt, at) = (before.tree(), after.tree());
        // A flag's place: the call, and its index path to the flag.
        let place = |t: &Tree, flag: NodeId| -> Option<(NodeId, Vec<usize>)> {
            let mut path = Vec::new();
            let mut child = flag;
            for parent in t.ancestors(flag).take(2) {
                path.push(t.code_children(parent).position(|c| c == child)?);
                if matches!(t.node(parent).role, Role::Call(_)) {
                    path.reverse();
                    return Some((parent, path));
                }
                child = parent;
            }
            None
        };
        let calls: Vec<NodeId> = (a.iter().copied())
            .filter(|&id| matches!(at.node(id).role, Role::Call(_)))
            .collect();
        (b.iter().copied())
            .filter(|&id| self.frontend.construct(bt, id) == Some(Construct::Flag))
            .filter_map(|flag| place(bt, flag))
            .any(|(call, path)| {
                calls.iter().any(|&other| {
                    let (old, new) = (bt.node(call), at.node(other));
                    old.role == new.role
                        && old.name == new.name
                        && self.differs_only_at(bt, call, at, other, &path)
                })
            })
    }

    /// The subtrees are equal but along `path`, where the after side holds a
    /// computed value: not a literal, a flag or a constant.
    fn differs_only_at(&self, bt: &Tree, b: NodeId, at: &Tree, a: NodeId, path: &[usize]) -> bool {
        let Some((&i, rest)) = path.split_first() else {
            // Computed: it reads a variable or calls something.
            return std::iter::once(a).chain(at.descendants(a)).any(|n| {
                matches!(
                    at.node(n).role,
                    Role::Call(_)
                        | Role::Reference(
                            NameKind::Variable
                                | NameKind::Parameter
                                | NameKind::Field
                                | NameKind::Member
                                | NameKind::Global
                        )
                )
            });
        };
        let (old, new): (Vec<NodeId>, Vec<NodeId>) =
            (bt.code_children(b).collect(), at.code_children(a).collect());
        old.len() == new.len()
            && bt.node(b).kind == at.node(a).kind
            && old.iter().zip(&new).enumerate().all(|(j, (&o, &n))| {
                if j == i {
                    self.differs_only_at(bt, o, at, n, rest)
                } else {
                    bt.node(o).hash == at.node(n).hash
                }
            })
    }

    /// Assertions starting on the hunk's changed lines, removed and added,
    /// outside lines `relocated` verbatim to or from another hunk.
    fn assertions(&self, hunk: &Hunk, relocated: &[Relocation]) -> (usize, usize) {
        let count = |facts: &TreeFacts, changed: &[usize], added: bool| {
            let moved = |line: &usize| {
                relocated
                    .iter()
                    .any(|r| r.added == added && r.lines.contains(line))
            };
            (facts.assertions.iter())
                .filter(|l| changed.binary_search(l).is_ok() && !moved(l))
                .count()
        };
        (
            count(&self.before, &hunk.removed, false),
            count(&self.after, &hunk.added, true),
        )
    }

    /// A changed code line on either side lies in a test item.
    fn touches_tests(&self, hunk: &Hunk) -> bool {
        let touches = |facts: &TreeFacts, changed: &[usize]| {
            (changed.iter())
                .filter(|&&l| facts.tree.line(l).code)
                .any(|&l| facts.tests.iter().any(|&(f, t)| f <= l && l <= t))
        };
        touches(&self.before, &hunk.removed) || touches(&self.after, &hunk.added)
    }

    /// Every changed code line on the side lies in a test item.
    fn in_tests(&self, side: &Side) -> bool {
        side.changed
            .iter()
            .filter(|&&l| side.tree().line(l).code)
            .all(|&l| side.facts.tests.iter().any(|&(f, t)| f <= l && l <= t))
    }

    fn no_code(&self, before: &Side, after: &Side) -> bool {
        [before, after]
            .iter()
            .all(|s| s.changed.iter().all(|&l| !s.tree().line(l).code))
    }

    /// A method on both sides, touched by the change, whose signature differs.
    fn signature_changed(&self, before: &Side, after: &Side) -> bool {
        let signatures = |side: &Side, ids: &[NodeId]| {
            let mut sigs: Vec<u64> = ids
                .iter()
                .map(|&id| self.signature(side.tree(), id))
                .collect();
            sigs.sort_unstable();
            sigs
        };
        self.before.definitions.iter().any(|(key, old)| {
            let Some(new) = self.after.definitions.get(key) else {
                return false;
            };
            let method = |t: &Tree, ids: &[NodeId]| {
                ids.iter()
                    .any(|&id| t.node(id).role == Role::Definition(NameKind::Method))
            };
            let touched = |side: &Side, ids: &[NodeId]| {
                ids.iter()
                    .any(|&id| side.touches_span(&side.tree().node(id).extent))
            };
            method(before.tree(), old)
                && (touched(before, old) || touched(after, new))
                && signatures(before, old) != signatures(after, new)
        })
    }

    /// Hash of a definition's parameter list and annotations (return type,
    /// generics, visibility), whitespace aside.
    fn signature(&self, tree: &Tree, id: NodeId) -> u64 {
        let mut h = DefaultHasher::new();
        for &c in &tree.node(id).children {
            let child = tree.node(c);
            if child.role == Role::Annotation
                || self.frontend.construct(tree, c) == Some(Construct::Parameters)
            {
                child.hash.hash(&mut h);
            }
        }
        h.finish()
    }

    /// The constructs on the side's changed lines, outside `skip`, sorted.
    fn keys(&self, side: &Side, skip: &[Span]) -> Vec<Key> {
        let tree = side.tree();
        let mut keys = Vec::new();
        let mut stack = vec![Tree::ROOT];
        while let Some(id) = stack.pop() {
            let node = tree.node(id);
            if !side.touches_span(&node.extent) || skip.iter().any(|s| contains(s, &node.extent)) {
                continue;
            }
            stack.extend(node.children.iter().copied());
            let Some(construct) = self.frontend.construct(tree, id) else {
                continue;
            };
            let own = tree.own_bytes(id);
            // Where the construct's own syntax is (`?`, `rescue`, `>`), or
            // its first line when it has none of its own (`Err(e)`).
            let own_touched = || match own.is_empty() {
                true => side.touches_span(&(node.extent.start..node.extent.start + 1)),
                false => own.iter().any(|&(line, _)| side.touches((line, line))),
            };
            let hashes: Vec<u64> = match construct {
                Construct::Branch { test } => {
                    let tests: Vec<NodeId> = tree.code_children(id).take(test).collect();
                    if !tests
                        .iter()
                        .any(|&c| side.touches_span(&tree.node(c).extent))
                    {
                        continue;
                    }
                    tests.iter().map(|&c| tree.node(c).hash).collect()
                }
                Construct::Comparison | Construct::Negation | Construct::Error => {
                    if !own_touched() {
                        continue;
                    }
                    node.children
                        .iter()
                        .map(|&c| tree.node(c))
                        .filter(|c| {
                            matches!(
                                c.role,
                                Role::Reference(
                                    NameKind::Class | NameKind::Constant | NameKind::Module
                                )
                            )
                        })
                        .map(|c| c.hash)
                        .collect()
                }
                _ => continue,
            };
            let text = match construct {
                Construct::Branch { .. } => Vec::new(),
                _ => own.iter().map(|&(_, b)| b).collect(),
            };
            keys.push((
                construct,
                node.kind,
                node.name.as_deref().map(str::to_string),
                text,
                hashes,
            ));
        }
        keys.sort();
        keys
    }

    /// Both regions say the same once literals and comments are masked, and
    /// hold the same number of literals, but not the same code reordered
    /// (swapped match arms keep every literal).
    fn literal_only(&self, before: &Side, after: &Side) -> bool {
        let inside = |side: &Side| {
            let tree = side.tree();
            let region = side
                .region
                .last()
                .map_or(0..0, |last| tree.line_span(side.region.start, last));
            let mut hashes: Vec<u64> = (tree.touching(&region))
                .map(|id| tree.node(id))
                .filter(|n| !n.comment && contains(&region, &n.extent))
                .map(|n| n.hash)
                .collect();
            hashes.sort_unstable();
            hashes
        };
        if inside(before) == inside(after) {
            return false;
        }
        let masked = |side: &Side| {
            let tree = side.tree();
            let last = side.region.last()?;
            let region = tree.line_span(side.region.start, last);
            let overlapping: Vec<(NodeId, &crate::tree::Node)> = (tree.touching(&region))
                .map(|id| (id, tree.node(id)))
                .filter(|(_, n)| n.extent.start < region.end && region.start < n.extent.end)
                .collect();
            let literals: Vec<Span> = overlapping
                .iter()
                .filter(|&&(id, n)| {
                    n.role != Role::DynamicName
                        && matches!(
                            self.frontend.construct(tree, id),
                            Some(Construct::Literal | Construct::Flag)
                        )
                })
                .map(|(_, n)| n.extent.clone())
                .collect();
            let count = literals.len();
            let mut skip = literals;
            skip.extend(
                overlapping
                    .iter()
                    .filter(|(_, n)| n.comment)
                    .map(|(_, n)| n.extent.clone()),
            );
            let bytes = super::site::Significance::of(tree, side.region).bytes(tree, &skip);
            Some((bytes, count))
        };
        match (masked(before), masked(after)) {
            (Some((b, n)), Some((a, m))) => n > 0 && n == m && a == b,
            _ => false,
        }
    }
}

/// How the literal regular expressions on the changed lines changed: those
/// on one side and not the other, paired in order when they are as many
/// (none added or dropped outright). `None` when none changed that way.
fn regexes<'s>(before: &Side<'s>, after: &Side<'s>) -> Option<regex::Change> {
    fn on<'s>(side: &Side<'s>) -> Vec<&'s Pattern> {
        (side.facts.patterns.iter())
            .filter(|(line, _)| side.changed.binary_search(line).is_ok())
            .map(|(_, p)| p)
            .collect()
    }
    let (mut old, mut new) = (on(before), on(after));
    old.retain(|p| match new.iter().position(|q| q == p) {
        Some(i) => {
            new.remove(i);
            false
        }
        None => true,
    });
    if old.is_empty() || old.len() != new.len() {
        return None;
    }
    let widened = old.iter().zip(&new).all(|(o, n)| {
        o.flags == n.flags
            && !o.extended
            && !n.extended
            && regex::compare(&o.body, &n.body) == regex::Change::Widened
    });
    Some(match widened {
        true => regex::Change::Widened,
        false => regex::Change::Changed,
    })
}

/// The elements of `a` left once each of `b`'s is taken out, one for one.
fn minus(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut left = b.to_vec();
    a.iter()
        .copied()
        .filter(|h| match left.iter().position(|x| x == h) {
            Some(i) => {
                left.swap_remove(i);
                false
            }
            None => true,
        })
        .collect()
}

/// The extents of definitions a side gained or lost outright.
fn skips(side: &Side, ids: &[NodeId]) -> Vec<Span> {
    ids.iter()
        .map(|&id| side.tree().node(id).extent.clone())
        .collect()
}

fn contains(outer: &Span, inner: &Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALL` holds the variants in declaration order. The match is
    /// exhaustive, so a new variant fails to compile here: add it to `ALL`.
    #[test]
    fn all_holds_every_facet() {
        for (i, &facet) in Facet::ALL.iter().enumerate() {
            assert_eq!(facet as usize, i, "{facet:?}");
            match facet {
                Facet::Test
                | Facet::AddsDefinition
                | Facet::RemovesDefinition
                | Facet::Signature
                | Facet::Condition
                | Facet::ErrorHandling
                | Facet::LiteralOnly
                | Facet::RemovesAssertions
                | Facet::RegexWidened
                | Facet::RegexChanged
                | Facet::OutputText => {}
                Facet::SerializedKey => {
                    assert_eq!(i, Facet::ALL.len() - 1, "the last variant")
                }
            }
        }
    }
}
