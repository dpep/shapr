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
use crate::frontend::{self, Construct, Frontend, Pattern};
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
}

impl Facet {
    pub const ALL: [Facet; 10] = [
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
                parsed.facets(&hunk, &mut facets);
                let test = file.category == Category::Test || facets.contains(&Facet::Test);
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

    fn facets(&self, hunk: &Hunk, facets: &mut BTreeSet<Facet>) {
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
        if differs(|c| matches!(c, Construct::Branch { .. } | Construct::Comparison)) {
            facets.insert(Facet::Condition);
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
                Construct::Comparison | Construct::Error => {
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
                        && self.frontend.construct(tree, id) == Some(Construct::Literal)
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
                | Facet::RegexWidened => {}
                Facet::RegexChanged => {
                    assert_eq!(i, Facet::ALL.len() - 1, "the last variant")
                }
            }
        }
    }
}
