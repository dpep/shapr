//! `rename`: a hunk's two sides align node for node and differ only in
//! identifiers, every use of each old name in its scope became the new name,
//! and the new name wasn't there before.
//!
//! A local's scope is the nearest scope boundary its frontend marks
//! (`Tree::scope`: a method, class or module); a field's, its
//! file; a member's (a field any file reaches), a method's or a constant's,
//! every file of the change. These can be used in code shapr never sees, so
//! their confidence is graded down. An opaque name (in a macro's tokens) may
//! be any of them.

use super::align::{Alignments, Cover, Diff};
use super::hazard::{Op, blocks};
use super::proof::{self, Assumption, Search, Verified};
use super::site::{self, CodeLines};
use super::{Claim, Confidence, Kind, Labels, Outside, Part};
use crate::change::{Change, Trees};
use crate::tree::{Flags, Hazard, NameKind, NodeId, Role, Tree, spells};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    Local,
    Field,
    /// A field any file can reach: scoped like a method.
    Member,
    Method,
    Constant,
}

impl Scope {
    fn of(role: Role) -> Option<Scope> {
        use NameKind::*;
        match role {
            Role::Bind
            | Role::Definition(Variable | Parameter | KeywordParameter | BlockParameter)
            | Role::Reference(Variable | Parameter | KeywordParameter | BlockParameter) => {
                Some(Scope::Local)
            }
            Role::Definition(Field) | Role::Reference(Field) => Some(Scope::Field),
            Role::Definition(Member) | Role::Reference(Member) => Some(Scope::Member),
            Role::Definition(Method) | Role::Reference(Method) | Role::Call(_) => {
                Some(Scope::Method)
            }
            Role::Definition(Class | Module | Constant)
            | Role::Reference(Class | Module | Constant) => Some(Scope::Constant),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Scope::Local => "local",
            Scope::Field | Scope::Member => "field",
            Scope::Method => "method",
            Scope::Constant => "constant",
        }
    }

    /// Uses can live in code shapr never parsed: other files, subclasses.
    fn unseen(self) -> bool {
        self != Scope::Local
    }
}

/// Where a name lives: for a local, its file and enclosing method or class
/// (the before side's node); for a field, its file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Space {
    scope: Scope,
    file: Option<usize>,
    gate: Option<NodeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pair {
    space: Space,
    from: String,
    to: String,
}

struct Hunk<'a> {
    file: usize,
    hunk: usize,
    alignment: &'a Cover,
}

#[derive(Default)]
struct Facts {
    renamed: usize,
    defined: bool,
    /// Hazards in a renamed method's own body that stop its rename.
    hazards: BTreeSet<Hazard>,
    /// A renamed definition's name is visible beyond code.
    exposed: bool,
    /// A method name the language reaches without a visible call.
    reserved: Option<String>,
    /// A renamed class or module, which an autoloader may find by name.
    typed: bool,
    /// A renamed module whose body is its own file (Rust's `mod n;`).
    file_module: bool,
    after_gates: BTreeSet<NodeId>,
    /// The renamed nodes on the after side, by file.
    after_nodes: BTreeSet<(usize, NodeId)>,
    /// The renamed definitions on the before side, by file.
    definitions: Vec<(usize, NodeId)>,
    /// Why a use under the same names can't follow (a member, a macro).
    blocked: Option<String>,
}

type Verdicts<'f> = BTreeMap<&'f Pair, Result<Verdict, String>>;

struct Verdict {
    renamed: usize,
    uses: usize,
    score: f64,
    /// What the rename relies on that the search couldn't settle.
    assumptions: BTreeSet<Assumption>,
    /// Files searched for uses outside the change; `None` for a local.
    searched: Option<usize>,
}

pub(super) fn run(
    change: &Change,
    alignments: &Alignments,
    outside: &dyn Outside,
    labels: &mut Labels,
) {
    let hunks = aligned(alignments, labels);
    let trees = |f: usize| {
        change.files[f]
            .trees
            .as_ref()
            .expect("aligned hunks are parsed")
    };

    // Every rename the change makes, gathered across hunks and files.
    let mut facts: BTreeMap<Pair, Facts> = BTreeMap::new();
    // Diffs no rename passes through, as (local space, from, to, why).
    let mut blocked: Vec<(Space, String, String, String)> = Vec::new();
    for hunk in &hunks {
        let t = trees(hunk.file);
        for diff in &hunk.alignment.diffs {
            if let Some(why) = stopped(t, diff) {
                let name = |tree: &Tree, id| {
                    tree.node(id)
                        .name
                        .as_deref()
                        .unwrap_or_default()
                        .to_string()
                };
                let local = Space {
                    scope: Scope::Local,
                    file: Some(hunk.file),
                    gate: Some(t.before.scope(diff.before)),
                };
                blocked.push((
                    local,
                    name(&t.before, diff.before),
                    name(&t.after, diff.after),
                    why,
                ));
            }
            let Ok(pair) = pair_of(t, hunk.file, diff) else {
                continue;
            };
            let reserved = [(&t.before, &pair.from), (&t.after, &pair.to)]
                .into_iter()
                .find(|(tree, name)| {
                    pair.space.scope == Scope::Method && tree.semantics.reserves(name)
                })
                .map(|(_, name)| name.clone());
            let entry = facts.entry(pair).or_default();
            entry.reserved = entry.reserved.take().or(reserved);
            let role = t.before.node(diff.before).role;
            entry.renamed += usize::from(!is_name_node(&t.before, diff.before));
            entry.typed |= matches!(role, Role::Definition(NameKind::Class | NameKind::Module));
            entry.file_module |= role == Role::Definition(NameKind::Module)
                && t.before.semantics.file_scoped
                && t.before.code_children(diff.before).count() == 1;
            if role.defines().is_some() {
                entry.defined = true;
                entry.definitions.push((hunk.file, diff.before));
            }
            if role == Role::Definition(NameKind::Method) {
                entry.hazards.extend(own_hazards(&t.before, diff.before));
            }
            entry.exposed |= role.is_declaration() && exposed(&t.before, diff.before);
            entry.after_gates.insert(t.after.scope(diff.after));
            entry.after_nodes.insert((hunk.file, diff.after));
        }
    }
    // A use that can't follow leaves the rename partial, so none of it holds.
    for (pair, f) in facts.iter_mut() {
        f.blocked = blocked
            .iter()
            .find(|(local, from, to, _)| {
                *from == pair.from
                    && *to == pair.to
                    && (pair.space.scope != Scope::Local || pair.space == *local)
            })
            .map(|(.., why)| why.clone());
    }
    let verdicts: Verdicts = facts
        .iter()
        .map(|(pair, f)| (pair, judge(change, pair, f, &facts, outside)))
        .collect();

    for hunk in &hunks {
        let t = trees(hunk.file);
        let label = &mut labels[hunk.file][hunk.hunk];
        let changed = label.changed.clone();
        let (accepted, rejected): (Vec<_>, Vec<_>) = hunk
            .alignment
            .diffs
            .iter()
            .map(|diff| (diff, judged(t, hunk.file, diff, &verdicts)))
            .partition(|(_, verdict)| verdict.is_ok());
        if let Some((_, Err(why))) = rejected.first() {
            label.residual.reason = why.clone();
        }
        // A line holding code that differs, or a rename that fails, is logic.
        let code = (CodeLines::of(&t.before), CodeLines::of(&t.after));
        let lines = |diffs: &[(&Diff, _)]| -> Claim {
            let own = |code: &CodeLines, tree: &Tree, lines, side: fn(&Diff) -> NodeId| {
                let own: BTreeSet<usize> = diffs
                    .iter()
                    .flat_map(|(d, _)| tree.own_offsets(side(d)))
                    .collect();
                code.touched(lines, |at| own.contains(&at))
            };
            Claim {
                before: own(&code.0, &t.before, &changed.before, |d| d.before),
                after: own(&code.1, &t.after, &changed.after, |d| d.after),
            }
        };
        let (b, a) = &hunk.alignment.changed();
        let differs = Claim {
            before: code.0.touched(&changed.before, |at| site::in_any(b, at)),
            after: code.1.touched(&changed.after, |at| site::in_any(a, at)),
        };
        let claimed = lines(&accepted).minus(&lines(&rejected)).minus(&differs);
        if claimed.is_empty() {
            continue;
        }
        let pairs: BTreeMap<&Pair, &Verdict> = accepted
            .into_iter()
            .filter_map(|(_, verdict)| verdict.ok())
            .collect();
        let score = pairs.values().map(|v| v.score).fold(1.0, f64::min);
        let renamed: usize = pairs.values().map(|v| v.renamed).sum();
        let uses: usize = pairs.values().map(|v| v.uses).sum();
        let assumptions: BTreeSet<Assumption> = pairs
            .values()
            .flat_map(|v| v.assumptions.iter().copied())
            .collect();
        let searched = pairs.values().filter_map(|v| v.searched).max();
        let renames: Vec<Value> = pairs
            .iter()
            .map(|(p, v)| {
                json!({
                    "from": p.from,
                    "to": p.to,
                    "scope": p.space.scope.as_str(),
                    "occurrences": v.renamed,
                })
            })
            .collect();
        let mut evidence = label.residual.evidence.clone();
        let values = &mut evidence.values;
        values.insert("renamed".into(), renamed.into());
        values.insert("uses".into(), uses.into());
        values.insert("searched".into(), searched.into());
        let reason = if assumptions.contains(&Assumption::PublicApi) {
            "public API rename: callers outside this repository can break"
        } else if assumptions.is_empty() {
            "only identifiers change, consistently in their scope"
        } else {
            "only identifiers change, but uses outside the change weren't checked"
        };
        // Not proven: every line is read.
        let review = if assumptions.is_empty() {
            Claim::default()
        } else {
            Claim {
                before: code.0.touched(&claimed.before, |_| true),
                after: code.1.touched(&claimed.after, |_| true),
            }
        };
        label.claim(Part {
            kind: Kind::Rename,
            context: Vec::new(),
            assumptions: assumptions.into_iter().collect(),
            lines: claimed,
            review,
            confidence: Confidence::new(score),
            reason: reason.into(),
            evidence,
            detail: Map::from_iter([("renames".to_string(), Value::Array(renames))]),
            variants: Vec::new(),
        });
    }
}

/// A name a rename may change.
pub(super) fn renamable(role: Role) -> bool {
    Scope::of(role).is_some() || matches!(role, Role::DynamicName | Role::Opaque)
}

/// Open hunks of cleanly parsed files whose covers align where names differ.
fn aligned<'a>(alignments: &'a Alignments, labels: &Labels) -> Vec<Hunk<'a>> {
    let mut out = Vec::new();
    for (file, aligned) in alignments.files() {
        for (hunk, (alignment, label)) in aligned.covers.iter().zip(&labels[file]).enumerate() {
            if label.unclaimed().is_empty() {
                continue;
            }
            if !alignment.diffs.is_empty() {
                out.push(Hunk {
                    file,
                    hunk,
                    alignment,
                });
            }
        }
    }
    out
}

/// The rename a diff makes, or why it isn't one shapr can vouch for.
fn pair_of(trees: &Trees, file: usize, diff: &Diff) -> Result<Pair, String> {
    let node = trees.before.node(diff.before);
    let from = node.name.as_deref().unwrap_or_default().to_string();
    let to = trees
        .after
        .node(diff.after)
        .name
        .as_deref()
        .unwrap_or_default()
        .to_string();
    let scope = match node.role {
        Role::Definition(NameKind::KeywordParameter) => {
            return Err(format!(
                "keyword parameter {from} becomes {to}, a name callers pass"
            ));
        }
        role => Scope::of(role).ok_or_else(|| format!("{from} becomes {to}"))?,
    };
    let space = Space {
        scope,
        file: matches!(scope, Scope::Local | Scope::Field).then_some(file),
        gate: (scope == Scope::Local).then(|| trees.before.scope(diff.before)),
    };
    Ok(Pair { space, from, to })
}

/// The rename a diff makes and its verdict, or why it stays logic.
fn judged<'v>(
    trees: &Trees,
    file: usize,
    diff: &Diff,
    verdicts: &'v Verdicts,
) -> Result<(&'v Pair, &'v Verdict), String> {
    if let Some(why) = stopped(trees, diff) {
        return Err(why);
    }
    let node = trees.before.node(diff.before);
    let (pair, verdict) = if matches!(node.role, Role::DynamicName | Role::Opaque) {
        // Accepted when it follows a rename made elsewhere in the change;
        // an opaque name may follow a local's, in the same scope.
        let from = node.name.as_deref().unwrap_or_default();
        let to = trees.after.node(diff.after).name.as_deref();
        let local = Space {
            scope: Scope::Local,
            file: Some(file),
            gate: Some(trees.before.scope(diff.before)),
        };
        let backing = verdicts.iter().find(|(p, v)| {
            let reaches = match p.space.scope {
                Scope::Local => node.role == Role::Opaque && p.space == local,
                _ => true,
            };
            reaches && p.from == from && Some(p.to.as_str()) == to && v.is_ok()
        });
        let Some((pair, verdict)) = backing else {
            return Err(format!(
                "the dynamic reference {from} changes, matching no rename"
            ));
        };
        (*pair, verdict)
    } else {
        let pair = pair_of(trees, file, diff)?;
        let (pair, verdict) = verdicts
            .get_key_value(&pair)
            .expect("every diff's pair was judged");
        (*pair, verdict)
    };
    match verdict {
        Ok(v) => Ok((pair, v)),
        Err(why) => Err(why.clone()),
    }
}

/// Why no rename passes through this diff: its name also names a member
/// (`S { a }`, `S { mut a }`, Ruby's `g(x:)`), or a macro may read it as written.
fn stopped(trees: &Trees, diff: &Diff) -> Option<String> {
    let tree = &trees.before;
    let node = tree.node(diff.before);
    let from = node.name.as_deref().unwrap_or_default();
    let member = |id: NodeId| tree.node(id).flags.contains(Flags::MEMBER);
    if member(diff.before) || node.parent.is_some_and(member) {
        let to = trees
            .after
            .node(diff.after)
            .name
            .as_deref()
            .unwrap_or_default();
        return Some(format!(
            "{from} also names a field or key, which becomes {to}"
        ));
    }
    let mut ancestors = tree.ancestors(diff.before);
    let spelled = ancestors.any(|a| tree.node(a).flags.contains(Flags::SPELLS));
    spelled.then(|| format!("a macro may read {from} as written, which changes"))
}

/// Whether a rename holds across its whole scope, and how strongly, or why
/// it isn't one: a call whose new name is a method of its own calls another
/// method.
fn judge(
    change: &Change,
    pair: &Pair,
    facts: &Facts,
    all: &BTreeMap<Pair, Facts>,
    outside: &dyn Outside,
) -> Result<Verdict, String> {
    if !facts.defined && pair.space.scope == Scope::Method && defined(change, pair, facts, outside)
    {
        return Err(format!("calls {} instead of {}", pair.to, pair.from));
    }
    holds(change, pair, facts, all, outside)
        .map_err(|why| format!("renames {} to {}, but {why}", pair.from, pair.to))
}

/// Whether method `pair.to` has a definition, in the change's after side or
/// in a file outside it.
fn defined(change: &Change, pair: &Pair, facts: &Facts, outside: &dyn Outside) -> bool {
    let defines = |tree: &Tree| {
        tree.descendants(Tree::ROOT).any(|id| {
            let n = tree.node(id);
            n.role == Role::Definition(NameKind::Method) && n.name.as_deref() == Some(&pair.to)
        })
    };
    let parsed = || change.files.iter().filter_map(|f| f.trees.as_ref().ok());
    if parsed().any(|t| defines(&t.after)) {
        return true;
    }
    let Some(&(home, _)) = facts.after_nodes.first() else {
        return false;
    };
    let after = &parsed_after(change, home);
    let suffixes = after.semantics.spelling.suffixes;
    let word = pair.to.trim_end_matches(|c| suffixes.contains(&c));
    let Search::Done { found, .. } = outside.uses(change.files[home].diff.path(), word) else {
        return false;
    };
    found
        .iter()
        .any(|file| outside.parse(file).is_some_and(|tree| defines(&tree)))
}

fn parsed_after(change: &Change, f: usize) -> &Tree {
    &change.files[f]
        .trees
        .as_ref()
        .expect("renamed in a parsed file")
        .after
}

/// Whether a rename holds across its whole scope, and how strongly.
fn holds(
    change: &Change,
    pair: &Pair,
    facts: &Facts,
    all: &BTreeMap<Pair, Facts>,
    outside: &dyn Outside,
) -> Result<Verdict, String> {
    let (from, to) = (&pair.from, &pair.to);
    for other in all.keys().filter(|o| o.space == pair.space && *o != pair) {
        if other.from == *from {
            return Err(format!("{from} also becomes {}", other.to));
        }
        if other.to == *to {
            return Err(format!("{} also becomes {to}", other.from));
        }
    }
    if !facts.defined {
        return Err(format!("{from}'s definition isn't renamed"));
    }
    if let Some(why) = &facts.blocked {
        return Err(why.clone());
    }
    if let Some(name) = &facts.reserved {
        return Err(format!(
            "{name} is a name the language calls without a visible call"
        ));
    }
    if facts.file_module {
        return Err(format!("{from} names a module whose body is its own file"));
    }
    if facts.typed && autoloads(change, facts, outside) {
        return Err(format!(
            "{from} names a class or module, which an autoloader finds by its file's name"
        ));
    }
    if facts.exposed {
        return Err(format!("an attribute may read the name {from}"));
    }
    if let Some(&hazard) = facts.hazards.first() {
        return Err(hazard_reason(hazard, from, to));
    }
    let scope = pair.space.scope;
    let (before, after) = roots(change, pair, facts);
    if scope == Scope::Local {
        let hazard = before.iter().chain(&after).find_map(|&(_, t, root)| {
            t.descendants(root)
                .filter_map(|d| t.node(d).role.hazard())
                .find(|&h| blocks(Op::RenameLocal, h))
        });
        if let Some(hazard) = hazard {
            return Err(hazard_reason(hazard, from, to));
        }
    }
    if after
        .iter()
        .any(|&(_, t, root)| conflicts(t, root, scope, from))
    {
        return Err(format!("{from} is still used"));
    }
    if before
        .iter()
        .any(|&(_, t, root)| conflicts(t, root, scope, to))
    {
        return Err(format!("{to} already exists"));
    }
    // Every definition of the new name is a renamed one: new code defining
    // it too would share the name with the renamed code.
    let defined_anew = after.iter().any(|&(f, t, root)| {
        t.descendants(root).any(|id| {
            let n = t.node(id);
            n.name.as_deref() == Some(to.as_str())
                && n.role.defines().is_some()
                && Scope::of(n.role) == Some(scope)
                && !facts.after_nodes.contains(&(f, id))
        })
    });
    if defined_anew {
        return Err(format!("{to} is also defined by new code"));
    }
    let uses: usize = before
        .iter()
        .map(|&(_, t, root)| uses(t, root, scope, from))
        .sum();
    let renamed = facts.renamed;
    // Every use of a local is in sight: one not renamed became other code.
    if scope == Scope::Local && renamed < uses {
        return Err(format!("{} of {from}'s {uses} uses become {to}", renamed));
    }
    let (assumptions, searched) = reach(change, pair, facts, outside)?;
    let uses = uses.max(renamed);
    // Every visible use is renamed; uses shapr can't see are smoothed in as
    // one unknown (Laplace's rule of succession).
    let score = if scope.unseen() {
        (renamed + 1) as f64 / (uses + 2) as f64
    } else {
        renamed as f64 / uses.max(1) as f64
    };
    Ok(Verdict {
        renamed,
        uses,
        score,
        assumptions,
        searched,
    })
}

/// The repository loads classes by their file's names (Zeitwerk, a Rails
/// application), in a language whose files load by statements: a renamed
/// class in an unrenamed file isn't found.
fn autoloads(change: &Change, facts: &Facts, outside: &dyn Outside) -> bool {
    let Some(&(home, _)) = facts.definitions.first() else {
        return false;
    };
    let file = &change.files[home];
    let Ok(trees) = &file.trees else {
        return false;
    };
    if trees.before.semantics.file_scoped {
        return false;
    }
    let zeitwerk = match outside.uses(file.diff.path(), "Zeitwerk") {
        proof::Search::Done {
            found, mentioned, ..
        } => !found.is_empty() || !mentioned.is_empty(),
        _ => false,
    };
    zeitwerk || outside.read("config/application.rb").is_some()
}

/// For a name code outside the change can reach: fails when a file outside
/// the change still uses the old name at the after revision; otherwise what
/// stays assumed (a search that couldn't run, a library's public surface)
/// and how many files were searched.
fn reach(
    change: &Change,
    pair: &Pair,
    facts: &Facts,
    outside: &dyn Outside,
) -> Result<(BTreeSet<Assumption>, Option<usize>), String> {
    let mut assumptions = BTreeSet::new();
    let scope = pair.space.scope;
    let Some(&(home, _)) = facts.definitions.first() else {
        return Ok((assumptions, None));
    };
    if !scope.unseen() {
        return Ok((assumptions, None));
    }
    let file = |f: usize| &change.files[f];
    let before = |f: usize| {
        &file(f)
            .trees
            .as_ref()
            .expect("renamed in a parsed file")
            .before
    };
    let paths = |f: usize| {
        let diff = &file(f).diff;
        [diff.old_path.as_deref(), diff.new_path.as_deref()]
            .into_iter()
            .flatten()
    };
    // Files whose trees the scope already covered.
    let covered: BTreeSet<&str> = match pair.space.file {
        Some(f) => paths(f).collect(),
        None => (0..change.files.len())
            .filter(|&f| file(f).trees.is_ok())
            .flat_map(paths)
            .collect(),
    };
    // A method's callers may spell it without its suffix (`x.name = 1`).
    let suffixes = before(home).semantics.spelling.suffixes;
    let word = |name: &'_ str| match scope {
        Scope::Method => name.trim_end_matches(|c| suffixes.contains(&c)).to_string(),
        _ => name.to_string(),
    };
    let mut searched: Option<usize> = None;
    // The new name is held to the old one's rule: an unchanged file mentioning
    // it (a parent's def, a subclass's ivar, a call) would now bind to the
    // renamed code, or already bound elsewhere.
    for (name, found) in [(&pair.from, "is still used"), (&pair.to, "is already used")] {
        let mentions = |tree: &Tree| conflicts(tree, Tree::ROOT, scope, name);
        let path = file(home).diff.path();
        match proof::verify(outside, path, &word(name), &covered, &mentions) {
            Verified::Used(files) => {
                return Err(format!("{name} {found} in {}", proof::listing(&files)));
            }
            Verified::Open(assumption) => {
                assumptions.insert(assumption);
            }
            Verified::Clear { searched: n } => searched = Some(searched.unwrap_or(0).max(n)),
        }
    }
    // A field (`@x`) is never another package's to call.
    let public = scope != Scope::Field
        && facts
            .definitions
            .iter()
            .any(|&(f, id)| outside.library(file(f).diff.path()) && proof::public(before(f), id));
    if public {
        assumptions.insert(Assumption::PublicApi);
    }
    Ok((assumptions, searched))
}

/// Subtrees of one side, each with its file.
type Roots<'c> = Vec<(usize, &'c Tree, NodeId)>;

/// The subtrees a name's scope covers, before and after the change.
fn roots<'c>(change: &'c Change, pair: &Pair, facts: &Facts) -> (Roots<'c>, Roots<'c>) {
    let parsed = |f: usize| change.files[f].trees.as_ref().ok();
    match (pair.space.file, pair.space.gate) {
        (Some(f), Some(gate)) => {
            let t = parsed(f).expect("a local's file is parsed");
            let after = facts
                .after_gates
                .iter()
                .map(|&g| (f, &t.after, g))
                .collect();
            (vec![(f, &t.before, gate)], after)
        }
        (Some(f), None) => {
            let t = parsed(f).expect("a field's file is parsed");
            (
                vec![(f, &t.before, Tree::ROOT)],
                vec![(f, &t.after, Tree::ROOT)],
            )
        }
        _ => {
            let all: Vec<(usize, &Trees)> = (0..change.files.len())
                .filter_map(|f| parsed(f).map(|t| (f, t)))
                .collect();
            (
                all.iter()
                    .map(|&(f, t)| (f, &t.before, Tree::ROOT))
                    .collect(),
                all.iter()
                    .map(|&(f, t)| (f, &t.after, Tree::ROOT))
                    .collect(),
            )
        }
    }
}

/// Uses of `name` in its scope, each declaration's name counted once.
fn uses(tree: &Tree, root: NodeId, scope: Scope, name: &str) -> usize {
    tree.descendants(root)
        .filter(|&id| {
            let n = tree.node(id);
            n.name.as_deref() == Some(name)
                && Scope::of(n.role) == Some(scope)
                && !is_name_node(tree, id)
        })
        .count()
}

/// Anything in the scope that `name` could mean: a use, a bare call a local
/// would shadow, or a dynamic reference (for a field, an accessor of the
/// same name).
fn conflicts(tree: &Tree, root: NodeId, scope: Scope, name: &str) -> bool {
    let spelling = tree.semantics.spelling;
    let bare = |s: &str| spelling.bare(s).to_string();
    tree.descendants(root).any(|id| {
        let n = tree.node(id);
        let Some(found) = n.name.as_deref() else {
            return scope != Scope::Local
                && (literal_names(tree, id, &bare(name)) || directive_names(tree, id, name));
        };
        match n.role {
            // A computed name reaches methods (`send(n)`, `define_method`).
            Role::DynamicName if found.contains('*') => {
                scope == Scope::Method && spells(found, name)
            }
            Role::DynamicName if scope == Scope::Field => bare(found) == bare(name),
            Role::DynamicName => scope != Scope::Local && found == name,
            Role::Opaque => found == name,
            Role::Call(crate::tree::Receiver::Implicit) if scope == Scope::Local => found == name,
            role => found == name && Scope::of(role) == Some(scope),
        }
    })
}

/// Whether `tree` uses method `name` anywhere: a definition, call or
/// reference, a dynamic name that could spell it, or a plain literal.
pub(super) fn mentions_method(tree: &Tree, name: &str) -> bool {
    conflicts(tree, Tree::ROOT, Scope::Method, name)
}

/// A plain literal spelling `name` (`:name`, `"name"`, `name:`): DSLs such
/// as callbacks and `autoload` name methods and constants this way, through
/// calls no frontend can list.
pub(super) fn literal_names(tree: &Tree, id: NodeId, name: &str) -> bool {
    let n = tree.node(id);
    if n.role != Role::Other || n.comment || !n.children.is_empty() {
        return false;
    }
    let text = String::from_utf8_lossy(tree.text(&n.span));
    tree.semantics.spelling.literal_spells(&text, name)
}

/// A directive (a Rust doctest line) holding `name` as a word: its code isn't
/// parsed into the tree, so any mention may be a use.
fn directive_names(tree: &Tree, id: NodeId, name: &str) -> bool {
    let n = tree.node(id);
    if !n.flags.contains(Flags::DIRECTIVE) {
        return false;
    }
    let word = tree.semantics.spelling.bare(name);
    let word = word.trim_end_matches(tree.semantics.spelling.suffixes);
    String::from_utf8_lossy(tree.text(&n.span))
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| w == word)
}

/// A class, module or constant's name, which the tree also holds as a
/// reference under the declaration.
fn is_name_node(tree: &Tree, id: NodeId) -> bool {
    tree.node(id).parent.is_some_and(|p| {
        matches!(
            tree.node(p).role,
            Role::Definition(NameKind::Class | NameKind::Module | NameKind::Constant)
        ) && tree.name_child(p) == Some(id)
    })
}

/// Whether the definition, or one enclosing it, carries syntax that makes its
/// name visible beyond code.
fn exposed(tree: &Tree, def: NodeId) -> bool {
    let mut at = Some(def);
    while let Some(id) = at.filter(|&id| id != Tree::ROOT) {
        let node = tree.node(id);
        if node
            .children
            .iter()
            .any(|&c| tree.node(c).role == Role::Exposes)
        {
            return true;
        }
        // A Rust impl keeps its attributes in its header.
        if node.children.first().is_some_and(|&h| {
            tree.node(h)
                .children
                .iter()
                .any(|&c| tree.node(c).role == Role::Exposes)
        }) {
            return true;
        }
        at = node.parent;
    }
    false
}

/// Hazards in method `def`'s own body (not a nested definition's) that stop
/// renaming it.
fn own_hazards(tree: &Tree, def: NodeId) -> impl Iterator<Item = Hazard> + '_ {
    tree.descendants(def)
        .filter(move |&id| tree.scope(id) == def)
        .filter_map(|id| tree.node(id).role.hazard())
        .filter(|&h| blocks(Op::RenameMethod, h))
}

/// Why `hazard` stops renaming `from` to `to`.
fn hazard_reason(hazard: Hazard, from: &str, to: &str) -> String {
    match hazard {
        Hazard::Super => format!("its super call now reaches the parent's {to}"),
        Hazard::Frame => format!("it reads its own name (`__method__`), which becomes {to}"),
        Hazard::Binding => format!("its scope reads locals by name at runtime, {from} among them"),
        other => format!("it holds {other:?}, bound to {from}"),
    }
}
