//! When files load: the statements that load a file, in the change and in
//! the repository beyond it, and whether code moved into a new file still
//! runs where it ran before.

use super::crossing::window;
use super::{Unit, members};
use crate::change::Change;
use crate::classify::Outside;
use crate::classify::align::transparent;
use crate::classify::proof::Search;
use crate::frontend::{self, Load};
use crate::tree::{NodeId, Tree};
use std::collections::{BTreeSet, HashMap};

/// Words that may start loading a file by a name computed at runtime.
const LOADER_WORDS: &[&str] = &[
    "require",
    "require_relative",
    "load",
    "autoload",
    "Zeitwerk",
];

/// One statement loading a file, as [`Loading`] finds it.
struct Point {
    /// The index of the file in the change, when it is in it.
    file: Option<usize>,
    stmt: NodeId,
    load: Load,
    /// A statement of its own in a statement list, and so for the next one
    /// in the list.
    plain: bool,
    /// At the top level of its file.
    top: bool,
    /// The load in the statement before it and after it in its list.
    prev: Option<Load>,
    next: Option<Load>,
}

/// When files load, from the statements that load them: in the change's
/// after trees and in the repository beyond it, searched once per file.
pub(super) struct Loading<'c> {
    change: &'c Change,
    outside: &'c dyn Outside,
    hazard: Option<bool>,
    points: HashMap<String, Option<Vec<Point>>>,
}

impl<'c> Loading<'c> {
    pub(super) fn new(change: &'c Change, outside: &'c dyn Outside) -> Self {
        Loading {
            change,
            outside,
            hazard: None,
            points: HashMap::new(),
        }
    }

    fn covered(&self) -> BTreeSet<&'c str> {
        let paths = self.change.files.iter();
        paths
            .flat_map(|f| [f.diff.old_path.as_deref(), f.diff.new_path.as_deref()])
            .flatten()
            .collect()
    }

    /// Some file of the language loads files by a computed name, or one
    /// can't be read for it: any file may load at any time.
    fn hazard(&mut self, path: &str) -> bool {
        if let Some(hazard) = self.hazard {
            return hazard;
        }
        let computed = |tree: &Tree, file: &str| {
            let frontend = frontend::for_path(file);
            !tree.errors.is_empty()
                || frontend.is_some_and(|fe| {
                    tree.descendants(Tree::ROOT)
                        .any(|id| fe.load(tree, id, file) == Some(Load::Computed))
                })
        };
        let in_change = self
            .change
            .files
            .iter()
            .any(|f| match (&f.trees, &f.diff.new_path) {
                (Ok(t), Some(file))
                    if frontend::for_path(file).is_some_and(|fe| fe.language() == t.language) =>
                {
                    computed(&t.after, file)
                }
                _ => false,
            });
        let covered = self.covered();
        let hazard = in_change
            || LOADER_WORDS
                .iter()
                .any(|word| match self.outside.uses(path, word) {
                    Search::Done { found, .. } => found
                        .iter()
                        .filter(|f| !covered.contains(f.as_str()))
                        .any(|f| self.outside.parse(f).is_none_or(|tree| computed(&tree, f))),
                    _ => true,
                });
        self.hazard = Some(hazard);
        hazard
    }

    /// Every statement loading the file at `path`; `None` when some file
    /// that may hold one can't be read for it.
    fn points(&mut self, path: &str) -> Option<&[Point]> {
        if !self.points.contains_key(path) {
            let found = self.find(path);
            self.points.insert(path.to_string(), found);
        }
        self.points[path].as_deref()
    }

    fn find(&self, path: &str) -> Option<Vec<Point>> {
        let base = path.rsplit('/').next().unwrap_or(path);
        let stem = base.split('.').next().unwrap_or(base);
        let mut out = Vec::new();
        for (f, file) in self.change.files.iter().enumerate() {
            let (Ok(trees), Some(name)) = (&file.trees, &file.diff.new_path) else {
                continue;
            };
            let Some(fe) = frontend::for_path(name) else {
                continue;
            };
            if !trees.after.errors.is_empty() {
                return None;
            }
            out.extend(scan(fe, &trees.after, name, path).map(|p| Point { file: Some(f), ..p }));
        }
        let covered = self.covered();
        match self.outside.uses(path, stem) {
            Search::Done {
                found, mentioned, ..
            } if mentioned.is_empty() => {
                for name in found.iter().filter(|f| !covered.contains(f.as_str())) {
                    let fe = frontend::for_path(name)?;
                    let tree = self.outside.parse(name)?;
                    if !tree.errors.is_empty() {
                        return None;
                    }
                    out.extend(scan(fe, &tree, name, path));
                }
            }
            _ => return None,
        }
        Some(out)
    }

    /// Whether the new file holding `to` runs `from`'s code where it ran before,
    /// nothing in between depending on it: whether it runs after the old
    /// file's own statements, and whether other files load it (after the
    /// old one) rather than the old file itself; `None` when not shown.
    ///
    /// Shown two ways. The old file loads the new one (`require_relative`)
    /// at its top level, and every other load of the new file follows it or
    /// a load of the old file. Or every load of the new file directly
    /// follows a load of the old one, and every load of the old file is
    /// followed by one of the new: it runs right after the old file. Either
    /// way no file may load files by a computed name.
    pub(super) fn runs(&mut self, from: &Unit, to: &Unit) -> Option<(bool, bool)> {
        let change = self.change;
        let old = change.files[from.file].diff.new_path.as_deref()?;
        let new = change.files[to.file].diff.path();
        if self.hazard(new) {
            return None;
        }
        let is = |load: &Option<Load>, path: &str| matches!(load, Some(Load::Now(t)) if t.is(path));
        let old_points: Vec<(bool, bool)> = self
            .points(old)?
            .iter()
            .map(|p| (matches!(p.load, Load::Now(_)) && p.plain, is(&p.next, new)))
            .collect();
        let points = self.points(new)?;
        let first = points
            .iter()
            .filter(|p| p.file == Some(from.file) && p.top && matches!(p.load, Load::Now(_)))
            .min_by_key(|p| p.stmt.0);
        let attached = |p: &Point| matches!(p.load, Load::Now(_)) && p.plain && is(&p.prev, old);
        let trees = from.trees(change);
        let at = match first {
            Some(first) => {
                let rest = points
                    .iter()
                    .filter(|p| p.stmt != first.stmt || p.file != first.file);
                let fine = |p: &Point| attached(p) || (p.file == Some(from.file) && p.top);
                if !rest.into_iter().all(fine) {
                    return None;
                }
                let tops: Vec<NodeId> = members(&trees.after, Tree::ROOT).collect();
                Some(tops.iter().position(|&t| t == first.stmt)?)
            }
            None => {
                let loaded = !points.is_empty() && points.iter().all(attached);
                let follows =
                    !old_points.is_empty() && old_points.iter().all(|&(plain, next)| plain && next);
                if !loaded || !follows {
                    return None;
                }
                None
            }
        };
        let after = window(&trees.before, from.node, &trees.after, at)?;
        Some((after, at.is_none()))
    }

    /// The statements in the change that load the file `to`, which another
    /// file lost code to.
    pub(super) fn loaders(&mut self, to: usize) -> Vec<(usize, NodeId)> {
        let new = self.change.files[to].diff.path().to_string();
        let points = self.points(&new).unwrap_or_default();
        points
            .iter()
            .filter_map(|p| Some((p.file?, p.stmt)))
            .collect()
    }
}

/// The statements of `tree` (the file at `name`) that load the file at
/// `path`.
fn scan<'t>(
    fe: &'t dyn crate::frontend::Frontend,
    tree: &'t Tree,
    name: &'t str,
    path: &'t str,
) -> impl Iterator<Item = Point> + 't {
    tree.descendants(Tree::ROOT).filter_map(move |id| {
        let load = fe.load(tree, id, name)?;
        let (Load::Now(target) | Load::Other(target)) = &load else {
            return None;
        };
        if !target.is(path) {
            return None;
        }
        let parent = tree.node(id).parent?;
        let plain = transparent(tree, parent);
        let top = tree
            .ancestors(id)
            .all(|a| a == Tree::ROOT || transparent(tree, a));
        let siblings: Vec<NodeId> = tree.code_children(parent).collect();
        let at = siblings.iter().position(|&s| s == id)?;
        let near = |i: Option<usize>| {
            i.and_then(|i| siblings.get(i))
                .and_then(|&s| fe.load(tree, s, name))
                .filter(|_| plain)
        };
        Some(Point {
            file: None,
            stmt: id,
            load,
            plain,
            top,
            prev: near(at.checked_sub(1)),
            next: near(Some(at + 1)),
        })
    })
}
