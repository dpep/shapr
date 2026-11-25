//! Ruby, parsed with prism.

pub(crate) mod generated;
mod reserved;
mod roles;
mod signature;
mod sorbet;
pub(crate) mod variants;

use super::{Arguments, Construct, Frontend, Signature};
use crate::norms::Snapshot;
use crate::tree::{NodeId, Role, Semantics, Span, Spelling, Tree, TreeBuilder};
use ruby_prism::{Location, Node, Visit};

pub struct Ruby;

/// Magic-comment keys Ruby acts on, normalized (lowercase, `_`). prism
/// reports every `# key: value` comment, so the key decides. Sorbet's
/// `typed` sigil is a comment: a type checker reads it, Ruby doesn't.
const MAGIC_KEYS: &[&str] = &[
    "frozen_string_literal",
    "encoding",
    "coding",
    "warn_indent",
    "shareable_constant_value",
];

/// Ruby DSL files without an extension, as RuboCop includes them by default.
const FILES: &[&str] = &[
    "Appraisals",
    "Berksfile",
    "Brewfile",
    "Buildfile",
    "Capfile",
    "Cheffile",
    "Dangerfile",
    "Deliverfile",
    "Fastfile",
    "Gemfile",
    "Guardfile",
    "Jarfile",
    "Mavenfile",
    "Podfile",
    "Puppetfile",
    "Rakefile",
    "Snapfile",
    "Steepfile",
    "Thorfile",
    "Vagabondfile",
    "Vagrantfile",
    "buildfile",
    "rakefile",
];

/// Comment directives prism doesn't report as magic: they have no `: value`.
const DIRECTIVE_PREFIXES: &[&str] = &["rubocop:disable", "rubocop:enable", "rubocop:todo"];

impl Frontend for Ruby {
    fn language(&self) -> &'static str {
        "ruby"
    }

    fn handles(&self, path: &str) -> bool {
        let base = path.rsplit('/').next().unwrap_or(path);
        FILES.contains(&base)
            || base
                .rsplit_once('.')
                .is_some_and(|(_, ext)| matches!(ext, "rb" | "rake" | "gemspec" | "ru"))
    }

    fn parse(&self, source: Vec<u8>) -> Tree {
        let mut builder = TreeBuilder::new(source.clone());
        builder.semantics(Semantics {
            reserved: reserved::RESERVED,
            definitions_run: true,
            definition_hooks: &["method_added", "singleton_method_added"],
            default_arguments: true,
            spelling: Spelling {
                sigils: &['@', '$'],
                suffixes: &['?', '!', '='],
                glued: &['?', '!'],
                field_sigil: Some('@'),
                symbol: Some(':'),
                quotes: &['"', '\''],
            },
            ..Semantics::default()
        });
        let result = ruby_prism::parse(&source);

        Walker {
            builder: &mut builder,
            roles: roles::Roles::default(),
            sorbet: sorbet::Sorbet::of(&result),
            entered: Entered::default(),
            parents: Vec::new(),
            variants: variants::Context::default(),
            gaps: Vec::new(),
            receivers: Vec::new(),
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
            // A shebang picks the interpreter and its flags.
            let shebang = at.start == 0 && comment.text().starts_with(b"#!");
            let directive = shebang
                || magic.iter().any(|m| at.contains(m))
                || is_directive(comment.text())
                || sets_encoding(&source, &at);
            if directive {
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

    fn dispatch_words(&self) -> &'static [&'static str] {
        roles::DISPATCH_WORDS
    }

    /// Templates call helpers and models by name too.
    fn search_globs(&self) -> &'static [&'static str] {
        &[
            "*.rb",
            "*.rake",
            "*.gemspec",
            "*.ru",
            "*.erb",
            "*.haml",
            "*.slim",
            "*.builder",
            "*.jbuilder",
            "Appraisals",
            "Berksfile",
            "Brewfile",
            "Buildfile",
            "Capfile",
            "Cheffile",
            "Dangerfile",
            "Deliverfile",
            "Fastfile",
            "Gemfile",
            "Guardfile",
            "Jarfile",
            "Mavenfile",
            "Podfile",
            "Puppetfile",
            "Rakefile",
            "Snapfile",
            "Steepfile",
            "Thorfile",
            "Vagabondfile",
            "Vagrantfile",
            "buildfile",
            "rakefile",
        ]
    }

    /// A Rails application's or engine's controllers, mailers, jobs and
    /// channels: routes, `deliver_later`, `perform_later` and clients reach
    /// their public methods by name, and a template by the action's name.
    /// In a repository holding a `config/application.rb` or an `engine.rb`
    /// anywhere, every such `app/` directory at any depth counts.
    fn called_by_name(&self, path: &str, paths: &[&str]) -> bool {
        const DIRS: &[&str] = &[
            "app/controllers/",
            "app/mailers/",
            "app/jobs/",
            "app/channels/",
        ];
        // A pack's or engine's `app/` needn't sit beside what makes it Rails.
        let rails = paths.iter().any(|p| {
            let name = p.rsplit('/').next().unwrap_or(p);
            *p == "config/application.rb"
                || p.ends_with("/config/application.rb")
                || name == "engine.rb"
        });
        rails
            && (0..path.len())
                .filter(|&i| i == 0 || path.as_bytes()[i - 1] == b'/')
                .any(|i| DIRS.iter().any(|d| path[i..].starts_with(d)))
    }

    /// A gem's code: under `lib/` beside a `*.gemspec`.
    fn library(&self, path: &str, files: &dyn Snapshot) -> bool {
        let segments: Vec<&str> = path.split('/').collect();
        (0..segments.len().saturating_sub(1))
            .filter(|&i| segments[i] == "lib")
            .any(|i| {
                files
                    .list(&segments[..i].join("/"))
                    .iter()
                    .any(|f| f.ends_with(".gemspec"))
            })
    }

    fn construct(&self, tree: &Tree, id: NodeId) -> Option<Construct> {
        roles::construct(tree, id)
    }

    fn pattern(&self, tree: &Tree, id: NodeId) -> Option<super::Pattern> {
        roles::pattern(tree, id)
    }

    fn surface(&self, tree: &Tree, id: NodeId) -> Option<super::Surface> {
        roles::surface(tree, id)
    }

    fn signature(&self, tree: &Tree, def: NodeId) -> Option<Signature> {
        signature::signature(tree, def)
    }

    fn arguments(&self, tree: &Tree, call: NodeId) -> Option<Arguments> {
        signature::arguments(tree, call)
    }

    fn plain_literal(&self, tree: &Tree, id: NodeId) -> bool {
        signature::plain_literal(tree, id)
    }

    fn reorder(
        &self,
        tree: &Tree,
        container: NodeId,
        moved: &[NodeId],
    ) -> Option<Vec<super::order::Dependence>> {
        super::order::ruby(tree, container, moved)
    }

    fn rebinds_self(&self, tree: &Tree, id: NodeId) -> bool {
        signature::rebinds_self(tree, id)
    }

    fn load(&self, tree: &Tree, id: NodeId, path: &str) -> Option<super::Load> {
        signature::load(tree, id, path)
    }

    fn types_loaded(&self, path: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> bool {
        sorbet::runtime(path, read)
    }
}

fn is_directive(text: &[u8]) -> bool {
    let body = String::from_utf8_lossy(text);
    let body = body.trim_start_matches('#').trim_start();
    DIRECTIVE_PREFIXES.iter().any(|p| body.starts_with(p))
}

/// A comment Ruby scans for a source encoding when it isn't a `key: value`
/// magic comment (a vim modeline's `fileencoding=binary`): on line 1 after
/// any byte order mark, or line 2 after a shebang, alone on its line,
/// holding `coding`, optional whitespace, then `:` or `=`. Mirrors
/// `set_file_encoding` in Ruby's parse.y, widened to any name after the
/// separator: one Ruby rejects is an error, still code.
fn sets_encoding(source: &[u8], at: &Span) -> bool {
    // parse.y's ISSPACE, which unlike `is_ascii_whitespace` includes `\v`.
    let space = |b: &u8| matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r');
    let bom = if source.starts_with(b"\xef\xbb\xbf") {
        3
    } else {
        0
    };
    let line = source[..at.start].iter().filter(|&&b| b == b'\n').count();
    let line_start = source[..at.start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(bom, |i| i + 1);
    let top = line == 0 || (line == 1 && source[bom..].starts_with(b"#!"));
    if !top || !source[line_start..at.start].iter().all(space) {
        return false;
    }
    let text = source[at.clone()].to_ascii_lowercase();
    (0..text.len()).any(|i| {
        text[i..].starts_with(b"coding")
            && text[i + 6..]
                .iter()
                .find(|b| !space(b))
                .is_some_and(|&b| b == b':' || b == b'=')
    })
}

fn span(loc: &Location<'_>) -> Span {
    loc.start_offset()..loc.end_offset()
}

struct Walker<'b> {
    builder: &'b mut TreeBuilder,
    roles: roles::Roles,
    sorbet: sorbet::Sorbet,
    entered: Entered,
    /// The open prism nodes, as (kind, start), for [`variants::respell`].
    parents: Vec<(&'static str, usize)>,
    /// What [`variants::respell`] gathers from the code around a node.
    variants: variants::Context,
    /// Per open node, the stretches of an interpolated heredoc's body no
    /// part holds, in order, to report as leaves between its parts.
    gaps: Vec<Vec<Span>>,
    /// Per open node, the span of its receiver when it is a call: a unary
    /// operator's call starts at the operator, before its receiver.
    receivers: Vec<Option<Span>>,
}

/// The node an enter hook just saw, as (kind, start, end), for the
/// overrides `generated::concrete_visits!` adds to a prism visitor: a walk
/// that sets it in its enter hooks reaches every node through them.
#[derive(Default)]
pub(crate) struct Entered(Option<(&'static str, usize, usize)>);

impl Entered {
    pub(crate) fn enter(&mut self, node: &Node<'_>) {
        self.0 = Some(key(node));
    }

    /// Whether `node` arrived through `visit`, which already ran the enter hook.
    pub(crate) fn dispatched(&mut self, node: &Node<'_>) -> bool {
        self.0.take() == Some(key(node))
    }
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
    fn respell(&mut self, node: &Node<'_>) {
        let parent = self.parents.last().copied();
        let source = self.builder.source();
        if let Some((variant, canonical)) =
            variants::respell(node, parent, source, &mut self.variants)
        {
            self.builder.respell(variant, canonical);
        }
    }

    fn dispatched(&mut self, node: &Node<'_>) -> bool {
        self.entered.dispatched(node)
    }

    /// Reports the open node's gaps that start before `at`.
    fn gaps_before(&mut self, at: usize) {
        let Some(gaps) = self.gaps.last_mut() else {
            return;
        };
        let due = gaps.partition_point(|g| g.start < at);
        for gap in gaps.drain(..due) {
            self.builder.leaf("heredoc_indent", gap, None, Role::Other);
        }
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
        self.entered.enter(&node);
        self.gaps_before(node.location().start_offset());
        let (kind, name, role) = self.facts(&node);
        self.builder
            .open(kind, span(&node.location()), name.as_deref(), role);
        let parent = self.parents.last().copied();
        let statement = parent.is_some_and(|(k, _)| k == "statements");
        let receiver = self.receivers.last().cloned().flatten() == Some(span(&node.location()));
        let flags = self.roles.flags(&node) | self.sorbet.flags(&node, statement, receiver);
        self.builder.flag(flags);
        self.respell(&node);
        self.parents.push((kind, node.location().start_offset()));
        self.gaps.push(heredoc_gaps(&node, self.builder.source()));
        self.receivers.push(receiver_of(&node));
    }

    fn visit_branch_node_leave(&mut self) {
        self.gaps_before(usize::MAX);
        self.gaps.pop();
        self.receivers.pop();
        self.parents.pop();
        self.builder.close();
    }

    fn visit_leaf_node_enter(&mut self, node: Node<'pr>) {
        self.entered.enter(&node);
        self.gaps_before(node.location().start_offset());
        let (kind, name, role) = self.facts(&node);
        let name = name.as_deref();
        // A heredoc's node spans only its opener; the body and terminator
        // live on later lines and must still be compared byte for byte.
        if let Some(body) = heredoc_body(&node) {
            self.builder.open(kind, span(&node.location()), name, role);
            self.respell(&node);
            self.builder.leaf("heredoc_body", body, None, Role::Other);
            self.builder.close();
        } else {
            self.builder.leaf(kind, span(&node.location()), name, role);
            self.respell(&node);
        }
    }
}

/// The span of a call's receiver, in any of prism's call forms.
fn receiver_of(node: &Node<'_>) -> Option<Span> {
    let receiver = if let Some(n) = node.as_call_node() {
        n.receiver()
    } else if let Some(n) = node.as_call_operator_write_node() {
        n.receiver()
    } else if let Some(n) = node.as_call_and_write_node() {
        n.receiver()
    } else if let Some(n) = node.as_call_or_write_node() {
        n.receiver()
    } else if let Some(n) = node.as_call_target_node() {
        Some(n.receiver())
    } else if let Some(n) = node.as_index_operator_write_node() {
        n.receiver()
    } else if let Some(n) = node.as_index_and_write_node() {
        n.receiver()
    } else if let Some(n) = node.as_index_or_write_node() {
        n.receiver()
    } else {
        node.as_index_target_node().map(|n| n.receiver())
    };
    receiver.map(|r| span(&r.location()))
}

/// The bytes of an interpolated heredoc's body outside its parts: the
/// indentation a `<<~` heredoc strips where it leaves no string part, which
/// still sets how much every line loses.
fn heredoc_gaps(node: &Node<'_>, source: &[u8]) -> Vec<Span> {
    let (opening, parts, closing) = if let Some(n) = node.as_interpolated_string_node() {
        (n.opening_loc(), n.parts(), n.closing_loc())
    } else if let Some(n) = node.as_interpolated_x_string_node() {
        (Some(n.opening_loc()), n.parts(), Some(n.closing_loc()))
    } else {
        return Vec::new();
    };
    let heredoc = opening.is_some_and(|o| o.as_slice().starts_with(b"<<"));
    let spans: Vec<Span> = parts.iter().map(|p| span(&p.location())).collect();
    let (true, Some(first)) = (heredoc, spans.first()) else {
        return Vec::new();
    };
    // The body starts a line.
    let start = source[..first.start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let end = closing.map_or(spans[spans.len() - 1].end, |c| c.start_offset());
    let bounds = std::iter::once(start)
        .chain(spans.iter().flat_map(|s| [s.start, s.end]))
        .chain(std::iter::once(end));
    let bounds: Vec<usize> = bounds.collect();
    bounds
        .chunks(2)
        .map(|g| g[0]..g[1])
        .filter(|g| g.start < g.end)
        .collect()
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
            "Guardfile",
            "ios/Podfile",
        ] {
            assert!(Ruby.handles(path), "{path}");
        }
        // Every DSL file is searched for uses too.
        for name in FILES {
            assert!(Ruby.search_globs().contains(name), "{name}");
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
        assert_eq!(directives, 2);
        // Sorbet's sigil: a type checker reads it, Ruby doesn't.
        assert_eq!(kinds(&tree, true), ["comment", "comment", "comment"]);
        assert!(tree.line(3).comment && !tree.line(3).code);
        assert!(tree.line(1).code && !tree.line(1).comment);
        assert!(tree.line(2).comment && !tree.line(2).code);
    }

    /// The one table of computed-name families: each dispatch word's
    /// computed name is a pattern flagged with what it reaches (a method,
    /// constant or field), which rename blocks that family's names by. A new
    /// word lands here with its family.
    #[test]
    fn every_dispatch_word_computes_a_name_in_its_family() {
        use crate::tree::Flags;
        const CONSTANT: &[&str] = &[
            "const_get",
            "qualified_const_get",
            "const_defined?",
            "constantize",
            "safe_constantize",
        ];
        const FIELD: &[&str] = &[
            "instance_variable_get",
            "instance_variable_set",
            "instance_variable_defined?",
            "remove_instance_variable",
            "class_variable_get",
            "class_variable_set",
            "class_variable_defined?",
            "remove_class_variable",
        ];
        let computed = |source: String| {
            let tree = parse(&source);
            let n = tree.nodes.iter().find(|n| n.role == Role::DynamicName)?;
            Some((n.name.as_deref()?.to_string(), n.flags))
        };
        for &word in Ruby.dispatch_words() {
            let (pattern, family) = match word {
                w if CONSTANT.contains(&w) => ("get_*", Flags::CONSTANT),
                w if FIELD.contains(&w) => ("@get_*", Flags::FIELD),
                _ => ("get_*", Flags::default()),
            };
            let (name, flags) = computed(format!("{word}(\"get_#{{n}}\")\n")).expect(word);
            assert_eq!(name, pattern, "{word}");
            for f in [Flags::CONSTANT, Flags::FIELD] {
                assert_eq!(flags.contains(f), family == f, "{word}");
            }
        }
        for &word in FIELD {
            let (name, flags) = computed(format!("{word}(n, v)\n")).expect(word);
            assert!(name == "@*" && flags.contains(Flags::FIELD), "{word}");
        }
        assert_eq!(computed("puts(\"get_#{n}\")\n".to_string()), None);
    }

    #[test]
    fn an_encoding_comment_on_top_is_code() {
        let code = |src: &str| {
            parse(src)
                .nodes
                .iter()
                .any(|n| n.kind == "directive" && !n.comment)
        };
        assert!(code("# vim: set fileencoding=binary :\nx = 1\n"));
        assert!(code(
            "#!/usr/bin/env ruby\n# vim:fenc=latin1 coding=latin1\nx = 1\n"
        ));
        assert!(code("  # coding = binary\nx = 1\n"));
        assert!(code("# coding\x0b: binary\nx = 1\n"));
        assert!(!code("x = 1\n# vim: set fileencoding=binary :\n"));
        assert!(!code("# a\n# vim: set fileencoding=binary :\n"));
        assert!(!code("# vim: set ts=2 :\nx = 1\n"));
        assert!(!code("# coding is fun\nx = 1\n"));
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
    fn roles_mark_computed_dynamic_names_as_patterns() {
        let found = roles(
            "send(n)\npublic_send(\"get_\" + n)\ndefine_method(\"get_#{x}_now\") { 1 }\n\
             attr_reader :\"a_#{b}\"\nsend(:go, n)\nputs(n)\n",
        );
        let dynamic: Vec<&str> = found
            .iter()
            .filter(|(r, _)| *r == Role::DynamicName)
            .filter_map(|(_, n)| n.as_deref())
            .collect();
        assert_eq!(dynamic, ["*", "*", "get_*_now", "a_*", "go"]);
    }

    #[test]
    fn constant_lookups_mark_constant_dynamic_names() {
        let tree = parse(
            "Object.const_get(\"App::Widget\")\nconst_get(n)\nconst_defined?(\"#{m}::Wid#{x}\")\n\
             name.constantize\n\"#{a}Widget\".safe_constantize\n\"Widget\".constantize\nsend(n)\n",
        );
        let dynamic: Vec<(&str, bool)> = (tree.nodes.iter())
            .filter(|n| n.role == Role::DynamicName)
            .map(|n| {
                let constant = n.flags.contains(crate::tree::Flags::CONSTANT);
                (n.name.as_deref().unwrap_or_default(), constant)
            })
            .collect();
        let expected = [
            ("Widget", false),
            ("*", true),
            ("Wid*", true),
            ("*", true),
            ("*Widget", true),
            ("*", false),
        ];
        assert_eq!(dynamic, expected);
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
    fn whole_literals_spelling_a_field_name_it() {
        let dynamic = |source: &str| -> Vec<String> {
            roles(source)
                .into_iter()
                .filter(|(r, _)| *r == Role::DynamicName)
                .filter_map(|(_, n)| n)
                .collect()
        };
        assert_eq!(
            dynamic("[:@a, :\"@b\", \"@c\", '@@d']\n%i[@e]\n"),
            ["@a", "@b", "@c", "@@d", "@e"]
        );
        assert!(dynamic("[\"#{x}@a\", %w[@b], :c, \"@d e\", \"@1\"]\n").is_empty());
    }

    #[test]
    fn roles_mark_jumps_and_calls() {
        use crate::tree::Hazard::*;
        let found = roles(
            "def run(&blk)\n  return 1 if block_given?\n  loop { break }\n  loop { redo }\n  [1].each { next }\n\
             yield\n  super\n  super(1)\n  begin; rescue; retry; end\n\
             go\n  self.go\n  blk.call\nend\n",
        );
        for jump in [Return, BlockGiven, Break, Redo, Next, Yield, Super, Retry] {
            assert!(
                found.iter().any(|(r, _)| *r == Role::Hazard(jump)),
                "{jump:?}"
            );
        }
        let supers = found
            .iter()
            .filter(|(r, _)| *r == Role::Hazard(Super))
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
    fn a_constant_holding_a_new_class_is_marked() {
        let marked = |src: &str| {
            let tree = parse(src);
            let names = tree
                .nodes
                .iter()
                .filter(|n| n.flags.contains(crate::tree::Flags::CLASS_VALUE));
            names
                .filter_map(|n| n.name.as_deref().map(str::to_string))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            marked(
                "P = Struct.new(:x)\nQ = Class.new(B) do\nend\nM::R = Module.new\nD = Data.define(:a)\nX = Foo.new\nY = 1\n"
            ),
            ["P", "Q", "R", "D"]
        );
    }

    #[test]
    fn roles_mark_frame_and_position_reads() {
        use crate::tree::Hazard::*;
        let jumps = |src: &str| -> Vec<crate::tree::Hazard> {
            roles(src)
                .into_iter()
                .filter_map(|(r, _)| match r {
                    Role::Hazard(j) => Some(j),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            jumps("binding\neval('1')\nlocal_variables\nx.instance_eval('y')\n"),
            [Binding, Binding, Binding, Binding]
        );
        assert_eq!(
            jumps("__method__\ncaller\ncaller_locations\n$~\n$1\n$&\n"),
            [Frame, Stack, Stack, Frame, Frame, Frame]
        );
        assert_eq!(
            jumps(
                "Kernel.caller\n::Kernel.caller_locations(0)\nThread.current.backtrace_locations\n"
            ),
            [Stack; 3]
        );
        // Another object's stack: an exception's, another thread's.
        assert!(jumps("e.backtrace\nt.backtrace_locations\nx.caller\n").is_empty());
        assert_eq!(
            jumps("__LINE__\n__FILE__\n__dir__\nrequire_relative 'a'\n"),
            [Line, File, File, File]
        );
        assert_eq!(jumps("Proc.new\nproc\nlambda\n"), [BlockGiven; 3]);
        // A block makes them ordinary calls; a receiver makes them methods.
        assert!(jumps("proc { 1 }\nx.binding\nx.instance_eval { 1 }\n$stdout\n").is_empty());
    }

    #[test]
    fn methods_classes_and_modules_bound_locals() {
        use crate::tree::Flags;
        let tree = parse(
            "module M\n  class C\n    def a\n      [1].each { |x| x }\n    end\n  end\nend\n",
        );
        let scopes: Vec<&str> = tree
            .nodes
            .iter()
            .filter(|n| n.flags.contains(Flags::SCOPE))
            .map(|n| n.kind)
            .collect();
        assert_eq!(scopes, ["module", "class", "def"]);
    }

    #[test]
    fn a_comment_in_a_modifier_form_skips_the_hash() {
        // The condition comes before the body among the children.
        for (a, b) in [
            (
                "begin\n  go # one\nend while c\n",
                "begin\n  go # two\nend while c\n",
            ),
            ("go(1, # one\n  2) if c\n", "go(1, # two\n  2) if c\n"),
        ] {
            let root = |s: &str| parse(s).node(Tree::ROOT).hash;
            assert_eq!(root(a), root(b), "{a}");
        }
    }

    #[test]
    fn receivers_argument_lists_and_calls_on_self_are_marked() {
        use crate::tree::Flags;
        let tree = parse(
            "def self.a(x)\n  go(x)\n  self.go\n  self&.go\n  -self\nend\ndef self::b; end\ndef c; end\n",
        );
        let flagged = |flag: Flags| -> Vec<&str> {
            tree.nodes
                .iter()
                .filter(|n| n.flags.contains(flag))
                .map(|n| n.kind)
                .collect()
        };
        assert_eq!(flagged(Flags::RECEIVER), ["def", "def"]);
        assert_eq!(flagged(Flags::ARGUMENTS), ["arguments"]);
        let calls: Vec<Receiver> = tree
            .nodes
            .iter()
            .filter_map(|n| match n.role {
                Role::Call(r) => Some(r),
                _ => None,
            })
            .collect();
        use Receiver::*;
        assert_eq!(calls, [Implicit, SelfRef, Explicit, Explicit]);
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
