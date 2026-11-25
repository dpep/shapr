//! The language-neutral tree every frontend produces, and the builder that
//! normalizes it. Frontends report raw structure (kinds, spans, comments,
//! errors); hashing, comment placement and per-line facts are computed here,
//! once, so every frontend gets the same semantics.

use std::borrow::Cow;
use std::cell::OnceCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::{Range, RangeInclusive};

/// A byte range into a file's source.
pub type Span = Range<usize>;

/// An index into [`Tree::nodes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// What a node means, in terms the core can act on. Set by the frontend;
/// most nodes are [`Role::Other`]. The identifier, where there is one, is
/// [`Node::name`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Role {
    #[default]
    Other,
    /// Declares `name`.
    Definition(NameKind),
    /// A plain assignment: binds the local `name` to the node's only code
    /// child, its value, and does nothing else (Ruby's `v = x`, Rust's
    /// `let v = x;`). A local definition everywhere else.
    Bind,
    /// Reads a variable or constant called `name`.
    Reference(NameKind),
    /// Calls the method `name`.
    Call(Receiver),
    /// A string or symbol literal whose value (`name`) the language resolves
    /// to code at runtime, as in `send(:name)`; or the arguments of such a
    /// call reaching a method by a computed name, with `name` a pattern where `*` stands
    /// for the computed text (`send("get_#{x}")` is `get_*`, `send(n)` is
    /// `*`). See [`spells`].
    DynamicName,
    /// Changes the visibility of the definitions after it, or of its arguments.
    Visibility(Visibility),
    /// Code bound to the enclosing method or block (control flow, its
    /// frame) or to its own position: it can mean something else once
    /// moved or renamed. `classify::hazard::blocks` says which operations
    /// each stops.
    Hazard(Hazard),
    /// An identifier the parser can't resolve, as in a Rust macro's token
    /// tree: it may name anything in scope, locals included.
    Opaque,
    /// Makes the enclosing definition's name visible beyond code: an FFI
    /// export, a route, generated bindings, a derive that prints it
    /// (Rust's `#[no_mangle]`, `#[derive(Debug)]`).
    Exposes,
    /// Syntax that only states types or visibility (a parameter's type, a
    /// return type, generics): no code runs there.
    Annotation,
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

    pub fn hazard(self) -> Option<Hazard> {
        match self {
            Role::Hazard(hazard) => Some(hazard),
            _ => None,
        }
    }

    /// The kind of name the node declares, a [`Role::Bind`] included.
    pub fn defines(self) -> Option<NameKind> {
        match self {
            Role::Definition(kind) => Some(kind),
            Role::Bind => Some(NameKind::Variable),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameKind {
    Method,
    Class,
    Module,
    Constant,
    /// A local variable.
    Variable,
    /// An instance or class variable: state kept on an object or class.
    Field,
    /// A named member of a type that any file can reach through a value (a
    /// Rust struct field), unlike a [`NameKind::Field`].
    Member,
    Global,
    Parameter,
    /// A parameter callers name at the call site (`key:`), so its name is
    /// part of the method's interface.
    KeywordParameter,
    /// The parameter receiving the method's block (`&block`).
    BlockParameter,
    /// The parameter bound to the receiver (Rust's `self`): passed by the
    /// call's receiver, not its arguments.
    SelfParameter,
    /// A macro: expanded at compile time, so where it's defined can matter.
    Macro,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Receiver {
    /// `name(...)`.
    Implicit,
    /// `self.name(...)`.
    SelfRef,
    /// Anything else: the receiver is the call's first child.
    Explicit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Visibility {
    Public,
    Protected,
    Private,
    ModuleFunction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Hazard {
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
    /// Suspends the enclosing async function (`.await`).
    Await,
    /// A macro the frontend can't see into, which may expand to any of the
    /// above (Rust's `bail!`).
    Macro,
    /// Reads or writes the enclosing method's locals by name at runtime
    /// (`binding`, `eval`): a local can't be renamed under it.
    Binding,
    /// Reads the enclosing method's frame: its name (`__method__`), the
    /// last regexp match (`$~`, `$1`).
    Frame,
    /// Reads the call stack (`caller`, `caller_locations`, Rust's
    /// `Location::caller()`): its frames' method names, files and lines,
    /// its own among them.
    Stack,
    /// Reads its own source line (`__LINE__`, `line!()`): moving the code
    /// changes the value.
    Line,
    /// Reads its own file (`__FILE__`, `file!()`): moving the code to
    /// another file changes the value.
    File,
}

/// Facts a frontend marks on a node beside its role. Not in the hash or the
/// output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags(u16);

impl Flags {
    /// Bounds local variables: a local's scope is the nearest node so
    /// marked (a method, class or module body), else the file.
    pub const SCOPE: Flags = Flags(1);
    /// A method definition whose first code child is the object it is
    /// defined on (Ruby's `def self.name`).
    pub const RECEIVER: Flags = Flags(1 << 1);
    /// A call's argument list, holding exactly its positional arguments
    /// and their separators: a block or a block argument is not one.
    pub const ARGUMENTS: Flags = Flags(1 << 2);
    /// A comment the language reads as code ([`TreeBuilder::directive`]);
    /// set by the builder.
    pub const DIRECTIVE: Flags = Flags(1 << 3);
    /// A definition visible outside its crate or package, in a language whose
    /// definitions are private unless marked ([`Semantics::private_by_default`]):
    /// Rust's `pub`, and a `pub` trait's items or `pub` enum's variants.
    pub const PUBLIC: Flags = Flags(1 << 4);
    /// An identifier, or the node holding it, that names a member as well as
    /// the local it reads or binds (Rust's `S { a }`, `S { mut a }`, Ruby's
    /// `{x:}`): renaming it changes the member too.
    pub const MEMBER: Flags = Flags(1 << 5);
    /// A macro call whose expansion may spell its tokens out as text (Rust's
    /// `stringify!`, `dbg!`, a message-less `assert!`, any macro outside
    /// std), or an item an attribute macro reads as written: renaming a
    /// name in it changes what the macro sees.
    pub const SPELLS: Flags = Flags(1 << 6);
    /// A constant whose value is a new class or module (Ruby's
    /// `Point = Struct.new(:x)`, `Class.new`, `Module.new`, `Data.define`):
    /// its name is a class's name.
    pub const CLASS_VALUE: Flags = Flags(1 << 7);
    /// A [`Role::DynamicName`] that reaches a constant, not a method (Ruby's
    /// `const_get(n)`, `n.constantize`).
    pub const CONSTANT: Flags = Flags(1 << 8);
    /// A [`Role::DynamicName`] that reaches a field, not a method (Ruby's
    /// `instance_variable_get(n)`).
    pub const FIELD: Flags = Flags(1 << 9);
    /// A statement that only declares types for a type checker (Sorbet's
    /// `sig { ... }`, `extend T::Sig`): `typing` compares the code without it.
    pub const TYPING: Flags = Flags(1 << 10);
    /// A call that returns its first positional argument, perhaps after
    /// checking its type (Sorbet's `T.let(x, T)`, `T.must(x)`): `typing`
    /// compares the code with the argument in its place.
    pub const TYPE_CAST: Flags = Flags(1 << 11);
    /// A [`Flags::TYPING`] or [`Flags::TYPE_CAST`] node no runtime checks
    /// (`T.unsafe(x)`, `T::Sig::WithoutRuntime.sig`), once its type library
    /// is loaded ([`Frontend::types_loaded`](crate::frontend::Frontend::types_loaded)).
    pub const UNCHECKED: Flags = Flags(1 << 12);
    /// A statement in a namespace's body that qualifies the namespace itself
    /// (Rust's inner attribute `#![cfg(unix)]`): part of its owner's key.
    pub const HEADER: Flags = Flags(1 << 13);

    pub fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for Flags {
    type Output = Flags;

    fn bitor(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }
}

/// Facts about a language's semantics that the core acts on, set by the
/// frontend. By default none holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Semantics {
    /// Each file is its own namespace (a Rust module): a definition moved to
    /// another file is reached by another path, and the names inside it
    /// resolve against another file.
    pub file_scoped: bool,
    /// A local's value is destroyed, observably, when its block ends (Rust's
    /// `Drop`): a binding moved into a new function dies sooner.
    pub scoped_drop: bool,
    /// Method names the language or its runtime reaches without a call
    /// shapr can see: what every object already responds to (Ruby's
    /// `inspect`, `puts`), protocol and hook methods (`each`, `to_str`,
    /// `inherited`), an entry point (`main`). Defining, removing or renaming
    /// one reaches callers beyond the change.
    pub reserved: &'static [&'static str],
    pub spelling: Spelling,
    /// A definition is private unless the frontend marks it
    /// [`Flags::PUBLIC`] (Rust). Otherwise visibility comes from
    /// [`Role::Visibility`] statements, and a definition is public by default
    /// (Ruby).
    pub private_by_default: bool,
    /// A definition takes effect when its statement runs (Ruby's `def`), so
    /// code running beside it at load time may observe it, through
    /// reflection or a hook.
    pub definitions_run: bool,
    /// Methods the runtime calls when a method is defined (Ruby's
    /// `method_added`): defining one changes what the definitions after it do.
    pub definition_hooks: &'static [&'static str],
}

/// Stands for a child in a node's own text ([`Tree::own_text`], the hash). A
/// control byte, so never part of a token next to it (see [`Spelling::mask`]).
pub const MARK: u8 = 0x01;

/// Stands for a masked name in [`Tree::own_text`].
pub const MASKED: u8 = 0x00;

/// What [`Tree::walk_own`] visits.
pub enum Piece {
    Text(Span),
    Child(NodeId),
}

/// One node on a [`Tree::path`].
#[derive(Clone, Copy, Debug)]
pub struct Step<'t> {
    pub kind: &'static str,
    pub role: Role,
    pub name: Option<&'t str>,
    /// For a namespace with no name, its [`Tree::header`].
    pub header: Option<u64>,
}

/// Whether a [`Role::DynamicName`]'s name can spell `name`: `*` in it
/// stands for any text computed at runtime.
pub fn spells(pattern: &str, name: &str) -> bool {
    let mut pieces = pattern.split('*');
    let first = pieces.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let pieces: Vec<&str> = pieces.collect();
    let Some((last, middle)) = pieces.split_last() else {
        return rest.is_empty();
    };
    for piece in middle {
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// How a language spells names in source text, for the few places the core
/// reads a name out of text: masking it in a node's own text, telling an
/// identifier from an operator, matching a literal that spells it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spelling {
    /// Sigils that start a variable's name and belong to it (Ruby's `@x`,
    /// `$x`).
    pub sigils: &'static [char],
    /// Punctuation a method's name may end with (Ruby's `x?`, `x!`, `x=`).
    pub suffixes: &'static [char],
    /// Of the suffixes, those that, glued to a name, spell another name:
    /// `x?` is not `x`, but `x=1` assigns `x`.
    pub glued: &'static [char],
    /// The sigil of an object's own state (Ruby's `@x`), which an
    /// accessor's name drops.
    pub field_sigil: Option<char>,
    /// Marks a symbol before or after its name (Ruby's `:x`, `x:`).
    pub symbol: Option<char>,
    /// Quotes around a string literal.
    pub quotes: &'static [char],
}

impl Spelling {
    /// Replaces the first occurrence of `name` in `text` that stands as a
    /// whole token with [`MASKED`].
    pub fn mask(&self, text: &mut Vec<u8>, name: &[u8]) {
        if let Some(i) = self.find(text, name) {
            text.splice(i..i + name.len(), [MASKED]);
        }
    }

    /// Where `name` first stands as a whole token in `text`.
    fn find(&self, text: &[u8], name: &[u8]) -> Option<usize> {
        let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
        let has = |set: &[char], b: u8| set.contains(&(b as char));
        let suffixed = name.last().is_some_and(|&b| has(self.suffixes, b));
        (0..text.len()).find(|&i| {
            text[i..].starts_with(name)
                && (i == 0 || !(word(text[i - 1]) || has(self.sigils, text[i - 1])))
                && text
                    .get(i + name.len())
                    .is_none_or(|&n| !word(n) && (suffixed || !has(self.glued, n)))
        })
    }

    /// A name a rename can change: an identifier, not an operator.
    pub fn is_identifier(&self, name: &str) -> bool {
        let bare = name.trim_start_matches(self.sigils);
        let bare = bare.strip_suffix(self.suffixes).unwrap_or(bare);
        let mut chars = bare.chars();
        chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && chars.all(|c| c.is_alphanumeric() || c == '_')
    }

    /// `name` without the field sigil.
    pub fn bare<'n>(&self, name: &'n str) -> &'n str {
        match self.field_sigil {
            Some(sigil) => name.trim_start_matches(sigil),
            None => name,
        }
    }

    /// Whether a literal's text (`:name`, `"name"`, `name:`) spells `name`.
    pub fn literal_spells(&self, text: &str, name: &str) -> bool {
        let text = match self.symbol {
            Some(symbol) => text.trim_start_matches(symbol),
            None => text,
        };
        let text = self.bare(text).trim_matches(self.quotes);
        text == name
            || self
                .symbol
                .is_some_and(|s| text.strip_suffix(s) == Some(name))
    }

    /// The name-like core of a token's text: punctuation trimmed but a
    /// method name's suffix.
    pub fn word<'t>(&self, text: &'t str) -> &'t str {
        text.trim_matches(|c: char| {
            !(c.is_alphanumeric() || c == '_' || self.suffixes.contains(&c))
        })
    }
}

impl Semantics {
    pub fn reserves(&self, name: &str) -> bool {
        self.reserved.contains(&name)
    }
}

#[derive(Clone, Debug)]
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
    pub flags: Flags,
    /// Equal for subtrees that differ only in whitespace and comments.
    /// Process-local: never persisted or printed.
    pub hash: u64,
    /// Like `hash`, but also equal where the frontend vouches that two
    /// spellings are the same program ([`TreeBuilder::respell`]); equals
    /// `hash` in a subtree nothing respells.
    pub canon: u64,
    /// Set when the frontend gave this node a canonical form.
    pub respell: Option<Respell>,
}

/// A frontend's canonical form for a node whose syntax has equivalent
/// spellings ([`TreeBuilder::respell`]). It replaces what [`Node::canon`]
/// reads of the node, so it must hold everything the node means.
pub enum Canonical {
    /// The node's whole meaning; its kind, own text and children aren't read
    /// (a string literal's value and encoding, however it is quoted), so the
    /// value starts with a tag of what it is.
    Value(Vec<u8>),
    /// Stands for the node's own text; its code children follow in the
    /// order the frontend reported them, not source order (a modifier `if`
    /// lists its condition first either way).
    Syntax(Vec<u8>),
    /// Stands for the node's own text; its code children are read as a set,
    /// in no order (a braced import list resolves by name).
    Unordered(Vec<u8>),
    /// The node means its only code child (redundant parentheses).
    Transparent,
}

/// A respelled node: the variant family it belongs to, named by the
/// frontend, and whether it stands for its only child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Respell {
    pub variant: &'static str,
    pub transparent: bool,
}

#[derive(Clone, Debug)]
pub struct Tree {
    pub source: Vec<u8>,
    pub nodes: Vec<Node>,
    /// Spans the parser reported as syntax errors.
    pub errors: Vec<Span>,
    pub semantics: Semantics,
    /// Separators the language ignores, sorted: hashed like whitespace.
    ignored: Vec<Span>,
    /// Spans whose whitespace is hashed like code, sorted and disjoint.
    verbatim: Vec<Span>,
    /// Comments' extents, sorted and disjoint.
    comments: Vec<Span>,
    line_starts: Vec<usize>,
    lines: Vec<LineFacts>,
    /// The deepest nesting of nodes below the root.
    pub depth: usize,
    /// Per node, whether it has text of its own ([`Tree::has_own_text`]),
    /// found on first ask: a sweep of a wide node's span is costly to repeat.
    own_text: Vec<OnceCell<bool>>,
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

    /// The 1-based lines `span` touches, first to last.
    pub fn span_lines(&self, span: &Span) -> RangeInclusive<usize> {
        self.line_of(span.start)..=self.line_of(span.end.saturating_sub(1).max(span.start))
    }

    /// The lines node `id`'s extent touches.
    pub fn lines(&self, id: NodeId) -> RangeInclusive<usize> {
        self.span_lines(&self.node(id).extent)
    }

    /// `id`'s children that aren't comments, in order.
    pub fn code_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let children = self.node(id).children.iter().copied();
        children.filter(|&c| !self.node(c).comment)
    }

    /// `id`'s parent, its parent, and so on to the root.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.node(id).parent, |&p| self.node(p).parent)
    }

    /// The nearest ancestor of `id` that `pick` accepts.
    pub fn enclosing(&self, id: NodeId, pick: impl Fn(&Node) -> bool) -> Option<NodeId> {
        self.ancestors(id).find(|&p| pick(self.node(p)))
    }

    /// The scope a local inside `id` lives in: the nearest ancestor marked
    /// [`Flags::SCOPE`], else the root.
    pub fn scope(&self, id: NodeId) -> NodeId {
        self.enclosing(id, |n| n.flags.contains(Flags::SCOPE))
            .unwrap_or(Self::ROOT)
    }

    /// Offsets of the non-whitespace bytes of `id`'s own span that no
    /// child's extent holds: an operator, a keyword, punctuation.
    pub fn own_offsets(&self, id: NodeId) -> impl Iterator<Item = usize> + '_ {
        self.own_offsets_in(id, &self.node(id).span)
    }

    /// Whether [`Tree::own_offsets`] holds any byte.
    pub fn has_own_text(&self, id: NodeId) -> bool {
        *self.own_text[id.0 as usize].get_or_init(|| self.own_offsets(id).next().is_some())
    }

    /// [`Tree::own_offsets`] inside `within`.
    pub fn own_offsets_in(&self, id: NodeId, within: &Span) -> impl Iterator<Item = usize> + '_ {
        let node = self.node(id);
        // One sweep over children sorted by start: `at` is held when the
        // furthest end among children starting at or before it lies past it.
        let mut held: Vec<Span> = node
            .children
            .iter()
            .map(|&c| self.node(c).extent.clone())
            .collect();
        held.sort_by_key(|s| s.start);
        let (mut next, mut reach) = (0, 0);
        let span = node.span.start.max(within.start)..node.span.end.min(within.end);
        span.filter(move |&at| {
            while next < held.len() && held[next].start <= at {
                reach = reach.max(held[next].end);
                next += 1;
            }
            at >= reach && !self.source[at].is_ascii_whitespace()
        })
    }

    /// [`Tree::own_offsets`] as bytes, with the line each is on.
    pub fn own_bytes(&self, id: NodeId) -> Vec<(usize, u8)> {
        self.own_offsets(id)
            .map(|at| (self.line_of(at), self.source[at]))
            .collect()
    }

    /// Walks `id`'s own span in source order, as the hash reads it: the
    /// stretch of text before, between and after the children, and each
    /// child. A child reaching back over text already read reads none twice.
    pub fn walk_own(&self, id: NodeId, mut visit: impl FnMut(Piece)) {
        let span = &self.node(id).span;
        let start = |c: &NodeId| self.node(*c).extent.start;
        let mut children = Cow::from(&self.node(id).children);
        // A modifier form (`x if y`) lists its condition first.
        if !children.is_sorted_by_key(start) {
            children.to_mut().sort_by_key(start);
        }
        let mut cursor = span.start;
        for &c in children.iter() {
            let extent = &self.node(c).extent;
            let end = extent.start.min(span.end);
            if cursor < end {
                visit(Piece::Text(cursor..end));
            }
            cursor = cursor.max(extent.end);
            visit(Piece::Child(c));
        }
        if cursor < span.end {
            visit(Piece::Text(cursor..span.end));
        }
    }

    /// `id`'s [`Node::hash`] with each code child's hash replaced by
    /// `child`'s, the child left out where it gives none. For a node with
    /// children: a leaf's hash reads its bytes.
    pub fn hash_with(&self, id: NodeId, child: impl Fn(NodeId) -> Option<u64>) -> u64 {
        hash(self, id.0 as usize, false, child)
    }

    /// The stretches of `id`'s own span outside its children.
    pub fn own_spans(&self, id: NodeId) -> Vec<Span> {
        let mut out = Vec::new();
        self.walk_own(id, |piece| {
            if let Piece::Text(s) = piece {
                out.push(s);
            }
        });
        out
    }

    /// `id`'s own text as the hash reads it, whitespace dropped (but in a
    /// [`TreeBuilder::verbatim`] span) and [`MARK`] where each code child
    /// sits. With `mask`, the node's name, where the text spells it as a
    /// whole token, becomes [`MASKED`], so nodes that differ only in name
    /// compare equal.
    pub fn own_text(&self, id: NodeId, mask: bool) -> Vec<u8> {
        // Each byte, and whether it is kept though whitespace.
        let mut out = Vec::new();
        let mut kept = Vec::new();
        self.walk_own(id, |piece| match piece {
            Piece::Text(s) => {
                for at in s.filter(|&at| !self.ignored(at)) {
                    out.push(self.source[at]);
                    kept.push(self.verbatim(at));
                }
            }
            Piece::Child(c) if !self.node(c).comment => {
                out.push(MARK);
                kept.push(false);
            }
            Piece::Child(_) => {}
        });
        let name = self.node(id).name.as_deref().filter(|_| mask);
        let spelling = &self.semantics.spelling;
        if let Some((name, i)) = name.and_then(|n| Some((n, spelling.find(&out, n.as_bytes())?))) {
            out.splice(i..i + name.len(), [MASKED]);
            kept.splice(i..i + name.len(), [false]);
        }
        let keep = |(b, k): &(u8, bool)| *k || !b.is_ascii_whitespace();
        out.into_iter()
            .zip(kept)
            .filter(keep)
            .map(|(b, _)| b)
            .collect()
    }

    /// Whether the byte at `at` is a separator the language ignores
    /// ([`TreeBuilder::ignore`]), read like whitespace.
    pub fn ignored(&self, at: usize) -> bool {
        let i = self.ignored.partition_point(|s| s.end <= at);
        self.ignored.get(i).is_some_and(|s| s.contains(&at))
    }

    /// Whether whitespace at `at` is code ([`TreeBuilder::verbatim`]).
    /// Whether byte `at` is inside a comment.
    pub fn in_comment(&self, at: usize) -> bool {
        let i = self.comments.partition_point(|s| s.end <= at);
        self.comments.get(i).is_some_and(|s| s.contains(&at))
    }

    fn verbatim(&self, at: usize) -> bool {
        let i = self.verbatim.partition_point(|s| s.end <= at);
        self.verbatim.get(i).is_some_and(|s| s.contains(&at))
    }

    /// Hash of the first code child of a namespace with no name (a Rust
    /// `impl`, by its header): what stands for its name.
    pub fn header(&self, id: NodeId) -> Option<u64> {
        let node = self.node(id);
        let nameless = node.name.is_none()
            && matches!(
                node.role,
                Role::Definition(NameKind::Class | NameKind::Module)
            );
        nameless
            .then(|| self.code_children(id).next().map(|h| self.node(h).hash))
            .flatten()
    }

    /// `id` and each of its ancestors, innermost first: where a definition
    /// sits.
    pub fn path(&self, id: NodeId) -> Vec<Step<'_>> {
        std::iter::once(id)
            .chain(self.ancestors(id))
            .map(|at| {
                let node = self.node(at);
                Step {
                    kind: node.kind,
                    role: node.role,
                    name: node.name.as_deref(),
                    header: self.header(at),
                }
            })
            .collect()
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

    /// The root and each node whose extent touches `span` (overlaps it or
    /// meets an end), in [`Tree::descendants`] order: every node inside or
    /// overlapping `span`, without walking the rest of the tree.
    pub fn touching(&self, span: &Span) -> impl Iterator<Item = NodeId> + '_ {
        let span = span.clone();
        let mut stack = vec![Self::ROOT];
        std::iter::from_fn(move || {
            let at = stack.pop()?;
            // A node's extent holds its children's; a comment under the root
            // may lie outside the root's.
            let children = self.node(at).children.iter().rev();
            stack.extend(children.filter(|&&c| {
                let extent = &self.node(c).extent;
                extent.start <= span.end && span.start <= extent.end
            }));
            Some(at)
        })
    }

    /// The child naming definition `id`, as `A::B` does in Ruby's
    /// `class A::B`: its first code child referring to the definition's own
    /// name.
    pub fn name_child(&self, id: NodeId) -> Option<NodeId> {
        let def = self.node(id);
        def.name.as_ref()?;
        def.children.iter().copied().find(|&c| {
            let child = self.node(c);
            !child.comment
                && child.name == def.name
                && matches!(
                    child.role,
                    Role::Reference(NameKind::Class | NameKind::Module | NameKind::Constant)
                )
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
    semantics: Semantics,
    ignored: Vec<Span>,
    verbatim: Vec<Span>,
    canonical: Vec<Option<(&'static str, Canonical)>>,
    depth: usize,
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
            semantics: Semantics::default(),
            ignored: Vec::new(),
            verbatim: Vec::new(),
            canonical: Vec::new(),
            depth: 0,
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
        self.depth = self.depth.max(self.stack.len());
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

    /// A separator whose presence never changes the parse, such as Rust's
    /// trailing comma before a closing bracket: hashed like whitespace, so
    /// adding or dropping one is a format change.
    pub fn ignore(&mut self, span: Span) {
        self.ignored.push(span);
    }

    /// A span whose whitespace the language may read, such as the tokens of
    /// a Rust macro that spells them out: hashed byte for byte, so a
    /// whitespace change in it is a code change.
    pub fn verbatim(&mut self, span: Span) {
        self.verbatim.push(span);
    }

    /// Gives the node just opened or reported as a leaf a canonical form, in
    /// the frontend's `variant` family: equivalent spellings must produce the
    /// same form, and different programs different ones.
    pub fn respell(&mut self, variant: &'static str, canonical: Canonical) {
        if let Some(last) = self.canonical.last_mut() {
            *last = Some((variant, canonical));
        }
    }

    /// Marks the innermost open node.
    pub fn flag(&mut self, flags: Flags) {
        if let [_, .., open] = self.stack[..] {
            let node = &mut self.nodes[open.0 as usize];
            node.flags = node.flags | flags;
        }
    }

    pub fn semantics(&mut self, semantics: Semantics) {
        self.semantics = semantics;
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
            flags: Flags::default(),
            hash: 0,
            canon: 0,
            respell: None,
        });
        self.leaf.push(leaf);
        self.canonical.push(None);
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
            depth: self.depth,
            source: self.source,
            nodes: self.nodes,
            errors: self.errors,
            semantics: self.semantics,
            ignored: {
                self.ignored.sort_by_key(|s| s.start);
                self.ignored
            },
            verbatim: disjoint(self.verbatim),
            comments: Vec::new(),
            own_text: Vec::new(),
        };
        tree.own_text = vec![OnceCell::new(); tree.nodes.len()];
        tree.comments = disjoint(
            (tree.nodes.iter())
                .filter(|n| n.comment)
                .map(|n| n.extent.clone())
                .collect(),
        );
        for i in (0..tree.nodes.len()).rev() {
            let own = hash(&tree, i, self.leaf[i], |c| Some(tree.node(c).hash));
            tree.nodes[i].hash = own;
            let canonical = self.canonical[i].as_ref();
            let (canon, transparent) = canon(&tree, i, self.leaf[i], canonical.map(|c| &c.1));
            tree.nodes[i].canon = canon;
            tree.nodes[i].respell = canonical.map(|&(variant, _)| Respell {
                variant,
                transparent,
            });
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
                flags: match directive {
                    true => Flags::DIRECTIVE,
                    false => Flags::default(),
                },
                hash: 0,
                canon: 0,
                respell: None,
            });
            self.leaf.push(true);
            self.canonical.push(None);
            let siblings = &self.nodes[at].children;
            let pos =
                siblings.partition_point(|s| self.nodes[s.0 as usize].extent.start <= span.start);
            self.nodes[at].children.insert(pos, id);
        }
    }
}

/// Kind, then children in order with the non-whitespace text of the node's
/// own span between them. Comments are skipped; leaves hash every byte.
/// `child` reads a child's hash (its `hash` or its `canon`); a child it
/// gives none for is left out, as a comment is.
fn hash(tree: &Tree, i: usize, leaf: bool, child: impl Fn(NodeId) -> Option<u64>) -> u64 {
    let node = &tree.nodes[i];
    let mut h = DefaultHasher::new();
    node.kind.hash(&mut h);
    if leaf && node.children.is_empty() {
        tree.source[node.span.clone()].hash(&mut h);
        return h.finish();
    }
    tree.walk_own(NodeId(i as u32), |piece| match piece {
        Piece::Text(text) => hash_text(tree, text, &mut h),
        Piece::Child(c) if !tree.node(c).comment => {
            if let Some(child) = child(c) {
                // Marks where a child sits, so text cannot slide across it.
                h.write_u8(MARK);
                child.hash(&mut h);
            }
        }
        Piece::Child(_) => {}
    });
    h.finish()
}

/// [`Node::canon`]: the hash over children's canons, or the frontend's
/// canonical form; and whether the node stands for its only code child.
fn canon(tree: &Tree, i: usize, leaf: bool, canonical: Option<&Canonical>) -> (u64, bool) {
    let node = &tree.nodes[i];
    let code = || {
        node.children
            .iter()
            .map(|&c| tree.node(c))
            .filter(|c| !c.comment)
    };
    let mut h = DefaultHasher::new();
    match canonical {
        Some(Canonical::Value(value)) => {
            h.write_u8(b'v');
            value.hash(&mut h);
        }
        Some(Canonical::Syntax(text)) => {
            node.kind.hash(&mut h);
            h.write_u8(b's');
            text.hash(&mut h);
            for child in code() {
                h.write_u8(MARK);
                child.canon.hash(&mut h);
            }
        }
        Some(Canonical::Unordered(text)) => {
            node.kind.hash(&mut h);
            h.write_u8(b'u');
            text.hash(&mut h);
            let mut children: Vec<u64> = code().map(|c| c.canon).collect();
            children.sort_unstable();
            for child in children {
                h.write_u8(MARK);
                child.hash(&mut h);
            }
        }
        Some(Canonical::Transparent) if code().count() == 1 => {
            return (code().next().expect("one child").canon, true);
        }
        Some(Canonical::Transparent) | None => {
            return (hash(tree, i, leaf, |c| Some(tree.node(c).canon)), false);
        }
    }
    (h.finish(), false)
}

fn hash_text(tree: &Tree, span: Span, h: &mut DefaultHasher) {
    if span.start >= span.end {
        return;
    }
    for at in span {
        let b = tree.source[at];
        if (!b.is_ascii_whitespace() || tree.verbatim(at)) && !tree.ignored(at) {
            h.write_u8(b);
        }
    }
}

/// `spans` sorted, with overlapping ones merged.
fn disjoint(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by_key(|s| s.start);
    let mut out: Vec<Span> = Vec::new();
    for s in spans {
        match out.last_mut() {
            Some(last) if s.start < last.end => last.end = last.end.max(s.end),
            _ => out.push(s),
        }
    }
    out
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
    fn ignored_separators_hash_like_whitespace() {
        let tree = |s: &str, ignore: Option<usize>| {
            let mut b = TreeBuilder::new(s.as_bytes().to_vec());
            b.open("call", 0..s.len(), None, Role::Other);
            b.leaf("ident", 2..3, Some(b"a"), Role::Other);
            if let Some(at) = ignore {
                b.ignore(at..at + 1);
            }
            b.close();
            b.finish()
        };
        assert_eq!(call(&tree("f(a,)", Some(3))), call(&tree("f(a )", None)));
        assert_ne!(call(&tree("f(a,)", None)), call(&tree("f(a )", None)));
    }

    #[test]
    fn canon_reads_a_respelled_node_by_its_canonical_form() {
        // A "call" of one leaf: `f(a)`, `f a` or `f((a))`, the leaf a quoted string.
        let tree = |s: &str, respell: bool| {
            let mut b = TreeBuilder::new(s.as_bytes().to_vec());
            b.open("call", 0..s.len(), None, Role::Other);
            if respell {
                b.respell("parens", Canonical::Syntax(b"f".to_vec()));
            }
            let (start, end) = (
                s.find(['\'', '"']).unwrap(),
                s.rfind(['\'', '"']).unwrap() + 1,
            );
            if s.contains("((") {
                b.open("paren", start - 1..end + 1, None, Role::Other);
                b.respell("parens", Canonical::Transparent);
            }
            b.leaf("string", start..end, None, Role::Other);
            if respell {
                b.respell(
                    "quotes",
                    Canonical::Value(s.as_bytes()[start + 1..end - 1].to_vec()),
                );
            }
            if s.contains("((") {
                b.close();
            }
            b.close();
            b.finish()
        };
        let node = |t: &Tree| t.node(t.node(Tree::ROOT).children[0]).clone();
        let base = node(&tree("f('a')", true));
        for same in ["f \"a\"", "f(('a'))"] {
            let other = node(&tree(same, true));
            assert_ne!(base.hash, other.hash, "{same}");
            assert_eq!(base.canon, other.canon, "{same}");
        }
        assert_ne!(base.canon, node(&tree("f('b')", true)).canon);
        let plain = node(&tree("f('a')", false));
        assert_eq!(plain.canon, plain.hash);
        assert_eq!(
            base.respell,
            Some(Respell {
                variant: "parens",
                transparent: false
            })
        );
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
    fn patterns_spell_names() {
        assert!(spells("*", "anything"));
        assert!(spells("get_*", "get_a2") && !spells("get_*", "set_a"));
        assert!(spells("*_old", "x_old") && spells("a*b*c", "abc"));
        assert!(!spells("a*b*b", "ab") && !spells("name", "names"));
        assert!(spells("name", "name"));
    }

    /// Ruby's, as its frontend sets it.
    const RUBY: Spelling = Spelling {
        sigils: &['@', '$'],
        suffixes: &['?', '!', '='],
        glued: &['?', '!'],
        field_sigil: Some('@'),
        symbol: Some(':'),
        quotes: &['"', '\''],
    };

    fn masked(text: &str, name: &str) -> String {
        let mut bytes = text.as_bytes().to_vec();
        RUBY.mask(&mut bytes, name.as_bytes());
        String::from_utf8_lossy(&bytes).replace('\0', "_")
    }

    #[test]
    fn mask_takes_whole_tokens_only() {
        assert_eq!(masked("def f(", "f"), "def _(");
        assert_eq!(masked("def de", "de"), "def _");
        assert_eq!(masked("x += ", "x"), "_ += ");
        assert_eq!(masked("x? x", "x"), "x? _");
        assert_eq!(masked(":@x", "@x"), ":_");
        assert_eq!(masked("@x", "x"), "@x");
    }

    #[test]
    fn identifiers_exclude_operators() {
        for ok in ["a", "@b", "@@c", "d?", "e=", "Foo", "_x1", "é"] {
            assert!(RUBY.is_identifier(ok), "{ok}");
        }
        for bad in ["+", "[]", "<=>", "", "1a", "a b"] {
            assert!(!RUBY.is_identifier(bad), "{bad}");
        }
        assert!(!Spelling::default().is_identifier("x?"));
    }

    #[test]
    fn literals_spell_names_the_language_way() {
        for text in [":go", "\"go\"", "'go'", "go:", ":@go"] {
            assert!(RUBY.literal_spells(text, "go"), "{text}");
        }
        assert!(Spelling::default().literal_spells("go", "go"));
        assert!(!Spelling::default().literal_spells(":go", "go"));
        assert_eq!(RUBY.word(":empty?,"), "empty?");
        assert_eq!(Spelling::default().word("empty?"), "empty");
    }

    #[test]
    fn own_text_marks_children_and_masks_the_name() {
        let tree = build("a + z", None);
        let call = tree.node(Tree::ROOT).children[0];
        assert_eq!(tree.own_text(call, false), [MARK, b'+', MARK]);
        assert_eq!(tree.own_spans(call), vec![Range { start: 1, end: 4 }]);
        assert_eq!(tree.own_bytes(call), [(1, b'+')]);
        assert_eq!(tree.own_offsets_in(call, &(0..2)).count(), 0);
        assert_eq!(tree.own_offsets_in(call, &(2..5)).collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn touching_walks_only_the_nodes_near_a_span() {
        let tree = build("a + z # one\n", Some(6..11));
        for start in 0..12 {
            for end in start..12 {
                let span = start..end;
                let near: Vec<NodeId> = tree.touching(&span).collect();
                let wanted = tree.descendants(Tree::ROOT).filter(|&id| {
                    let e = &tree.node(id).extent;
                    (e.start < span.end && span.start < e.end)
                        || (span.start <= e.start && e.end <= span.end)
                });
                // In walk order, and holding every node inside or overlapping.
                let walk: Vec<NodeId> = tree.descendants(Tree::ROOT).collect();
                let order: Vec<usize> = near
                    .iter()
                    .map(|id| walk.iter().position(|w| w == id).unwrap())
                    .collect();
                assert!(order.is_sorted(), "{span:?}");
                for id in wanted {
                    assert!(near.contains(&id), "{span:?} {id:?}");
                }
            }
        }
    }

    #[test]
    fn comment_bytes_are_known_by_offset() {
        let tree = build("a + z # one\n", Some(6..11));
        let inside: Vec<usize> = (0..12).filter(|&at| tree.in_comment(at)).collect();
        assert_eq!(inside, (6..11).collect::<Vec<_>>());
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
