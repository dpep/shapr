//! `default_argument`: a call drops an argument that restates its parameter's
//! literal default (`fetch(x, retries: 3)` to `fetch(x)` where `fetch` says
//! `retries: 3`), or adds one.
//!
//! A hunk's changed region sits in the smallest node on each side whose text
//! outside the region is the same; the two must be equal but for calls that
//! differ by one such argument. Each call is proven to reach exactly one
//! definition: it has no receiver but `self`, sits in a method with no block
//! or other body around it that could change `self`, and the method it calls
//! is defined once in the whole repository, in the same class or module and
//! on the same receiver, with nothing else naming it (`send`, `alias`,
//! `define_method`, a symbol). That definition takes no rest of its arguments
//! (`*args`, `**opts`), and the argument and the default are the same plain
//! literal (a number, symbol, plain string, `nil`, `true`, `false`, or an
//! empty collection), compared by canonical form.

use super::hazard::{Op, blocks};
use super::proof::{self, Assumption, Naming, Search};
use super::site::{CodeLines, Side as SiteSide};
use super::{Claim, Confidence, Evidence, Kind, Labels, Method, Outside, Part};
use crate::change::{Change, Trees};
use crate::frontend::{self, Arguments, Frontend};
use crate::tree::{Flags, MARK, NameKind, NodeId, Receiver, Role, Span, Tree};
use serde_json::{Map, Value, json};
use std::cell::OnceCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

pub(super) fn run(
    change: &Change,
    _: &super::align::Alignments,
    outside: &dyn Outside,
    labels: &mut Labels,
) {
    let mut lookups = Lookups::new(change, outside);
    for (f, file) in change.files.iter().enumerate() {
        let Ok(trees) = &file.trees else { continue };
        let Some(frontend) = frontend::for_path(file.diff.path()) else {
            continue;
        };
        if !trees.clean() || !trees.before.semantics.default_arguments {
            continue;
        }
        let counts = OnceCell::new();
        for (h, site) in trees.sites.iter().enumerate() {
            let free = labels[f][h].unclaimed();
            if free.is_empty() {
                continue;
            }
            let counts =
                counts.get_or_init(|| (Counts::of(&trees.before), Counts::of(&trees.after)));
            let Some((b, a)) = anchors(trees, counts, &site.before, &site.after) else {
                continue;
            };
            let pair = Pair {
                trees,
                frontend,
                near: Default::default(),
            };
            let mut restated = Vec::new();
            if !pair.equivalent(b, a, &mut restated) {
                continue;
            }
            let near = pair.near.take();
            if restated.is_empty() && near.is_empty() {
                continue;
            }
            let results: Vec<Result<Proof, String>> = restated
                .iter()
                .map(|r| lookups.prove(f, &pair, r))
                .collect();
            let label = &mut labels[f][h];
            // Every call the hunk restates a default in, near misses included.
            let total = restated.len() + near.len();
            let mut failures: Vec<String> = near;
            failures.extend(results.iter().filter_map(|r| r.as_ref().err().cloned()));
            if !failures.is_empty() {
                if label.residual.detail.is_empty() {
                    let n = failures.len();
                    label.residual.reason = match (n, total) {
                        (_, 1) => failures.remove(0),
                        (n, total) if n == total => format!("{n} calls: {}", failures.join("; ")),
                        (n, total) => format!("{n} of {total} calls: {}", failures.join("; ")),
                    };
                }
                continue;
            }
            let proofs: Vec<Proof> = results.into_iter().flatten().collect();
            let code = (CodeLines::of(&trees.before), CodeLines::of(&trees.after));
            let (eb, ea) = (
                trees.before.node(b).extent.clone(),
                trees.after.node(a).extent.clone(),
            );
            let lines = Claim {
                before: code.0.touched(&free.before, |at| eb.contains(&at)),
                after: code.1.touched(&free.after, |at| ea.contains(&at)),
            };
            let assumptions: BTreeSet<Assumption> = proofs
                .iter()
                .flat_map(|p| p.assumptions.iter().copied())
                .collect();
            let searched = proofs.iter().filter_map(|p| p.searched).max();
            let review = match assumptions.is_empty() {
                true => Claim::default(),
                false => lines.clone(),
            };
            let detail: Vec<Value> = restated
                .iter()
                .zip(&proofs)
                .map(|(r, p)| {
                    json!({
                        "method": p.method,
                        "parameter": r.param.to_string(),
                        "value": pair.text(r.side, r.value),
                        "added": r.side == Side::After,
                    })
                })
                .collect();
            let mut reason = describe_all(&pair, &restated, &proofs);
            if assumptions.contains(&Assumption::PublicApi) {
                reason += ", but a library's subclass outside the repository may override it";
            } else if !assumptions.is_empty() {
                reason += ", but other definitions outside the change weren't checked";
            }
            label.claim(Part {
                kind: Kind::DefaultArgument,
                context: Vec::new(),
                assumptions: assumptions.into_iter().collect(),
                lines,
                review,
                // An exact comparison, as for a restyle: what it relies on is
                // in `assumptions`.
                confidence: Confidence::new(1.0),
                reason,
                evidence: Evidence {
                    method: Method::Ast,
                    values: Map::from_iter([
                        ("restated".to_string(), Value::from(restated.len())),
                        ("searched".to_string(), json!(searched)),
                    ]),
                },
                detail: Map::from_iter([("restated".to_string(), Value::Array(detail))]),
                variants: Vec::new(),
            });
        }
    }
}

/// What the calls restate: "drops `k: 1`, `m`'s default", or for several,
/// "2 calls: drops `k: 1`, adds `j: 2`, `m`'s default" (each its method's
/// when they differ).
fn describe_all(pair: &Pair, restated: &[Restated], proofs: &[Proof]) -> String {
    let what: Vec<String> = restated.iter().map(|r| pair.describe(r)).collect();
    let one = proofs.iter().all(|p| p.method == proofs[0].method);
    let list = match one {
        true => format!("{}, `{}`'s default", what.join(", "), proofs[0].method),
        false => (what.iter().zip(proofs))
            .map(|(w, p)| format!("{w}, `{}`'s default", p.method))
            .collect::<Vec<_>>()
            .join("; "),
    };
    match restated.len() {
        1 => list,
        n => format!("{n} calls: {list}"),
    }
}

/// Which side of the change holds the argument.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Before,
    After,
}

/// The parameter an argument is passed to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Param {
    Keyword(String),
    /// A positional one, by its 0-based place in the call.
    Position(usize),
}

impl std::fmt::Display for Param {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Param::Keyword(k) => write!(f, "{k}:"),
            Param::Position(i) => write!(f, "argument {}", i + 1),
        }
    }
}

/// A call on each side that differs only by an argument on `side`.
struct Restated {
    calls: (NodeId, NodeId),
    side: Side,
    param: Param,
    /// The argument's value, in `side`'s tree.
    value: NodeId,
}

struct Proof {
    method: String,
    assumptions: Vec<Assumption>,
    searched: Option<usize>,
}

/// The region a hunk changed on one side: its significant bytes, its blank
/// lines, or the point an insertion or deletion sits at.
fn region(tree: &Tree, side: &SiteSide) -> Span {
    if let Some(span) = &side.span {
        return span.clone();
    }
    match side.lines.last() {
        Some(last) => tree.line_span(side.lines.start, last),
        None => {
            let at = tree.line_start(side.lines.start);
            at..at
        }
    }
}

/// The smallest node on each side holding the hunk's region whose text
/// around the region, whitespace aside, is the same on both sides: one node,
/// the region swapped.
fn anchors(
    trees: &Trees,
    (cb, ca): &(Counts, Counts),
    before: &SiteSide,
    after: &SiteSide,
) -> Option<(NodeId, NodeId)> {
    let (tb, ta) = (&trees.before, &trees.after);
    let (rb, ra) = (region(tb, before), region(ta, after));
    let (mut b, mut a) = (tb.deepest_containing(&rb), ta.deepest_containing(&ra));
    loop {
        match (around(tb, b, &rb), around(ta, a, &ra)) {
            (Some(x), Some(y)) => {
                let (lx, ly) = (
                    cb.count(&x.0) + cb.count(&x.1),
                    ca.count(&y.0) + ca.count(&y.1),
                );
                if lx == ly && same(tb, &x.0, ta, &y.0) && same(tb, &x.1, ta, &y.1) {
                    return Some((b, a));
                }
                if lx <= ly {
                    b = tb.node(b).parent?;
                }
                if ly <= lx {
                    a = ta.node(a).parent?;
                }
            }
            (None, _) => b = tb.node(b).parent?,
            (_, None) => a = ta.node(a).parent?,
        }
    }
}

/// The node's spans before and after `region`; `None` when the node
/// doesn't hold the region.
fn around(tree: &Tree, id: NodeId, region: &Span) -> Option<(Span, Span)> {
    let extent = &tree.node(id).extent;
    if extent.start > region.start || region.end > extent.end {
        return None;
    }
    Some((extent.start..region.start, region.end..extent.end))
}

/// Whether two spans hold the same text, whitespace dropped.
fn same(tx: &Tree, x: &Span, ty: &Tree, y: &Span) -> bool {
    let bx = tx.text(x).iter().filter(|b| !b.is_ascii_whitespace());
    let by = ty.text(y).iter().filter(|b| !b.is_ascii_whitespace());
    bx.eq(by)
}

/// How many bytes before each offset of a tree's source aren't whitespace,
/// so a span's count is a subtraction.
struct Counts(Vec<usize>);

impl Counts {
    fn of(tree: &Tree) -> Counts {
        let mut seen = 0;
        let mut counts = Vec::with_capacity(tree.source.len() + 1);
        counts.push(0);
        for b in &tree.source {
            seen += usize::from(!b.is_ascii_whitespace());
            counts.push(seen);
        }
        Counts(counts)
    }

    /// The bytes of `span` that aren't whitespace.
    fn count(&self, span: &Span) -> usize {
        self.0[span.end] - self.0[span.start]
    }
}

/// The two trees of one file, compared through its frontend.
struct Pair<'a> {
    trees: &'a Trees,
    frontend: &'static dyn Frontend,
    /// Calls that differ by one argument shapr can't tie to a parameter, as
    /// reasons: compared as equal, so the rest of the code is still checked.
    near: std::cell::RefCell<Vec<String>>,
}

impl Pair<'_> {
    fn tree(&self, side: Side) -> &Tree {
        match side {
            Side::Before => &self.trees.before,
            Side::After => &self.trees.after,
        }
    }

    /// What the change does with the argument: "drops `k: 1`".
    fn describe(&self, r: &Restated) -> String {
        let verb = match r.side {
            Side::Before => "drops",
            Side::After => "adds",
        };
        let value = self.text(r.side, r.value);
        match &r.param {
            Param::Keyword(k) => format!("{verb} `{k}: {value}`"),
            Param::Position(_) => format!("{verb} `{value}` ({})", r.param),
        }
    }

    fn text(&self, side: Side, id: NodeId) -> String {
        let tree = self.tree(side);
        String::from_utf8_lossy(tree.text(&tree.node(id).extent)).into_owned()
    }

    /// `x` (before) and `y` (after) are the same code but for calls that
    /// restate a default, which are collected.
    fn equivalent(&self, x: NodeId, y: NodeId, found: &mut Vec<Restated>) -> bool {
        let (tb, ta) = (&self.trees.before, &self.trees.after);
        let (nx, ny) = (tb.node(x), ta.node(y));
        if nx.hash == ny.hash {
            return true;
        }
        if let Some(restates) = self.restates(x, y, found) {
            return restates;
        }
        let cx: Vec<NodeId> = tb.code_children(x).collect();
        let cy: Vec<NodeId> = ta.code_children(y).collect();
        // A leaf's text is its value, whitespace included.
        nx.kind == ny.kind
            && nx.role == ny.role
            && nx.name == ny.name
            && !cx.is_empty()
            && cx.len() == cy.len()
            && tb.own_text(x, false) == ta.own_text(y, false)
            && cx
                .iter()
                .zip(&cy)
                .all(|(&c, &d)| self.equivalent(c, d, found))
    }

    /// Whether two calls are the same but for one argument on one side:
    /// `None` when they don't differ that way.
    fn restates(&self, x: NodeId, y: NodeId, found: &mut Vec<Restated>) -> Option<bool> {
        let (tb, ta) = (&self.trees.before, &self.trees.after);
        let (nx, ny) = (tb.node(x), ta.node(y));
        let unmarked = |tree: &Tree, id| -> Vec<u8> {
            let text = tree.own_text(id, false).into_iter();
            text.filter(|&b| b != MARK).collect()
        };
        if !matches!(nx.role, Role::Call(_))
            || (nx.role, nx.kind, &nx.name) != (ny.role, ny.kind, &ny.name)
            || unmarked(tb, x) != unmarked(ta, y)
        {
            return None;
        }
        let others = |tree: &Tree, id| -> Vec<NodeId> {
            let children = tree.code_children(id);
            children
                .filter(|&c| !tree.node(c).flags.contains(Flags::ARGUMENTS))
                .collect()
        };
        let (ox, oy) = (others(tb, x), others(ta, y));
        let near = |why: &str, found: &mut Vec<Restated>| -> Option<bool> {
            let list = |tree: &Tree, id| {
                (tree.code_children(id)).find(|&c| tree.node(c).flags.contains(Flags::ARGUMENTS))
            };
            let (lx, ly) = (list(tb, x)?, list(ta, y)?);
            let (side, extra) = match (one_more(tb, lx, ta, ly), one_more(ta, ly, tb, lx)) {
                (Some(n), _) => (Side::Before, n),
                (_, Some(n)) => (Side::After, n),
                _ => return None,
            };
            let same = ox.len() == oy.len()
                && (ox.iter().zip(&oy)).all(|(&c, &d)| self.equivalent(c, d, found));
            if !same {
                return Some(false);
            }
            let verb = match side {
                Side::Before => "drops",
                Side::After => "adds",
            };
            let text = self.text(side, extra);
            (self.near.borrow_mut()).push(format!(
                "{verb} `{text}`, not shown to restate a default: {why}"
            ));
            Some(true)
        };
        let (Some(ax), Some(ay)) = (
            self.frontend.arguments(tb, x),
            self.frontend.arguments(ta, y),
        ) else {
            return near(
                "the call's arguments can't each be tied to a parameter \
                 (a splat, `...`, or a key that isn't a plain symbol)",
                found,
            );
        };
        let (side, param, value, pairs) = match (dropped(&ax, &ay), dropped(&ay, &ax)) {
            (Some((param, value, pairs)), _) => (Side::Before, param, value, pairs),
            (_, Some((param, value, pairs))) => {
                let pairs = pairs.into_iter().map(|(l, s)| (s, l)).collect();
                (Side::After, param, value, pairs)
            }
            _ if ax.positional.len().abs_diff(ay.positional.len()) == 1
                && !(ax.keywords.is_empty() && ay.keywords.is_empty()) =>
            {
                return near(
                    "a positional argument beside keyword arguments isn't tied to its parameter",
                    found,
                );
            }
            _ => return None,
        };
        let same = ox.len() == oy.len()
            && ox
                .iter()
                .zip(&oy)
                .all(|(&c, &d)| self.equivalent(c, d, found))
            && pairs.iter().all(|&(c, d)| self.equivalent(c, d, found));
        if same {
            found.push(Restated {
                calls: (x, y),
                side,
                param,
                value,
            });
        }
        Some(same)
    }
}

/// The one code child (or descendant, through children that differ alone)
/// `longer` holds beyond `shorter`, all else equal.
fn one_more(tl: &Tree, longer: NodeId, ts: &Tree, shorter: NodeId) -> Option<NodeId> {
    let cl: Vec<NodeId> = tl.code_children(longer).collect();
    let cs: Vec<NodeId> = ts.code_children(shorter).collect();
    let eq = |l: NodeId, s: NodeId| tl.node(l).hash == ts.node(s).hash;
    if cl.len() == cs.len() + 1 {
        return (0..cl.len()).map(|i| cl[i]).find(|&extra| {
            let kept = cl.iter().filter(|&&c| c != extra);
            kept.zip(&cs).all(|(&l, &s)| eq(l, s))
        });
    }
    let differ: Vec<(NodeId, NodeId)> = (cl.iter().zip(&cs))
        .filter(|&(&l, &s)| !eq(l, s))
        .map(|(&l, &s)| (l, s))
        .collect();
    match differ[..] {
        [(l, s)] if cl.len() == cs.len() => one_more(tl, l, ts, s),
        _ => None,
    }
}

/// Nodes to compare, one from each tree.
type Pairs = Vec<(NodeId, NodeId)>;

/// The argument `longer` passes and `shorter` doesn't, when that is all
/// that differs in their shape: one keyword (the others in order), or with
/// no keywords, the last positional one. With the pairs of arguments left
/// to compare, `longer`'s first.
fn dropped(longer: &Arguments, shorter: &Arguments) -> Option<(Param, NodeId, Pairs)> {
    let positional = |n: usize| -> Vec<(NodeId, NodeId)> {
        let pairs = longer.positional.iter().zip(&shorter.positional).take(n);
        pairs.map(|(&l, &s)| (l, s)).collect()
    };
    let (lp, sp) = (longer.positional.len(), shorter.positional.len());
    if lp == sp && longer.keywords.len() == shorter.keywords.len() + 1 {
        let names = |k: &[(String, NodeId)]| -> Vec<String> {
            k.iter().map(|(name, _)| name.clone()).collect()
        };
        let want = names(&shorter.keywords);
        let i = (0..longer.keywords.len()).find(|&i| {
            let mut kept = longer.keywords.clone();
            kept.remove(i);
            names(&kept) == want
        })?;
        let mut kept = longer.keywords.clone();
        let (name, value) = kept.remove(i);
        let mut pairs = positional(lp);
        pairs.extend(kept.iter().zip(&shorter.keywords).map(|(l, s)| (l.1, s.1)));
        return Some((Param::Keyword(name), value, pairs));
    }
    if lp == sp + 1 && longer.keywords.is_empty() && shorter.keywords.is_empty() {
        return Some((Param::Position(sp), longer.positional[sp], positional(sp)));
    }
    None
}

/// Where a call's method must be defined: the class or module body around
/// the method making the call, and that method's receiver.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Home {
    container: Vec<(&'static str, Option<String>, Option<u64>)>,
    receiver: Option<u64>,
}

/// Where the method a call in `id` reaches must be defined: the first node
/// around it that may change `self` must be a method definition.
fn home(tree: &Tree, frontend: &dyn Frontend, id: NodeId) -> Result<Home, String> {
    let def = (tree.ancestors(id))
        .find(|&p| frontend.rebinds_self(tree, p))
        .filter(|&d| tree.node(d).role == Role::Definition(NameKind::Method))
        .ok_or("the call is inside a block or a body where `self` may be another object")?;
    home_of_def(tree, frontend, def)
        .ok_or_else(|| "the calling method isn't defined directly in a class or module".into())
}

/// The receiver a method is defined on (`def self.x`), by its hash.
fn receiver(tree: &Tree, def: NodeId) -> Option<u64> {
    if !tree.node(def).flags.contains(Flags::RECEIVER) {
        return None;
    }
    tree.code_children(def).next().map(|r| tree.node(r).hash)
}

/// The definitions of a method name outside the change, and what the
/// search left open.
struct Lookup {
    /// (path, its tree, the definition).
    defs: Vec<(String, Rc<Tree>, NodeId)>,
    assumption: Option<Assumption>,
    searched: Option<usize>,
}

struct Lookups<'c> {
    change: &'c Change,
    outside: &'c dyn Outside,
    covered: BTreeSet<&'c str>,
    found: HashMap<String, Result<Lookup, String>>,
}

/// A definition of the method on one side: its file, tree and node.
type Def<'t> = (&'t str, &'t Tree, NodeId);

impl<'c> Lookups<'c> {
    fn new(change: &'c Change, outside: &'c dyn Outside) -> Self {
        let covered = (change.files.iter())
            .filter(|f| f.trees.is_ok())
            .flat_map(|f| [f.diff.old_path.as_deref(), f.diff.new_path.as_deref()])
            .flatten()
            .collect();
        Lookups {
            change,
            outside,
            covered,
            found: HashMap::new(),
        }
    }

    fn prove(&mut self, f: usize, pair: &Pair, r: &Restated) -> Result<Proof, String> {
        let (tree, frontend) = (pair.tree(r.side), pair.frontend);
        let value = pair.text(r.side, r.value);
        let what = pair.describe(r);
        let fail = |why: String| format!("{what}, not shown to restate a default: {why}");
        if !frontend.plain_literal(tree, r.value) {
            return Err(fail(format!("`{value}` isn't a plain literal")));
        }
        let (tb, ta) = (&pair.trees.before, &pair.trees.after);
        let method = tb
            .node(r.calls.0)
            .name
            .as_deref()
            .unwrap_or_default()
            .to_string();
        for (tree, call) in [(tb, r.calls.0), (ta, r.calls.1)] {
            if !matches!(
                tree.node(call).role,
                Role::Call(Receiver::Implicit | Receiver::SelfRef)
            ) {
                return Err(fail(format!(
                    "the call has a receiver, so which `{method}` it reaches is unknown"
                )));
            }
        }
        let home_b = home(tb, frontend, r.calls.0).map_err(fail)?;
        let home_a = home(ta, frontend, r.calls.1).map_err(fail)?;
        if home_b != home_a {
            return Err(fail("the call moved to another method".into()));
        }
        let path = self.change.files[f].diff.path().to_string();
        self.lookup(&path, &method, tree.semantics.spelling.suffixes)
            .map_err(&fail)?;
        let lookup = self.found[&method].as_ref().expect("looked up");
        let mut defs: [Vec<Def>; 2] = [Vec::new(), Vec::new()];
        for file in &self.change.files {
            let Ok(trees) = &file.trees else { continue };
            for (s, tree) in [&trees.before, &trees.after].into_iter().enumerate() {
                // A syntax error can hide a definition.
                let named = || {
                    tree.source
                        .windows(method.len())
                        .any(|w| w == method.as_bytes())
                };
                if !tree.errors.is_empty() && named() {
                    return Err(fail(format!(
                        "{} names `{method}` and doesn't parse",
                        file.diff.path()
                    )));
                }
                let found = (scan(tree, &method))
                    .map_err(|why| fail(format!("{why} in {}", file.diff.path())))?;
                defs[s].extend(found.into_iter().map(|d| (file.diff.path(), tree, d)));
            }
        }
        for d in &mut defs {
            d.extend(lookup.defs.iter().map(|(p, t, id)| (p.as_str(), &**t, *id)));
        }
        for (side, want) in defs.iter().zip([&home_b, &home_a]) {
            if side.len() == 1 {
                continue;
            }
            if side.is_empty() {
                return Err(fail(format!("`{method}` isn't defined in the repository")));
            }
            let at_home = |(path, tree, def): &Def| {
                let frontend = frontend::for_path(path).unwrap_or(frontend);
                home_of_def(tree, frontend, *def).as_ref() == Some(want)
            };
            let elsewhere: BTreeSet<&str> =
                (side.iter()).filter(|d| !at_home(d)).map(|d| d.0).collect();
            let files = |paths: BTreeSet<&str>| paths.into_iter().collect::<Vec<_>>().join(", ");
            return Err(fail(match elsewhere.is_empty() {
                true => format!(
                    "`{method}` is defined more than once in {}",
                    files(side.iter().map(|d| d.0).collect())
                ),
                false => format!(
                    "`{method}` is also defined in {}, which may override it",
                    files(elsewhere)
                ),
            }));
        }
        let [def_b, def_a] = defs.map(|d| d[0]);
        let mut lists = Vec::new();
        for ((path, tree, def), want) in [(def_b, &home_b), (def_a, &home_a)] {
            let frontend = frontend::for_path(path).unwrap_or(frontend);
            let at = home_of_def(tree, frontend, def);
            if at.as_ref() != Some(want) {
                return Err(fail(format!(
                    "`{method}` is defined elsewhere than where the call reaches"
                )));
            }
            if let Some(hazard) = (tree.descendants(def))
                .filter(|&id| tree.scope(id) == def)
                .filter_map(|id| tree.node(id).role.hazard())
                .find(|&h| blocks(Op::RestateDefault, h))
            {
                return Err(fail(format!("`{method}` reads its own frame ({hazard:?})")));
            }
            let signature = frontend
                .signature(tree, def)
                .ok_or_else(|| fail(format!("`{method}` collects the rest of its arguments")))?;
            lists.push((tree, signature));
        }
        let canon = |(tree, list): (&Tree, Option<NodeId>)| list.map(|l| tree.node(l).canon);
        if canon((lists[0].0, lists[0].1.list)) != canon((lists[1].0, lists[1].1.list)) {
            return Err(fail(format!("`{method}`'s parameters change")));
        }
        let (def_tree, signature) = match r.side {
            Side::Before => &lists[0],
            Side::After => &lists[1],
        };
        let default = match &r.param {
            Param::Keyword(k) => signature
                .keywords
                .iter()
                .find(|(name, _)| name == k)
                .and_then(|(_, d)| *d),
            Param::Position(i) if signature.keywords.is_empty() => i
                .checked_sub(signature.required)
                .and_then(|o| signature.optional.get(o).copied()),
            Param::Position(_) => None,
        };
        let Some(default) = default else {
            return Err(fail(format!("`{method}` has no default for {}", r.param)));
        };
        let default_text = String::from_utf8_lossy(def_tree.text(&def_tree.node(default).extent));
        if !frontend.plain_literal(def_tree, default)
            || def_tree.node(default).canon != tree.node(r.value).canon
        {
            return Err(fail(format!("the default is `{default_text}`")));
        }
        // The files' directives (a magic comment's encoding or frozen
        // strings) decide what the same literal means.
        let directives = [tb, ta, def_b.1, def_a.1].map(directives);
        if directives.iter().any(|d| *d != directives[0]) {
            return Err(fail(
                "the files' magic comments differ, and with them what the literal means".into(),
            ));
        }
        let mut assumptions: Vec<Assumption> = lookup.assumption.into_iter().collect();
        if [def_b.0, def_a.0].iter().any(|p| self.outside.library(p)) {
            assumptions.push(Assumption::PublicApi);
        }
        assumptions.sort_unstable();
        assumptions.dedup();
        Ok(Proof {
            method,
            assumptions,
            searched: lookup.searched,
        })
    }

    /// Searches the repository outside the change for `method`'s
    /// definitions, once per name.
    fn lookup(&mut self, path: &str, method: &str, suffixes: &[char]) -> Result<(), String> {
        if !self.found.contains_key(method) {
            let found = self.search(path, method, suffixes);
            self.found.insert(method.to_string(), found);
        }
        self.found[method]
            .as_ref()
            .map(|_| ())
            .map_err(Clone::clone)
    }

    fn search(&self, path: &str, method: &str, suffixes: &[char]) -> Result<Lookup, String> {
        let word = method.trim_end_matches(|c| suffixes.contains(&c));
        let mut lookup = Lookup {
            defs: Vec::new(),
            assumption: None,
            searched: None,
        };
        match self.outside.uses(path, word) {
            Search::Done {
                searched,
                found,
                mentioned,
            } => {
                lookup.searched = Some(searched);
                if !mentioned.is_empty() {
                    lookup.assumption = Some(Assumption::UnseenUses);
                }
                for file in found.iter().filter(|f| !self.covered.contains(f.as_str())) {
                    let tree = (self.outside.parse(file))
                        .filter(|tree| tree.errors.is_empty())
                        .ok_or_else(|| {
                            format!("`{method}` is named in {file}, which can't be parsed")
                        })?;
                    let defs = scan(&tree, method).map_err(|why| format!("{why} in {file}"))?;
                    if let [def] = defs[..] {
                        lookup.defs.push((file.clone(), tree, def));
                    } else if !defs.is_empty() {
                        return Err(format!("`{method}` is defined more than once in {file}"));
                    }
                }
            }
            Search::Capped => lookup.assumption = Some(Assumption::SearchCapped),
            Search::Unavailable => lookup.assumption = Some(Assumption::UnseenUses),
        }
        Ok(lookup)
    }
}

/// `method`'s definitions in `tree`; an error when anything else could
/// name or define it: a dynamic name (`send(:m)`, `define_method`), a plain
/// literal (`:m`), a reference. Calls to it are fine.
fn scan(tree: &Tree, method: &str) -> Result<Vec<NodeId>, &'static str> {
    let mut defs = Vec::new();
    for id in tree.descendants(Tree::ROOT) {
        match proof::names_method(tree, id, method) {
            None | Some(Naming::Call) => {}
            Some(Naming::Definition) => defs.push(id),
            Some(Naming::Literal) => return Err("a literal names it"),
            Some(Naming::Directive) => return Err("a directive names it"),
            Some(Naming::Computed) => return Err("a computed name could spell it"),
            Some(Naming::Reference) => return Err("something besides a call names it"),
        }
    }
    Ok(defs)
}

/// The class or module body definition `def` sits directly in, and its
/// receiver.
fn home_of_def(tree: &Tree, frontend: &dyn Frontend, def: NodeId) -> Option<Home> {
    let container = tree
        .ancestors(def)
        .find(|&p| frontend.rebinds_self(tree, p))
        .filter(|&c| {
            matches!(
                tree.node(c).role,
                Role::Definition(NameKind::Class | NameKind::Module)
            )
        })?;
    // Only statement lists and a visibility call (`private def x`) between:
    // a definition under a condition may not exist, and the call then
    // reaches an inherited method.
    let plain = |p: NodeId| {
        let node = tree.node(p);
        let visibility = |id: NodeId| matches!(tree.node(id).role, Role::Visibility(_));
        super::align::transparent(tree, p)
            || visibility(p)
            || (node.flags.contains(Flags::ARGUMENTS) && node.parent.is_some_and(visibility))
    };
    if !tree
        .ancestors(def)
        .take_while(|&p| p != container)
        .all(plain)
    {
        return None;
    }
    let path = tree.path(container);
    Some(Home {
        container: path
            .iter()
            .map(|s| (s.kind, s.name.map(str::to_string), s.header))
            .collect(),
        receiver: receiver(tree, def),
    })
}

/// The tree's directives (comments the language reads as code), sorted.
fn directives(tree: &Tree) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = (tree.nodes.iter())
        .filter(|n| n.flags.contains(Flags::DIRECTIVE))
        .map(|n| tree.text(&n.span).to_vec())
        .collect();
    out.sort();
    out
}
