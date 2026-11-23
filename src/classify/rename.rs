//! `rename`: a hunk's two sides align node for node and differ only in
//! identifiers, every use of each old name in its scope became the new name,
//! and the new name wasn't there before.
//!
//! A local's scope is its enclosing method, class or module; a field's, its
//! file; a method's or constant's, every file of the change. Fields, methods
//! and constants can be used in code shapr never sees, so their confidence
//! is graded down.

use super::{Confidence, Kind, Labels};
use crate::change::{Change, Trees};
use crate::tree::{NameKind, NodeId, Role, Tree};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    Local,
    Field,
    Method,
    Constant,
}

impl Scope {
    fn of(role: Role) -> Option<Scope> {
        use NameKind::*;
        match role {
            Role::Definition(Variable | Parameter | KeywordParameter | BlockParameter)
            | Role::Reference(Variable | Parameter | KeywordParameter | BlockParameter) => {
                Some(Scope::Local)
            }
            Role::Definition(Field) | Role::Reference(Field) => Some(Scope::Field),
            Role::Definition(Method) | Role::Call(_) => Some(Scope::Method),
            Role::Definition(Class | Module | Constant)
            | Role::Reference(Class | Module | Constant) => Some(Scope::Constant),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Scope::Local => "local",
            Scope::Field => "field",
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

/// An aligned node whose name changed.
struct Diff {
    before: NodeId,
    after: NodeId,
}

struct Hunk {
    file: usize,
    hunk: usize,
    diffs: Vec<Diff>,
}

#[derive(Default)]
struct Facts {
    renamed: usize,
    defined: bool,
    calls_super: bool,
    after_gates: BTreeSet<NodeId>,
}

struct Verdict {
    renamed: usize,
    uses: usize,
    score: f64,
}

pub(super) fn run(change: &Change, labels: &mut Labels) {
    let hunks = aligned(change, labels);
    let trees = |f: usize| {
        change.files[f]
            .trees
            .as_ref()
            .expect("aligned hunks are parsed")
    };

    // Every rename the change makes, gathered across hunks and files.
    let mut facts: BTreeMap<Pair, Facts> = BTreeMap::new();
    for hunk in &hunks {
        let t = trees(hunk.file);
        for diff in &hunk.diffs {
            let Ok(pair) = pair_of(t, hunk.file, diff) else {
                continue;
            };
            let entry = facts.entry(pair).or_default();
            let role = t.before.node(diff.before).role;
            entry.renamed += usize::from(!is_name_node(&t.before, diff.before));
            entry.defined |= matches!(role, Role::Definition(_));
            entry.calls_super |=
                role == Role::Definition(NameKind::Method) && calls_super(&t.before, diff.before);
            entry.after_gates.insert(gate(&t.after, diff.after));
        }
    }
    let verdicts: BTreeMap<&Pair, Result<Verdict, String>> = facts
        .iter()
        .map(|(pair, f)| (pair, judge(change, pair, f, &facts)))
        .collect();

    for hunk in &hunks {
        let t = trees(hunk.file);
        let label = &mut labels[hunk.file][hunk.hunk];
        match hunk_pairs(t, hunk, &verdicts) {
            Err(why) => label.reason = why,
            Ok(pairs) => {
                let verdicts: Vec<&Verdict> = pairs.iter().map(|p| verdict(&verdicts, p)).collect();
                let score = verdicts.iter().map(|v| v.score).fold(1.0, f64::min);
                let renamed: usize = verdicts.iter().map(|v| v.renamed).sum();
                let uses: usize = verdicts.iter().map(|v| v.uses).sum();
                let unseen = pairs.iter().any(|p| p.space.scope.unseen());
                let renames: Vec<Value> = pairs
                    .iter()
                    .zip(&verdicts)
                    .map(|(p, v)| {
                        json!({
                            "from": p.from,
                            "to": p.to,
                            "kind": p.space.scope.as_str(),
                            "occurrences": v.renamed,
                        })
                    })
                    .collect();
                label.kind = Kind::Rename;
                label.confidence = Confidence::new(score);
                label.reason = "only identifiers change, consistently in their scope".into();
                let values = &mut label.evidence.values;
                values.insert("renamed".into(), renamed.into());
                values.insert("uses".into(), uses.into());
                values.insert("unseen_uses".into(), unseen.into());
                label.detail.insert("renames".into(), Value::Array(renames));
            }
        }
    }
}

fn verdict<'v>(verdicts: &'v BTreeMap<&Pair, Result<Verdict, String>>, pair: &Pair) -> &'v Verdict {
    match &verdicts[pair] {
        Ok(v) => v,
        Err(_) => unreachable!("hunk_pairs returns only accepted pairs"),
    }
}

/// Logic hunks of parsed files whose sides align, with the names that differ.
fn aligned(change: &Change, labels: &Labels) -> Vec<Hunk> {
    let mut out = Vec::new();
    for (file, (fc, labels)) in change.files.iter().zip(labels).enumerate() {
        let Ok(trees) = &fc.trees else { continue };
        if !trees.clean() {
            continue;
        }
        for (hunk, (site, label)) in trees.sites.iter().zip(labels).enumerate() {
            if label.kind != Kind::Logic {
                continue;
            }
            let (before, after) = (&site.before.cover, &site.after.cover);
            if before.is_empty() || before.len() != after.len() {
                continue;
            }
            let mut diffs = Vec::new();
            let all = before
                .iter()
                .zip(after)
                .all(|(&b, &a)| align(&trees.before, b, &trees.after, a, &mut diffs));
            if all && !diffs.is_empty() {
                out.push(Hunk { file, hunk, diffs });
            }
        }
    }
    out
}

/// Same shape, same kinds and roles, same text but for names; collects the
/// nodes whose names differ. Comments are ignored.
fn align(b: &Tree, bi: NodeId, a: &Tree, ai: NodeId, diffs: &mut Vec<Diff>) -> bool {
    let (bn, an) = (b.node(bi), a.node(ai));
    if bn.kind != an.kind || bn.role != an.role {
        return false;
    }
    if bn.hash == an.hash {
        return true;
    }
    let code = |t: &Tree, id: NodeId| -> Vec<NodeId> {
        t.node(id)
            .children
            .iter()
            .copied()
            .filter(|&c| !t.node(c).comment)
            .collect()
    };
    let (bc, ac) = (code(b, bi), code(a, ai));
    if bc.len() != ac.len() || own_text(b, bi) != own_text(a, ai) {
        return false;
    }
    if bn.name != an.name {
        let renamable = Scope::of(bn.role).is_some() || bn.role == Role::DynamicName;
        let names = [&bn.name, &an.name].map(|n| n.as_deref().is_some_and(is_identifier));
        if !renamable || names.contains(&false) {
            return false;
        }
        diffs.push(Diff {
            before: bi,
            after: ai,
        });
    }
    bc.into_iter()
        .zip(ac)
        .all(|(b2, a2)| align(b, b2, a, a2, diffs))
}

/// The node's own source, outside its children, with whitespace dropped and
/// its name (where the text spells it) masked, so two nodes differing only
/// in name compare equal.
fn own_text(tree: &Tree, id: NodeId) -> Vec<u8> {
    let node = tree.node(id);
    let mut text = Vec::new();
    let mut cursor = node.span.start;
    for &child in &node.children {
        let c = tree.node(child);
        let end = c.extent.start.min(node.span.end);
        if cursor < end {
            text.extend_from_slice(&tree.source[cursor..end]);
        }
        if !c.comment {
            text.push(0x01);
        }
        cursor = cursor.max(c.extent.end);
    }
    if cursor < node.span.end {
        text.extend_from_slice(&tree.source[cursor..node.span.end]);
    }
    if let Some(name) = &node.name {
        mask(&mut text, name.as_bytes());
    }
    text.retain(|b| !b.is_ascii_whitespace());
    text
}

/// Replaces the first occurrence of `name` that stands as a whole token.
fn mask(text: &mut Vec<u8>, name: &[u8]) {
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let suffixed = name.last().is_some_and(|b| b"?!=".contains(b));
    let at = (0..text.len()).find(|&i| {
        text[i..].starts_with(name)
            && (i == 0 || !(word(text[i - 1]) || b"@$".contains(&text[i - 1])))
            && text
                .get(i + name.len())
                .is_none_or(|&n| !word(n) && (suffixed || !b"?!".contains(&n)))
    });
    if let Some(i) = at {
        text.splice(i..i + name.len(), [0]);
    }
}

fn is_identifier(name: &str) -> bool {
    let bare = name.trim_start_matches(['@', '$']);
    let bare = bare.strip_suffix(['?', '!', '=']).unwrap_or(bare);
    let mut chars = bare.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
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
        gate: (scope == Scope::Local).then(|| gate(&trees.before, diff.before)),
    };
    Ok(Pair { space, from, to })
}

/// The accepted renames a hunk makes, or why it stays logic.
fn hunk_pairs(
    trees: &Trees,
    hunk: &Hunk,
    verdicts: &BTreeMap<&Pair, Result<Verdict, String>>,
) -> Result<Vec<Pair>, String> {
    let mut pairs = BTreeSet::new();
    for diff in &hunk.diffs {
        let node = trees.before.node(diff.before);
        if node.role == Role::DynamicName {
            // Accepted when it follows a rename made elsewhere in the change.
            let from = node.name.as_deref().unwrap_or_default();
            let to = trees
                .after
                .node(diff.after)
                .name
                .as_deref()
                .unwrap_or_default();
            let backing = verdicts.iter().find(|(p, v)| {
                p.space.scope != Scope::Local && p.from == from && p.to == to && v.is_ok()
            });
            match backing {
                Some((p, _)) => pairs.insert((*p).clone()),
                None => {
                    return Err(format!(
                        "the dynamic reference {from} becomes {to}, matching no rename"
                    ));
                }
            };
            continue;
        }
        let pair = pair_of(trees, hunk.file, diff)?;
        if let Err(why) = &verdicts[&pair] {
            return Err(format!("renames {} to {}, but {why}", pair.from, pair.to));
        }
        pairs.insert(pair);
    }
    Ok(pairs.into_iter().collect())
}

/// Whether a rename holds across its whole scope, and how strongly.
fn judge(
    change: &Change,
    pair: &Pair,
    facts: &Facts,
    all: &BTreeMap<Pair, Facts>,
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
    if facts.calls_super {
        return Err(format!("its super call now reaches the parent's {to}"));
    }
    let scope = pair.space.scope;
    let (before, after) = roots(change, pair, facts);
    if after
        .iter()
        .any(|&(t, root)| conflicts(t, root, scope, from))
    {
        return Err(format!("{from} is still used"));
    }
    if before
        .iter()
        .any(|&(t, root)| conflicts(t, root, scope, to))
    {
        return Err(format!("{to} already exists"));
    }
    let uses: usize = before
        .iter()
        .map(|&(t, root)| uses(t, root, scope, from))
        .sum();
    let renamed = facts.renamed;
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
    })
}

type Roots<'c> = Vec<(&'c Tree, NodeId)>;

/// The subtrees a name's scope covers, before and after the change.
fn roots<'c>(change: &'c Change, pair: &Pair, facts: &Facts) -> (Roots<'c>, Roots<'c>) {
    let parsed = |f: usize| change.files[f].trees.as_ref().ok();
    match (pair.space.file, pair.space.gate) {
        (Some(f), Some(gate)) => {
            let t = parsed(f).expect("a local's file is parsed");
            let after = facts.after_gates.iter().map(|&g| (&t.after, g)).collect();
            (vec![(&t.before, gate)], after)
        }
        (Some(f), None) => {
            let t = parsed(f).expect("a field's file is parsed");
            (vec![(&t.before, Tree::ROOT)], vec![(&t.after, Tree::ROOT)])
        }
        _ => {
            let all: Vec<&Trees> = (0..change.files.len()).filter_map(parsed).collect();
            (
                all.iter().map(|t| (&t.before, Tree::ROOT)).collect(),
                all.iter().map(|t| (&t.after, Tree::ROOT)).collect(),
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
    let bare = |s: &str| s.trim_start_matches('@').to_string();
    tree.descendants(root).any(|id| {
        let n = tree.node(id);
        let Some(found) = n.name.as_deref() else {
            return scope != Scope::Local && literal_names(tree, id, &bare(name));
        };
        match n.role {
            Role::DynamicName if scope == Scope::Field => bare(found) == bare(name),
            Role::DynamicName => scope != Scope::Local && found == name,
            Role::Call(crate::tree::Receiver::Implicit) if scope == Scope::Local => found == name,
            role => found == name && Scope::of(role) == Some(scope),
        }
    })
}

/// A plain literal spelling `name` (`:name`, `"name"`, `name:`): DSLs such
/// as callbacks and `autoload` name methods and constants this way, through
/// calls no frontend can list.
fn literal_names(tree: &Tree, id: NodeId, name: &str) -> bool {
    let n = tree.node(id);
    if n.role != Role::Other || n.comment || !n.children.is_empty() {
        return false;
    }
    let text = String::from_utf8_lossy(tree.text(&n.span));
    let text = text.trim_start_matches(':').trim_start_matches('@');
    let text = text.trim_matches(['"', '\'']);
    text == name || text.strip_suffix(':') == Some(name)
}

/// A class, module or constant's name, which the tree also holds as a
/// constant reference under the declaration.
fn is_name_node(tree: &Tree, id: NodeId) -> bool {
    let node = tree.node(id);
    let Some(parent) = node.parent.map(|p| tree.node(p)) else {
        return false;
    };
    node.role == Role::Reference(NameKind::Constant)
        && matches!(
            parent.role,
            Role::Definition(NameKind::Class | NameKind::Module | NameKind::Constant)
        )
        && parent.name == node.name
        && parent.children.iter().find(|&&c| !tree.node(c).comment) == Some(&id)
}

/// The method, class or module enclosing `id`: where a local lives.
fn gate(tree: &Tree, id: NodeId) -> NodeId {
    let mut at = tree.node(id).parent;
    while let Some(p) = at {
        if matches!(
            tree.node(p).role,
            Role::Definition(NameKind::Method | NameKind::Class | NameKind::Module)
        ) {
            return p;
        }
        at = tree.node(p).parent;
    }
    Tree::ROOT
}

fn calls_super(tree: &Tree, def: NodeId) -> bool {
    tree.descendants(def).any(|id| {
        tree.node(id).role == Role::Jump(crate::tree::Jump::Super) && gate(tree, id) == def
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked(text: &str, name: &str) -> String {
        let mut bytes = text.as_bytes().to_vec();
        mask(&mut bytes, name.as_bytes());
        String::from_utf8_lossy(&bytes).replace('\0', "_")
    }

    #[test]
    fn mask_takes_whole_tokens_only() {
        assert_eq!(masked("def f(", "f"), "def _(");
        assert_eq!(masked("def de", "de"), "def _");
        assert_eq!(masked("x += ", "x"), "_ += ");
        assert_eq!(masked("x? x", "x"), "x? _");
        assert_eq!(masked(":@x", "@x"), ":_");
        assert_eq!(masked("@x", "x"), "@x");
    }

    #[test]
    fn identifiers_exclude_operators() {
        for ok in ["a", "@b", "@@c", "d?", "e=", "Foo", "_x1", "é"] {
            assert!(is_identifier(ok), "{ok}");
        }
        for bad in ["+", "[]", "<=>", "", "1a", "a b"] {
            assert!(!is_identifier(bad), "{bad}");
        }
    }
}
