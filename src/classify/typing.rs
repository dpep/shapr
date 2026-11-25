//! `typing`: code that changed only in its type annotations. The frontend
//! marks statements that only declare types ([`Flags::TYPING`]) and calls
//! that return their argument ([`Flags::TYPE_CAST`]); two subtrees are the
//! same code but for types when their hashes agree with the first left out
//! and the second replaced by its argument. Not structure while a runtime
//! may check the types (a value they reject raises), so the part assumes
//! [`Assumption::RuntimeCheck`] unless every annotation it holds is one no
//! runtime checks ([`Flags::UNCHECKED`]) and the frontend knows the type
//! library loaded.

use super::align::{Alignments, lcs};
use super::site::{CodeLines, in_any};
use super::{Assumption, Claim, Confidence, Evidence, Kind, Labels, Method, Outside, Part};
use crate::change::{Change, Trees};
use crate::frontend;
use crate::tree::{Flags, MARK, NodeId, Span, Tree};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub(super) fn run(change: &Change, _: &Alignments, outside: &dyn Outside, labels: &mut Labels) {
    for (file, labels) in change.files.iter().zip(labels.iter_mut()) {
        let Ok(trees) = &file.trees else { continue };
        if !trees.clean() || !(annotated(&trees.before) || annotated(&trees.after)) {
            continue;
        }
        let path = file.diff.path();
        let loaded = frontend::for_path(path)
            .is_some_and(|f| f.types_loaded(path, &|p: &str| outside.read(p)));
        let stripped = (Stripped::of(&trees.before), Stripped::of(&trees.after));
        let code = (CodeLines::of(&trees.before), CodeLines::of(&trees.after));
        for (site, label) in trees.sites.iter().zip(labels.iter_mut()) {
            let free = label.unclaimed();
            if free.is_empty() {
                continue;
            }
            let mut found = Found::default();
            let covers = |tree: &Tree, cover: &[NodeId]| -> Vec<NodeId> {
                cover
                    .iter()
                    .copied()
                    .filter(|&n| !tree.node(n).comment)
                    .collect()
            };
            found.lists(
                trees,
                &stripped,
                covers(&trees.before, &site.before.cover),
                covers(&trees.after, &site.after.cover),
            );
            let logic = Claim {
                before: code
                    .0
                    .touched(&free.before, |at| in_any(&found.differs.0, at)),
                after: code
                    .1
                    .touched(&free.after, |at| in_any(&found.differs.1, at)),
            };
            let held = |notes: &[Note], at| notes.iter().any(|n| in_any(&n.spans, at));
            let typed = |notes: &[Note], bare: &[Span], at| held(notes, at) || in_any(bare, at);
            let lines = Claim {
                before: code
                    .0
                    .touched(&free.before, |at| typed(&found.notes.0, &found.bare.0, at)),
                after: code
                    .1
                    .touched(&free.after, |at| typed(&found.notes.1, &found.bare.1, at)),
            }
            .minus(&logic);
            if lines.is_empty() {
                continue;
            }
            // The annotations on the part's lines, each side.
            let on = |notes: &[Note], code: &CodeLines, lines: &BTreeSet<usize>| -> Vec<bool> {
                (notes.iter())
                    .filter(|n| !code.touched(lines, |at| in_any(&n.spans, at)).is_empty())
                    .map(|n| n.unchecked)
                    .collect()
            };
            let notes: Vec<bool> = [
                on(&found.notes.0, &code.0, &lines.before),
                on(&found.notes.1, &code.1, &lines.after),
            ]
            .concat();
            let unchecked = notes.iter().filter(|&&u| u).count();
            let structure = loaded && unchecked == notes.len();
            label.claim(Part {
                kind: Kind::Typing,
                context: Vec::new(),
                assumptions: match structure {
                    true => Vec::new(),
                    false => vec![Assumption::RuntimeCheck],
                },
                review: lines.clone(),
                lines,
                // A tree comparison, as for a reformat.
                confidence: Confidence::new(1.0),
                reason: match (structure, loaded) {
                    (true, _) => "types only; nothing checks them at runtime",
                    (false, true) => "types only; sorbet-runtime checks them at runtime",
                    // Unchecked or not, the annotation needs a `T`.
                    (false, false) => {
                        "types only; without sorbet-runtime loaded, T may be undefined or not Sorbet's"
                    }
                }
                .into(),
                evidence: Evidence {
                    method: Method::Ast,
                    values: Map::from_iter([
                        ("annotations".to_string(), Value::from(notes.len())),
                        ("unchecked".to_string(), Value::from(unchecked)),
                    ]),
                },
                detail: Map::new(),
                variants: Vec::new(),
            });
        }
    }
}

fn annotated(tree: &Tree) -> bool {
    tree.nodes.iter().any(|n| marked(n.flags))
}

fn marked(flags: Flags) -> bool {
    flags.contains(Flags::TYPING) || flags.contains(Flags::TYPE_CAST)
}

/// The value a [`Flags::TYPE_CAST`] returns: the first code child of its
/// argument list.
fn value(tree: &Tree, cast: NodeId) -> Option<NodeId> {
    let arguments = tree
        .code_children(cast)
        .find(|&c| tree.node(c).flags.contains(Flags::ARGUMENTS))?;
    tree.code_children(arguments).next()
}

/// Per node, its hash with type annotations stripped: [`Flags::TYPING`]
/// children left out, each [`Flags::TYPE_CAST`] its value. Equal to
/// [`Node::hash`](crate::tree::Node::hash) where nothing is stripped.
struct Stripped(Vec<u64>);

impl Stripped {
    fn of(tree: &Tree) -> Stripped {
        let n = tree.nodes.len();
        let mut marked_below = vec![false; n];
        let mut hashes = vec![0; n];
        // A child is pushed after its parent, comments last: in reverse, every
        // child is done before its parent.
        for i in (0..n).rev() {
            let id = NodeId(i as u32);
            let node = tree.node(id);
            marked_below[i] =
                marked(node.flags) || node.children.iter().any(|&c| marked_below[c.0 as usize]);
            hashes[i] = if !marked_below[i] {
                node.hash
            } else if node.flags.contains(Flags::TYPE_CAST)
                && let Some(v) = value(tree, id)
            {
                hashes[v.0 as usize]
            } else {
                tree.hash_with(id, |c| {
                    let child = tree.node(c);
                    (!child.flags.contains(Flags::TYPING)).then(|| hashes[c.0 as usize])
                })
            };
        }
        Stripped(hashes)
    }

    fn get(&self, id: NodeId) -> u64 {
        self.0[id.0 as usize]
    }
}

/// A type annotation's code: a [`Flags::TYPING`] statement's whole extent, a
/// cast's extent around its value.
struct Note {
    spans: Vec<Span>,
    unchecked: bool,
}

/// What a hunk's covers hold, each side: annotations in code that is the
/// same once they are stripped, and the extents of code that differs.
#[derive(Default)]
struct Found {
    notes: (Vec<Note>, Vec<Note>),
    differs: (Vec<Span>, Vec<Span>),
    /// Code annotated on the other side only, the value a cast wraps.
    bare: (Vec<Span>, Vec<Span>),
}

impl Found {
    /// Two runs of siblings: statements that only declare types left out,
    /// the rest matched by longest common subsequence of stripped hashes;
    /// what is left over pairs up in order when the counts agree, and
    /// differs otherwise. A type declaration annotating the statement after
    /// it (a `sig` above its method) differs with that statement when it
    /// differs as a whole: the types of new code are new code.
    fn lists(
        &mut self,
        trees: &Trees,
        stripped: &(Stripped, Stripped),
        before: Vec<NodeId>,
        after: Vec<NodeId>,
    ) {
        let (tb, ta) = (&trees.before, &trees.after);
        let rest = |tree: &Tree, ids: &[NodeId]| -> Vec<NodeId> {
            let typing = |n: &&NodeId| tree.node(**n).flags.contains(Flags::TYPING);
            ids.iter().filter(|n| !typing(n)).copied().collect()
        };
        let (b, a) = (rest(tb, &before), rest(ta, &after));
        let matched = lcs(
            b.len(),
            a.len(),
            |i, j| stripped.0.get(b[i]) == stripped.1.get(a[j]),
            |_| 1,
        );
        for &(i, j) in &matched {
            self.same(trees, b[i], a[j]);
        }
        let left = |ids: &[NodeId], pick: fn(&(usize, usize)) -> usize| -> Vec<NodeId> {
            let taken: Vec<usize> = matched.iter().map(pick).collect();
            let ids = ids.iter().enumerate().filter(|(i, _)| !taken.contains(i));
            ids.map(|(_, &id)| id).collect()
        };
        let (lb, la) = (left(&b, |p| p.0), left(&a, |p| p.1));
        let mut whole: (Vec<NodeId>, Vec<NodeId>) = (Vec::new(), Vec::new());
        if lb.len() == la.len() {
            for (b, a) in lb.into_iter().zip(la) {
                if !self.pair(trees, stripped, b, a) {
                    whole.0.push(b);
                    whole.1.push(a);
                }
            }
        } else {
            whole = (lb, la);
        }
        self.differs
            .0
            .extend(whole.0.iter().map(|&n| tb.node(n).extent.clone()));
        self.differs
            .1
            .extend(whole.1.iter().map(|&n| ta.node(n).extent.clone()));
        let declared = |tree: &Tree,
                        ids: &[NodeId],
                        whole: &[NodeId],
                        notes: &mut Vec<Note>,
                        differs: &mut Vec<Span>| {
            for (i, &n) in ids.iter().enumerate() {
                if !tree.node(n).flags.contains(Flags::TYPING) {
                    continue;
                }
                let next = ids[i + 1..]
                    .iter()
                    .find(|m| !tree.node(**m).flags.contains(Flags::TYPING));
                match next {
                    Some(m) if whole.contains(m) => differs.push(tree.node(n).extent.clone()),
                    _ => notes.push(note(tree, n)),
                }
            }
        };
        declared(
            tb,
            &before,
            &whole.0,
            &mut self.notes.0,
            &mut self.differs.0,
        );
        declared(ta, &after, &whole.1, &mut self.notes.1, &mut self.differs.1);
    }

    /// Two nodes in the same place: the same once stripped, or the same
    /// syntax whose children are compared in turn; false when they differ
    /// as a whole.
    fn pair(
        &mut self,
        trees: &Trees,
        stripped: &(Stripped, Stripped),
        b: NodeId,
        a: NodeId,
    ) -> bool {
        let (tb, ta) = (&trees.before, &trees.after);
        if stripped.0.get(b) == stripped.1.get(a) {
            self.same(trees, b, a);
            return true;
        }
        let unmarked = |t: Vec<u8>| -> Vec<u8> { t.into_iter().filter(|&c| c != MARK).collect() };
        let (nb, na) = (tb.node(b), ta.node(a));
        let alike = nb.kind == na.kind
            && nb.role == na.role
            && !nb.children.is_empty()
            && !na.children.is_empty()
            && unmarked(tb.own_text(b, false)) == unmarked(ta.own_text(a, false));
        if alike {
            let b = tb.code_children(b).collect();
            let a = ta.code_children(a).collect();
            self.lists(trees, stripped, b, a);
        }
        alike
    }

    /// Two subtrees equal once stripped: their annotations, where they differ.
    fn same(&mut self, trees: &Trees, b: NodeId, a: NodeId) {
        if trees.before.node(b).hash == trees.after.node(a).hash {
            return;
        }
        let (nb, na) = (self.notes.0.len(), self.notes.1.len());
        notes(&trees.before, b, &mut self.notes.0);
        notes(&trees.after, a, &mut self.notes.1);
        match (self.notes.0.len() > nb, self.notes.1.len() > na) {
            (false, true) => self.bare.0.push(trees.before.node(b).extent.clone()),
            (true, false) => self.bare.1.push(trees.after.node(a).extent.clone()),
            _ => {}
        }
    }
}

/// Every annotation in `id`'s subtree.
fn notes(tree: &Tree, id: NodeId, out: &mut Vec<Note>) {
    let mut stack = vec![id];
    while let Some(n) = stack.pop() {
        let flags = tree.node(n).flags;
        if marked(flags) {
            out.push(note(tree, n));
        }
        if !flags.contains(Flags::TYPING) {
            stack.extend(tree.code_children(n));
        }
    }
}

fn note(tree: &Tree, id: NodeId) -> Note {
    let node = tree.node(id);
    let extent = node.extent.clone();
    let spans = match value(tree, id).filter(|_| node.flags.contains(Flags::TYPE_CAST)) {
        Some(v) => {
            let inner = &tree.node(v).extent;
            vec![extent.start..inner.start, inner.end..extent.end]
        }
        None => vec![extent],
    };
    Note {
        spans,
        unchecked: node.flags.contains(Flags::UNCHECKED),
    }
}
