//! `move`: a declaration that leaves one place and reappears in another, in
//! the same file or another.
//!
//! Each file's two trees are aligned container by container (the file, then
//! each class or module body): a heaviest common subsequence over the
//! children, declarations keyed by name (so one edited in place still
//! aligns, and a namespace's body aligns in turn), other statements by hash.
//! A declaration (`Role::is_declaration`) left out of the alignment is a
//! *unit*, removed or added. A removed unit pairs with an
//! added one by equal hash or by name and subtree similarity (a body that
//! also changed). A pair is structure only when the code around it means the
//! same in both places: the same owner, the same visibility section, the
//! same order relative to statements that name it at load time and, across
//! files, the same file-level directives, in a language whose files aren't
//! modules of their own. Otherwise its part names each [`Context`] that
//! differs: relocated code, but not structure.
//!
//! A hunk's part comes from the trees, not from git's line placement: it
//! holds the changed lines in a paired unit or in a node the alignment kept
//! unchanged (git may slide a removal across identical lines), except the
//! unit's lines that changed in the move, which stay logic.

use super::align::{Alignments, Side, lcs, namespace_name, size, transparent};
use super::format::{dice, subtree_hashes};
use super::hazard::{Op, blocks};
use super::{Claim, Confidence, Kind, Labels, Part, rounded};
use crate::change::{Change, Trees};
use crate::tree::{Flags, Hazard, NameKind, NodeId, Role, Tree, Visibility, spells};
use serde::Serialize;
use serde_json::{Map, json};
use std::collections::HashMap;
use std::ops::RangeInclusive;

/// Fewest nodes a unit needs. Size evidence is `1 - 1/nodes`: one node is
/// no evidence, two a coin flip, so three is the first size whose match
/// beats chance. It keeps out a bare name or a constant set to one literal.
const FLOOR: usize = 3;

/// Least subtree similarity for a near match: at least half the subtrees
/// shared, so the bodies are more alike than not.
const NEAR: f64 = 0.5;

pub(super) fn run(change: &Change, alignments: &Alignments, labels: &mut Labels) {
    let mut units = Vec::new();
    // Files whose declarations take part: parsed cleanly, with lines left.
    let mut aligned = vec![false; change.files.len()];
    for (f, file) in alignments.files() {
        if labels[f].iter().all(|l| l.unclaimed().is_empty()) {
            continue;
        }
        aligned[f] = true;
        let Ok(trees) = &change.files[f].trees else {
            continue;
        };
        let units_here = file.unmatched.iter().filter(|&&(side, node)| {
            let tree = match side {
                Side::Removed => &trees.before,
                Side::Added => &trees.after,
            };
            is_unit(tree, node)
        });
        units.extend(units_here.map(|&(side, node)| Unit {
            file: f,
            side,
            node,
        }));
    }
    let pairs = pair(change, &units);
    let ids = hunk_ids(change);
    let mut by_unit: HashMap<usize, usize> = HashMap::new();
    for (p, pair) in pairs.iter().enumerate() {
        // Both halves must be lines the diff changed, or the trees and the
        // diff disagree about what moved.
        if units[pair.from].hunk(change).is_none() || units[pair.to].hunk(change).is_none() {
            continue;
        }
        by_unit.insert(pair.from, p);
        by_unit.insert(pair.to, p);
    }

    for (f, (file, labels)) in change.files.iter().zip(labels.iter_mut()).enumerate() {
        let Ok(trees) = &file.trees else { continue };
        for (h, label) in labels.iter_mut().enumerate() {
            if label.unclaimed().is_empty() || !aligned[f] {
                continue;
            }
            let file = alignments.file(f).expect("aligned files parsed cleanly");
            let sides = [
                (Side::Removed, &trees.before, &label.changed.before),
                (Side::Added, &trees.after, &label.changed.after),
            ];
            let mut mine = Vec::new();
            // Changed lines in a paired unit or in code the alignment kept
            // (git may slide a removal across identical lines).
            let mut explained = Claim::default();
            for (side, tree, changed) in sides {
                let here: Vec<(usize, RangeInclusive<usize>)> = (0..units.len())
                    .filter(|&u| units[u].file == f && units[u].side == side)
                    .filter(|u| by_unit.contains_key(u))
                    .map(|u| (u, tree.lines(units[u].node)))
                    .filter(|(_, r)| changed.iter().any(|&l| r.contains(&l) && tree.line(l).code))
                    .collect();
                mine.extend(here.iter().filter_map(|(u, _)| by_unit.get(u).copied()));
                let ranges = file.kept(side);
                let lines = changed.iter().copied().filter(|l| {
                    ranges.iter().any(|r| r.contains(l)) || here.iter().any(|(_, r)| r.contains(l))
                });
                match side {
                    Side::Removed => explained.before.extend(lines),
                    Side::Added => explained.after.extend(lines),
                }
            }
            if mine.is_empty() {
                continue;
            }
            mine.sort_unstable();
            mine.dedup();
            let mine: Vec<&Pair> = mine.iter().map(|&p| &pairs[p]).collect();
            let exact = mine.iter().all(|p| p.similarity == 1.0);
            let mut context: Vec<Context> = mine.iter().flat_map(|p| p.context.clone()).collect();
            context.sort_unstable();
            context.dedup();
            let mut reason: String = if exact {
                "declarations moved unchanged".into()
            } else {
                "declarations moved, and their bodies changed".into()
            };
            if !context.is_empty() {
                let why: Vec<&str> = context.iter().map(|c| c.reason()).collect();
                reason = format!("{reason}, but {}", why.join("; "));
            }
            let confidence = Confidence::new(
                mine.iter()
                    .map(|p| p.confidence())
                    .fold(f64::INFINITY, f64::min),
            );
            let mut evidence = label.residual.evidence.clone();
            let values = &mut evidence.values;
            values.clear();
            let similarity = mine.iter().map(|p| p.similarity).fold(1.0, f64::min);
            values.insert("similarity".into(), json!(similarity));
            values.insert("nodes".into(), json!(mine.iter().map(|p| p.nodes).min()));
            values.insert(
                "candidates".into(),
                json!(mine.iter().map(|p| p.candidates).max()),
            );
            let entries: Vec<Entry> = mine
                .iter()
                .map(|p| entry(change, &units, &ids, p))
                .collect();
            let (changed, first) = moved_changes(&entries, ids[f] + h);
            let names: Vec<&str> = context.iter().map(|c| c.as_str()).collect();
            let lines = explained.minus(&changed);
            let part = Part {
                kind: Kind::Move,
                review: first.intersect(&lines),
                lines,
                confidence,
                reason,
                evidence,
                detail: Map::from_iter([
                    ("moves".to_string(), json!(entries)),
                    ("context".to_string(), json!(names)),
                ]),
                context,
            };
            label.claim(part);
        }
    }
}

/// The lines inside the hunk's moved units that changed in the move
/// (before-side lines of the units it removed, after-side lines of the units
/// it added), and the first line of each unit moved into a new context,
/// where that context shows.
fn moved_changes(entries: &[Entry], id: usize) -> (Claim, Claim) {
    let (mut changed, mut first) = (Claim::default(), Claim::default());
    for entry in entries {
        let start = |line: Option<usize>| line.filter(|_| !entry.context.is_empty());
        if entry.from_hunk == Some(id) {
            changed.before.extend(&entry.changed_before);
            first.before.extend(start(entry.from_line));
        }
        if entry.to_hunk == Some(id) {
            changed.after.extend(&entry.changed_after);
            first.after.extend(start(entry.to_line));
        }
    }
    (changed, first)
}

/// A declaration the alignment left out: removed from the before tree or
/// added to the after tree.
struct Unit {
    file: usize,
    side: Side,
    node: NodeId,
}

impl Unit {
    fn trees<'c>(&self, change: &'c Change) -> &'c Trees {
        change.files[self.file]
            .trees
            .as_ref()
            .expect("units come from parsed files")
    }

    fn tree<'c>(&self, change: &'c Change) -> &'c Tree {
        let trees = self.trees(change);
        match self.side {
            Side::Removed => &trees.before,
            Side::Added => &trees.after,
        }
    }

    /// The hunk whose changed lines on this unit's side first touch it.
    fn hunk(&self, change: &Change) -> Option<usize> {
        let range = self.tree(change).lines(self.node);
        change.files[self.file].diff.hunks.iter().position(|h| {
            let changed = match self.side {
                Side::Removed => &h.removed,
                Side::Added => &h.added,
            };
            changed.iter().any(|l| range.contains(l))
        })
    }
}

fn is_unit(tree: &Tree, node: NodeId) -> bool {
    tree.node(node).role.is_declaration()
        && size(tree, node) >= FLOOR
        && owner(tree, node).is_some()
}

/// The namespaces enclosing `node`, outermost first, each as its name's
/// subtree hash. `None` when some ancestor is neither a namespace nor
/// transparent: a block, a conditional, a call. A declaration there runs
/// under that construct, so moving it out or in is not a reorder.
fn owner(tree: &Tree, node: NodeId) -> Option<Vec<u64>> {
    let mut chain = Vec::new();
    let mut at = tree.node(node).parent;
    while let Some(id) = at {
        let n = tree.node(id);
        if matches!(n.role, Role::Definition(NameKind::Class | NameKind::Module)) {
            chain.push(namespace_name(tree, id, true)?);
        } else if !transparent(tree, id) {
            return None;
        }
        at = n.parent;
    }
    chain.reverse();
    Some(chain)
}

/// What differs around a moved unit in its new place: the code it runs
/// among, apart from any change to its own body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Context {
    /// Each file is a module (Rust): another file is another module.
    Module,
    /// Another enclosing class, module or `impl`.
    Owner,
    /// Another visibility section (it crossed `private`).
    Visibility,
    /// Another order relative to a sibling that names it, or that it names,
    /// at load time.
    LoadOrder,
    /// Files with different file-level directives.
    Directives,
    /// The code reads its own line (`__LINE__`), or its own file
    /// (`__FILE__`) and moved to another file.
    Position,
}

impl Context {
    pub const ALL: [Context; 6] = [
        Context::Module,
        Context::Owner,
        Context::Visibility,
        Context::LoadOrder,
        Context::Directives,
        Context::Position,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Context::Module => "module",
            Context::Owner => "owner",
            Context::Visibility => "visibility",
            Context::LoadOrder => "load_order",
            Context::Directives => "directives",
            Context::Position => "position",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Context::Module => "its file is its module",
            Context::Owner => "its owner (class or module) changes",
            Context::Visibility => "its visibility section changes",
            Context::LoadOrder => {
                "it moves past a statement that names it or that it names at load time"
            }
            Context::Directives => "the files' file-level directives differ",
            Context::Position => "it reads its own line or file",
        }
    }
}

/// A matched removal and addition.
struct Pair {
    from: usize,
    to: usize,
    similarity: f64,
    /// Units on the other side that matched equally well.
    candidates: usize,
    /// The smaller unit's node count.
    nodes: usize,
    /// What differs around the unit in its new place.
    context: Vec<Context>,
}

impl Pair {
    fn confidence(&self) -> f64 {
        self.similarity * (1.0 - 1.0 / self.nodes as f64) / self.candidates as f64
    }
}

/// Exact matches first, then near matches among what's left. Ties go to the
/// pairing whose context checks pass, then to the same file.
fn pair(change: &Change, units: &[Unit]) -> Vec<Pair> {
    let removed: Vec<usize> = (0..units.len())
        .filter(|&u| units[u].side == Side::Removed)
        .collect();
    let added: Vec<usize> = (0..units.len())
        .filter(|&u| units[u].side == Side::Added)
        .collect();
    let hash = |u: usize| units[u].tree(change).node(units[u].node).hash;
    let nodes = |u: usize| size(units[u].tree(change), units[u].node);

    let mut pairs = Vec::new();
    let mut taken = vec![false; units.len()];

    let mut exact = Vec::new();
    for &r in &removed {
        let partners: Vec<usize> = added
            .iter()
            .copied()
            .filter(|&a| hash(a) == hash(r))
            .collect();
        for &a in &partners {
            let rivals = removed.iter().filter(|&&o| hash(o) == hash(r)).count();
            exact.push((r, a, 1.0, partners.len().max(rivals)));
        }
    }
    claim(change, units, exact, &mut taken, &mut pairs, nodes);

    let similar = |r: usize, a: usize| -> Option<f64> {
        let (ur, ua) = (&units[r], &units[a]);
        let (nr, na) = (ur.tree(change).node(ur.node), ua.tree(change).node(ua.node));
        let same = nr.role == na.role && nr.name.is_some() && nr.name == na.name;
        if !same {
            return None;
        }
        let sim = rounded(dice(
            subtree_hashes(ur.tree(change), &[ur.node]),
            subtree_hashes(ua.tree(change), &[ua.node]),
        ));
        (sim >= NEAR).then_some(sim)
    };
    let free_r: Vec<usize> = removed.iter().copied().filter(|&u| !taken[u]).collect();
    let free_a: Vec<usize> = added.iter().copied().filter(|&u| !taken[u]).collect();
    let mut near = Vec::new();
    for &r in &free_r {
        let partners: Vec<(usize, f64)> = free_a
            .iter()
            .filter_map(|&a| similar(r, a).map(|s| (a, s)))
            .collect();
        for &(a, sim) in &partners {
            let rivals = free_r.iter().filter(|&&o| similar(o, a).is_some()).count();
            near.push((r, a, sim, partners.len().max(rivals)));
        }
    }
    claim(change, units, near, &mut taken, &mut pairs, nodes);
    pairs
}

type Candidate = (usize, usize, f64, usize);

fn claim(
    change: &Change,
    units: &[Unit],
    candidates: Vec<Candidate>,
    taken: &mut [bool],
    pairs: &mut Vec<Pair>,
    nodes: impl Fn(usize) -> usize,
) {
    let mut ranked: Vec<(Candidate, Vec<Context>)> = candidates
        .into_iter()
        .map(|c| (c, check(change, &units[c.0], &units[c.1])))
        .collect();
    ranked.sort_by(|((r1, a1, s1, _), w1), ((r2, a2, s2, _), w2)| {
        let same_file = |r: &usize, a: &usize| units[*r].file == units[*a].file;
        s2.total_cmp(s1)
            .then((!w1.is_empty()).cmp(&!w2.is_empty()))
            .then(same_file(r2, a2).cmp(&same_file(r1, a1)))
    });
    for ((from, to, similarity, candidates), context) in ranked {
        if taken[from] || taken[to] {
            continue;
        }
        taken[from] = true;
        taken[to] = true;
        pairs.push(Pair {
            from,
            to,
            similarity,
            candidates,
            nodes: nodes(from).min(nodes(to)),
            context,
        });
    }
}

/// What differs around the unit in its new place.
fn check(change: &Change, from: &Unit, to: &Unit) -> Vec<Context> {
    let (tb, ta) = (from.tree(change), to.tree(change));
    let across = from.file != to.file;
    // A visibility section applies to methods only.
    let method = tb.node(from.node).role == Role::Definition(NameKind::Method);
    [
        (
            Context::Module,
            across && (tb.semantics.file_scoped || ta.semantics.file_scoped),
        ),
        (Context::Owner, owner(tb, from.node) != owner(ta, to.node)),
        (
            Context::Visibility,
            method && visibility(tb, from.node) != visibility(ta, to.node),
        ),
        (
            Context::LoadOrder,
            dependencies(tb, from.node) != dependencies(ta, to.node),
        ),
        (
            Context::Directives,
            across && directives(tb) != directives(ta),
        ),
        (Context::Position, positional(tb, from.node, across)),
    ]
    .into_iter()
    .filter_map(|(context, differs)| differs.then_some(context))
    .collect()
}

/// Whether the unit reads its own line, or its own file when it changed files.
fn positional(tree: &Tree, node: NodeId, across: bool) -> bool {
    let op = if across {
        Op::MoveAcrossFiles
    } else {
        Op::Move
    };
    tree.descendants(node)
        .any(|d| tree.node(d).role.hazard().is_some_and(|h| blocks(op, h)))
}

/// The section a declaration sits in: the last bare visibility statement
/// (`private` with no arguments) before it among its siblings.
fn visibility(tree: &Tree, node: NodeId) -> Option<Visibility> {
    let parent = tree.node(tree.node(node).parent?);
    parent
        .children
        .iter()
        .take_while(|&&c| c != node)
        .filter_map(|&c| match tree.node(c).role {
            Role::Visibility(v) if tree.node(c).children.iter().all(|&g| tree.node(g).comment) => {
                Some(v)
            }
            _ => None,
        })
        .last()
}

/// Siblings whose order relative to `node` matters, as (hash, comes first),
/// sorted: those that name what it declares at load time (`alias_method`,
/// `private :x`, a constant read), that declare the same name, or that
/// declare a name its own load-time code reads. For anything but a method,
/// whose body runs later, every statement that runs code counts (not a
/// declaration or a bare `private`): a `require` or a call may read it.
fn dependencies(tree: &Tree, node: NodeId) -> Vec<(u64, bool)> {
    let Some(parent) = tree.node(node).parent else {
        return Vec::new();
    };
    let declared = declared_name(tree, node);
    let reads = mentions(tree, node);
    // A method body runs later; anything else runs where it stands, so a
    // statement beside it may need it (or change it) by the time it runs.
    let runs_now = tree.node(node).role != Role::Definition(NameKind::Method);
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
        let theirs = declared_name(tree, sibling);
        let depends = (runs_now && !inert(tree, sibling))
            || declared.is_some_and(|d| {
                theirs == Some(d) || mentions(tree, sibling).iter().any(|m| spells(m, d))
            })
            || theirs.is_some_and(|t| reads.contains(&t))
            || macro_name(tree, sibling).is_some_and(|m| invokes(tree, node, m));
        if depends {
            out.push((tree.node(sibling).hash, before));
        }
    }
    out.sort_unstable();
    out
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
    match n.role {
        Role::Definition(NameKind::Constant) => !calls(),
        Role::Visibility(_) => n.children.iter().all(|&c| tree.node(c).comment),
        role => role.is_declaration(),
    }
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

/// File-level directives (a magic comment) apply to everything in the file.
fn directives(tree: &Tree) -> Vec<u64> {
    let mut out: Vec<u64> = tree
        .node(Tree::ROOT)
        .children
        .iter()
        .map(|&c| tree.node(c))
        .filter(|n| n.flags.contains(Flags::DIRECTIVE))
        .map(|n| n.hash)
        .collect();
    out.sort_unstable();
    out
}

/// Each hunk's output `id`, by `[file][hunk]`.
fn hunk_ids(change: &Change) -> Vec<usize> {
    let mut next = 0;
    change
        .files
        .iter()
        .map(|f| {
            let first = next;
            next += f.diff.hunks.len();
            first
        })
        .collect()
}

/// One pair touching a hunk, as `detail.moves` lists it.
#[derive(Serialize)]
struct Entry {
    name: Option<String>,
    from_hunk: Option<usize>,
    to_hunk: Option<usize>,
    from_line: Option<usize>,
    to_line: Option<usize>,
    #[serde(serialize_with = "context_names")]
    context: Vec<Context>,
    similarity: f64,
    changed_before: Vec<usize>,
    changed_after: Vec<usize>,
}

fn context_names<S: serde::Serializer>(context: &[Context], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(context.iter().map(|c| c.as_str()))
}

fn entry(change: &Change, units: &[Unit], ids: &[usize], pair: &Pair) -> Entry {
    let (from, to) = (&units[pair.from], &units[pair.to]);
    let id = |u: &Unit| u.hunk(change).map(|h| ids[u.file] + h);
    let first = |u: &Unit| {
        let tree = u.tree(change);
        tree.lines(u.node).find(|&l| tree.line(l).code)
    };
    let (changed_before, changed_after) = if pair.similarity < 1.0 {
        changed_lines(from.tree(change), from.node, to.tree(change), to.node)
    } else {
        (Vec::new(), Vec::new())
    };
    Entry {
        name: from
            .tree(change)
            .node(from.node)
            .name
            .as_deref()
            .map(str::to_string),
        from_hunk: id(from),
        to_hunk: id(to),
        from_line: first(from),
        to_line: first(to),
        context: pair.context.clone(),
        similarity: pair.similarity,
        changed_before,
        changed_after,
    }
}

/// Lines of each unit, whitespace aside, that the other doesn't share: a
/// longest common subsequence over the two units' non-blank lines.
fn changed_lines(
    before: &Tree,
    from: NodeId,
    after: &Tree,
    to: NodeId,
) -> (Vec<usize>, Vec<usize>) {
    let texts = |tree: &Tree, node: NodeId| -> Vec<(usize, Vec<u8>)> {
        tree.lines(node)
            .map(|l| {
                let text: Vec<u8> = tree
                    .text(&tree.line_span(l, l))
                    .iter()
                    .copied()
                    .filter(|b| !b.is_ascii_whitespace())
                    .collect();
                (l, text)
            })
            .filter(|(_, t)| !t.is_empty())
            .collect()
    };
    let (a, b) = (texts(before, from), texts(after, to));
    let (ta, tb): (Vec<&[u8]>, Vec<&[u8]>) = (
        a.iter().map(|(_, t)| t.as_slice()).collect(),
        b.iter().map(|(_, t)| t.as_slice()).collect(),
    );
    let common = lcs(ta.len(), tb.len(), |i, j| ta[i] == tb[j], |_| 1);
    let only = |side: &[(usize, Vec<u8>)], pick: fn(&(usize, usize)) -> usize| -> Vec<usize> {
        side.iter()
            .enumerate()
            .filter(|(i, _)| !common.iter().any(|c| pick(c) == *i))
            .map(|(_, (line, _))| *line)
            .collect()
    };
    (only(&a, |c| c.0), only(&b, |c| c.1))
}
