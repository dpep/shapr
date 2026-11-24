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
//! arguments, and a hunk may change nothing else.

use super::align::{Alignments, siblings};
use super::hazard::{Op, blocks};
use super::proof::{self, Assumption, Verified};
use super::site::CodeLines;
use super::{Claim, Confidence, Kind, Label, Labels, Outside, Part};
use crate::change::{Change, Trees};
use crate::tree::{Flags, MARK, NameKind, NodeId, Piece, Receiver, Role, Span, Tree};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Chunks with fewer syntax nodes than this stay `logic`. Equality of a body
/// this small says little, and a one-call method is more often a hook for a
/// subclass to override, a behavior seam, than a tidying.
const FLOOR: usize = 8;

pub(super) fn run(change: &Change, _: &Alignments, outside: &dyn Outside, labels: &mut Labels) {
    for (f, file) in change.files.iter().enumerate() {
        let Ok(trees) = &file.trees else { continue };
        if !trees.clean() {
            continue;
        }
        for dir in [Dir::Extract, Dir::Inline] {
            let (calls, chunks) = dir.sides(trees);
            let methods: Vec<NodeId> = ids(calls)
                .filter(|&id| calls.node(id).role == Role::Definition(NameKind::Method))
                .collect();
            let defs = def_counts(chunks);
            let sites = call_sites(calls);
            for b in methods {
                // B is new for an extract and gone after an inline.
                if defs.get(&key(calls, b)) == Some(&1) {
                    continue;
                }
                match find(change, calls, chunks, &sites, b, dir) {
                    Ok(mut found) => {
                        reach(change, outside, f, calls, b, &mut found);
                        apply(trees, &file.ids, &found, &mut labels[f]);
                    }
                    Err(why) => crate::verbose!(
                        "{}: `{}` not an {}: {why}",
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
    /// Code follows the chunk in its block, and a value the chunk binds or
    /// is passed would be destroyed at another point: the language runs
    /// destructors when a block ends.
    Drop,
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
            Risk::Drop => "drop",
            Risk::Collision => "collision",
            Risk::Tiny => "tiny",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Risk::Jump => "the statements hold control flow bound to their method",
            Risk::BlockParameter => "the statements use the caller's block parameter",
            Risk::Wiring => "the arguments don't match the statements' free variables",
            Risk::Drop => "values the statements hold would be dropped at another point",
            Risk::Collision => "the method's name is already in use",
            Risk::Tiny => "too few statements to be evidence",
        }
    }
}

struct Found {
    dir: Dir,
    method: String,
    caller: String,
    /// Where the refactoring's hunks may change code, before and after.
    regions: [Region; 2],
    outcome: Result<Stats, Risk>,
    /// Which value doesn't flow, for a [`Risk::Wiring`] refusal's reason.
    wiring: Option<String>,
    /// Files outside the change that already use the method's name.
    used_in: Vec<String>,
}

/// One side's view of the refactoring.
struct Region {
    /// The container of A and B.
    scope: Span,
    /// Siblings that changed for reasons of their own.
    holes: Vec<Span>,
    /// A and B: a hunk changing code here belongs to the refactoring.
    touch: Vec<Span>,
}

impl Region {
    fn touches(&self, at: &usize) -> bool {
        self.touch.iter().any(|t| t.contains(at))
    }

    /// Inside the container but not in a sibling that changed on its own:
    /// any change here is the refactoring, or diff noise in equal code.
    fn explains(&self, at: &usize) -> bool {
        self.scope.contains(at) && !self.holes.iter().any(|h| h.contains(at))
    }
}

struct Stats {
    statements: usize,
    nodes: usize,
    renamed: usize,
    params: usize,
    returns: bool,
    /// What the refactoring relies on that the search couldn't settle.
    assumptions: Vec<Assumption>,
    /// Files searched for the method's name outside the change.
    searched: Option<usize>,
}

impl Stats {
    /// Share of the chunk's nodes matched without renaming a parameter.
    fn verbatim(&self) -> f64 {
        super::rounded(1.0 - self.renamed as f64 / self.nodes as f64)
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
    sites: &HashMap<&str, Vec<NodeId>>,
    b: NodeId,
    dir: Dir,
) -> Result<Found, &'static str> {
    let method = calls.node(b).name.as_deref().ok_or("no name")?.to_string();
    let Some(&[call]) = sites.get(method.as_str()).map(Vec::as_slice) else {
        return Err("not called exactly once");
    };
    let (stmt, assigned) = statement(calls, call);
    let a_call = calls
        .enclosing(call, |n| n.role == Role::Definition(NameKind::Method))
        .ok_or("called outside a method")?;
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
    let mut changed = siblings(
        (calls, scope, &[a_call, b]),
        (chunks, chunk_scope, &[a_chunk]),
    )
    .ok_or("the methods' container changed")?;
    // A bare `private` added (or removed) with B, governing only B, is B's.
    changed.0.retain(|&v| !governs_only(calls, scope, v, b));
    let (params, body) = method_body(calls, b).ok_or("unsupported parameters or body")?;
    let chunk = splice(calls, a_call, stmt, calls.node(body).kind, chunks, a_chunk)
        .ok_or("caller differs beyond the call")?;
    let b_stmts = code_children(calls, body);
    let args = arguments(calls, call);
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
    let assigned_set: HashSet<&str> = assigned.iter().map(String::as_str).collect();
    let block_params: HashSet<&str> = chunks
        .descendants(a_chunk)
        .map(|n| chunks.node(n))
        .filter(|n| n.role == Role::Definition(NameKind::BlockParameter))
        .filter_map(|n| n.name.as_deref())
        .collect();
    let in_chunk = || chunk.iter().flat_map(|&s| chunks.descendants(s));

    let blocks = |op: Op, n: NodeId| chunks.node(n).role.hazard().is_some_and(|h| blocks(op, h));
    let mut wiring = None;
    let outcome = if in_chunk().any(|n| blocks(Op::Extract, n))
        || chunks
            .descendants(a_chunk)
            .any(|n| blocks(Op::ExtractCaller, n))
    {
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
    } else if let Some(why) = miswired(exact.is_some() && free == arg_set, &flow, &assigned_set) {
        wiring = Some(why);
        Err(Risk::Wiring)
    } else if chunks.semantics.scoped_drop && outlives(chunks, &chunk, assigned.as_deref(), &params)
    {
        Err(Risk::Drop)
    } else if mentioned(change, &method, dir, calls, b) {
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
            assumptions: Vec::new(),
            searched: None,
        })
    };
    let region = |tree: &Tree, scope: NodeId, holes: &[NodeId], touch: &[NodeId]| Region {
        scope: tree.node(scope).extent.clone(),
        holes: holes.iter().map(|&n| tree.node(n).extent.clone()).collect(),
        touch: touch.iter().map(|&n| tree.node(n).extent.clone()).collect(),
    };
    let call_side = region(calls, scope, &changed.0, &[a_call, b]);
    let chunk_side = region(chunks, chunk_scope, &changed.1, &[a_chunk]);
    let regions = match dir {
        Dir::Extract => [chunk_side, call_side],
        Dir::Inline => [call_side, chunk_side],
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
        regions,
        outcome,
        wiring,
        used_in: Vec::new(),
    })
}

/// Whether `v` is a bare visibility call (`private`) whose section in `scope`
/// holds nothing but B: no other statement follows it before the next one.
fn governs_only(tree: &Tree, scope: NodeId, v: NodeId, b: NodeId) -> bool {
    use crate::tree::Visibility::*;
    if !matches!(
        tree.node(v).role,
        Role::Visibility(Private | Protected | Public)
    ) || !code_children(tree, v).is_empty()
    {
        return false;
    }
    let siblings = code_children(tree, scope);
    let Some(at) = siblings.iter().position(|&c| c == v) else {
        return false;
    };
    let section: Vec<NodeId> = siblings[at + 1..]
        .iter()
        .copied()
        .take_while(|&c| !matches!(tree.node(c).role, Role::Visibility(_)))
        .collect();
    section == [b]
}

/// Why values don't flow between the call and the statements the way they
/// did, naming the local where one is to blame (`{caller}` stands for A's
/// name): `None` when they do. `wired`: the arguments carry exactly the
/// statements' free variables into B's parameters.
fn miswired(wired: bool, flow: &Flow, assigned: &HashSet<&str>) -> Option<String> {
    let first = |names: &mut dyn Iterator<Item = &str>| names.min().map(str::to_string);
    if let Some(name) = first(&mut flow.clobbers.iter().map(String::as_str)) {
        return Some(format!(
            "the statements assign {name}, which {{caller}} also assigns"
        ));
    }
    let outflow: HashSet<&str> = flow.outflow.iter().map(String::as_str).collect();
    if let Some(name) = first(&mut outflow.difference(assigned).copied()) {
        return Some(format!(
            "{{caller}} reads {name} after the statements, but the call doesn't return it"
        ));
    }
    if let Some(name) = first(&mut assigned.difference(&outflow).copied()) {
        return Some(format!(
            "the call's result is assigned to {name}, which the statements don't set for {{caller}}"
        ));
    }
    (!wired).then(|| Risk::Wiring.reason().to_string())
}

/// Checks a proven candidate against the repository: a file outside the
/// change that already uses B's name (a subclass's method of that name, a
/// caller of the removed method) makes it a collision. Removing a public
/// method of a library (an inline) leaves `public_api`.
fn reach(
    change: &Change,
    outside: &dyn Outside,
    file: usize,
    calls: &Tree,
    b: NodeId,
    found: &mut Found,
) {
    let Ok(stats) = &mut found.outcome else {
        return;
    };
    let covered: BTreeSet<&str> = change
        .files
        .iter()
        .filter(|f| f.trees.is_ok())
        .flat_map(|f| [f.diff.old_path.as_deref(), f.diff.new_path.as_deref()])
        .flatten()
        .collect();
    let path = change.files[file].diff.path();
    let suffixes = calls.semantics.spelling.suffixes;
    let word = found.method.trim_end_matches(|c| suffixes.contains(&c));
    let method = found.method.clone();
    let uses = |tree: &Tree| super::rename::mentions_method(tree, &method);
    match proof::verify(outside, path, word, &covered, &uses) {
        Verified::Used(files) => {
            found.used_in = files;
            found.outcome = Err(Risk::Collision);
            return;
        }
        Verified::Open(assumption) => stats.assumptions.push(assumption),
        Verified::Clear { searched } => stats.searched = Some(searched),
    }
    if found.dir == Dir::Inline && outside.library(path) && proof::public(calls, b) {
        stats.assumptions.push(Assumption::PublicApi);
    }
    stats.assumptions.sort_unstable();
}

/// Labels every hunk the refactoring touches, or none when another part
/// already claims a line it explains: the lines it explains become its
/// part, and a sibling that changed on its own stays logic.
fn apply(trees: &Trees, hunk_ids: &[usize], found: &Found, labels: &mut [Label]) {
    // (hunk, the lines the refactoring explains)
    let mut touched = Vec::new();
    let [before, after] = &found.regions;
    let code = (CodeLines::of(&trees.before), CodeLines::of(&trees.after));
    for (h, label) in labels.iter().enumerate() {
        let free = &label.changed;
        let touches = |code: &CodeLines, lines, region: &Region| {
            !code.touched(lines, |at| region.touches(&at)).is_empty()
        };
        if !touches(&code.0, &free.before, before) && !touches(&code.1, &free.after, after) {
            continue;
        }
        let lines = Claim {
            before: code.0.within(&free.before, |at| before.explains(&at)),
            after: code.1.within(&free.after, |at| after.explains(&at)),
        };
        touched.push((h, lines));
    }
    let mut ids: Vec<usize> = touched.iter().map(|&(h, _)| hunk_ids[h]).collect();
    // Runs of one hunk share its id.
    ids.dedup();
    let mut detail = Map::new();
    detail.insert("method".into(), Value::from(found.method.as_str()));
    detail.insert("caller".into(), Value::from(found.caller.as_str()));
    detail.insert("hunks".into(), json!(ids));
    match &found.outcome {
        Ok(stats) => {
            let mut reason = match found.dir {
                Dir::Extract => format!(
                    "statements of `{}` moved into new method `{}`, replaced by a call",
                    found.caller, found.method
                ),
                Dir::Inline => format!(
                    "method `{}` removed; its body replaces its call in `{}`",
                    found.method, found.caller
                ),
            };
            if stats.assumptions.contains(&Assumption::PublicApi) {
                reason = format!("public API removed: {reason}");
            } else if !stats.assumptions.is_empty() {
                reason = format!("{reason}; uses outside the change weren't checked");
            }
            let contested = touched.iter().any(|(h, lines)| {
                let taken = labels[*h].changed.minus(&labels[*h].unclaimed());
                !lines.intersect(&taken).is_empty()
            });
            if contested {
                return;
            }
            for (h, lines) in &touched {
                let label = &mut labels[*h];
                let mut evidence = label.residual.evidence.clone();
                let values = &mut evidence.values;
                values.insert("statements".into(), Value::from(stats.statements));
                values.insert("nodes".into(), Value::from(stats.nodes));
                values.insert("params".into(), Value::from(stats.params));
                values.insert("verbatim".into(), Value::from(stats.verbatim()));
                values.insert("returns".into(), Value::from(stats.returns));
                values.insert("searched".into(), json!(stats.searched));
                let part = Part {
                    kind: found.dir.kind(),
                    context: Vec::new(),
                    assumptions: stats.assumptions.clone(),
                    lines: lines.clone(),
                    // Structure, but where refactoring bugs cluster: still read.
                    review: lines.clone(),
                    confidence: stats.confidence(),
                    reason: reason.clone(),
                    evidence,
                    detail: detail.clone(),
                    variants: Vec::new(),
                };
                label.claim(part);
            }
        }
        Err(risk) => {
            let near = found.dir.kind().as_str();
            detail.insert("near".into(), Value::from(near));
            detail.insert("risk".into(), Value::from(risk.as_str()));
            for (h, _) in &touched {
                let label = &mut labels[*h];
                if !label.residual.detail.is_empty() {
                    continue;
                }
                let residual = &mut label.residual;
                let why = match &found.wiring {
                    Some(why) => why.replace("{caller}", &found.caller),
                    None => risk.reason().to_string(),
                };
                residual.reason = format!("not an {near} of `{}`: {why}", found.method);
                if !found.used_in.is_empty() {
                    residual.reason += &format!(" in {}", proof::listing(&found.used_in));
                }
                residual.detail = detail.clone();
            }
        }
    }
}

/// Calls that may reach a method of the same receiver (`go`, `self.go`), by
/// name: indexed once per tree, as every new method looks up its own.
fn call_sites(tree: &Tree) -> HashMap<&str, Vec<NodeId>> {
    let mut sites: HashMap<&str, Vec<NodeId>> = HashMap::new();
    for id in ids(tree) {
        let node = tree.node(id);
        if let (Role::Call(Receiver::Implicit | Receiver::SelfRef), Some(name)) =
            (node.role, node.name.as_deref())
        {
            sites.entry(name).or_default().push(id);
        }
    }
    sites
}

fn ids(tree: &Tree) -> impl Iterator<Item = NodeId> + '_ {
    (0..tree.nodes.len() as u32).map(NodeId)
}

fn code_children(tree: &Tree, id: NodeId) -> Vec<NodeId> {
    tree.code_children(id).collect()
}

/// The receiver a method is defined on (`def self.x`), by its hash.
fn receiver(tree: &Tree, def: NodeId) -> Option<u64> {
    if !tree.node(def).flags.contains(Flags::RECEIVER) {
        return None;
    }
    tree.code_children(def).next().map(|r| tree.node(r).hash)
}

/// Where a method sits: its ancestors' kinds and names, its own, and its
/// receiver. A namespace with no name (a Rust `impl`) stands by its header.
type Key<'t> = (
    Vec<(&'static str, Option<&'t str>, Option<u64>)>,
    Option<u64>,
);

fn key(tree: &Tree, def: NodeId) -> Key<'_> {
    let path = tree.path(def);
    let path = path.iter().map(|s| (s.kind, s.name, s.header)).collect();
    (path, receiver(tree, def))
}

/// How many methods sit at each [`Key`].
fn def_counts(tree: &Tree) -> HashMap<Key<'_>, usize> {
    let mut counts = HashMap::new();
    for id in ids(tree).filter(|&id| tree.node(id).role == Role::Definition(NameKind::Method)) {
        *counts.entry(key(tree, id)).or_insert(0) += 1;
    }
    counts
}

fn unique_def(tree: &Tree, want: &Key<'_>) -> Option<NodeId> {
    let mut found = ids(tree)
        .filter(|&id| tree.node(id).role == Role::Definition(NameKind::Method))
        .filter(|&id| key(tree, id) == *want);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// The statement holding the call: the call itself, or a plain assignment of
/// its result to a local, whose name is returned.
fn statement(tree: &Tree, call: NodeId) -> (NodeId, Option<String>) {
    if let Some(parent) = tree.node(call).parent {
        let node = tree.node(parent);
        if let Some(name) = local_name(node)
            && node.role == Role::Bind
            && code_children(tree, parent) == [call]
        {
            return (parent, Some(name.to_string()));
        }
    }
    (call, None)
}

/// B's parameter names and body, when every parameter is a plain required
/// positional one. Annotations (types, generics, visibility) run no code
/// and are passed over; a receiver parameter (`self`) is the call's receiver,
/// not one of its arguments.
fn method_body(tree: &Tree, def: NodeId) -> Option<(Vec<String>, NodeId)> {
    let mut children = code_children(tree, def);
    if receiver(tree, def).is_some() {
        children.remove(0);
    }
    children.retain(|&c| tree.node(c).role != Role::Annotation);
    let is_param = |k: NodeId| {
        matches!(
            tree.node(k).role,
            Role::Definition(
                NameKind::Parameter
                    | NameKind::KeywordParameter
                    | NameKind::BlockParameter
                    | NameKind::SelfParameter
            )
        )
    };
    let is_params = |id: NodeId| code_children(tree, id).into_iter().all(is_param);
    let (params, body) = match children[..] {
        [body] if code_children(tree, body).is_empty() || !is_params(body) => (Vec::new(), body),
        [params, body] if is_params(params) => {
            let mut names = Vec::new();
            for p in code_children(tree, params) {
                let node = tree.node(p);
                if node.role == Role::Definition(NameKind::SelfParameter) {
                    continue;
                }
                let name = node.name.as_deref()?;
                // Defaults, splats and keywords all add text or code beside
                // the name; a type annotation (`x: T`) adds neither.
                let typed = code_children(tree, p)
                    .iter()
                    .all(|&c| tree.node(c).role == Role::Annotation);
                let text = unmarked(own(tree, p, Some(name)));
                if node.role != Role::Definition(NameKind::Parameter)
                    || !typed
                    || !(text.is_empty() || (text == b":" && !node.children.is_empty()))
                {
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
/// else (a block, a splat, a keyword) rides along: besides its receiver, the
/// call holds at most its argument list ([`Flags::ARGUMENTS`]).
fn arguments(tree: &Tree, call: NodeId) -> Option<Vec<String>> {
    let mut children = code_children(tree, call);
    if tree.node(call).role == Role::Call(Receiver::SelfRef) {
        children.remove(0);
    }
    match children[..] {
        [] => Some(Vec::new()),
        [args] if tree.node(args).flags.contains(Flags::ARGUMENTS) => code_children(tree, args)
            .into_iter()
            .map(|a| {
                let node = tree.node(a);
                (node.role == Role::Reference(NameKind::Variable) && node.children.is_empty())
                    .then(|| local_name(node))
                    .flatten()
                    .map(str::to_string)
            })
            .collect(),
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
            || (child != stmt && unmarked(own(calls, x, None)) != unmarked(own(chunks, y, None)))
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
            // Separators between the chunk's statements go into B with them.
            return (same(&cx[..i], &cy[..i])
                && same(&cx[i + 1..], &cy[end..])
                && outside(calls, x, stmt, stmt) == outside(chunks, y, cy[i], cy[end - 1]))
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

/// A container's own text, as [`own`] reads it, with the run of children
/// `first..=last` and everything between them standing as one child.
fn outside(tree: &Tree, id: NodeId, first: NodeId, last: NodeId) -> Vec<u8> {
    let region = tree.node(first).extent.start..tree.node(last).extent.end;
    let mut out = Vec::new();
    tree.walk_own(id, |piece| match piece {
        Piece::Text(text) => out.extend(
            text.filter(|at| !tree.source[*at].is_ascii_whitespace() && !region.contains(at))
                .map(|at| tree.source[at]),
        ),
        Piece::Child(c) => {
            let child = tree.node(c);
            if !child.comment && (c == first || !region.contains(&child.extent.start)) {
                out.push(MARK);
            }
        }
    });
    out
}

/// Whether something the chunk holds lives to the end of its block in one
/// version and to the end of B in the other: code follows the chunk, and it
/// binds a local (other than the value returned) or takes arguments, which a
/// call moves into B.
fn outlives(tree: &Tree, chunk: &[NodeId], assigned: Option<&str>, params: &[String]) -> bool {
    let Some((&last, _)) = chunk.split_last() else {
        return false;
    };
    let follows = tree
        .node(last)
        .parent
        .is_some_and(|block| code_children(tree, block).last() != Some(&last));
    let binds = chunk.iter().flat_map(|&s| tree.descendants(s)).any(|id| {
        let node = tree.node(id);
        node.role.defines() == Some(NameKind::Variable) && node.name.as_deref() != assigned
    });
    follows && (binds || !params.is_empty())
}

/// Local variables around the chunk, in the chunk side's A.
struct Flow {
    /// Read in the chunk, defined outside it: they must arrive as arguments.
    free: HashSet<String>,
    /// Locals the chunk assigns that A also defines outside it.
    clobbers: HashSet<String>,
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
        // An opaque name (in a macro's tokens) spelling a local may read it.
        for id in tree.descendants(a) {
            let node = tree.node(id);
            let Some(name) = node.name.as_deref().filter(|_| node.role == Role::Opaque) else {
                continue;
            };
            if defs_in.contains(name) || defs_out.contains(name) {
                match inside.contains(&id) {
                    true => reads_in.insert(name.to_string()),
                    false => reads_out.insert(name.to_string()),
                };
            }
        }
        Flow {
            free: reads_in.difference(&defs_in).cloned().collect(),
            clobbers: defs_in.intersection(&defs_out).cloned().collect(),
            outflow: defs_in.intersection(&reads_out).cloned().collect(),
        }
    }
}

/// The name of a local variable or parameter the node defines or reads.
/// Fields and globals live beyond the method, so a new method sees them
/// unchanged.
fn local_name(node: &crate::tree::Node) -> Option<&str> {
    use NameKind::*;
    let local = matches!(
        node.role,
        Role::Reference(Variable)
            | Role::Bind
            | Role::Definition(Variable | Parameter | KeywordParameter | BlockParameter)
    );
    node.name.as_deref().filter(|_| local)
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

    /// Two opaque names correspond when the renaming already pairs them,
    /// or neither is renamed and they're equal. Opaque names never start a
    /// pairing: what they mean is unknown.
    fn opaque(&self, b: Option<&str>, chunk: Option<&str>, renamed: &mut usize) -> bool {
        let (Some(b), Some(chunk)) = (b, chunk) else {
            return b == chunk;
        };
        let ok = match (self.forward.get(b), self.back.get(chunk)) {
            (Some(x), _) => x == chunk,
            (None, Some(_)) => false,
            (None, None) => b == chunk,
        };
        *renamed += usize::from(ok && b != chunk);
        ok
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
        if nx.role == Role::Opaque && ny.role == Role::Opaque {
            return self
                .names
                .opaque(nx.name.as_deref(), ny.name.as_deref(), &mut self.renamed);
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
    (node.role == Role::Bind && local_name(node) == Some(v) && kids.len() == 1).then(|| kids[0])
}

/// B's name already means something: one the language reserves (an
/// inherited, protocol or entry-point name), or used on the side where B doesn't
/// exist, in any file of the change (a def, a call or a dynamic reference
/// that defining B would capture, or that removing it would break), or
/// defined again beside B, so B takes part in dispatch beyond its one call.
/// New calls to the new method are not collisions.
fn mentioned(change: &Change, method: &str, dir: Dir, ours: &Tree, b: NodeId) -> bool {
    let named = |tree: &Tree, id: NodeId| {
        let node = tree.node(id);
        match node.name.as_deref() {
            Some(name) if node.role == Role::DynamicName => crate::tree::spells(name, method),
            name => name == Some(method),
        }
    };
    ours.semantics.reserves(method)
        || change
            .files
            .iter()
            .filter_map(|f| f.trees.as_ref().ok())
            .any(|trees| {
                let (calls, chunks) = dir.sides(trees);
                ids(chunks).any(|id| {
                    super::rename::literal_names(chunks, id, method)
                        || named(chunks, id)
                            && matches!(
                                chunks.node(id).role,
                                Role::Definition(NameKind::Method)
                                    | Role::Reference(NameKind::Method)
                                    | Role::Call(_)
                                    | Role::DynamicName
                                    | Role::Opaque
                            )
                }) || ids(calls).any(|id| {
                    named(calls, id)
                        && calls.node(id).role == Role::Definition(NameKind::Method)
                        && !(std::ptr::eq(calls, ours) && id == b)
                })
            })
}

/// What the tree's hash reads of a node's own text ([`Tree::own_text`]). A
/// local's name is dropped, so renamed locals compare equal: the leading
/// name, or for a [`Role::Bind`] (`let v = …`) the first whole-token one.
fn own(tree: &Tree, id: NodeId, name: Option<&str>) -> Vec<u8> {
    let bind = tree.node(id).role == Role::Bind;
    let mut out = tree.own_text(id, name.is_some() && bind);
    if let Some(name) = name
        && !bind
        && out.starts_with(name.as_bytes())
    {
        out.drain(..name.len());
    }
    out
}

fn unmarked(text: Vec<u8>) -> Vec<u8> {
    text.into_iter().filter(|&b| b != MARK).collect()
}
