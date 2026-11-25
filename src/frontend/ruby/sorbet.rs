//! Sorbet's annotations, for `typing`: statements that only declare types
//! ([`Flags::TYPING`]), calls that return their argument
//! ([`Flags::TYPE_CAST`]), and which of them sorbet-runtime never checks
//! ([`Flags::UNCHECKED`]).

use crate::tree::Flags;
use ruby_prism::{ConstantPathNode, ConstantReadNode, Node, ParseResult, Visit};

/// What a file shows of Sorbet, read before the walk: a method named `sig`
/// is Sorbet's only in a file that has a `typed` sigil or names `T`, and
/// `abstract!` only in one that names `T::Helpers` (Rails defines one too).
pub(super) struct Sorbet {
    uses: bool,
    helpers: bool,
}

impl Sorbet {
    pub(super) fn of(result: &ParseResult<'_>) -> Sorbet {
        let typed = result
            .magic_comments()
            .any(|m| m.key().eq_ignore_ascii_case(b"typed"));
        let mut scan = Scan::default();
        scan.visit(&result.node());
        Sorbet {
            uses: typed || scan.names_t,
            helpers: scan.helpers,
        }
    }

    /// `node`'s annotation flags; `statement` when it sits directly in a
    /// statement list, so its value is unused.
    pub(super) fn flags(&self, node: &Node<'_>, statement: bool) -> Flags {
        let Some(call) = node.as_call_node() else {
            return Flags::default();
        };
        if call.is_safe_navigation() {
            return Flags::default();
        }
        let name = call.name().as_slice();
        let braces = call.block().and_then(|b| b.as_block_node()).is_some();
        let blockless = call.block().is_none();
        let args: Vec<Node<'_>> = call
            .arguments()
            .map(|a| a.arguments().iter().collect())
            .unwrap_or_default();
        let Some(receiver) = call.receiver() else {
            if !statement || !self.uses {
                return Flags::default();
            }
            let typing = match name {
                // `sig(:final) { ... }` too.
                b"sig" => braces && args.iter().all(|a| a.as_symbol_node().is_some()),
                b"extend" | b"include" => {
                    blockless
                        && matches!(&args[..], [a] if matches!(path(a).as_deref(), Some([b"T", b"Sig" | b"Helpers"])))
                }
                b"abstract!" | b"interface!" | b"sealed!" | b"final!" => {
                    self.helpers && blockless && args.is_empty()
                }
                _ => false,
            };
            return if typing {
                Flags::TYPING
            } else {
                Flags::default()
            };
        };
        match path(&receiver).as_deref() {
            Some([b"T", b"Sig", b"WithoutRuntime"]) if name == b"sig" && braces && statement => {
                Flags::TYPING | Flags::UNCHECKED
            }
            Some([b"T"]) if blockless => cast(name, &args, statement),
            _ => Flags::default(),
        }
    }
}

/// `T.<name>(args)`: a cast when its arguments are plain, positional but for
/// a literal `checked:`, which `checked: false` turns off.
fn cast(name: &[u8], args: &[Node<'_>], statement: bool) -> Flags {
    let (positional, checked) = match args {
        [rest @ .., last] if last.as_keyword_hash_node().is_some() => (rest, checked(last)),
        _ => (args, Some(true)),
    };
    let plain = positional.iter().all(|a| {
        a.as_splat_node().is_none()
            && a.as_forwarding_arguments_node().is_none()
            && a.as_block_argument_node().is_none()
    });
    let Some(checked) = checked.filter(|_| plain) else {
        return Flags::default();
    };
    let unchecked = |flags: Flags| match checked {
        true => flags,
        false => flags | Flags::UNCHECKED,
    };
    let keywords = positional.len() < args.len();
    match (name, positional.len()) {
        (b"let" | b"cast", 2) => unchecked(Flags::TYPE_CAST),
        // Its value is `self`, unused as a statement.
        (b"bind", 2) if statement => unchecked(Flags::TYPING),
        (b"bind", 2) => unchecked(Flags::TYPE_CAST),
        (b"must", 1) if !keywords => Flags::TYPE_CAST,
        (b"unsafe", 1) if !keywords => Flags::TYPE_CAST | Flags::UNCHECKED,
        _ => Flags::default(),
    }
}

/// Whether a keyword hash holding only `checked:` checks: `Some(false)` for
/// a literal `false`, `Some(true)` for a literal `true`, `None` for anything
/// else (another key, a computed value).
fn checked(hash: &Node<'_>) -> Option<bool> {
    let hash = hash.as_keyword_hash_node()?;
    let elements: Vec<Node<'_>> = hash.elements().iter().collect();
    let [element] = &elements[..] else {
        return None;
    };
    let assoc = element.as_assoc_node()?;
    let key = assoc.key();
    (key.as_symbol_node()?.unescaped() == b"checked").then_some(())?;
    let value = assoc.value();
    if value.as_false_node().is_some() {
        Some(false)
    } else if value.as_true_node().is_some() {
        Some(true)
    } else {
        None
    }
}

/// A constant path's segments from the top (`T::Sig` is `[T, Sig]`, as is
/// `::T::Sig`); `None` for anything else.
fn path(node: &Node<'_>) -> Option<Vec<&'static [u8]>> {
    fn segments<'a>(node: &Node<'a>, out: &mut Vec<&'a [u8]>) -> Option<()> {
        if let Some(read) = node.as_constant_read_node() {
            out.push(read.name().as_slice());
            return Some(());
        }
        let path = node.as_constant_path_node()?;
        if let Some(parent) = path.parent() {
            segments(&parent, out)?;
        }
        out.push(path.name()?.as_slice());
        Some(())
    }
    let mut out = Vec::new();
    segments(node, &mut out)?;
    // Only compared against literals: the names needn't outlive the parse.
    let known: &[&'static [u8]] = &[b"T", b"Sig", b"Helpers", b"WithoutRuntime"];
    out.iter()
        .map(|s| known.iter().find(|k| *k == s).copied())
        .collect()
}

/// Whether the file names `T` anywhere, and `T::Helpers`.
#[derive(Default)]
struct Scan {
    names_t: bool,
    helpers: bool,
}

impl<'pr> Visit<'pr> for Scan {
    fn visit_constant_read_node(&mut self, node: &ConstantReadNode<'pr>) {
        self.names_t |= node.name().as_slice() == b"T";
    }

    fn visit_constant_path_node(&mut self, node: &ConstantPathNode<'pr>) {
        // `::T` has no parent to visit.
        let segments = path(&node.as_node());
        self.names_t |= segments.as_deref() == Some(&[b"T".as_slice()][..]);
        self.helpers |= segments.as_deref() == Some(&[b"T".as_slice(), b"Helpers"][..]);
        ruby_prism::visit_constant_path_node(self, node);
    }
}

/// Whether the nearest lockfile above `path` (`Gemfile.lock`,
/// `gems.locked`) shows the project loading sorbet-runtime, so `T` is its module: then
/// [`Flags::UNCHECKED`] annotations do nothing at runtime.
pub(super) fn runtime(path: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> bool {
    let mut dirs: Vec<&str> = path.split('/').collect();
    dirs.pop();
    loop {
        let dir = dirs.join("/");
        for name in ["Gemfile.lock", "gems.locked"] {
            let file = if dir.is_empty() {
                name.to_string()
            } else {
                format!("{dir}/{name}")
            };
            if let Some(lock) = read(&file) {
                return names_runtime(&lock);
            }
        }
        if dirs.pop().is_none() {
            return false;
        }
    }
}

/// Whether the lockfile shows sorbet-runtime (or sorbet-static-and-runtime)
/// loaded by the project itself: a top-level entry under `DEPENDENCIES`, or a
/// runtime dependency of the gemspec at the lockfile's root (its `PATH` gem
/// with `remote: .`). Another gem's dependency (a development tool's) doesn't
/// count.
fn names_runtime(lock: &[u8]) -> bool {
    let runtime = |line: &str| {
        let gem = line.split_whitespace().next().unwrap_or_default();
        matches!(
            gem.trim_end_matches('!'),
            "sorbet-runtime" | "sorbet-static-and-runtime"
        )
    };
    let indent = |line: &str| line.len() - line.trim_start().len();
    let mut section = "";
    let mut root = false;
    for line in String::from_utf8_lossy(lock).lines() {
        if indent(line) == 0 {
            section = line.trim();
            root = false;
            continue;
        }
        match section {
            "DEPENDENCIES" if indent(line) == 2 && runtime(line) => return true,
            "PATH" if indent(line) == 2 => root |= line.trim() == "remote: .",
            "PATH" if root && indent(line) == 6 && runtime(line) => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{Frontend, Ruby};

    /// Each annotated node's source, with whether no runtime checks it.
    fn annotated(src: &str) -> Vec<(String, bool)> {
        let tree = Ruby.parse(src.as_bytes().to_vec());
        tree.nodes
            .iter()
            .filter(|n| n.flags.contains(Flags::TYPING) || n.flags.contains(Flags::TYPE_CAST))
            .map(|n| {
                let text = String::from_utf8_lossy(&tree.source[n.span.clone()]).into_owned();
                (text, n.flags.contains(Flags::UNCHECKED))
            })
            .collect()
    }

    #[test]
    fn marks_sorbet_annotations() {
        let found = annotated(
            "class A\n  extend T::Sig\n  extend T::Helpers\n  abstract!\n  sig { void }\n  def a; end\n\
             T::Sig::WithoutRuntime.sig { void }\n  def b\n    T.bind(self, A)\n    x = T.let(1, Integer)\n\
             T.must(x) + T.unsafe(x) + T.cast(x, Integer, checked: false) + T.cast(x, Integer)\n  end\nend\n",
        );
        let found: Vec<(&str, bool)> = found.iter().map(|(t, u)| (t.as_str(), *u)).collect();
        assert_eq!(
            found,
            [
                ("extend T::Sig", false),
                ("extend T::Helpers", false),
                ("abstract!", false),
                ("sig { void }", false),
                ("T::Sig::WithoutRuntime.sig { void }", true),
                ("T.bind(self, A)", false),
                ("T.let(1, Integer)", false),
                ("T.must(x)", false),
                ("T.unsafe(x)", true),
                ("T.cast(x, Integer, checked: false)", true),
                ("T.cast(x, Integer)", false),
            ]
        );
    }

    #[test]
    fn needs_sorbet_in_sight() {
        // No sigil and no `T`: a method of its own named `sig`.
        assert!(annotated("class A\n  sig { void }\n  abstract!\nend\n").is_empty());
        // A sigil makes `sig` Sorbet's, but `abstract!` needs `T::Helpers`.
        assert_eq!(
            annotated("# typed: true\nclass A\n  sig { void }\n  abstract!\nend\n"),
            [("sig { void }".to_string(), false)]
        );
        // Not a statement, a block argument, another key, a splat.
        assert!(
            annotated(
                "x = T.let(1)\ny = sig { void }\nT.must(x) { 1 }\nT.cast(x, I, strict: false)\nT.must(*a)\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn the_nearest_lockfile_names_the_runtime() {
        let files = |path: &str| -> Option<Vec<u8>> {
            match path {
                "Gemfile.lock" => Some(
                    b"GEM\n  specs:\n    sorbet-runtime (0.5.1)\n\nDEPENDENCIES\n  sorbet-runtime\n"
                        .to_vec(),
                ),
                "engines/x/Gemfile.lock" => {
                    Some(b"GEM\n  specs:\n    sorbet-runtime-stub (0.2)\n".to_vec())
                }
                _ => None,
            }
        };
        assert!(runtime("lib/a.rb", &files));
        assert!(runtime("a.rb", &files));
        assert!(!runtime("engines/x/lib/a.rb", &files));
        assert!(!runtime("a.rb", &|_| None));
    }

    #[test]
    fn the_runtime_is_a_direct_dependency() {
        let lock = |s: &str| names_runtime(s.as_bytes());
        assert!(lock("DEPENDENCIES\n  sorbet-runtime\n"));
        assert!(lock("DEPENDENCIES\n  sorbet-static-and-runtime (>= 0.5)\n"));
        assert!(lock(
            "PATH\n  remote: .\n  specs:\n    w (0.1)\n      sorbet-runtime (>= 0.5)\n\nDEPENDENCIES\n  w!\n"
        ));
        // Only another gem's dependency.
        assert!(!lock(
            "GEM\n  specs:\n    tapioca (0.16)\n      sorbet-static-and-runtime\n    sorbet-static-and-runtime (0.5)\n\nDEPENDENCIES\n  tapioca\n"
        ));
        // A path gem elsewhere isn't the project's gemspec.
        assert!(!lock(
            "PATH\n  remote: vendor/x\n  specs:\n    x (0.1)\n      sorbet-runtime\n\nDEPENDENCIES\n  x!\n"
        ));
    }
}
