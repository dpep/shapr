//! Proof status: what a part relies on that the change itself doesn't show.
//!
//! A part is proven structure only when its [`Assumption`]s are empty. A
//! detector whose operation reaches code outside the change (a method's
//! callers, a new method's name) checks it against the repository at the
//! after revision through [`Outside`]: remaining uses make the operation
//! logic; a search that couldn't run, or a library's public surface, leaves
//! an assumption on the part, so it counts as behavior.

use super::moves::visibility;
use crate::frontend;
use crate::tree::{Flags, NameKind, NodeId, Role, Tree, Visibility};
use serde::Serialize;
use std::collections::BTreeSet;

/// Something a part relies on and shapr hasn't verified. Closed vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Assumption {
    /// Code outside the change may use a name the part changes, removes or
    /// adds, and the repository couldn't be searched (no repository).
    UnseenUses,
    /// The repository holds more files than shapr searches: uses outside
    /// the change weren't checked.
    SearchCapped,
    /// The name is part of a library's public surface: callers outside this
    /// repository can break.
    PublicApi,
    /// A moved unit matched several others equally well, so which one it
    /// came from is a guess.
    AmbiguousMatch,
}

impl Assumption {
    pub const ALL: [Assumption; 4] = [
        Assumption::UnseenUses,
        Assumption::SearchCapped,
        Assumption::PublicApi,
        Assumption::AmbiguousMatch,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Assumption::UnseenUses => "unseen_uses",
            Assumption::SearchCapped => "search_capped",
            Assumption::PublicApi => "public_api",
            Assumption::AmbiguousMatch => "ambiguous_match",
        }
    }
}

/// What a search of the repository found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Search {
    /// `searched` files were read; `found` holds the word, sorted.
    Done { searched: usize, found: Vec<String> },
    /// Too many files to search.
    Capped,
    /// No repository to search, or the search failed.
    Unavailable,
}

/// The repository beyond the change, at its after revision.
pub trait Outside {
    /// Files holding `word` as a whole word, among those that can hold code
    /// reaching names in the language of the file at `path`.
    fn uses(&self, path: &str, word: &str) -> Search;

    /// The file at `path` is library code: other packages can depend on its
    /// public definitions.
    fn library(&self, path: &str) -> bool;

    /// The contents of the file at `path`.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

/// No repository: nothing can be verified.
pub struct Nowhere;

impl Outside for Nowhere {
    fn uses(&self, _: &str, _: &str) -> Search {
        Search::Unavailable
    }

    fn library(&self, _: &str) -> bool {
        false
    }

    fn read(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
}

/// The outcome of checking one name outside the change.
pub(super) enum Verified {
    /// No file outside the change uses it; `searched` files were read.
    Clear { searched: usize },
    /// These files outside the change use it.
    Used(Vec<String>),
    /// It couldn't be checked.
    Open(Assumption),
}

/// Checks `word` against the repository, ignoring the files in `covered`,
/// whose trees the detector already checked. A file the search finds is
/// parsed and held to `uses`, the detector's own rule for a use in a tree,
/// so the word in a comment or a sentence doesn't count; one shapr can't
/// parse cleanly (a template) counts as a use.
pub(super) fn verify(
    outside: &dyn Outside,
    path: &str,
    word: &str,
    covered: &BTreeSet<&str>,
    uses: &dyn Fn(&Tree) -> bool,
) -> Verified {
    let used_in = |file: &str| {
        let tree = frontend::for_path(file)
            .zip(outside.read(file))
            .map(|(frontend, source)| frontend.parse(source));
        tree.is_none_or(|tree| !tree.errors.is_empty() || uses(&tree))
    };
    match outside.uses(path, word) {
        Search::Done { searched, found } => {
            let used: Vec<String> = found
                .into_iter()
                .filter(|f| !covered.contains(f.as_str()) && used_in(f))
                .collect();
            if used.is_empty() {
                Verified::Clear { searched }
            } else {
                Verified::Used(used)
            }
        }
        Search::Capped => Verified::Open(Assumption::SearchCapped),
        Search::Unavailable => Verified::Open(Assumption::UnseenUses),
    }
}

/// Names files for a reason: the first, and how many more.
pub(super) fn listing(paths: &[String]) -> String {
    match paths {
        [] => String::new(),
        [one] => one.clone(),
        [first, rest @ ..] => format!(
            "{first} and {} more file{}",
            rest.len(),
            if rest.len() == 1 { "" } else { "s" }
        ),
    }
}

/// Whether definition `def` is visible to code outside its package: in Rust,
/// marked `pub` ([`Flags::PUBLIC`]); in Ruby, anything but a private method
/// (a bare `private` section above it, `private def`, or `private :name`). A
/// singleton method (`def self.x`) isn't affected by `private`.
pub(super) fn public(tree: &Tree, def: NodeId) -> bool {
    let node = tree.node(def);
    if tree.semantics.private_by_default {
        return node.flags.contains(Flags::PUBLIC);
    }
    if node.role != Role::Definition(NameKind::Method) || node.flags.contains(Flags::RECEIVER) {
        return true;
    }
    // `private def x`: the definition is the visibility call's argument.
    let mut at = node.parent;
    while let Some(id) = at {
        let n = tree.node(id);
        if let Role::Visibility(v) = n.role {
            return v != Visibility::Private;
        }
        if n.flags.contains(Flags::SCOPE) {
            break;
        }
        at = n.parent;
    }
    let Some(parent) = node.parent else {
        return true;
    };
    // `private :x`, before or after the definition; not a name inside a
    // `private def`'s body.
    let names = |c: NodeId| {
        let mut found = tree.descendants(c).map(|d| tree.node(d));
        !tree
            .descendants(c)
            .any(|d| tree.node(d).role.defines().is_some())
            && found.any(|n| n.role == Role::DynamicName && n.name == node.name)
    };
    let named = tree
        .node(parent)
        .children
        .iter()
        .find_map(|&c| match tree.node(c).role {
            Role::Visibility(v) if names(c) => Some(v),
            _ => None,
        });
    named.or_else(|| visibility(tree, def)) != Some(Visibility::Private)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{Frontend, Ruby};

    fn methods(source: &str) -> Vec<(String, bool)> {
        let tree = Ruby.parse(source.as_bytes().to_vec());
        tree.descendants(Tree::ROOT)
            .filter(|&id| tree.node(id).role == Role::Definition(NameKind::Method))
            .map(|id| {
                (
                    tree.node(id).name.as_deref().unwrap().to_string(),
                    public(&tree, id),
                )
            })
            .collect()
    }

    #[test]
    fn ruby_methods_are_public_unless_made_private() {
        let found = methods(
            "class A\n  def a; end\n  private def b; send(:a); end\n  def c; end\n  private :c\n  \
             protected\n  def d; end\n  private\n  def e; end\n  def self.f; end\nend\n",
        );
        let expected = [
            ("a", true),
            ("b", false),
            ("c", false),
            ("d", true),
            ("e", false),
            ("f", true),
        ];
        let found: Vec<(&str, bool)> = found.iter().map(|(n, p)| (n.as_str(), *p)).collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn listing_names_the_first_file_and_counts_the_rest() {
        let paths = |n: usize| (0..n).map(|i| format!("f{i}.rb")).collect::<Vec<_>>();
        assert_eq!(listing(&paths(1)), "f0.rb");
        assert_eq!(listing(&paths(2)), "f0.rb and 1 more file");
        assert_eq!(listing(&paths(4)), "f0.rb and 3 more files");
    }

    #[test]
    fn uses_in_covered_files_are_not_outside() {
        struct Canned;
        impl Outside for Canned {
            fn uses(&self, _: &str, _: &str) -> Search {
                Search::Done {
                    searched: 3,
                    found: vec!["a.rb".into(), "b.rb".into()],
                }
            }
            fn library(&self, _: &str) -> bool {
                false
            }
            fn read(&self, _: &str) -> Option<Vec<u8>> {
                Some(b"x\n".to_vec())
            }
        }
        let covered = BTreeSet::from(["a.rb", "b.rb"]);
        assert!(matches!(
            verify(&Canned, "a.rb", "x", &covered, &|_| true),
            Verified::Clear { searched: 3 }
        ));
        let covered = BTreeSet::from(["a.rb"]);
        assert!(matches!(
            verify(&Canned, "a.rb", "x", &covered, &|_| true),
            Verified::Used(u) if u == ["b.rb"]
        ));
        assert!(matches!(
            verify(&Nowhere, "a.rb", "x", &covered, &|_| true),
            Verified::Open(Assumption::UnseenUses)
        ));
    }
}
