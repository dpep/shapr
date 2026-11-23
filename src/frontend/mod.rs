//! The language seam. A frontend turns source into a [`Tree`]; the
//! classification core sees only trees and this trait, never a parser's own
//! types, so adding a language touches nothing outside its frontend.

mod ruby;

use crate::tree::Tree;

pub use ruby::Ruby;

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
}

static FRONTENDS: &[&dyn Frontend] = &[&Ruby];

/// The frontend for `path`, if any language claims it.
pub fn for_path(path: &str) -> Option<&'static dyn Frontend> {
    FRONTENDS.iter().copied().find(|f| f.handles(path))
}
