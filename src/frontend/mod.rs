//! The language seam. A frontend turns source into a [`Tree`]; the
//! classification core sees only trees and this trait, never a parser's own
//! types, so adding a language touches nothing outside its frontend.

pub(crate) mod ruby;
mod rust;

use crate::norms::Snapshot;
use crate::tree::{NodeId, Tree};

pub use ruby::Ruby;
pub use rust::Rust;

pub trait Frontend: Sync {
    /// The name reported in output, e.g. `ruby`.
    fn language(&self) -> &'static str;

    /// Whether this frontend parses the file at `path` (repo-relative).
    fn handles(&self, path: &str) -> bool;

    /// Parses `source`. Never fails: syntax errors are recorded in
    /// [`Tree::errors`] and the recovered structure is returned.
    ///
    /// Language knowledge the core can't have is expressed in the tree. A
    /// comment the language reads as code (a Ruby magic comment) is emitted
    /// as a leaf with `comment: false`, so changing it is a code change.
    fn parse(&self, source: Vec<u8>) -> Tree;

    /// What `id`, in a tree this frontend parsed, means to a reviewer
    /// sorting logic changes (see `classify::facets`).
    fn construct(&self, tree: &Tree, id: NodeId) -> Option<Construct>;

    /// Files whose code can reach this language's names, each `*.ext` or a
    /// file name: where uses of a renamed or removed name are searched.
    fn search_globs(&self) -> &'static [&'static str];

    /// Method names a call can reach another method through by a computed
    /// name (a `DynamicName` pattern: Ruby's `send(n)`): a file whose text
    /// holds none of them as a word holds no such call.
    fn dispatch_words(&self) -> &'static [&'static str];

    /// Whether the file at `path` is library code another package can depend
    /// on, judged from the repository's layout at the after revision.
    fn library(&self, path: &str, files: &dyn Snapshot) -> bool;

    /// How method definition `def` receives arguments, when every parameter
    /// is required, optional or a keyword (or a block): `None` for one that
    /// collects the rest (`*args`, `**opts`, `...`), and so can tell what it
    /// was passed, and in a language without default arguments.
    fn signature(&self, tree: &Tree, def: NodeId) -> Option<Signature>;

    /// How `call` passes its arguments, when each is a plain expression or a
    /// keyword named by a literal: `None` when one splats or forwards.
    fn arguments(&self, tree: &Tree, call: NodeId) -> Option<Arguments>;

    /// `id` is a literal every evaluation of which gives an equal value no
    /// other evaluation shares: a number, symbol, plain string, `nil`,
    /// `true`, `false`, or an empty collection.
    fn plain_literal(&self, tree: &Tree, id: NodeId) -> bool;

    /// Code inside `id` may run with another `self` than the code around it
    /// (a Ruby block, which `instance_eval` can rebind, or a class body).
    fn rebinds_self(&self, tree: &Tree, id: NodeId) -> bool;
}

/// A method's parameters, as [`Frontend::signature`] reads them.
#[derive(Debug, Default, PartialEq)]
pub struct Signature {
    /// The parameter list, absent for a method with none.
    pub list: Option<NodeId>,
    /// Required positional parameters, all before the optional ones.
    pub required: usize,
    /// Each optional positional parameter's default, in order.
    pub optional: Vec<NodeId>,
    /// Keyword parameters in order, with an optional one's default.
    pub keywords: Vec<(String, Option<NodeId>)>,
}

/// A call's arguments, as [`Frontend::arguments`] reads them.
#[derive(Debug, Default, PartialEq)]
pub struct Arguments {
    pub positional: Vec<NodeId>,
    /// Keyword arguments in order: the keyword and its value.
    pub keywords: Vec<(String, NodeId)>,
}

/// A language construct that tells one kind of logic change from another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Construct {
    /// A conditional (`if`, `while`, `case`, a `when`, a match arm): its
    /// first `test` code children are what it tests.
    Branch { test: usize },
    /// An operator comparing or combining truth values (`>`, `==`, `&&`, `!`).
    Comparison,
    /// A number, string, symbol, boolean or nil written out.
    Literal,
    /// Raises, propagates or rescues an error (`raise`, `rescue`, `?`,
    /// `unwrap`, `panic!`).
    Error,
    /// A definition's parameter list.
    Parameters,
    /// An item that only tests run (Rust's `#[test]` fn, `#[cfg(test)]` mod).
    Test,
}

static FRONTENDS: &[&dyn Frontend] = &[&Ruby, &Rust];

/// The frontend for `path`, if any language claims it.
pub fn for_path(path: &str) -> Option<&'static dyn Frontend> {
    FRONTENDS.iter().copied().find(|f| f.handles(path))
}
