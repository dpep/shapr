//! Ruby, parsed with prism.

mod generated;
mod roles;

use super::{Construct, Frontend};
use crate::tree::{NodeId, Role, Span, Tree, TreeBuilder};
use ruby_prism::{Location, Node, Visit};

pub struct Ruby;

/// Magic-comment keys Ruby or Sorbet act on, normalized (lowercase, `_`).
/// prism reports every `# key: value` comment, so the key decides.
const MAGIC_KEYS: &[&str] = &[
    "frozen_string_literal",
    "encoding",
    "coding",
    "warn_indent",
    "shareable_constant_value",
    "typed",
];

/// Comment directives prism doesn't report as magic: they have no `: value`.
const DIRECTIVE_PREFIXES: &[&str] = &["rubocop:disable", "rubocop:enable", "rubocop:todo"];

impl Frontend for Ruby {
    fn language(&self) -> &'static str {
        "ruby"
    }

    fn handles(&self, path: &str) -> bool {
        let base = path.rsplit('/').next().unwrap_or(path);
        matches!(base, "Gemfile" | "Rakefile")
            || base
                .rsplit_once('.')
                .is_some_and(|(_, ext)| matches!(ext, "rb" | "rake" | "gemspec" | "ru"))
    }

    fn parse(&self, source: Vec<u8>) -> Tree {
        let mut builder = TreeBuilder::new(source.clone());
        let result = ruby_prism::parse(&source);

        Walker {
            builder: &mut builder,
            roles: roles::Roles::default(),
            entered: None,
        }
        .visit(&result.node());
        if let Some(data) = result.data_loc() {
            builder.leaf("data", span(&data), None, Role::Other);
        }

        let magic: Vec<usize> = result
            .magic_comments()
            .filter(|m| {
                let key = String::from_utf8_lossy(m.key())
                    .to_ascii_lowercase()
                    .replace('-', "_");
                MAGIC_KEYS.contains(&key.as_str())
            })
            .map(|m| m.key().as_ptr() as usize - source.as_ptr() as usize)
            .collect();
        for comment in result.comments() {
            let at = span(&comment.location());
            if magic.iter().any(|m| at.contains(m)) || is_directive(comment.text()) {
                builder.directive(at);
            } else {
                builder.comment(at);
            }
        }
        for error in result.errors() {
            builder.error(span(&error.location()));
        }
        builder.finish()
    }

    fn construct(&self, tree: &Tree, id: NodeId) -> Option<Construct> {
        roles::construct(tree, id)
    }
}

fn is_directive(text: &[u8]) -> bool {
    let body = String::from_utf8_lossy(text);
    let body = body.trim_start_matches('#').trim_start();
    DIRECTIVE_PREFIXES.iter().any(|p| body.starts_with(p))
}

fn span(loc: &Location<'_>) -> Span {
    loc.start_offset()..loc.end_offset()
}

struct Walker<'b> {
    builder: &'b mut TreeBuilder,
    roles: roles::Roles,
    /// The node the enter hook just saw, as (kind, start, end).
    entered: Option<(&'static str, usize, usize)>,
}

fn key(node: &Node<'_>) -> (&'static str, usize, usize) {
    let loc = node.location();
    (
        generated::kind_and_name(node).0,
        loc.start_offset(),
        loc.end_offset(),
    )
}

impl Walker<'_> {
    /// Whether `node` arrived through `visit`, which already ran the enter hook.
    fn dispatched(&mut self, node: &Node<'_>) -> bool {
        self.entered.take() == Some(key(node))
    }

    fn facts(&mut self, node: &Node<'_>) -> (&'static str, Option<Vec<u8>>, Role) {
        let (kind, name) = generated::kind_and_name(node);
        let (role, renamed) = self.roles.of(node);
        (kind, renamed.or_else(|| name.map(<[u8]>::to_vec)), role)
    }
}

impl<'pr> Visit<'pr> for Walker<'_> {
    generated::concrete_visits!('pr);

    fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
        self.entered = Some(key(&node));
        let (kind, name, role) = self.facts(&node);
        self.builder
            .open(kind, span(&node.location()), name.as_deref(), role);
    }

    fn visit_branch_node_leave(&mut self) {
        self.builder.close();
    }

    fn visit_leaf_node_enter(&mut self, node: Node<'pr>) {
        self.entered = Some(key(&node));
        let (kind, name, role) = self.facts(&node);
        let name = name.as_deref();
        // A heredoc's node spans only its opener; the body and terminator
        // live on later lines and must still be compared byte for byte.
        if let Some(body) = heredoc_body(&node) {
            self.builder.open(kind, span(&node.location()), name, role);
            self.builder.leaf("heredoc_body", body, None, Role::Other);
            self.builder.close();
        } else {
            self.builder.leaf(kind, span(&node.location()), name, role);
        }
    }
}

/// From a plain heredoc's first body byte through its terminator.
fn heredoc_body(node: &Node<'_>) -> Option<Span> {
    let (opening, content, closing) = if let Some(n) = node.as_string_node() {
        (n.opening_loc(), n.content_loc(), n.closing_loc())
    } else {
        let n = node.as_x_string_node()?;
        (
            Some(n.opening_loc()),
            n.content_loc(),
            Some(n.closing_loc()),
        )
    };
    let heredoc = opening.is_some_and(|o| o.as_slice().starts_with(b"<<"));
    heredoc
        .then(|| content.start_offset()..closing.map_or(content.end_offset(), |c| c.end_offset()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Receiver;

    fn parse(src: &str) -> Tree {
        Ruby.parse(src.as_bytes().to_vec())
    }

    fn kinds(tree: &Tree, comment: bool) -> Vec<&'static str> {
        tree.nodes
            .iter()
            .filter(|n| n.comment == comment)
            .map(|n| n.kind)
            .collect()
    }

    #[test]
    fn recognizes_ruby_paths() {
        for path in [
            "a.rb",
            "lib/tasks/x.rake",
            "x.gemspec",
            "config.ru",
            "Gemfile",
            "sub/Rakefile",
        ] {
            assert!(Ruby.handles(path), "{path}");
        }
        for path in ["a.py", "README.md", "Gemfile.lock", "rb"] {
            assert!(!Ruby.handles(path), "{path}");
        }
    }

    #[test]
    fn magic_comments_are_code_and_notes_are_comments() {
        let tree = parse(
            "# frozen_string_literal: true\n# Note: not magic\n# typed: strict\n\
             def a\n  # rubocop:disable Style/Foo\n  1 # trailing\nend\n",
        );
        let directives = tree
            .nodes
            .iter()
            .filter(|n| n.kind == "directive" && !n.comment)
            .count();
        assert_eq!(directives, 3);
        assert_eq!(kinds(&tree, true), ["comment", "comment"]);
        assert!(tree.line(1).code && !tree.line(1).comment);
        assert!(tree.line(2).comment && !tree.line(2).code);
    }

    #[test]
    fn records_names_and_errors() {
        let tree = parse("def greet(name)\n  puts name\nend\n");
        let names: Vec<&str> = tree
            .nodes
            .iter()
            .filter_map(|n| n.name.as_deref())
            .collect();
        assert_eq!(names, ["greet", "name", "puts", "name"]);
        assert!(tree.errors.is_empty());
        assert!(!parse("def broken(\n").errors.is_empty());
    }

    /// Every node with a role, as (role, name).
    fn roles(src: &str) -> Vec<(Role, Option<String>)> {
        let tree = parse(src);
        assert!(tree.errors.is_empty(), "{src}");
        tree.nodes
            .iter()
            .filter(|n| n.role != Role::Other)
            .map(|n| (n.role, n.name.as_deref().map(str::to_string)))
            .collect()
    }

    fn has(found: &[(Role, Option<String>)], role: Role, name: Option<&str>) -> bool {
        found
            .iter()
            .any(|(r, n)| *r == role && n.as_deref() == name)
    }

    #[test]
    fn roles_mark_definitions_and_references() {
        use crate::tree::NameKind::*;
        let found = roles(
            "module Shop\n  class Cart\n    LIMIT = 3\n    Shop::MAX = 4\n\
             def add(item, qty = 1, *rest, key:, **opts, &blk)\n\
             @items = item\n      $seen = @@count\n      total = LIMIT\n      Shop::Cart\n    end\n  end\nend\n",
        );
        for (role, name) in [
            (Role::Definition(Module), "Shop"),
            (Role::Definition(Class), "Cart"),
            (Role::Definition(Constant), "LIMIT"),
            (Role::Definition(Constant), "MAX"),
            (Role::Definition(Method), "add"),
            (Role::Definition(Parameter), "item"),
            (Role::Definition(Parameter), "qty"),
            (Role::Definition(Parameter), "rest"),
            (Role::Definition(KeywordParameter), "key"),
            (Role::Definition(Parameter), "opts"),
            (Role::Definition(BlockParameter), "blk"),
            (Role::Definition(Field), "@items"),
            (Role::Definition(Global), "$seen"),
            (Role::Reference(Field), "@@count"),
            (Role::Bind, "total"),
            (Role::Reference(Variable), "item"),
            (Role::Reference(Constant), "LIMIT"),
            (Role::Reference(Constant), "Cart"),
        ] {
            assert!(has(&found, role, Some(name)), "{role:?} {name}: {found:?}");
        }
        assert!(Role::Definition(Method).is_declaration());
        assert!(!Role::Definition(Variable).is_declaration());
    }

    #[test]
    fn roles_mark_dynamic_names() {
        let found = roles(
            "send(:greet)\nobj.public_send(\"wave\")\nrespond_to?(:nod)\n\
             attr_reader :size, :color\nalias_method :fresh, :stale\nalias newer older\n\
             instance_variable_get(:@cache)\nputs :not_a_name\n",
        );
        let dynamic: Vec<&str> = found
            .iter()
            .filter(|(r, _)| *r == Role::DynamicName)
            .filter_map(|(_, n)| n.as_deref())
            .collect();
        assert_eq!(
            dynamic,
            [
                "greet", "wave", "nod", "size", "color", "fresh", "stale", "newer", "older",
                "@cache"
            ]
        );
    }

    #[test]
    fn roles_mark_visibility() {
        use crate::tree::Visibility::*;
        let found = roles(
            "class A\n  private\n  def a; end\n  protected def b; end\n  public :a\n\
             module_function\n  other.private\nend\n",
        );
        let visibility: Vec<Role> = found
            .iter()
            .map(|(r, _)| *r)
            .filter(|r| matches!(r, Role::Visibility(_)))
            .collect();
        let expected = [Private, Protected, Public, ModuleFunction].map(Role::Visibility);
        assert_eq!(visibility, expected);
        assert!(has(&found, Role::DynamicName, Some("a")));
        assert!(has(&found, Role::Call(Receiver::Explicit), Some("private")));
    }

    #[test]
    fn roles_mark_jumps_and_calls() {
        use crate::tree::Jump::*;
        let found = roles(
            "def run(&blk)\n  return 1 if block_given?\n  loop { break }\n  loop { redo }\n  [1].each { next }\n\
             yield\n  super\n  super(1)\n  begin; rescue; retry; end\n\
             go\n  self.go\n  blk.call\nend\n",
        );
        for jump in [Return, BlockGiven, Break, Redo, Next, Yield, Super, Retry] {
            assert!(
                found.iter().any(|(r, _)| *r == Role::Jump(jump)),
                "{jump:?}"
            );
        }
        let supers = found
            .iter()
            .filter(|(r, _)| *r == Role::Jump(Super))
            .count();
        assert_eq!(supers, 2);
        assert!(has(&found, Role::Call(Receiver::Implicit), Some("go")));
        assert!(has(&found, Role::Call(Receiver::SelfRef), Some("go")));
        assert!(has(&found, Role::Call(Receiver::Explicit), Some("call")));
        assert!(has(
            &found,
            Role::Reference(crate::tree::NameKind::Variable),
            Some("blk")
        ));
    }

    #[test]
    fn fields_of_a_fixed_node_type_reach_the_tree() {
        let tree = parse("def a(x, &b)\n  foo(1) if x\nend\n");
        let kinds = kinds(&tree, false);
        for kind in ["parameters", "block_parameter", "arguments"] {
            assert!(kinds.contains(&kind), "{kind}: {kinds:?}");
        }
        assert_eq!(kinds.iter().filter(|k| **k == "statements").count(), 3);
    }

    #[test]
    fn heredoc_body_is_compared_verbatim() {
        let a = parse("x = <<~EOS\n  a  b\nEOS\n");
        let b = parse("x = <<~EOS\n  a b\nEOS\n");
        let root = |t: &Tree| t.node(Tree::ROOT).hash;
        assert_ne!(root(&a), root(&b));
        assert_eq!(root(&parse("x = 1 +  2\n")), root(&parse("x = 1 + 2\n")));
    }
}
