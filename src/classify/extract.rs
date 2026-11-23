//! `extract` and `inline`: statements leave a method and become the body of a
//! new method, with a call to it in their old spot; or the inverse, a method
//! and its only call replaced by its body.
//!
//! The two are one search with the sides swapped. The *call side* holds the
//! new method B and the call to it; the *chunk side* holds the statements.
//! For an extract the call side is after the change, for an inline before.
//! A candidate is proven, not scored into existence: the caller A must be
//! identical on both sides once the call is swapped for the chunk, B's body
//! must equal the chunk once its parameters are renamed to the call's
//! arguments, and everything else beside A and B must be unchanged.

use super::{Confidence, Kind, Label, Labels};
use crate::change::{Change, FileChange, Trees};
use crate::tree::{NameKind, NodeId, Receiver, Role, Span, Tree};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

/// Chunks with fewer syntax nodes than this stay `logic`. Equality of a body
/// this small says little, and a one-call method is more often a hook for a
/// subclass to override, a behavior seam, than a tidying.
const FLOOR: usize = 8;

pub(super) fn run(change: &Change, labels: &mut Labels) {
    let mut first_id = 0;
    for (f, file) in change.files.iter().enumerate() {
        let base = first_id;
        first_id += file.diff.hunks.len();
        let Ok(trees) = &file.trees else { continue };
        if !trees.clean() {
            continue;
        }
        for dir in [Dir::Extract, Dir::Inline] {
            let (calls, chunks) = dir.sides(trees);
            let methods: Vec<NodeId> = ids(calls)
                .filter(|&id| calls.node(id).role == Role::Definition(NameKind::Method))
                .collect();
            for b in methods {
                // B is new for an extract and gone after an inline.
                if unique_def(chunks, &key(calls, b)).is_some() {
                    continue;
                }
                match find(change, calls, chunks, b, dir) {
                    Ok(found) => apply(file, trees, base, &found, &mut labels[f]),
                    Err(why) => crate::verbose!(
                        "{}: `{}` not a {}: {why}",
                        file.diff.path(),
                        calls.node(b).name.as_deref().unwrap_or_default(),
                        dir.kind().as_str()
                    ),
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Extract,
    Inline,
}

impl Dir {
    /// (call side, chunk side).
    fn sides(self, trees: &Trees) -> (&Tree, &Tree) {
        match self {
            Dir::Extract => (&trees.after, &trees.before),
            Dir::Inline => (&trees.before, &trees.after),
        }
    }

    fn kind(self) -> Kind {
        match self {
            Dir::Extract => Kind::Extract,
            Dir::Inline => Kind::Inline,
        }
    }
}

/// Why a structural match would still change behavior.
#[derive(Clone, Copy)]
enum Risk {
    /// The chunk holds control flow bound to its method: `return`, `yield`…
    Jump,
    /// The chunk reads the caller's block parameter.
    BlockParameter,
    /// The call's arguments don't carry exactly the chunk's free variables
    /// into B's parameters, or values don't flow back the way they left.
    Wiring,
    /// B's name is already used, so defining or removing it reaches code
    /// beyond this change.
    Collision,
    /// The chunk is below [`FLOOR`].
    Tiny,
}

impl Risk {
    fn as_str(self) -> &'static str {
        match self {
            Risk::Jump => "jump",
            Risk::BlockParameter => "block_parameter",
            Risk::Wiring => "wiring",
            Risk::Collision => "collision",
            Risk::Tiny => "tiny",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Risk::Jump => "the statements hold control flow bound to their method",
            Risk::BlockParameter => "the statements use the caller's block parameter",
            Risk::Wiring => "the arguments don't match the statements' free variables",
            Risk::Collision => "the method's name is already in use",
            Risk::Tiny => "too few statements to be evidence",
        }
    }
}

struct Found {
    dir: Dir,
    method: String,
    caller: String,
    /// The statement container holding A and B on each side: every changed
    /// byte inside it is explained by the refactoring.
    scope_before: Span,
    scope_after: Span,
    outcome: Result<Stats, Risk>,
}

struct Stats {
    statements: usize,
    nodes: usize,
    renamed: usize,
    params: usize,
    returns: bool,
}

impl Stats {
    /// Share of the chunk's nodes matched without renaming a parameter.
    fn verbatim(&self) -> f64 {
        Confidence::new(1.0 - self.renamed as f64 / self.nodes as f64).get()
    }

    /// Size saturates at twice the floor; a renamed node is an inferred
    /// correspondence, so it counts half.
    fn confidence(&self) -> Confidence {
        let size = (self.nodes as f64 / (2 * FLOOR) as f64).min(1.0);
        Confidence::new(size * (1.0 + self.verbatim()) / 2.0)
    }
}

/// A candidate, or the first test it fails (for `-v`).
fn find(
    change: &Change,
    calls: &Tree,
    chunks: &Tree,
    b: NodeId,
    dir: Dir,
) -> Result<Found, &'static str> {
    let method = calls.node(b).name.as_deref().ok_or("no name")?.to_string();
    let sites: Vec<NodeId> = ids(calls)
        .filter(|&id| {
            let node = calls.node(id);
            matches!(
                node.role,
                Role::Call(Receiver::Implicit | Receiver::SelfRef)
            ) && node.name.as_deref() == Some(method.as_str())
        })
        .collect();
    let [call] = sites[..] else {
        return Err("not called exactly once");
    };
    let (stmt, assigned) = statement(calls, call);
    let a_call = enclosing_def(calls, call).ok_or("called outside a method")?;
    let scope = calls.node(b).parent.ok_or("no parent")?;
    if calls.node(a_call).parent != Some(scope) || receiver(calls, a_call) != receiver(calls, b) {
        return Err("caller is not a sibling with the same receiver");
    }
    // `module_function` copies methods to the module; B may not get the copy.
    if code_children(calls, scope)
        .iter()
        .any(|&c| calls.node(c).role == Role::Visibility(crate::tree::Visibility::ModuleFunction))
    {
        return Err("module_function scope");
    }
    let a_chunk = unique_def(chunks, &key(calls, a_call)).ok_or("caller not on the other side")?;
    let chunk_scope = chunks.node(a_chunk).parent.ok_or("no parent")?;
    if !rest_unchanged(calls, scope, b, a_call, chunks, chunk_scope, a_chunk) {
        return Err("other code beside the methods changed");
    }
    let (params, body) = method_body(calls, b).ok_or("unsupported parameters or body")?;
    let chunk = splice(calls, a_call, stmt, calls.node(body).kind, chunks, a_chunk)
        .ok_or("caller differs beyond the call")?;
    let b_stmts = code_children(calls, body);
    let args = arguments(calls, call, &method);
    let flow = Flow::of(chunks, a_chunk, &chunk);

    let wired = args
        .as_ref()
        .filter(|args| args.len() == params.len())
        .and_then(|args| Names::wired(&params, args));
    let exact = wired.and_then(|names| {
        Matcher::new(calls, chunks, names).body(&b_stmts, &chunk, assigned.as_deref())
    });
    let matched = exact
        .or_else(|| {
            Matcher::new(calls, chunks, Names::default()).body(
                &b_stmts,
                &chunk,
                assigned.as_deref(),
            )
        })
        .ok_or("body differs from the statements")?;

    let nodes = chunk
        .iter()
        .flat_map(|&s| chunks.descendants(s))
        .filter(|&n| !chunks.node(n).comment)
        .count();
    let arg_set: HashSet<&str> = args.iter().flatten().map(String::as_str).collect();
    let free: HashSet<&str> = flow.free.iter().map(String::as_str).collect();
    let outflow: HashSet<&str> = flow.outflow.iter().map(String::as_str).collect();
    let assigned_set: HashSet<&str> = assigned.iter().map(String::as_str).collect();
    let block_params: HashSet<&str> = chunks
        .descendants(a_chunk)
        .map(|n| chunks.node(n))
        .filter(|n| n.role == Role::Definition(NameKind::BlockParameter))
        .filter_map(|n| n.name.as_deref())
        .collect();
    let in_chunk = || chunk.iter().flat_map(|&s| chunks.descendants(s));

    let outcome = if in_chunk().any(|n| matches!(chunks.node(n).role, Role::Jump(_))) {
        Err(Risk::Jump)
    } else if in_chunk().any(|n| {
        let node = chunks.node(n);
        node.role == Role::Reference(NameKind::Variable)
            && node
                .name
                .as_deref()
                .is_some_and(|n| block_params.contains(n))
    }) {
        Err(Risk::BlockParameter)
    } else if exact.is_none() || free != arg_set || flow.clobbers || outflow != assigned_set {
        Err(Risk::Wiring)
    } else if mentioned(change, &method, calls, &[b, call]) {
        Err(Risk::Collision)
    } else if nodes < FLOOR {
        Err(Risk::Tiny)
    } else {
        Ok(Stats {
            statements: chunk.len(),
            nodes,
            renamed: matched,
            params: params.len(),
            returns: assigned.is_some(),
        })
    };
    let (scope_before, scope_after) = match dir {
        Dir::Extract => (
            chunks.node(chunk_scope).extent.clone(),
            calls.node(scope).extent.clone(),
        ),
        Dir::Inline => (
            calls.node(scope).extent.clone(),
            chunks.node(chunk_scope).extent.clone(),
        ),
    };
    Ok(Found {
        dir,
        method,
        caller: calls
            .node(a_call)
            .name
            .as_deref()
            .unwrap_or_default()
            .to_string(),
        scope_before,
        scope_after,
        outcome,
    })
}

/// Labels the hunks the refactoring touches, all or none.
fn apply(file: &FileChange, trees: &Trees, base: usize, found: &Found, labels: &mut [Label]) {
    // (hunk, whether all its changed code lies in the refactoring's scope)
    let mut touched = Vec::new();
    for (h, hunk) in file.diff.hunks.iter().enumerate() {
        let before = code_bytes(&trees.before, &hunk.removed);
        let after = code_bytes(&trees.after, &hunk.added);
        let inside = |at: &usize, scope: &Span| scope.contains(at);
        let touches = before.iter().any(|at| inside(at, &found.scope_before))
            || after.iter().any(|at| inside(at, &found.scope_after));
        if touches {
            let explained = before.iter().all(|at| inside(at, &found.scope_before))
                && after.iter().all(|at| inside(at, &found.scope_after));
            touched.push((h, explained));
        }
    }
    // A hunk an earlier pass proved structural (a reformatted sibling) keeps
    // its label and doesn't block the refactoring.
    touched.retain(|&(h, _)| {
        let kind = labels[h].kind;
        kind == Kind::Logic || !kind.is_structure()
    });
    let ids: Vec<usize> = touched.iter().map(|(h, _)| base + h).collect();
    let mut detail = Map::new();
    detail.insert("method".into(), Value::from(found.method.as_str()));
    detail.insert("caller".into(), Value::from(found.caller.as_str()));
    detail.insert("hunks".into(), json!(ids));
    match &found.outcome {
        Ok(stats) => {
            let all = touched
                .iter()
                .all(|&(h, explained)| explained && labels[h].kind == Kind::Logic);
            if touched.is_empty() || !all {
                return;
            }
            let reason = match found.dir {
                Dir::Extract => format!(
                    "statements of `{}` moved into new method `{}`, replaced by a call",
                    found.caller, found.method
                ),
                Dir::Inline => format!(
                    "method `{}` removed; its body replaces its call in `{}`",
                    found.method, found.caller
                ),
            };
            for &(h, _) in &touched {
                let label = &mut labels[h];
                label.kind = found.dir.kind();
                label.confidence = stats.confidence();
                label.reason = reason.clone();
                let values = &mut label.evidence.values;
                values.insert("statements".into(), Value::from(stats.statements));
                values.insert("nodes".into(), Value::from(stats.nodes));
                values.insert("params".into(), Value::from(stats.params));
                values.insert("verbatim".into(), Value::from(stats.verbatim()));
                values.insert("returns".into(), Value::from(stats.returns));
                label.detail = detail.clone();
            }
        }
        Err(risk) => {
            let near = found.dir.kind().as_str();
            detail.insert("near".into(), Value::from(near));
            detail.insert("risk".into(), Value::from(risk.as_str()));
            for &(h, _) in &touched {
                let label = &mut labels[h];
                if label.kind != Kind::Logic || !label.detail.is_empty() {
                    continue;
                }
                label.reason = format!("not an {near} of `{}`: {}", found.method, risk.reason());
                label.detail = detail.clone();
            }
        }
    }
}

/// Offsets of the bytes on `lines` that carry code: not whitespace, not in a
/// comment.
fn code_bytes(tree: &Tree, lines: &[usize]) -> Vec<usize> {
    let comments: Vec<&Span> = tree
        .nodes
        .iter()
        .filter(|n| n.comment)
        .map(|n| &n.extent)
        .collect();
    lines
        .iter()
        .flat_map(|&line| tree.line_span(line, line))
        .filter(|&at| !tree.source[at].is_ascii_whitespace())
        .filter(|at| !comments.iter().any(|c| c.contains(at)))
        .collect()
}

fn ids(tree: &Tree) -> impl Iterator<Item = NodeId> + '_ {
    (0..tree.nodes.len() as u32).map(NodeId)
}

fn code_children(tree: &Tree, id: NodeId) -> Vec<NodeId> {
    tree.node(id)
        .children
        .iter()
        .copied()
        .filter(|&c| !tree.node(c).comment)
        .collect()
}

fn enclosing_def(tree: &Tree, id: NodeId) -> Option<NodeId> {
    let mut at = tree.node(id).parent;
    while let Some(p) = at {
        if tree.node(p).role == Role::Definition(NameKind::Method) {
            return Some(p);
        }
        at = tree.node(p).parent;
    }
    None
}

/// The receiver a method is defined on (`def self.x`), by its hash.
fn receiver(tree: &Tree, def: NodeId) -> Option<u64> {
    let first = *code_children(tree, def).first()?;
    let end = tree.node(first).extent.end;
    // Only a receiver is followed by the `.` before the method's name.
    let next = tree.source[end..]
        .iter()
        .find(|b| !b.is_ascii_whitespace())?;
    (*next == b'.').then(|| tree.node(first).hash)
}

/// Where a method sits: its ancestors' kinds and names, its own, and its
/// receiver.
type Key<'t> = (Vec<(&'static str, Option<&'t str>)>, Option<u64>);

fn key(tree: &Tree, def: NodeId) -> Key<'_> {
    let mut path = Vec::new();
    let mut at = Some(def);
    while let Some(id) = at {
        let node = tree.node(id);
        path.push((node.kind, node.name.as_deref()));
        at = node.parent;
    }
    (path, receiver(tree, def))
}

fn unique_def(tree: &Tree, want: &Key<'_>) -> Option<NodeId> {
    let mut found = ids(tree)
        .filter(|&id| tree.node(id).role == Role::Definition(NameKind::Method))
        .filter(|&id| key(tree, id) == *want);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// Every sibling of A and B is unchanged, and so is their container's own
/// text: the refactoring is the only change in that scope.
fn rest_unchanged(
    calls: &Tree,
    scope: NodeId,
    b: NodeId,
    a_call: NodeId,
    chunks: &Tree,
    chunk_scope: NodeId,
    a_chunk: NodeId,
) -> bool {
    let left: Vec<NodeId> = code_children(calls, scope)
        .into_iter()
        .filter(|&c| c != b)
        .collect();
    let right = code_children(chunks, chunk_scope);
    calls.node(scope).kind == chunks.node(chunk_scope).kind
        && unmarked(own(calls, scope, None)) == unmarked(own(chunks, chunk_scope, None))
        && left.len() == right.len()
        && left.iter().zip(&right).all(|(&l, &r)| {
            (l == a_call && r == a_chunk)
                || (l != a_call && r != a_chunk && calls.node(l).hash == chunks.node(r).hash)
        })
}

/// The statement holding the call: the call itself, or a plain assignment of
/// its result to a local, whose name is returned.
fn statement(tree: &Tree, call: NodeId) -> (NodeId, Option<String>) {
    if let Some(parent) = tree.node(call).parent {
        let node = tree.node(parent);
        if let Some(name) = local_name(node)
            && node.role == Role::Definition(NameKind::Variable)
            && code_children(tree, parent) == [call]
            && unmarked(own(tree, parent, Some(name))) == b"="
        {
            return (parent, Some(name.to_string()));
        }
    }
    (call, None)
}

/// B's parameter names and body, when every parameter is a plain required
/// positional one.
fn method_body(tree: &Tree, def: NodeId) -> Option<(Vec<String>, NodeId)> {
    let mut children = code_children(tree, def);
    if receiver(tree, def).is_some() {
        children.remove(0);
    }
    let is_params = |id: NodeId| {
        let kids = code_children(tree, id);
        !kids.is_empty()
            && kids
                .iter()
                .all(|&k| tree.node(k).role == Role::Definition(NameKind::Parameter))
    };
    let (params, body) = match children[..] {
        [body] if !is_params(body) => (Vec::new(), body),
        [params, body] if is_params(params) => {
            let mut names = Vec::new();
            for p in code_children(tree, params) {
                let node = tree.node(p);
                let name = node.name.as_deref()?;
                // Defaults, splats and keywords all add text beside the name.
                if !node.children.is_empty() || unmarked(own(tree, p, None)) != name.as_bytes() {
                    return None;
                }
                names.push(name.to_string());
            }
            (names, body)
        }
        _ => return None,
    };
    Some((params, body))
}

/// The call's arguments, when each is a plain local variable and nothing
/// else (a block, a splat, a keyword) rides along.
fn arguments(tree: &Tree, call: NodeId, method: &str) -> Option<Vec<String>> {
    let mut children = code_children(tree, call);
    if tree.node(call).role == Role::Call(Receiver::SelfRef) {
        children.remove(0);
    }
    let text = unmarked(own(tree, call, None));
    let forms = [
        method.to_string(),
        format!("{method}()"),
        format!(".{method}"),
        format!(".{method}()"),
    ];
    if !forms.iter().any(|f| f.as_bytes() == text) {
        return None;
    }
    match children[..] {
        [] => Some(Vec::new()),
        [args] => {
            if unmarked(own(tree, args, None)).iter().any(|&b| b != b',') {
                return None;
            }
            code_children(tree, args)
                .into_iter()
                .map(|a| {
                    let node = tree.node(a);
                    (node.role == Role::Reference(NameKind::Variable) && node.children.is_empty())
                        .then(|| local_name(node))
                        .flatten()
                        .map(str::to_string)
                })
                .collect()
        }
        _ => None,
    }
}

/// Walks A on both sides down to the call statement, requiring every node off
/// the path to be equal; at the statement's container, the call side's
/// statement stands where the chunk side has one or more. Returns them.
fn splice(
    calls: &Tree,
    a_call: NodeId,
    stmt: NodeId,
    body_kind: &str,
    chunks: &Tree,
    a_chunk: NodeId,
) -> Option<Vec<NodeId>> {
    let mut path = vec![stmt];
    while let Some(&top) = path.last()
        && top != a_call
    {
        path.push(calls.node(top).parent?);
    }
    path.reverse();
    let (mut x, mut y) = (a_call, a_chunk);
    for &child in &path[1..] {
        let (cx, cy) = (code_children(calls, x), code_children(chunks, y));
        if calls.node(x).kind != chunks.node(y).kind
            || unmarked(own(calls, x, None)) != unmarked(own(chunks, y, None))
        {
            return None;
        }
        let i = cx.iter().position(|&c| c == child)?;
        let same = |l: &[NodeId], r: &[NodeId]| {
            l.len() == r.len()
                && l.iter()
                    .zip(r)
                    .all(|(&l, &r)| calls.node(l).hash == chunks.node(r).hash)
        };
        if child == stmt {
            // Statements only: the chunk becomes a method body.
            if calls.node(x).kind != body_kind || cy.len() < cx.len() {
                return None;
            }
            let tail = cx.len() - i - 1;
            let end = cy.len() - tail;
            return (same(&cx[..i], &cy[..i]) && same(&cx[i + 1..], &cy[end..]))
                .then(|| cy[i..end].to_vec());
        }
        if cx.len() != cy.len() || !same(&cx[..i], &cy[..i]) || !same(&cx[i + 1..], &cy[i + 1..]) {
            return None;
        }
        x = child;
        y = cy[i];
    }
    None
}

/// Local variables around the chunk, in the chunk side's A.
struct Flow {
    /// Read in the chunk, defined outside it: they must arrive as arguments.
    free: HashSet<String>,
    /// The chunk assigns a local A also defines outside it.
    clobbers: bool,
    /// Defined in the chunk and read outside it: they must come back.
    outflow: HashSet<String>,
}

impl Flow {
    fn of(tree: &Tree, a: NodeId, chunk: &[NodeId]) -> Flow {
        let inside: HashSet<NodeId> = chunk.iter().flat_map(|&s| tree.descendants(s)).collect();
        let (mut defs_in, mut defs_out, mut reads_in, mut reads_out) = (
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
            HashSet::new(),
        );
        for id in tree.descendants(a) {
            let node = tree.node(id);
            let Some(name) = local_name(node) else {
                continue;
            };
            let name = name.to_string();
            let is_in = inside.contains(&id);
            match (node.role, is_in) {
                (Role::Reference(_), true) => reads_in.insert(name),
                (Role::Reference(_), false) => reads_out.insert(name),
                (_, true) => defs_in.insert(name),
                (_, false) => defs_out.insert(name),
            };
        }
        Flow {
            free: reads_in.difference(&defs_in).cloned().collect(),
            clobbers: !defs_in.is_disjoint(&defs_out),
            outflow: defs_in.intersection(&reads_out).cloned().collect(),
        }
    }
}

/// The name of a local variable or parameter the node defines or reads.
/// Sigiled names (`@ivar`, `$global`) live beyond the method, so a new
/// method sees them unchanged.
fn local_name(node: &crate::tree::Node) -> Option<&str> {
    let local = matches!(
        node.role,
        Role::Reference(NameKind::Variable)
            | Role::Definition(NameKind::Variable | NameKind::Parameter | NameKind::BlockParameter)
    );
    let name = node.name.as_deref().filter(|_| local)?;
    (!name.starts_with(['@', '$'])).then_some(name)
}

/// A one-to-one renaming between B's locals and the chunk's.
#[derive(Clone, Default)]
struct Names {
    forward: HashMap<String, String>,
    back: HashMap<String, String>,
}

impl Names {
    /// Each parameter bound to its argument; `None` when one local is passed
    /// twice.
    fn wired(params: &[String], args: &[String]) -> Option<Names> {
        let mut names = Names::default();
        params
            .iter()
            .zip(args)
            .all(|(p, a)| names.pair(p, a))
            .then_some(names)
    }

    fn pair(&mut self, b: &str, chunk: &str) -> bool {
        match (self.forward.get(b), self.back.get(chunk)) {
            (Some(x), _) if x != chunk => false,
            (_, Some(y)) if y != b => false,
            _ => {
                self.forward.insert(b.to_string(), chunk.to_string());
                self.back.insert(chunk.to_string(), b.to_string());
                true
            }
        }
    }
}

/// Compares B's body (call side) with the chunk under a renaming, counting
/// the nodes it had to rename.
#[derive(Clone)]
struct Matcher<'t> {
    calls: &'t Tree,
    chunks: &'t Tree,
    names: Names,
    renamed: usize,
}

impl<'t> Matcher<'t> {
    fn new(calls: &'t Tree, chunks: &'t Tree, names: Names) -> Self {
        Matcher {
            calls,
            chunks,
            names,
            renamed: 0,
        }
    }

    /// B's statements against the chunk. With no assignment at the call, B
    /// is the chunk exactly. When the call's result is assigned to `v`, B
    /// may also end with the last assignment's value, or append a read of
    /// `v`. Returns the renamed node count.
    fn body(mut self, b: &[NodeId], chunk: &[NodeId], assigned: Option<&str>) -> Option<usize> {
        let n = chunk.len();
        let Some(v) = assigned else {
            return self.all(b, chunk).then_some(self.renamed);
        };
        if b.len() == n + 1 {
            let read = self.calls.node(b[n]);
            let ok = self.all(&b[..n], chunk)
                && read.role == Role::Reference(NameKind::Variable)
                && read.children.is_empty()
                && local_name(read).is_some_and(|name| self.names.pair(name, v));
            return ok.then_some(self.renamed);
        }
        let last = *chunk.last()?;
        let (prefix_b, last_b) = b.split_last().map(|(l, p)| (p, *l))?;
        let value = statement_value(self.chunks, last, v)?;
        if b.len() != n || !self.all(prefix_b, &chunk[..n - 1]) {
            return None;
        }
        let mut whole = self.clone();
        if whole.eq(last_b, last) {
            return Some(whole.renamed);
        }
        self.eq(last_b, value).then_some(self.renamed)
    }

    fn all(&mut self, b: &[NodeId], chunk: &[NodeId]) -> bool {
        b.len() == chunk.len() && b.iter().zip(chunk).all(|(&x, &y)| self.eq(x, y))
    }

    fn eq(&mut self, x: NodeId, y: NodeId) -> bool {
        let (nx, ny) = (self.calls.node(x), self.chunks.node(y));
        if nx.kind != ny.kind {
            return false;
        }
        let (lx, ly) = (local_name(nx), local_name(ny));
        match (lx, ly) {
            (Some(p), Some(q)) => {
                if !self.names.pair(p, q) {
                    return false;
                }
                self.renamed += usize::from(p != q);
            }
            (None, None) if nx.children.is_empty() && ny.children.is_empty() => {
                return nx.hash == ny.hash;
            }
            (None, None) => {}
            _ => return false,
        }
        own(self.calls, x, lx) == own(self.chunks, y, ly)
            && self.all(
                &code_children(self.calls, x),
                &code_children(self.chunks, y),
            )
    }
}

/// The value of `stmt` when it is a plain assignment to the local `v`.
fn statement_value(tree: &Tree, stmt: NodeId, v: &str) -> Option<NodeId> {
    let node = tree.node(stmt);
    let kids = code_children(tree, stmt);
    (node.role == Role::Definition(NameKind::Variable)
        && local_name(node) == Some(v)
        && kids.len() == 1
        && unmarked(own(tree, stmt, Some(v))) == b"=")
        .then(|| kids[0])
}

/// B's name used anywhere in the change but at B and its one call: a def,
/// a call or a dynamic reference that defining or removing B would reach.
fn mentioned(change: &Change, method: &str, calls: &Tree, ours: &[NodeId]) -> bool {
    change
        .files
        .iter()
        .filter_map(|f| f.trees.as_ref().ok())
        .flat_map(|t| [&t.before, &t.after])
        .any(|tree| {
            ids(tree).any(|id| {
                let node = tree.node(id);
                matches!(
                    node.role,
                    Role::Definition(NameKind::Method) | Role::Call(_) | Role::DynamicName
                ) && node.name.as_deref() == Some(method)
                    && !(std::ptr::eq(tree, calls) && ours.contains(&id))
            })
        })
}

/// What the tree's hash reads of a node's own text: its non-whitespace bytes
/// between children, a marker where each non-comment child sits. A local's
/// leading name is dropped, so renamed locals compare equal.
fn own(tree: &Tree, id: NodeId, name: Option<&str>) -> Vec<u8> {
    let node = tree.node(id);
    let span = &node.span;
    let mut out = Vec::new();
    let mut cursor = span.start;
    let push = |from: usize, to: usize, out: &mut Vec<u8>| {
        if from < to {
            out.extend(
                tree.source[from..to]
                    .iter()
                    .filter(|b| !b.is_ascii_whitespace()),
            );
        }
    };
    for &child in &node.children {
        let c = tree.node(child);
        push(cursor, c.extent.start.min(span.end), &mut out);
        cursor = cursor.max(c.extent.end);
        if !c.comment {
            out.push(MARK);
        }
    }
    push(cursor, span.end, &mut out);
    if let Some(name) = name
        && out.starts_with(name.as_bytes())
    {
        out.drain(..name.len());
    }
    out
}

/// Stands for a child in [`own`]; never a byte of Ruby source text.
const MARK: u8 = 0xff;

fn unmarked(text: Vec<u8>) -> Vec<u8> {
    text.into_iter().filter(|&b| b != MARK).collect()
}
