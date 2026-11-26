//! Statements loading another Ruby file, read off the tree by prism's node
//! kinds.

use crate::frontend::{Load, Target};
use crate::tree::{Flags, NodeId, Receiver, Role, Tree};

/// Directories a bare `require` finds files under, by convention: a gem's
/// `lib`, the `-I` of a test runner, the working directory.
const LOAD_ROOTS: &[&str] = &["lib", "spec", "test", ""];

/// `require`, `require_relative`, `load` and `autoload` called without a
/// receiver, with a plain string for the path; a computed path is
/// [`Load::Computed`], as is any mention of Zeitwerk, which loads files by
/// their names.
pub(super) fn load(tree: &Tree, id: NodeId, from: &str) -> Option<Load> {
    let node = tree.node(id);
    let name = node.name.as_deref()?;
    if node.role == Role::Reference(crate::tree::NameKind::Constant) && name == "Zeitwerk" {
        return Some(Load::Computed);
    }
    let implicit = matches!(node.role, Role::Call(Receiver::Implicit) | Role::Hazard(_));
    if node.kind != "call" || !implicit {
        return None;
    }
    let (now, at) = match name {
        "require" | "require_relative" => (true, 0),
        "load" => (false, 0),
        "autoload" => (false, 1),
        _ => return None,
    };
    let list = tree
        .code_children(id)
        .find(|&c| tree.node(c).flags.contains(Flags::ARGUMENTS))?;
    let arg = tree.code_children(list).nth(at)?;
    let Some(path) = plain_string(tree, arg) else {
        return Some(Load::Computed);
    };
    let file = |path: &str| match path.rsplit('/').next().is_some_and(|b| b.contains('.')) {
        true => path.to_string(),
        false => format!("{path}.rb"),
    };
    let target = if name == "require_relative" {
        let dir = from.rsplit_once('/').map_or("", |(d, _)| d);
        Target::Exact(normalize(&format!("{dir}/{}", file(path)))?)
    } else if path.starts_with(['.', '/', '~']) {
        return Some(Load::Computed);
    } else {
        Target::Rooted {
            roots: LOAD_ROOTS,
            path: file(path),
        }
    };
    Some(if now {
        Load::Now(target)
    } else {
        Load::Other(target)
    })
}

/// A string literal's text: single or double quoted, with no escape or
/// interpolation.
fn plain_string(tree: &Tree, id: NodeId) -> Option<&str> {
    let node = tree.node(id);
    if node.kind != "string" {
        return None;
    }
    let text = std::str::from_utf8(tree.text(&node.span)).ok()?;
    let inner = text
        .strip_prefix('\'')
        .and_then(|t| t.strip_suffix('\''))
        .or_else(|| text.strip_prefix('"').and_then(|t| t.strip_suffix('"')))?;
    (!inner.contains(['\\', '#'])).then_some(inner)
}

/// `a/./b/../c` as `a/c`; `None` when it climbs out of the repository.
fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            _ => out.push(part),
        }
    }
    Some(out.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{Frontend, Ruby};

    #[test]
    fn loads_name_their_file() {
        let src = "require 'zoo/cart'\nrequire_relative '../a'\nautoload :C, \"zoo/c\"\nload 'x.rake'\n\
                   require f\nrequire \"a/#{x}\"\nYAML.load('x')\nZeitwerk::Loader.new\n";
        let tree = Ruby.parse(src.as_bytes().to_vec());
        let loads: Vec<Load> = tree
            .descendants(Tree::ROOT)
            .filter_map(|id| load(&tree, id, "lib/zoo/order.rb"))
            .collect();
        let rooted = |path: &str| Target::Rooted {
            roots: LOAD_ROOTS,
            path: path.into(),
        };
        assert_eq!(
            loads,
            [
                Load::Now(rooted("zoo/cart.rb")),
                Load::Now(Target::Exact("lib/a.rb".into())),
                Load::Other(rooted("zoo/c.rb")),
                Load::Other(rooted("x.rake")),
                Load::Computed,
                Load::Computed,
                Load::Computed,
            ]
        );
        assert!(rooted("zoo/cart.rb").is("lib/zoo/cart.rb"));
        assert!(rooted("zoo/cart.rb").is("gems/a/lib/zoo/cart.rb"));
        assert!(rooted("zoo/cart.rb").is("zoo/cart.rb"));
        assert!(!rooted("zoo/cart.rb").is("app/zoo/cart.rb"));
        assert!(!rooted("zoo/cart.rb").is("lib/xzoo/cart.rb"));
    }
}
