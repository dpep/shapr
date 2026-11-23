//! The language-neutral tree every frontend produces, and the builder that
//! normalizes it. Frontends report raw structure (kinds, spans, comments,
//! errors); hashing, comment placement and per-line facts are computed here,
//! once, so every frontend gets the same semantics.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

/// A byte range into a file's source.
pub type Span = Range<usize>;

/// An index into [`Tree::nodes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// What a node means, in terms the core can act on. Set by the frontend;
/// most nodes are [`Role::Other`]. The identifier, where there is one, is
/// [`Node::name`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    Other,
    /// Declares `name`.
    Definition(NameKind),
    /// Reads a variable or constant called `name`.
    Reference(NameKind),
    /// Calls the method `name`.
    Call(Receiver),
    /// A string or symbol literal whose value (`name`) the language resolves
    /// to code at runtime, as in `send(:name)`.
    DynamicName,
    /// Changes the visibility of the definitions after it, or of its arguments.
    Visibility(Visibility),
    /// Control flow bound to the enclosing method or block: it means
    /// something else once moved into a new method.
    Jump(Jump),
}

impl Role {
    /// A statement that defines a method, class, module or constant: the
    /// statements whose order doesn't matter to behavior.
    pub fn is_declaration(self) -> bool {
        matches!(
            self,
            Role::Definition(
                NameKind::Method | NameKind::Class | NameKind::Module | NameKind::Constant
            )
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    Method,
    Class,
    Module,
    Constant,
    /// A local variable.
    Variable,
    /// An instance or class variable: state kept on an object or class.
    Field,
    Global,
    Parameter,
    /// A parameter callers name at the call site (`key:`), so its name is
    /// part of the method's interface.
    KeywordParameter,
    /// The parameter receiving the method's block (`&block`).
    BlockParameter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receiver {
    /// `name(...)`.
    Implicit,
    /// `self.name(...)`.
    SelfRef,
    /// Anything else: the receiver is the call's first child.
    Explicit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Protected,
    Private,
    ModuleFunction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    Return,
    Break,
    Next,
    Redo,
    Retry,
    Yield,
    /// A call to the parent's method, whose implicit arguments are the
    /// enclosing method's.
    Super,
    /// Asks whether the enclosing method was given a block.
    BlockGiven,
}

#[derive(Debug)]
pub struct Node {
    /// The frontend's name for the syntax, e.g. `def` or `call`. Compared for
    /// equality, never interpreted by the core.
    pub kind: &'static str,
    /// The syntax's own span, as the parser reported it.
    pub span: Span,
    /// The node's full extent: its own span joined with every descendant's.
    /// Wider than the syntax's own span where children live elsewhere, as a
    /// heredoc body does.
    pub extent: Span,
    pub parent: Option<NodeId>,
    /// In source order, comments included.
    pub children: Vec<NodeId>,
    pub comment: bool,
    /// The identifier the syntax declares or references, where it has one.
    pub name: Option<Box<str>>,
    pub role: Role,
    /// Equal for subtrees that differ only in whitespace and comments.
    /// Process-local: never persisted or printed.
    pub hash: u64,
}

#[derive(Debug)]
pub struct Tree {
    pub source: Vec<u8>,
    pub nodes: Vec<Node>,
    /// Spans the parser reported as syntax errors.
    pub errors: Vec<Span>,
    line_starts: Vec<usize>,
    lines: Vec<LineFacts>,
}

/// What a source line carries, ignoring whitespace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineFacts {
    pub code: bool,
    pub comment: bool,
}

impl Tree {
    pub const ROOT: NodeId = NodeId(0);

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// The bytes of 1-based lines `first..=last`, newline included.
    pub fn line_span(&self, first: usize, last: usize) -> Span {
        let start = self.line_start(first);
        let end = self.line_start(last + 1);
        start..end
    }

    /// Byte offset where 1-based `line` starts; past the end, the source length.
    pub fn line_start(&self, line: usize) -> usize {
        line.checked_sub(1)
            .and_then(|i| self.line_starts.get(i))
            .copied()
            .unwrap_or(self.source.len())
    }

    /// The 1-based line holding byte `offset`.
    pub fn line_of(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&start| start <= offset)
    }

    /// Facts for 1-based `line`; a line past the end carries nothing.
    pub fn line(&self, line: usize) -> LineFacts {
        line.checked_sub(1)
            .and_then(|i| self.lines.get(i))
            .copied()
            .unwrap_or_default()
    }

    pub fn text(&self, span: &Span) -> &[u8] {
        &self.source[span.clone()]
    }

    /// The deepest node whose extent contains `span`.
    pub fn deepest_containing(&self, span: &Span) -> NodeId {
        let mut at = Self::ROOT;
        'descend: loop {
            for &child in &self.node(at).children {
                let extent = &self.node(child).extent;
                if extent.start <= span.start && span.end <= extent.end {
                    at = child;
                    continue 'descend;
                }
            }
            return at;
        }
    }

    /// `id` and every node under it, in source order.
    pub fn descendants(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut stack = vec![id];
        std::iter::from_fn(move || {
            let at = stack.pop()?;
            stack.extend(self.node(at).children.iter().rev());
            Some(at)
        })
    }

    /// Comment nodes under `id`, itself included, in source order.
    pub fn comments_under(&self, id: NodeId) -> Vec<NodeId> {
        self.descendants(id)
            .filter(|&n| self.node(n).comment)
            .collect()
    }
}

/// Builds a [`Tree`] from a frontend's walk. Calls nest: [`open`] a node,
/// report its children, [`close`] it; [`leaf`] is a node with no children.
/// Comments and errors may be reported in any order.
///
/// [`open`]: TreeBuilder::open
/// [`close`]: TreeBuilder::close
/// [`leaf`]: TreeBuilder::leaf
pub struct TreeBuilder {
    source: Vec<u8>,
    nodes: Vec<Node>,
    leaf: Vec<bool>,
    stack: Vec<NodeId>,
    /// Comment spans, each flagged when the language reads it as code.
    comments: Vec<(Span, bool)>,
    errors: Vec<Span>,
}

impl TreeBuilder {
    /// Starts a tree whose root spans the whole source.
    pub fn new(source: Vec<u8>) -> Self {
        let len = source.len();
        let mut builder = TreeBuilder {
            source,
            nodes: Vec::new(),
            leaf: Vec::new(),
            stack: Vec::new(),
            comments: Vec::new(),
            errors: Vec::new(),
        };
        builder.push("root", 0..len, None, Role::Other, false);
        builder.stack.push(Tree::ROOT);
        builder
    }

    pub fn source(&self) -> &[u8] {
        &self.source
    }

    pub fn open(&mut self, kind: &'static str, span: Span, name: Option<&[u8]>, role: Role) {
        let id = self.push(kind, span, name, role, false);
        self.stack.push(id);
    }

    pub fn close(&mut self) {
        // The root is closed by `finish`.
        if self.stack.len() > 1 {
            self.stack.pop();
        }
    }

    /// A node compared byte for byte, whitespace included: a token, a string's
    /// content, a directive the language reads from a comment.
    pub fn leaf(&mut self, kind: &'static str, span: Span, name: Option<&[u8]>, role: Role) {
        self.push(kind, span, name, role, true);
    }

    pub fn comment(&mut self, span: Span) {
        self.comments.push((span, false));
    }

    /// A comment the language reads as code. Placed like a comment, compared
    /// like a leaf.
    pub fn directive(&mut self, span: Span) {
        self.comments.push((span, true));
    }

    pub fn error(&mut self, span: Span) {
        self.errors.push(span);
    }

    fn push(
        &mut self,
        kind: &'static str,
        span: Span,
        name: Option<&[u8]>,
        role: Role,
        leaf: bool,
    ) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        let parent = self.stack.last().copied();
        if let Some(parent) = parent {
            self.nodes[parent.0 as usize].children.push(id);
        }
        self.nodes.push(Node {
            kind,
            span: span.clone(),
            extent: span.clone(),
            parent,
            children: Vec::new(),
            comment: false,
            name: name.map(|n| String::from_utf8_lossy(n).into()),
            role,
            hash: 0,
        });
        self.leaf.push(leaf);
        id
    }

    pub fn finish(mut self) -> Tree {
        // Children are pushed after their parents, so a reverse sweep sees
        // every child's extent before its parent's.
        for i in (0..self.nodes.len()).rev() {
            let extent = self.nodes[i]
                .children
                .iter()
                .map(|c| self.nodes[c.0 as usize].extent.clone())
                .fold(self.nodes[i].extent.clone(), |a, b| {
                    a.start.min(b.start)..a.end.max(b.end)
                });
            self.nodes[i].extent = extent;
        }
        self.attach_comments();
        let mut tree = Tree {
            line_starts: line_starts(&self.source),
            lines: Vec::new(),
            source: self.source,
            nodes: self.nodes,
            errors: self.errors,
        };
        for i in (0..tree.nodes.len()).rev() {
            tree.nodes[i].hash = hash(&tree, i, self.leaf[i]);
        }
        tree.lines = line_facts(&tree);
        tree
    }

    /// Each comment becomes a leaf child of the deepest node containing it.
    fn attach_comments(&mut self) {
        let mut comments = std::mem::take(&mut self.comments);
        comments.sort_by_key(|(c, _)| c.start);
        for (span, directive) in comments {
            let mut at = 0usize;
            'descend: loop {
                for &child in &self.nodes[at].children {
                    let c = &self.nodes[child.0 as usize];
                    if !self.leaf[child.0 as usize]
                        && c.extent.start <= span.start
                        && span.end <= c.extent.end
                    {
                        at = child.0 as usize;
                        continue 'descend;
                    }
                }
                break;
            }
            let id = NodeId(self.nodes.len() as u32);
            self.nodes.push(Node {
                kind: if directive { "directive" } else { "comment" },
                span: span.clone(),
                extent: span.clone(),
                parent: Some(NodeId(at as u32)),
                children: Vec::new(),
                comment: !directive,
                name: None,
                role: Role::Other,
                hash: 0,
            });
            self.leaf.push(true);
            let siblings = &self.nodes[at].children;
            let pos =
                siblings.partition_point(|s| self.nodes[s.0 as usize].extent.start <= span.start);
            self.nodes[at].children.insert(pos, id);
        }
    }
}

/// Kind, then children in order with the non-whitespace text of the node's
/// own span between them. Comments are skipped; leaves hash every byte.
fn hash(tree: &Tree, i: usize, leaf: bool) -> u64 {
    let node = &tree.nodes[i];
    let own = &node.span;
    let mut h = DefaultHasher::new();
    node.kind.hash(&mut h);
    if leaf && node.children.is_empty() {
        tree.source[own.clone()].hash(&mut h);
        return h.finish();
    }
    let mut cursor = own.start;
    for &child in &node.children {
        let c = &tree.nodes[child.0 as usize];
        hash_text(&tree.source, cursor..c.extent.start.min(own.end), &mut h);
        cursor = cursor.max(c.extent.end);
        if !c.comment {
            // Marks where a child sits, so text cannot slide across it.
            h.write_u8(0xff);
            c.hash.hash(&mut h);
        }
    }
    hash_text(&tree.source, cursor..own.end, &mut h);
    h.finish()
}

fn hash_text(source: &[u8], span: Span, h: &mut DefaultHasher) {
    if span.start < span.end {
        for &b in source[span].iter().filter(|b| !b.is_ascii_whitespace()) {
            h.write_u8(b);
        }
    }
}

fn line_starts(source: &[u8]) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        source
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i + 1),
    );
    if starts.last() == Some(&source.len()) && !source.is_empty() {
        starts.pop();
    }
    starts
}

fn line_facts(tree: &Tree) -> Vec<LineFacts> {
    let mut facts = vec![LineFacts::default(); tree.line_count()];
    let mut comments: Vec<Span> = tree
        .nodes
        .iter()
        .filter(|n| n.comment)
        .map(|n| n.extent.clone())
        .collect();
    comments.sort_by_key(|c| c.start);
    let mut next = comments.iter().peekable();
    let mut in_comment: Option<&Span> = None;
    for (offset, byte) in tree.source.iter().enumerate() {
        if in_comment.is_some_and(|c| offset >= c.end) {
            in_comment = None;
        }
        while next.peek().is_some_and(|c| c.start <= offset) {
            in_comment = next.next();
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        let line = &mut facts[tree.line_of(offset) - 1];
        if in_comment.is_some_and(|c| offset < c.end) {
            line.comment = true;
        } else {
            line.code = true;
        }
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `a + b` style: one branch, two leaves, optional comment.
    fn build(src: &str, comment: Option<Span>) -> Tree {
        let mut b = TreeBuilder::new(src.as_bytes().to_vec());
        let a = src.find('a').unwrap();
        let z = src.rfind('z').unwrap();
        b.open("call", a..z + 1, None, Role::Other);
        b.leaf("ident", a..a + 1, Some(b"a"), Role::Other);
        b.leaf("ident", z..z + 1, Some(b"z"), Role::Other);
        b.close();
        if let Some(c) = comment {
            b.comment(c);
        }
        b.finish()
    }

    fn call(tree: &Tree) -> u64 {
        tree.node(tree.node(Tree::ROOT).children[0]).hash
    }

    #[test]
    fn whitespace_does_not_move_the_hash() {
        assert_eq!(call(&build("a + z", None)), call(&build("a+\n   z", None)));
    }

    #[test]
    fn own_text_moves_the_hash() {
        assert_ne!(call(&build("a + z", None)), call(&build("a - z", None)));
    }

    #[test]
    fn leaves_keep_their_whitespace() {
        let tree = |s: &str| {
            let mut b = TreeBuilder::new(s.as_bytes().to_vec());
            b.leaf("string", 0..s.len(), None, Role::Other);
            b.finish()
        };
        assert_ne!(call(&tree("\"a b\"")), call(&tree("\"ab\"")));
    }

    #[test]
    fn comments_attach_inside_and_skip_the_hash() {
        let src = "a + # note\n z";
        let with = build(src, Some(4..10));
        let without = build("a +\n z", None);
        assert_eq!(call(&with), call(&without));
        let note = with.comments_under(Tree::ROOT);
        assert_eq!(note.len(), 1);
        assert_eq!(
            with.node(note[0]).parent,
            Some(with.node(Tree::ROOT).children[0])
        );
        assert_eq!(
            with.line(1),
            LineFacts {
                code: true,
                comment: true
            }
        );
        assert_eq!(
            with.line(2),
            LineFacts {
                code: true,
                comment: false
            }
        );
    }

    #[test]
    fn lines_map_to_bytes() {
        let tree = build("a\n+\nz\n", None);
        assert_eq!(tree.line_count(), 3);
        assert_eq!(tree.line_span(2, 3), 2..6);
        assert_eq!(tree.line_of(4), 3);
        assert_eq!(tree.line_start(4), 6);
    }
}
