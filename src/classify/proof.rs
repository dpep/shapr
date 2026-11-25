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
use crate::tree::{Flags, NameKind, NodeId, Role, Tree, Visibility, spells};
use serde::Serialize;
use std::collections::BTreeSet;
use std::rc::Rc;

/// Something a part relies on and shapr hasn't verified. Closed vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Assumption {
    /// Code outside the change may use a name the part changes, removes or
    /// adds, and the repository couldn't be searched (no repository), or
    /// holds the name in text shapr can't parse for a use (YAML, JavaScript).
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
    /// A name computed at runtime (`send(n)`) could spell the method a
    /// part adds or removes, so a call it makes may reach it.
    ComputedName,
    /// The part changes only type annotations, and a runtime may check them
    /// (sorbet-runtime): a value they reject raises.
    RuntimeCheck,
}

impl Assumption {
    pub const ALL: [Assumption; 6] = [
        Assumption::UnseenUses,
        Assumption::SearchCapped,
        Assumption::PublicApi,
        Assumption::AmbiguousMatch,
        Assumption::ComputedName,
        Assumption::RuntimeCheck,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Assumption::UnseenUses => "unseen_uses",
            Assumption::SearchCapped => "search_capped",
            Assumption::PublicApi => "public_api",
            Assumption::AmbiguousMatch => "ambiguous_match",
            Assumption::ComputedName => "computed_name",
            Assumption::RuntimeCheck => "runtime_check",
        }
    }
}

/// What a search of the repository found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Search {
    /// `searched` files were read; `found` holds the word, sorted;
    /// `mentioned`, files of no language shapr reads (YAML, JavaScript) that
    /// hold it as a word.
    Done {
        searched: usize,
        found: Vec<String>,
        mentioned: Vec<String>,
    },
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

    /// [`Outside::uses`] for files holding `word` as written, without the
    /// files whose computed names could spell it.
    fn literal(&self, path: &str, word: &str) -> Search;

    /// The file at `path` is library code: other packages can depend on its
    /// public definitions.
    fn library(&self, path: &str) -> bool;

    /// The contents of the file at `path`.
    fn read(&self, path: &str) -> Option<Vec<u8>>;

    /// A framework calls the public methods defined in the file at `path`
    /// by their names ([`Frontend::called_by_name`]).
    ///
    /// [`Frontend::called_by_name`]: crate::frontend::Frontend::called_by_name
    fn called_by_name(&self, path: &str) -> bool;

    /// The file at `path` parsed by its language's frontend; `None` for a
    /// file of no language shapr reads, or one it can't read.
    fn parse(&self, path: &str) -> Option<Rc<Tree>> {
        let frontend = frontend::for_path(path)?;
        Some(Rc::new(frontend.parse(self.read(path)?)))
    }
}

/// No repository: nothing can be verified.
pub struct Nowhere;

impl Outside for Nowhere {
    fn uses(&self, _: &str, _: &str) -> Search {
        Search::Unavailable
    }

    fn literal(&self, _: &str, _: &str) -> Search {
        Search::Unavailable
    }

    fn library(&self, _: &str) -> bool {
        false
    }

    fn read(&self, _: &str) -> Option<Vec<u8>> {
        None
    }

    fn called_by_name(&self, _: &str) -> bool {
        false
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
        let tree = outside.parse(file);
        tree.is_none_or(|tree| !tree.errors.is_empty() || uses(&tree))
    };
    match outside.uses(path, word) {
        Search::Done {
            searched,
            found,
            mentioned,
        } => {
            let used: Vec<String> = found
                .into_iter()
                .filter(|f| !covered.contains(f.as_str()) && used_in(f))
                .collect();
            if !used.is_empty() {
                Verified::Used(used)
            } else if !mentioned.is_empty() {
                // Text code may read (config, templates, another language)
                // can't be parsed for a use.
                Verified::Open(Assumption::UnseenUses)
            } else {
                Verified::Clear { searched }
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

/// How a node can name a method: what every pass that must see all of a
/// method's uses (rename, extract and inline, default_argument) reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Naming {
    /// Defines it.
    Definition,
    /// Calls it.
    Call,
    /// Names it some other way: a reference (`method(:m)`), a dynamic name
    /// spelled out (`send(:m)`, `alias`), a name in unparsed tokens.
    Reference,
    /// A name computed at runtime that could spell it (`send("m_#{x}")`).
    Computed,
    /// A plain literal spelling it ([`literal_names`]).
    Literal,
    /// A directive holding it as a word ([`directive_names`]).
    Directive,
}

/// How node `id` could name method `name`, if it could.
pub(super) fn names_method(tree: &Tree, id: NodeId, name: &str) -> Option<Naming> {
    let n = tree.node(id);
    let Some(found) = n.name.as_deref() else {
        if literal_names(tree, id, tree.semantics.spelling.bare(name)) {
            return Some(Naming::Literal);
        }
        return directive_names(tree, id, name).then_some(Naming::Directive);
    };
    match n.role {
        // A computed constant or field name (`const_get(n)`,
        // `instance_variable_get(n)`) can't reach a method.
        Role::DynamicName if found.contains('*') => (!n.flags.contains(Flags::CONSTANT)
            && !n.flags.contains(Flags::FIELD)
            && spells(found, name))
        .then_some(Naming::Computed),
        _ if found != name => None,
        Role::DynamicName | Role::Opaque | Role::Reference(NameKind::Method) => {
            Some(Naming::Reference)
        }
        Role::Definition(NameKind::Method) => Some(Naming::Definition),
        Role::Call(_) => Some(Naming::Call),
        _ => None,
    }
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
pub(super) fn directive_names(tree: &Tree, id: NodeId, name: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALL` holds the variants in declaration order. The match is
    /// exhaustive, so a new variant fails to compile here: add it to `ALL`.
    #[test]
    fn all_holds_every_assumption() {
        for (i, &a) in Assumption::ALL.iter().enumerate() {
            assert_eq!(a as usize, i, "{a:?}");
            match a {
                Assumption::UnseenUses
                | Assumption::SearchCapped
                | Assumption::PublicApi
                | Assumption::AmbiguousMatch
                | Assumption::ComputedName => {}
                Assumption::RuntimeCheck => {
                    assert_eq!(i, Assumption::ALL.len() - 1, "the last variant")
                }
            }
        }
    }
    use crate::frontend::{Frontend, Ruby, Rust};

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
                    mentioned: Vec::new(),
                }
            }
            fn literal(&self, path: &str, word: &str) -> Search {
                self.uses(path, word)
            }
            fn library(&self, _: &str) -> bool {
                false
            }
            fn read(&self, _: &str) -> Option<Vec<u8>> {
                Some(b"x\n".to_vec())
            }
            fn called_by_name(&self, _: &str) -> bool {
                false
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

    #[test]
    fn names_method_reads_each_way_code_can_name_a_method() {
        use Naming::*;
        let ruby: &dyn Frontend = &Ruby;
        let rust: &dyn Frontend = &Rust;
        // (frontend, source, method, how the tree names it)
        let table: &[(&dyn Frontend, &str, &str, &[Naming])] = &[
            (ruby, "def m; end\n", "m", &[Definition]),
            (ruby, "m(1)\n", "m", &[Call]),
            (ruby, "x.m\n", "m", &[Call]),
            (ruby, "method(:m)\n", "m", &[Reference]),
            (ruby, "send(:m)\n", "m", &[Reference]),
            (ruby, "send(\"m_#{x}\")\n", "m_a", &[Computed]),
            (ruby, "send(\"m_#{x}\")\n", "n_a", &[]),
            (ruby, "Object.const_get(x)\n", "m", &[]),
            (ruby, "instance_variable_get(x)\n", "m", &[]),
            (ruby, "before_save :m\n", "m", &[Literal]),
            (ruby, "x = { m: 1 }\n", "m", &[Literal]),
            (ruby, "# rubocop:disable Naming/m\nx\n", "m", &[Directive]),
            (ruby, "# m\nx\n", "m", &[]),
            (ruby, "def n; end\n", "m", &[]),
            (rust, "fn m() {}\n", "m", &[Definition]),
            (rust, "fn f() { m(); }\n", "m", &[Call]),
            (rust, "fn f() { mac!(m); }\n", "m", &[Reference]),
            (
                rust,
                "/// ```\n/// demo::m();\n/// ```\nfn f() {}\n",
                "m",
                &[Directive],
            ),
            (rust, "/// m\nfn f() {}\n", "m", &[]),
        ];
        for &(frontend, source, method, want) in table {
            let tree = frontend.parse(source.as_bytes().to_vec());
            let found: Vec<Naming> = (tree.descendants(Tree::ROOT))
                .filter_map(|id| names_method(&tree, id, method))
                .collect();
            assert_eq!(
                found,
                want,
                "{} {source:?} naming {method}",
                frontend.language()
            );
        }
    }
}
