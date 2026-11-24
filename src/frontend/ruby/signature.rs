//! How a Ruby method receives arguments and how a call passes them, read off
//! the tree by prism's node kinds, for proving that an argument restates its
//! parameter's default.

use crate::frontend::{Arguments, Signature};
use crate::tree::{Flags, NodeId, Tree};

pub(super) fn signature(tree: &Tree, def: NodeId) -> Option<Signature> {
    let mut signature = Signature::default();
    let list = tree
        .code_children(def)
        .find(|&c| tree.node(c).kind == "parameters");
    let Some(list) = list else {
        return Some(signature);
    };
    signature.list = Some(list);
    for p in tree.code_children(list) {
        let node = tree.node(p);
        let default = || tree.code_children(p).next();
        match node.kind {
            // A required parameter after an optional one (`def f(a = 1, b)`)
            // takes the last argument first.
            "required_parameter" | "multi_target" if signature.optional.is_empty() => {
                signature.required += 1;
            }
            "optional_parameter" => signature.optional.push(default()?),
            "required_keyword_parameter" => signature
                .keywords
                .push((node.name.as_deref()?.into(), None)),
            "optional_keyword_parameter" => {
                let name = node.name.as_deref()?.into();
                signature.keywords.push((name, Some(default()?)));
            }
            "block_parameter" => {}
            _ => return None,
        }
    }
    Some(signature)
}

pub(super) fn arguments(tree: &Tree, call: NodeId) -> Option<Arguments> {
    let mut arguments = Arguments::default();
    let list = tree
        .code_children(call)
        .find(|&c| tree.node(c).flags.contains(Flags::ARGUMENTS));
    let Some(list) = list else {
        return Some(arguments);
    };
    for a in tree.code_children(list) {
        match tree.node(a).kind {
            "keyword_hash" => {
                for pair in tree.code_children(a) {
                    let [key, value] = tree.code_children(pair).collect::<Vec<_>>()[..] else {
                        return None;
                    };
                    let name = (tree.node(pair).kind == "assoc")
                        .then(|| symbol(tree, key))
                        .flatten()?;
                    if arguments.keywords.iter().any(|(k, _)| *k == name) {
                        return None;
                    }
                    arguments.keywords.push((name, value));
                }
            }
            "splat" | "forwarding_arguments" => return None,
            _ if !arguments.keywords.is_empty() => return None,
            _ => arguments.positional.push(a),
        }
    }
    Some(arguments)
}

/// A plain symbol's name: `k:`, `:k`, `"k":` or `:"k"`, with no escape.
fn symbol(tree: &Tree, id: NodeId) -> Option<String> {
    let node = tree.node(id);
    if node.kind != "symbol" {
        return None;
    }
    let text = std::str::from_utf8(tree.text(&node.span)).ok()?;
    let text = match text.strip_prefix(':') {
        Some(rest) => rest,
        None => text.strip_suffix(':')?,
    };
    let text = ['"', '\'']
        .iter()
        .find_map(|&q| text.strip_prefix(q)?.strip_suffix(q))
        .unwrap_or(text);
    (!text.is_empty() && !text.contains(['\\', '#', '"', '\''])).then(|| text.to_string())
}

pub(super) fn plain_literal(tree: &Tree, id: NodeId) -> bool {
    let empty = tree.code_children(id).next().is_none();
    match tree.node(id).kind {
        "integer" | "float" | "symbol" | "nil" | "true" | "false" => true,
        // A string node with a child is a heredoc; an empty `{}` or `[]`.
        "string" | "hash" | "array" => empty,
        _ => false,
    }
}

/// A block or lambda (`instance_eval`, `instance_exec` and DSLs run one
/// with another `self`), and the bodies that open a `self` of their own.
pub(super) fn rebinds_self(tree: &Tree, id: NodeId) -> bool {
    matches!(
        tree.node(id).kind,
        "block" | "lambda" | "def" | "class" | "module" | "singleton_class"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{Frontend, Ruby};
    use crate::tree::{NameKind, Role};

    fn find(tree: &Tree, role: Role) -> NodeId {
        (0..tree.nodes.len() as u32)
            .map(NodeId)
            .find(|&id| tree.node(id).role == role)
            .unwrap()
    }

    fn text(tree: &Tree, id: NodeId) -> &str {
        std::str::from_utf8(tree.text(&tree.node(id).extent)).unwrap()
    }

    #[test]
    fn a_signature_lists_required_optional_and_keyword_parameters() {
        let tree = Ruby.parse(b"def f(a, b = 'x', k: 1, j:, &blk); end\n".to_vec());
        let s = signature(&tree, find(&tree, Role::Definition(NameKind::Method))).unwrap();
        assert_eq!(s.required, 1);
        let optional: Vec<&str> = s.optional.iter().map(|&d| text(&tree, d)).collect();
        assert_eq!(optional, ["'x'"]);
        let keywords: Vec<(&str, Option<&str>)> = (s.keywords.iter())
            .map(|(k, d)| (k.as_str(), d.map(|d| text(&tree, d))))
            .collect();
        assert_eq!(keywords, [("k", Some("1")), ("j", None)]);
        let none = Ruby.parse(b"def f; end\n".to_vec());
        let s = signature(&none, find(&none, Role::Definition(NameKind::Method))).unwrap();
        assert_eq!(s, Signature::default());
    }

    #[test]
    fn a_method_collecting_the_rest_has_no_signature() {
        for source in [
            "def f(*a); end\n",
            "def f(k: 1, **o); end\n",
            "def f(...); end\n",
            "def f(k: 1, **nil); end\n",
            "def f(a = 1, b); end\n",
        ] {
            let tree = Ruby.parse(source.as_bytes().to_vec());
            let def = find(&tree, Role::Definition(NameKind::Method));
            assert_eq!(signature(&tree, def), None, "{source}");
        }
    }

    #[test]
    fn arguments_are_positional_then_keywords_by_name() {
        let tree = Ruby.parse(b"go(x, {}, :a => 1, \"b\": 2, c: 3) { }\n".to_vec());
        let a = arguments(
            &tree,
            find(&tree, Role::Call(crate::tree::Receiver::Implicit)),
        )
        .unwrap();
        let positional: Vec<&str> = a.positional.iter().map(|&p| text(&tree, p)).collect();
        assert_eq!(positional, ["x", "{}"]);
        let keywords: Vec<(&str, &str)> = (a.keywords.iter())
            .map(|(k, v)| (k.as_str(), text(&tree, *v)))
            .collect();
        assert_eq!(keywords, [("a", "1"), ("b", "2"), ("c", "3")]);
    }

    #[test]
    fn splats_forwarding_and_odd_keys_have_no_arguments() {
        for source in [
            "go(*x)\n",
            "go(**x, k: 1)\n",
            "go(\"k\" => 1)\n",
            "go(k: 1, k: 2)\n",
            "go(\"a\\x62\": 1)\n",
            "def f(...) = go(...)\n",
        ] {
            let tree = Ruby.parse(source.as_bytes().to_vec());
            let call = (0..tree.nodes.len() as u32)
                .map(NodeId)
                .find(|&id| tree.node(id).name.as_deref() == Some("go"))
                .unwrap();
            assert_eq!(arguments(&tree, call), None, "{source}");
        }
    }

    #[test]
    fn plain_literals_are_values_without_shared_identity() {
        let tree = Ruby.parse(
            b"[1, -2.5, :s, 's', nil, true, false, {}, [], \"#{x}\", [1], /r/, 1r, X, <<~E]\n  h\nE\n"
                .to_vec(),
        );
        let list = (0..tree.nodes.len() as u32)
            .map(NodeId)
            .find(|&id| tree.node(id).kind == "array")
            .unwrap();
        let plain: Vec<bool> = (tree.code_children(list))
            .map(|e| plain_literal(&tree, e))
            .collect();
        let mut expected = vec![true; 9];
        expected.extend([false; 6]);
        assert_eq!(plain, expected);
    }
}
